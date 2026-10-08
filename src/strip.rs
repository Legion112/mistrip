//! Typed control surface for the Xiaomi Smart Lightstrip Pro
//! (`philips.light.strip5`), following its published MIoT spec:
//! <https://home.miot-spec.com/spec/philips.light.strip5>

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::miio::Connection;

const SIID_LIGHT: u8 = 2;
const SIID_EXTRA: u8 = 3;

// Light service (siid 2)
const PIID_ON: u8 = 1;
const PIID_MODE: u8 = 2;
const PIID_BRIGHTNESS: u8 = 3;
const PIID_COLOR: u8 = 4;
const AIID_TOGGLE: u8 = 1;

// Extra attributes service (siid 3)
const PIID_RHYTHM_ON: u8 = 3;
const PIID_SLEEP_TIMER: u8 = 4;
const PIID_RHYTHM_COLOR: u8 = 6;
const PIID_RHYTHM_SENSITIVITY: u8 = 7;
const PIID_RHYTHM_ANIMATION: u8 = 8;
const PIID_DIY_COLOR: u8 = 9;
const PIID_DIY_ID: u8 = 12;
const PIID_LENGTH: u8 = 16;
const PIID_DIY_FREE_ID: u8 = 19;

/// The nine built-in scenes, in spec order.
pub const MODES: [&str; 9] = [
    "Off / custom",
    "Heartbeat",
    "Cozy campfire",
    "Nordic aurora",
    "Beach walk",
    "Fantasy colorful",
    "Summer lime",
    "Instant change",
    "Four seasons song",
];

/// A snapshot of everything we read in one round trip.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub on: bool,
    pub mode: u8,
    pub brightness: u8,
    pub color: u32,
    pub rhythm_on: bool,
    pub rhythm_sensitivity: u8,
    pub rhythm_animation: u8,
    pub rhythm_color: String,
    pub sleep_timer: u32,
    pub diy_id: u8,
    pub diy_free_id: u8,
    /// Strip length in metres, 2–5.
    pub length_m: u8,
}

impl Status {
    /// The strip changes colour in 10 cm steps, so a 2 m strip has 20 segments.
    pub fn segments(&self) -> usize {
        self.length_m as usize * 10
    }

    pub fn mode_name(&self) -> &'static str {
        MODES.get(self.mode as usize).copied().unwrap_or("unknown")
    }

    /// Current colour as `(r, g, b)`.
    pub fn rgb(&self) -> (u8, u8, u8) {
        unpack_rgb(self.color)
    }
}

pub fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

pub fn unpack_rgb(color: u32) -> (u8, u8, u8) {
    (
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    )
}

/// A lightstrip we can talk to.
pub struct Strip {
    conn: Connection,
    did: String,
}

impl Strip {
    pub fn new(conn: Connection, did: String) -> Self {
        Self { conn, did }
    }

    /// Read several properties in a single request.
    fn get(&mut self, props: &[(u8, u8)]) -> Result<Vec<Value>> {
        let params: Vec<Value> = props
            .iter()
            .map(|(siid, piid)| json!({ "did": self.did, "siid": siid, "piid": piid }))
            .collect();
        let result = self.conn.call("get_properties", Value::Array(params))?;
        result
            .as_array()
            .cloned()
            .context("get_properties did not return an array")
    }

    fn set(&mut self, siid: u8, piid: u8, value: Value) -> Result<()> {
        let params = json!([{ "did": self.did, "siid": siid, "piid": piid, "value": value }]);
        let result = self.conn.call("set_properties", params)?;
        // Each entry carries its own result code; a non-zero code means the
        // device understood the request but refused the value.
        if let Some(entry) = result.as_array().and_then(|a| a.first()) {
            match entry.get("code").and_then(Value::as_i64) {
                Some(0) | None => Ok(()),
                Some(code) => bail!("device refused {siid}.{piid} = {value} (code {code})"),
            }
        } else {
            Ok(())
        }
    }

