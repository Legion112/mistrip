//! mistrip — native Linux control for the Xiaomi Smart Lightstrip Pro.

mod config;
mod miio;
mod strip;

use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::miio::{Connection, Token};
use crate::strip::{MODES, Strip, unpack_rgb};

const TIMEOUT: Duration = Duration::from_secs(5);

const USAGE: &str = "\
mistrip — control a Xiaomi Smart Lightstrip Pro over the local network

USAGE:
    mistrip [COMMAND]

COMMANDS:
    status                 show the current state (default)
    on | off               power
    brightness <1-100>     set brightness in percent
    color <RRGGBB>         set a single colour for the whole strip
    mode <0-8>             select a built-in scene
    rhythm <on|off>        music sync via the control box microphone
    segments <N:RRGGBB>... paint individual 10 cm segments
    devices                list every device in the credentials file

Credentials are read from ~/.config/mistrip/devices.json.
";

fn parse_hex_color(text: &str) -> Result<(u8, u8, u8)> {
    let text = text.trim_start_matches('#');
    if text.len() != 6 {
        bail!("colour must be 6 hex digits like FF0000, got {text:?}");
    }
    let value = u32::from_str_radix(text, 16).with_context(|| format!("{text:?} is not hex"))?;
    Ok(unpack_rgb(value))
}

fn connect() -> Result<Strip> {
    let path = config::default_path()?;
    let entry = config::find_strip(&path)?;
    let token = Token::from_hex(&entry.token_hex)?;
    println!("{} at {} (did {})", entry.name, entry.ip, entry.did);
    let conn = Connection::connect(&entry.ip, token, TIMEOUT)?;
    Ok(Strip::new(conn, entry.did))
}

fn print_status(strip: &mut Strip) -> Result<()> {
    let status = strip.status()?;
    let (r, g, b) = status.rgb();
    println!("  power       : {}", if status.on { "on" } else { "off" });
    println!("  brightness  : {}%", status.brightness);
    println!("  colour      : #{r:02X}{g:02X}{b:02X}");
    println!("  mode        : {} ({})", status.mode, status.mode_name());
    println!("  length      : {} m -> {} segments", status.length_m, status.segments());
    println!(
        "  music sync  : {} (sensitivity {}, animation {}, colours {:?})",
        if status.rhythm_on { "on" } else { "off" },
        status.rhythm_sensitivity,
        status.rhythm_animation,
        status.rhythm_color
    );
    println!("  sleep timer : {} s", status.sleep_timer);
    println!("  diy slot    : {} (next free {})", status.diy_id, status.diy_free_id);
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("status");

    match command {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return Ok(());
        }
        "devices" => {
            let path = config::default_path()?;
            for dev in config::load(&path)? {
                println!("{:<28} {:<26} {}", dev.model, dev.name, dev.ip);
            }
            return Ok(());
        }
        _ => {}
    }

    let mut strip = connect()?;

    match command {
        "status" => print_status(&mut strip)?,
        "on" => strip.set_on(true)?,
        "off" => strip.set_on(false)?,
        "brightness" => {
            let value: u8 = args.get(1).context("brightness needs a value, 1-100")?.parse()?;
            strip.set_brightness(value)?;
        }
        "color" | "colour" => {
            let (r, g, b) = parse_hex_color(args.get(1).context("colour needs RRGGBB")?)?;
            strip.set_color(r, g, b)?;
        }
        "mode" => {
            let value: u8 = args.get(1).context("mode needs 0-8")?.parse()?;
            strip.set_mode(value)?;
            println!("  mode -> {} ({})", value, MODES[value as usize]);
        }
        "rhythm" => match args.get(1).map(String::as_str) {
            Some("on") => strip.set_rhythm(true)?,
            Some("off") => strip.set_rhythm(false)?,
            _ => bail!("rhythm needs 'on' or 'off'"),
        },
        "segments" => {
            let mut segments = Vec::new();
            for spec in &args[1..] {
                let (index, colour) = spec
                    .split_once(':')
                    .with_context(|| format!("expected N:RRGGBB, got {spec:?}"))?;
                segments.push((index.parse::<u8>()?, parse_hex_color(colour)?));
            }
            if segments.is_empty() {
                bail!("segments needs at least one N:RRGGBB pair");
            }
            strip.set_segments(&segments)?;
            println!("  painted {} segment(s)", segments.len());
        }
        other => {
            eprintln!("unknown command {other:?}\n");
            print!("{USAGE}");
            std::process::exit(2);
        }
    }
    Ok(())
}
