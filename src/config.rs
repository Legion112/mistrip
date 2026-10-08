//! Reads device credentials from the file written by the cloud token extractor
//! (`~/.config/mistrip/devices.json`), so no secret is ever passed on the
//! command line or kept in the binary.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

/// Model id of the Xiaomi Smart Lightstrip Pro.
pub const STRIP_MODEL: &str = "philips.light.strip5";

#[derive(Debug, Deserialize)]
struct Server {
    #[serde(default)]
    homes: Vec<Home>,
}

#[derive(Debug, Deserialize)]
struct Home {
    #[serde(default)]
    devices: Vec<RawDevice>,
}

#[derive(Debug, Deserialize)]
struct RawDevice {
    did: String,
    name: String,
    model: String,
    token: String,
    #[serde(default)]
    localip: String,
}

/// One device worth talking to.
#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub did: String,
    pub name: String,
    pub model: String,
    pub token_hex: String,
    pub ip: String,
}

/// Default location of the credentials file.
pub fn default_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().ok_or_else(|| anyhow!("cannot determine the config directory"))?;
    Ok(dir.join("mistrip").join("devices.json"))
}

/// Load every device listed in the credentials file.
pub fn load(path: &Path) -> Result<Vec<DeviceEntry>> {
    let text = fs::read_to_string(path).with_context(|| {
        format!(
            "cannot read {}. Run the cloud token extractor first:\n  \
             python token_extractor.py -o {}",
            path.display(),
            path.display()
        )
    })?;
    let servers: Vec<Server> = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;

    let mut out = Vec::new();
    for server in servers {
        for home in server.homes {
            for dev in home.devices {
                out.push(DeviceEntry {
                    did: dev.did,
                    name: dev.name,
                    model: dev.model,
                    token_hex: dev.token,
                    ip: dev.localip,
                });
            }
        }
    }
    Ok(out)
}

/// Pick the lightstrip out of the credentials file.
pub fn find_strip(path: &Path) -> Result<DeviceEntry> {
    let devices = load(path)?;
    let mut strips = devices.into_iter().filter(|d| d.model == STRIP_MODEL);
    let strip = strips
        .next()
        .ok_or_else(|| anyhow!("no {STRIP_MODEL} in {}", path.display()))?;
    if strip.ip.is_empty() {
        bail!("{} has no local IP in the credentials file", strip.name);
    }
    Ok(strip)
}
