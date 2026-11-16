//! Dark charcoal + muted lavender theme and small custom widgets.

use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};

pub const BG: Color32 = Color32::from_rgb(0x1B, 0x1B, 0x1F);
pub const PANEL: Color32 = Color32::from_rgb(0x20, 0x20, 0x25);
pub const CARD: Color32 = Color32::from_rgb(0x26, 0x26, 0x2C);
pub const STROKE: Color32 = Color32::from_rgb(0x36, 0x36, 0x3F);
pub const TEXT: Color32 = Color32::from_rgb(0xE4, 0xE2, 0xEC);
pub const WEAK: Color32 = Color32::from_rgb(0x9B, 0x99, 0xA8);
pub const ACCENT: Color32 = Color32::from_rgb(0xA9, 0x9B, 0xD6);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x5E, 0x56, 0x82);
pub const OK: Color32 = Color32::from_rgb(0x86, 0xC9, 0xA3);
pub const WARN: Color32 = Color32::from_rgb(0xE3, 0xBD, 0x6F);
pub const ERR: Color32 = Color32::from_rgb(0xE0, 0x85, 0x85);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.extreme_bg_color = BG;
        v.faint_bg_color = Color32::from_rgb(0x2A, 0x2A, 0x31);
        v.code_bg_color = BG;
        v.window_stroke = Stroke::new(1.0, STROKE);
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_corner_radius = CornerRadius::same(8);
        let r = CornerRadius::same(5);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, STROKE);
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x30, 0x30, 0x38);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x30, 0x30, 0x38);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x3A, 0x38, 0x48);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x3A, 0x38, 0x48);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_DIM);
        v.widgets.active.bg_fill = ACCENT_DIM;
        v.widgets.active.weak_bg_fill = ACCENT_DIM;
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.animation_time = 0.0; // no decorative animation
    });
}

pub fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).strong().color(ACCENT).size(14.0));
            ui.add_space(4.0);
            add(ui)
        })
        .inner
}

pub fn dot(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, color);
        ui.label(egui::RichText::new(text).color(WEAK));
    });
}

pub fn value(ui: &mut egui::Ui, label: &str, v: impl Into<String>) {
    ui.label(egui::RichText::new(label).color(WEAK));
    ui.label(egui::RichText::new(v.into()).monospace().size(13.5));
    ui.end_row();
}

/// Horizontal peak meter, 0 dBFS at the right, -60 dBFS at the left.
pub fn meter(ui: &mut egui::Ui, peak: f32) {
    let w = ui.available_width().min(260.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 10.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, BG);
    let db = 20.0 * peak.max(1e-6).log10();
    let f = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let mut fill = rect;
    fill.set_width(rect.width() * f);
    let c = if db > -1.0 {
        ERR
    } else if db > -12.0 {
        WARN
    } else {
        ACCENT
    };
    p.rect_filled(fill, 3.0, c);
    ui.label(
        egui::RichText::new(if peak < 1e-6 {
            "  -inf dBFS".to_string()
        } else {
            format!("{db:6.1} dBFS")
        })
        .monospace()
        .color(WEAK),
    );
}

pub fn fmt_time(frames: f64, rate: u32) -> String {
    let s = (frames / rate.max(1) as f64).max(0.0);
    format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0)
}
