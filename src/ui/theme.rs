//! "Sticker" theme: palette, bundled fonts and egui visuals.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle,
};
use std::sync::Arc;

pub const CREAM: Color32 = Color32::from_rgb(0xFF, 0xF4, 0xE4);
pub const PAPER: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const INK: Color32 = Color32::from_rgb(0x26, 0x20, 0x3B);
pub const INK_SOFT: Color32 = Color32::from_rgb(0x6B, 0x63, 0x85);
/// Slightly darker secondary text used on tinted cards ("NEXT UP", "STEP X OF N" on tints).
pub const INK_TINT: Color32 = Color32::from_rgb(0x5A, 0x53, 0x73);
pub const CORAL: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5B);
pub const CORAL_TINT: Color32 = Color32::from_rgb(0xFF, 0xD9, 0xD3);
pub const MINT: Color32 = Color32::from_rgb(0x3F, 0xCB, 0x9B);
pub const MINT_TINT: Color32 = Color32::from_rgb(0xCD, 0xF4, 0xE5);
pub const LILAC_TINT: Color32 = Color32::from_rgb(0xEC, 0xE6, 0xFF);
pub const SUN_TINT: Color32 = Color32::from_rgb(0xFF, 0xF0, 0xC2);
pub const SUN: Color32 = Color32::from_rgb(0xFF, 0xC9, 0x4A);
pub const DASHED: Color32 = Color32::from_rgb(0x8A, 0x84, 0xA0);
pub const DISABLED: Color32 = Color32::from_rgb(0xB7, 0xB1, 0xC9);
pub const OFFLINE: Color32 = Color32::from_rgb(0xCF, 0xCA, 0xDB);
/// Error text on cream/white (coral is too light for small text).
pub const ERR_TEXT: Color32 = Color32::from_rgb(0xC2, 0x3B, 0x2E);
pub const PINK: Color32 = Color32::from_rgb(0xFF, 0x8F, 0xA3);

pub const STROKE_W: f32 = 2.5;

fn fam(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}

/// Fredoka SemiBold (buttons, bubbles, card titles).
pub fn fredoka(size: f32) -> FontId {
    FontId::new(size, fam("fredoka"))
}

/// Fredoka Bold (headings).
pub fn fredoka_bold(size: f32) -> FontId {
    FontId::new(size, fam("fredoka-bold"))
}

/// Nunito Regular (body).
pub fn nunito(size: f32) -> FontId {
    FontId::new(size, fam("nunito"))
}

/// Nunito ExtraBold (labels, emphasis).
pub fn nunito_x(size: f32) -> FontId {
    FontId::new(size, fam("nunito-x"))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let bundled: [(&str, &'static [u8]); 4] = [
        (
            "Fredoka-SemiBold",
            include_bytes!("../../assets/fonts/Fredoka-SemiBold.ttf"),
        ),
        (
            "Fredoka-Bold",
            include_bytes!("../../assets/fonts/Fredoka-Bold.ttf"),
        ),
        (
            "Nunito-Regular",
            include_bytes!("../../assets/fonts/Nunito-Regular.ttf"),
        ),
        (
            "Nunito-ExtraBold",
            include_bytes!("../../assets/fonts/Nunito-ExtraBold.ttf"),
        ),
    ];
    for (name, bytes) in bundled {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    // egui's default proportional fonts stay behind ours as glyph fallbacks.
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (family, font) in [
        (FontFamily::Proportional, "Nunito-Regular"),
        (fam("nunito"), "Nunito-Regular"),
        (fam("nunito-x"), "Nunito-ExtraBold"),
        (fam("fredoka"), "Fredoka-SemiBold"),
        (fam("fredoka-bold"), "Fredoka-Bold"),
    ] {
        let mut list = vec![font.to_owned()];
        list.extend(fallback.iter().cloned());
        fonts.families.insert(family, list);
    }
    ctx.set_fonts(fonts);
}

fn hard_shadow(y: i8) -> Shadow {
    Shadow {
        offset: [0, y],
        blur: 0,
        spread: 0,
        color: INK,
    }
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(|s| {
        s.visuals = egui::Visuals::light();
        let v = &mut s.visuals;
        v.panel_fill = CREAM;
        v.window_fill = PAPER;
        v.extreme_bg_color = CREAM;
        v.faint_bg_color = Color32::from_rgb(0xFB, 0xF7, 0xEE);
        v.code_bg_color = CREAM;
        v.hyperlink_color = INK;
        v.window_stroke = Stroke::new(STROKE_W, INK);
        v.window_corner_radius = CornerRadius::same(22);
        v.menu_corner_radius = CornerRadius::same(12);
        v.window_shadow = hard_shadow(5);
        v.popup_shadow = hard_shadow(4);
        v.selection.bg_fill = LILAC_TINT;
        v.selection.stroke = Stroke::new(1.5, INK);
        v.slider_trailing_fill = true;
        v.striped = false;

        let r = CornerRadius::same(12);
        v.widgets.noninteractive.bg_fill = PAPER;
        v.widgets.noninteractive.weak_bg_fill = PAPER;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.5, OFFLINE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.noninteractive.corner_radius = r;
        let states = [
            (&mut v.widgets.inactive, PAPER),
            (&mut v.widgets.hovered, Color32::from_rgb(0xFF, 0xF9, 0xEE)),
            (&mut v.widgets.active, LILAC_TINT),
            (&mut v.widgets.open, LILAC_TINT),
        ];
        for (w, fill) in states {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(2.0, INK);
            w.fg_stroke = Stroke::new(1.5, INK);
            w.corner_radius = r;
            w.expansion = 0.0;
        }

        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size.y = 32.0;
        s.spacing.slider_width = 160.0;
        s.spacing.window_margin = Margin::same(20);
        s.animation_time = 0.0; // only the mascot bobs
                                // Solid scroll bars: visible whenever content overflows, so clipped cards
                                // never look broken (floating bars only appear on hover).
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        s.text_styles.insert(TextStyle::Body, nunito(15.0));
        s.text_styles.insert(TextStyle::Button, nunito_x(15.0));
        s.text_styles.insert(TextStyle::Small, nunito(12.5));
        s.text_styles.insert(TextStyle::Heading, fredoka_bold(22.0));
    });
}

/// `m:ss` for a frame count.
pub fn fmt_time(frames: f64, rate: u32) -> String {
    let s = (frames / rate.max(1) as f64).max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
