//! Native Linux control for the Xiaomi Smart Lightstrip Pro
//! (`philips.light.strip5`) over the miIO protocol.
//!
//! - [`miio`] implements the wire protocol: packet framing, AES-128-CBC
//!   payload crypto, the handshake, and device clock tracking.
//! - [`strip`] is the typed control surface built from the device's published
//!   MIoT spec.
//! - [`config`] loads credentials from the file written by the cloud token
//!   extractor, so tokens never pass through `argv`.

pub mod config;
pub mod miio;
pub mod strip;
