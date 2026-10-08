//! Desktop GUI for the Xiaomi Smart Lightstrip Pro.
//!
//! All device traffic happens on a worker thread; the UI only exchanges
//! messages with it, so a slow or unreachable strip never freezes the window.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText, Vec2};
use mistrip::config;
use mistrip::miio::{Connection, Token};
use mistrip::strip::{MODES, Status, Strip};

const TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Handy starting colours, warm to cool.
const PRESETS: [(&str, [u8; 3]); 8] = [
    ("Red", [0xFF, 0x3B, 0x30]),
    ("Amber", [0xFF, 0x95, 0x00]),
    ("Warm", [0xFF, 0xA3, 0x5C]),
    ("Yellow", [0xFF, 0xD6, 0x0A]),
    ("Green", [0x34, 0xC7, 0x59]),
    ("Cyan", [0x32, 0xD7, 0xE0]),
    ("Blue", [0x0A, 0x84, 0xFF]),
    ("Violet", [0xBF, 0x5A, 0xF2]),
];

// ----------------------------------------------------------------- worker

enum Cmd {
    SetOn(bool),
    Brightness(u8),
    Color([u8; 3]),
    Mode(u8),
    Rhythm(bool),
    Sensitivity(u8),
    Timer(u32),
    Segments(Vec<(u8, (u8, u8, u8))>),
    Refresh,
}

enum Msg {
    Connected { name: String, ip: String },
    Disconnected(String),
    Status(Status),
    Failed(String),
}

fn open_device() -> anyhow::Result<(Strip, String, String)> {
    let path = config::default_path()?;
    let entry = config::find_strip(&path)?;
    let token = Token::from_hex(&entry.token_hex)?;
    let conn = Connection::connect(&entry.ip, token, TIMEOUT)?;
    Ok((Strip::new(conn, entry.did), entry.name, entry.ip))
}

fn apply(strip: &mut Strip, cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::SetOn(on) => strip.set_on(on),
        Cmd::Brightness(value) => strip.set_brightness(value),
        Cmd::Color([r, g, b]) => strip.set_color(r, g, b),
        Cmd::Mode(mode) => strip.set_mode(mode),
        Cmd::Rhythm(on) => strip.set_rhythm(on),
        Cmd::Sensitivity(level) => strip.set_rhythm_sensitivity(level),
        Cmd::Timer(seconds) => strip.set_sleep_timer(seconds),
        Cmd::Segments(segments) => strip.set_segments(&segments),
        Cmd::Refresh => Ok(()),
    }
}

fn worker(cmd_rx: Receiver<Cmd>, msg_tx: Sender<Msg>, ctx: egui::Context) {
    let mut strip: Option<Strip> = None;

    loop {
        // Reconnect whenever we have no usable connection.
        if strip.is_none() {
            match open_device() {
                Ok((device, name, ip)) => {
                    strip = Some(device);
                    if msg_tx.send(Msg::Connected { name, ip }).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    if msg_tx.send(Msg::Failed(format!("{err:#}"))).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    match cmd_rx.recv_timeout(RECONNECT_DELAY) {
                        Err(RecvTimeoutError::Disconnected) => return,
                        // Drop whatever was queued: it predates the connection.
                        _ => continue,
                    }
                }
            }
            ctx.request_repaint();
        }

        let device = strip.as_mut().expect("connected above");

        // Wait for a command, but never go longer than POLL_INTERVAL without
        // refreshing, so external changes (Mi Home, the physical button) show up.
        let pending = match cmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(cmd) => Some(cmd),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };

        if let Some(cmd) = pending
            && let Err(err) = apply(device, cmd)
        {
            let _ = msg_tx.send(Msg::Disconnected(format!("{err:#}")));
            ctx.request_repaint();
            strip = None;
            continue;
        }

        match device.status() {
            Ok(status) => {
                if msg_tx.send(Msg::Status(status)).is_err() {
                    return;
                }
            }
            Err(err) => {
                let _ = msg_tx.send(Msg::Disconnected(format!("{err:#}")));
                strip = None;
            }
        }
        ctx.request_repaint();
    }
}

// -------------------------------------------------------------------- app

struct App {
    cmd_tx: Sender<Cmd>,
    msg_rx: Receiver<Msg>,

    device: Option<(String, String)>,
    status: Option<Status>,
    error: Option<String>,
    last_update: Option<Instant>,

    // Local UI state. Device values overwrite these unless the user is editing.
    color: [u8; 3],
    brightness: u8,
    brightness_held: bool,
    timer_minutes: u32,
    /// What the user has painted, per 10 cm segment. `None` means "untouched".
    painted: Vec<Option<[u8; 3]>>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        style(ctx);
        let (cmd_tx, cmd_rx) = channel();
        let (msg_tx, msg_rx) = channel();
        let worker_ctx = ctx.clone();
        thread::spawn(move || worker(cmd_rx, msg_tx, worker_ctx));

