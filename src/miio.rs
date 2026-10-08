//! miIO protocol: packet framing, AES-128-CBC payload crypto, and the
//! JSON-RPC request/response cycle over UDP port 54321.
//!
//! Packet layout (32-byte header, big-endian):
//!
//! | offset | size | field                                          |
//! |--------|------|------------------------------------------------|
//! | 0      | 2    | magic, always `0x2131`                         |
//! | 2      | 2    | total packet length (header + payload)         |
//! | 4      | 4    | unknown; 0 normally, `0xFFFFFFFF` in handshake |
//! | 8      | 4    | device id                                      |
//! | 12     | 4    | stamp — the device's clock, in seconds         |
//! | 16     | 16   | MD5 checksum, keyed by the token               |
//!
//! The payload is JSON encrypted with AES-128-CBC/PKCS#7 where
//! `key = MD5(token)` and `iv = MD5(key || token)`. The checksum is computed
//! last, over the finished packet with the token written into the checksum
//! field.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use aes::Aes128;
use anyhow::{Context, Result, anyhow, bail};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use md5::{Digest, Md5};

pub const MIIO_PORT: u16 = 54321;
const MAGIC: u16 = 0x2131;
const HEADER_LEN: usize = 32;
const TOKEN_LEN: usize = 16;

type Encryptor = cbc::Encryptor<Aes128>;
type Decryptor = cbc::Decryptor<Aes128>;

/// A 16-byte device token, as printed by the cloud token extractors.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Token([u8; TOKEN_LEN]);

impl Token {
    /// Parse the usual 32-character hex form.
    pub fn from_hex(hex: &str) -> Result<Self> {
        let hex = hex.trim();
        if hex.len() != TOKEN_LEN * 2 {
            bail!("token must be {} hex chars, got {}", TOKEN_LEN * 2, hex.len());
        }
        let mut raw = [0u8; TOKEN_LEN];
        for (i, byte) in raw.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .with_context(|| format!("token is not valid hex at byte {i}"))?;
        }
        Ok(Self(raw))
    }

    fn key_iv(&self) -> ([u8; 16], [u8; 16]) {
        let key: [u8; 16] = Md5::digest(self.0).into();
        let mut seed = Vec::with_capacity(32);
        seed.extend_from_slice(&key);
        seed.extend_from_slice(&self.0);
        let iv: [u8; 16] = Md5::digest(&seed).into();
        (key, iv)
    }

    fn encrypt(&self, plain: &[u8]) -> Vec<u8> {
        let (key, iv) = self.key_iv();
        Encryptor::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plain)
    }

    fn decrypt(&self, cipher: &[u8]) -> Result<Vec<u8>> {
        let (key, iv) = self.key_iv();
        Decryptor::new(&key.into(), &iv.into())
            .decrypt_padded_vec::<Pkcs7>(cipher)
            .map_err(|e| anyhow!("payload decrypt failed (wrong token?): {e}"))
    }
}

impl std::fmt::Debug for Token {
    /// Never print a token in full: it is the device's only secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Token({:02x}{:02x}..{:02x}{:02x})", self.0[0], self.0[1], self.0[14], self.0[15])
    }
}

/// What the device told us in the handshake.
#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    pub device_id: u32,
    pub stamp: u32,
}

fn build_packet(device_id: u32, stamp: u32, token: &Token, payload: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(HEADER_LEN + payload.len() + 32);
    pkt.extend_from_slice(&MAGIC.to_be_bytes());
    pkt.extend_from_slice(&0u16.to_be_bytes()); // length, filled in below
    pkt.extend_from_slice(&0u32.to_be_bytes()); // unknown
    pkt.extend_from_slice(&device_id.to_be_bytes());
    pkt.extend_from_slice(&stamp.to_be_bytes());
    pkt.extend_from_slice(&token.0); // checksum field holds the token while hashing

    let encrypted = if payload.is_empty() { Vec::new() } else { token.encrypt(payload) };
    pkt.extend_from_slice(&encrypted);

    let total = pkt.len() as u16;
    pkt[2..4].copy_from_slice(&total.to_be_bytes());

    // Checksum is computed last, over the complete packet.
    let digest: [u8; 16] = Md5::digest(&pkt).into();
    pkt[16..32].copy_from_slice(&digest);
    pkt
}

fn parse_header(buf: &[u8]) -> Result<(u16, u32, u32)> {
    if buf.len() < HEADER_LEN {
        bail!("short packet: {} bytes, need at least {HEADER_LEN}", buf.len());
    }
    let magic = u16::from_be_bytes([buf[0], buf[1]]);
    if magic != MAGIC {
        bail!("bad magic {magic:#06x}, expected {MAGIC:#06x}");
    }
    let length = u16::from_be_bytes([buf[2], buf[3]]);
    let device_id = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let stamp = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
    Ok((length, device_id, stamp))
}

/// A connected device. Holds the socket, the token, and the clock offset
/// needed to stamp outgoing packets.
pub struct Connection {
    socket: UdpSocket,
    addr: SocketAddr,
    token: Token,
    device_id: u32,
    stamp: u32,
    stamp_taken_at: Instant,
    request_id: u32,
}