    pub fn call_action(&mut self, siid: u8, aiid: u8, args: Value) -> Result<Value> {
        let params = json!({ "did": self.did, "siid": siid, "aiid": aiid, "in": args });
        self.conn.call("action", params)
    }

    /// Read the full state in one round trip.
    pub fn status(&mut self) -> Result<Status> {
        let props = [
            (SIID_LIGHT, PIID_ON),
            (SIID_LIGHT, PIID_MODE),
            (SIID_LIGHT, PIID_BRIGHTNESS),
            (SIID_LIGHT, PIID_COLOR),
            (SIID_EXTRA, PIID_RHYTHM_ON),
            (SIID_EXTRA, PIID_RHYTHM_SENSITIVITY),
            (SIID_EXTRA, PIID_RHYTHM_ANIMATION),
            (SIID_EXTRA, PIID_RHYTHM_COLOR),
            (SIID_EXTRA, PIID_SLEEP_TIMER),
            (SIID_EXTRA, PIID_DIY_ID),
            (SIID_EXTRA, PIID_DIY_FREE_ID),
            (SIID_EXTRA, PIID_LENGTH),
        ];
        let entries = self.get(&props)?;

        let mut status = Status::default();
        for entry in entries {
            let siid = entry.get("siid").and_then(Value::as_u64).unwrap_or(0) as u8;
            let piid = entry.get("piid").and_then(Value::as_u64).unwrap_or(0) as u8;
            let value = entry.get("value");
            let Some(value) = value else { continue };
            match (siid, piid) {
                (SIID_LIGHT, PIID_ON) => status.on = value.as_bool().unwrap_or(false),
                (SIID_LIGHT, PIID_MODE) => status.mode = value.as_u64().unwrap_or(0) as u8,
                (SIID_LIGHT, PIID_BRIGHTNESS) => {
                    status.brightness = value.as_u64().unwrap_or(0) as u8
                }
                (SIID_LIGHT, PIID_COLOR) => status.color = value.as_u64().unwrap_or(0) as u32,
                (SIID_EXTRA, PIID_RHYTHM_ON) => status.rhythm_on = value.as_bool().unwrap_or(false),
                (SIID_EXTRA, PIID_RHYTHM_SENSITIVITY) => {
                    status.rhythm_sensitivity = value.as_u64().unwrap_or(0) as u8
                }
                (SIID_EXTRA, PIID_RHYTHM_ANIMATION) => {
                    status.rhythm_animation = value.as_u64().unwrap_or(0) as u8
                }
                (SIID_EXTRA, PIID_RHYTHM_COLOR) => {
                    status.rhythm_color = value.as_str().unwrap_or_default().to_string()
                }
                (SIID_EXTRA, PIID_SLEEP_TIMER) => {
                    status.sleep_timer = value.as_u64().unwrap_or(0) as u32
                }
                (SIID_EXTRA, PIID_DIY_ID) => status.diy_id = value.as_u64().unwrap_or(0) as u8,
                (SIID_EXTRA, PIID_DIY_FREE_ID) => {
                    status.diy_free_id = value.as_u64().unwrap_or(0) as u8
                }
                (SIID_EXTRA, PIID_LENGTH) => status.length_m = value.as_u64().unwrap_or(2) as u8,
                _ => {}
            }
        }
        if status.length_m == 0 {
            status.length_m = 2;
        }
        Ok(status)
    }

    /// Flip the power state using the device's own toggle action (siid 2, aiid 1),
    /// which avoids a read-then-write race against the Mi Home app.
    pub fn toggle(&mut self) -> Result<()> {
        self.call_action(SIID_LIGHT, AIID_TOGGLE, json!([]))?;
        Ok(())
    }

    pub fn set_on(&mut self, on: bool) -> Result<()> {
        self.set(SIID_LIGHT, PIID_ON, json!(on))
    }