        Self {
            cmd_tx,
            msg_rx,
            device: None,
            status: None,
            error: None,
            last_update: None,
            color: [0xFF, 0xA3, 0x5C],
            brightness: 60,
            brightness_held: false,
            timer_minutes: 30,
            painted: vec![None; 20],
        }
    }

    fn send(&mut self, cmd: Cmd) {
        if self.cmd_tx.send(cmd).is_err() {
            self.error = Some("worker thread stopped".into());
        }
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::Connected { name, ip } => {
                    self.device = Some((name, ip));
                    self.error = None;
                }
                Msg::Status(status) => {
                    if !self.brightness_held {
                        self.brightness = status.brightness.max(1);
                    }
                    let segments = status.segments();
                    if self.painted.len() != segments {
                        self.painted.resize(segments, None);
                    }
                    self.status = Some(status);
                    self.error = None;
                    self.last_update = Some(Instant::now());
                }
                Msg::Disconnected(err) | Msg::Failed(err) => {
                    self.device = None;
                    self.error = Some(err);
                }
            }
        }
    }
}

fn style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(0x15, 0x17, 0x1E);
    visuals.window_fill = Color32::from_rgb(0x1B, 0x1E, 0x27);
    visuals.extreme_bg_color = Color32::from_rgb(0x10, 0x12, 0x18);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x2B, 0x2F, 0x3A);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x37, 0x3C, 0x4A);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0x0A, 0x84, 0xFF);
    visuals.selection.bg_fill = Color32::from_rgb(0x0A, 0x84, 0xFF);
    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 6.0);
        style.spacing.slider_width = 240.0;
    });
}

/// A small filled circle, drawn rather than typed: the default font has no
/// dependable glyph for one.
fn status_dot(ui: &mut egui::Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, colour);
}

fn card<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgb(0x1B, 0x1E, 0x27))
        .inner_margin(12.0)
        .corner_radius(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(title.to_uppercase())
                    .size(11.0)
                    .color(Color32::from_gray(130))
                    .strong(),
            );
            ui.add_space(6.0);
            body(ui)
        })
        .inner
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();
        // Keep the "last updated" line ticking even without device traffic.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.header(ui);
                ui.add_space(4.0);

                let online = self.device.is_some() && self.status.is_some();
                ui.add_enabled_ui(online, |ui| {
                    self.power_and_brightness(ui);
                    self.colour(ui);
                    self.segments(ui);
                    self.scenes(ui);
                    self.music(ui);
                    self.timer(ui);
                });
            });
        });
    }
}