impl Connection {
    /// Handshake with the device and return a ready connection.
    pub fn connect(host: &str, token: Token, timeout: Duration) -> Result<Self> {
        let addr = (host, MIIO_PORT)
            .to_socket_addrs()
            .with_context(|| format!("cannot resolve {host}"))?
            .next()
            .ok_or_else(|| anyhow!("no address for {host}"))?;

        let socket = UdpSocket::bind("0.0.0.0:0").context("cannot bind local UDP socket")?;
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;

        let mut conn = Self {
            socket,
            addr,
            token,
            device_id: 0,
            stamp: 0,
            stamp_taken_at: Instant::now(),
            request_id: 1,
        };
        let hs = conn.handshake()?;
        conn.device_id = hs.device_id;
        conn.stamp = hs.stamp;
        conn.stamp_taken_at = Instant::now();
        Ok(conn)
    }

    /// Send the all-`0xFF` hello packet and read back the device id and clock.
    pub fn handshake(&mut self) -> Result<Handshake> {
        let mut hello = [0xFFu8; HEADER_LEN];
        hello[0..2].copy_from_slice(&MAGIC.to_be_bytes());
        hello[2..4].copy_from_slice(&(HEADER_LEN as u16).to_be_bytes());

        self.socket
            .send_to(&hello, self.addr)
            .with_context(|| format!("cannot send handshake to {}", self.addr))?;

        let mut buf = [0u8; 1024];
        let (n, _) = self
            .socket
            .recv_from(&mut buf)
            .context("no handshake reply — device unreachable, or UDP 54321 blocked")?;
        let (_, device_id, stamp) = parse_header(&buf[..n])?;
        Ok(Handshake { device_id, stamp })
    }

    /// The stamp to put on the next packet: device clock plus elapsed local time.
    fn next_stamp(&self) -> u32 {
        self.stamp
            .wrapping_add(self.stamp_taken_at.elapsed().as_secs() as u32)
    }

    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    /// Issue one JSON-RPC call and return the `result` value.
    pub fn call(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1).max(1);

        let request = serde_json::json!({ "id": id, "method": method, "params": params });
        let plain = serde_json::to_vec(&request)?;
        let packet = build_packet(self.device_id, self.next_stamp(), &self.token, &plain);

        self.socket
            .send_to(&packet, self.addr)
            .with_context(|| format!("cannot send {method} to {}", self.addr))?;

        let mut buf = [0u8; 4096];
        let (n, _) = self
            .socket
            .recv_from(&mut buf)
            .with_context(|| format!("no reply to {method}"))?;
        let (length, _, stamp) = parse_header(&buf[..n])?;

        // Re-anchor our clock on every reply so long-lived connections stay valid.
        self.stamp = stamp;
        self.stamp_taken_at = Instant::now();

        let end = (length as usize).min(n);
        if end <= HEADER_LEN {
            bail!("device replied to {method} with an empty payload");
        }
        let decrypted = self.token.decrypt(&buf[HEADER_LEN..end])?;
        // Firmware sometimes pads the JSON with NULs.
        let json = decrypted.split(|b| *b == 0).next().unwrap_or(&[]);
        let value: serde_json::Value = serde_json::from_slice(json)
            .with_context(|| format!("device reply to {method} is not valid JSON"))?;

        if let Some(error) = value.get("error") {
            bail!("device rejected {method}: {error}");
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("device reply to {method} has no result: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN_HEX: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn token_hex_roundtrip() {
        let token = Token::from_hex(TOKEN_HEX).unwrap();
        assert_eq!(token.0[0], 0x00);
        assert_eq!(token.0[15], 0xff);
        assert!(Token::from_hex("tooshort").is_err());
        assert!(Token::from_hex("zz112233445566778899aabbccddeeff").is_err());
    }

    #[test]
    fn debug_does_not_leak_the_token() {
        let shown = format!("{:?}", Token::from_hex(TOKEN_HEX).unwrap());
        assert!(!shown.contains("112233"), "token leaked in Debug: {shown}");
    }

    #[test]
    fn payload_crypto_roundtrips() {
        let token = Token::from_hex(TOKEN_HEX).unwrap();
        let plain = br#"{"id":1,"method":"get_properties"}"#;
        let cipher = token.encrypt(plain);
        assert_ne!(&cipher[..], &plain[..]);
        assert_eq!(token.decrypt(&cipher).unwrap(), plain);
    }

    #[test]
    fn packet_header_is_well_formed() {
        let token = Token::from_hex(TOKEN_HEX).unwrap();
        let pkt = build_packet(0x1234_5678, 42, &token, b"{}");
        let (length, device_id, stamp) = parse_header(&pkt).unwrap();
        assert_eq!(length as usize, pkt.len());
        assert_eq!(device_id, 0x1234_5678);
        assert_eq!(stamp, 42);
        // The checksum field must no longer contain the raw token.
        assert_ne!(&pkt[16..32], &token.0[..]);
    }

    #[test]
    fn checksum_is_md5_of_packet_with_token_in_place() {
        let token = Token::from_hex(TOKEN_HEX).unwrap();
        let pkt = build_packet(1, 2, &token, b"{}");
        let mut rebuilt = pkt.clone();
        rebuilt[16..32].copy_from_slice(&token.0);
        let expected: [u8; 16] = Md5::digest(&rebuilt).into();
        assert_eq!(&pkt[16..32], &expected[..]);
    }
}