    /// Brightness in percent, 1–100.
    pub fn set_brightness(&mut self, percent: u8) -> Result<()> {
        let percent = percent.clamp(1, 100);
        self.set(SIID_LIGHT, PIID_BRIGHTNESS, json!(percent))
    }

    pub fn set_color(&mut self, r: u8, g: u8, b: u8) -> Result<()> {
        self.set(SIID_LIGHT, PIID_COLOR, json!(pack_rgb(r, g, b)))
    }

    /// Select one of the nine built-in scenes.
    pub fn set_mode(&mut self, mode: u8) -> Result<()> {
        if mode as usize >= MODES.len() {
            bail!("mode must be 0..={}", MODES.len() - 1);
        }
        self.set(SIID_LIGHT, PIID_MODE, json!(mode))
    }

    pub fn set_rhythm(&mut self, on: bool) -> Result<()> {
        self.set(SIID_EXTRA, PIID_RHYTHM_ON, json!(on))
    }

    /// Music-sync sensitivity, 0–2.
    pub fn set_rhythm_sensitivity(&mut self, level: u8) -> Result<()> {
        self.set(SIID_EXTRA, PIID_RHYTHM_SENSITIVITY, json!(level.min(2)))
    }

    /// Sleep timer in seconds, 0–21600 (0 disables it).
    pub fn set_sleep_timer(&mut self, seconds: u32) -> Result<()> {
        self.set(SIID_EXTRA, PIID_SLEEP_TIMER, json!(seconds.min(21600)))
    }

    /// Paint individual 10 cm segments.
    ///
    /// The wire format is `[index,RRGGBB]` groups joined by `:` — e.g.
    /// `[0,FF0000]:[1,00FF00]`. This is the one part of the spec whose exact
    /// grammar is not reliably documented, so it is isolated here in
    /// [`format_segments`] where it can be corrected in one place.
    pub fn set_segments(&mut self, segments: &[(u8, (u8, u8, u8))]) -> Result<()> {
        if segments.is_empty() {
            return Ok(());
        }
        let payload = format_segments(segments);
        self.set(SIID_EXTRA, PIID_DIY_COLOR, json!(payload))
    }
}

/// Build the `diy-color` wire string for a set of segment/colour pairs.
pub fn format_segments(segments: &[(u8, (u8, u8, u8))]) -> String {
    segments
        .iter()
        .map(|(index, (r, g, b))| format!("[{index},{r:02X}{g:02X}{b:02X}]"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_packing_roundtrips() {
        assert_eq!(pack_rgb(0xFF, 0xA3, 0x5C), 16_753_500);
        assert_eq!(unpack_rgb(16_753_500), (0xFF, 0xA3, 0x5C));
        assert_eq!(unpack_rgb(pack_rgb(1, 2, 3)), (1, 2, 3));
    }

    #[test]
    fn segment_format_matches_the_documented_shape() {
        let formatted = format_segments(&[(0, (0xFF, 0, 0)), (1, (0, 0xFF, 0))]);
        assert_eq!(formatted, "[0,FF0000]:[1,00FF00]");
    }

    #[test]
    fn segment_format_pads_short_components() {
        assert_eq!(format_segments(&[(10, (0, 0, 0x0F))]), "[10,00000F]");
    }

    #[test]
    fn status_derives_segment_count_from_length() {
        let status = Status {
            length_m: 2,
            ..Default::default()
        };
        assert_eq!(status.segments(), 20);
        let extended = Status {
            length_m: 5,
            ..Default::default()
        };
        assert_eq!(extended.segments(), 50);
    }

    #[test]
    fn mode_names_cover_the_spec_range() {
        let status = Status {
            mode: 3,
            ..Default::default()
        };
        assert_eq!(status.mode_name(), "Nordic aurora");
        let bogus = Status {
            mode: 99,
            ..Default::default()
        };
        assert_eq!(bogus.mode_name(), "unknown");
    }
}