impl App {
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("mistrip").size(22.0).strong());
            ui.add_space(4.0);
            match &self.device {
                Some((name, ip)) => {
                    status_dot(ui, Color32::from_rgb(0x34, 0xC7, 0x59));
                    ui.label(
                        RichText::new(format!("{name} — {ip}")).color(Color32::from_gray(160)),
                    );
                }
                None => {
                    status_dot(ui, Color32::from_rgb(0xFF, 0x3B, 0x30));
                    ui.label(RichText::new("connecting…").color(Color32::from_gray(160)));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh").clicked() {
                    self.send(Cmd::Refresh);
                }
                if let Some(at) = self.last_update {
                    ui.label(
                        RichText::new(format!("{}s ago", at.elapsed().as_secs()))
                            .size(11.0)
                            .color(Color32::from_gray(110)),
                    );
                }
            });
        });

        if let Some(error) = self.error.clone() {
            ui.add_space(4.0);
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(0x3A, 0x1D, 0x1D))
                .corner_radius(8.0)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(error)
                            .color(Color32::from_rgb(0xFF, 0x8A, 0x80))
                            .size(12.0),
                    );
                });
        }
    }

    fn power_and_brightness(&mut self, ui: &mut egui::Ui) {
        let on = self.status.as_ref().is_some_and(|s| s.on);
        card(ui, "Power", |ui| {
            ui.horizontal(|ui| {
                let label = if on { "Turn off" } else { "Turn on" };
                let colour = if on {
                    Color32::from_rgb(0x3A, 0x2B, 0x2B)
                } else {
                    Color32::from_rgb(0x0A, 0x84, 0xFF)
                };
                if ui
                    .add(egui::Button::new(RichText::new(label).strong()).fill(colour))
                    .clicked()
                {
                    self.send(Cmd::SetOn(!on));
                }
                ui.label(if on { "on" } else { "off" });
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Brightness");
                let response =
                    ui.add(egui::Slider::new(&mut self.brightness, 1..=100).suffix(" %"));
                self.brightness_held = response.dragged();
                if response.drag_stopped() || (response.changed() && !response.dragged()) {
                    let value = self.brightness;
                    self.send(Cmd::Brightness(value));
                }
            });
        });
    }

    fn colour(&mut self, ui: &mut egui::Ui) {
        card(ui, "Colour", |ui| {
            ui.horizontal(|ui| {
                if ui.color_edit_button_srgb(&mut self.color).changed() {
                    let colour = self.color;
                    self.send(Cmd::Color(colour));
                }
                let [r, g, b] = self.color;
                ui.label(RichText::new(format!("#{r:02X}{g:02X}{b:02X}")).monospace());
                if let Some(status) = &self.status {
                    let (dr, dg, db) = status.rgb();
                    ui.label(
                        RichText::new(format!("device #{dr:02X}{dg:02X}{db:02X}"))
                            .size(11.0)
                            .color(Color32::from_gray(110)),
                    );
                }
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for (name, rgb) in PRESETS {
                    let fill = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    if ui
                        .add(
                            egui::Button::new(RichText::new(name).color(Color32::BLACK)).fill(fill),
                        )
                        .clicked()
                    {
                        self.color = rgb;
                        self.send(Cmd::Color(rgb));
                    }
                }
            });
        });
    }

    fn segments(&mut self, ui: &mut egui::Ui) {
        let count = self.status.as_ref().map_or(20, Status::segments);
        card(ui, &format!("Segments — {count} × 10 cm"), |ui| {
            ui.label(
                RichText::new("Click a segment to paint it with the current colour, then Apply.")
                    .size(11.0)
                    .color(Color32::from_gray(120)),
            );
            ui.add_space(6.0);

            let available = ui.available_width() - 4.0;
            let cell_w = (available / count as f32 - 3.0).max(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for index in 0..count {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(cell_w, 44.0), egui::Sense::click());
                    let painted = self.painted.get(index).copied().flatten();
                    let fill = match painted {
                        Some([r, g, b]) => Color32::from_rgb(r, g, b),
                        None => Color32::from_gray(if response.hovered() { 70 } else { 48 }),
                    };
                    ui.painter().rect_filled(rect, 3.0, fill);
                    if response.clicked()
                        && let Some(slot) = self.painted.get_mut(index)
                    {
                        *slot = Some(self.color);
                    }
                    response.on_hover_text(format!("segment {index}"));
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Fill all").clicked() {
                    let colour = self.color;
                    self.painted = vec![Some(colour); count];
                }
                if ui.button("Rainbow").clicked() {
                    self.painted = (0..count)
                        .map(|i| {
                            let hue = i as f32 / count as f32;
                            Some(hsv_to_rgb(hue, 0.85, 1.0))
                        })
                        .collect();
                }
                if ui.button("Clear").clicked() {
                    self.painted = vec![None; count];
                }
                let any = self.painted.iter().any(Option::is_some);
                if ui
                    .add_enabled(any, egui::Button::new(RichText::new("Apply").strong()))
                    .clicked()
                {
                    let segments: Vec<(u8, (u8, u8, u8))> = self
                        .painted
                        .iter()
                        .enumerate()
                        .filter_map(|(i, slot)| slot.map(|[r, g, b]| (i as u8, (r, g, b))))
                        .collect();
                    self.send(Cmd::Segments(segments));
                }
            });
        });
    }

    fn scenes(&mut self, ui: &mut egui::Ui) {
        let active = self.status.as_ref().map_or(0, |s| s.mode);
        card(ui, "Scenes", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, name) in MODES.iter().enumerate() {
                    let selected = index as u8 == active;
                    if ui.selectable_label(selected, *name).clicked() {
                        self.send(Cmd::Mode(index as u8));
                    }
                }
            });
        });
    }

    fn music(&mut self, ui: &mut egui::Ui) {
        let (mut on, sensitivity) = self
            .status
            .as_ref()
            .map_or((false, 1), |s| (s.rhythm_on, s.rhythm_sensitivity));
        card(ui, "Music sync", |ui| {
            ui.label(
                RichText::new("Driven by the microphone in the control box, not this computer.")
                    .size(11.0)
                    .color(Color32::from_gray(120)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut on, "Enabled").changed() {
                    self.send(Cmd::Rhythm(on));
                }
                ui.add_space(12.0);
                ui.label("Sensitivity");
                for (level, label) in [(0u8, "Low"), (1, "Medium"), (2, "High")] {
                    if ui.selectable_label(sensitivity == level, label).clicked() {
                        self.send(Cmd::Sensitivity(level));
                    }
                }
            });
        });
    }

    fn timer(&mut self, ui: &mut egui::Ui) {
        let active = self.status.as_ref().map_or(0, |s| s.sleep_timer);
        card(ui, "Sleep timer", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.timer_minutes, 0..=360).suffix(" min"));
                if ui.button("Set").clicked() {
                    let seconds = self.timer_minutes * 60;
                    self.send(Cmd::Timer(seconds));
                }
                if active > 0 {
                    ui.label(
                        RichText::new(format!("off in {} min", active / 60))
                            .color(Color32::from_rgb(0xFF, 0xD6, 0x0A)),
                    );
                    if ui.button("Cancel").clicked() {
                        self.send(Cmd::Timer(0));
                    }
                }
            });
        });
    }
}

/// Minimal HSV→RGB, used only for the rainbow preset.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32) % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([560.0, 820.0])
        .with_min_inner_size([440.0, 520.0])
        .with_title("mistrip");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-256.png"))
    {
        viewport = viewport.with_icon(icon);
    }

    eframe::run_native(
        "mistrip",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_covers_the_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), [0, 0, 255]);
    }

    #[test]
    fn hsv_zero_saturation_is_grey() {
        let [r, g, b] = hsv_to_rgb(0.5, 0.0, 1.0);
        assert_eq!((r, g), (b, b));
    }
}
