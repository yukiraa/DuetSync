//! Reusable "sticker" widgets: cards, pill buttons, stones, bubbles, meters.

use super::mascot::{mascot, Mood};
use super::theme::{
    self, fredoka, fredoka_bold, nunito, nunito_x, CORAL, DASHED, DISABLED, INK, INK_SOFT,
    INK_TINT, LILAC_TINT, MINT, OFFLINE, PAPER, SUN, SUN_TINT,
};
use crate::config::Mode;
use eframe::egui::{
    self, epaint::PathStroke, Align2, Color32, CornerRadius, CursorIcon, Frame, InnerResponse,
    Margin, Mesh, Pos2, Rect, Response, RichText, Role, Sense, Shadow, Shape, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2, Widget, WidgetInfo,
};

fn radius(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

fn hard_shadow(y: i8) -> Shadow {
    Shadow {
        offset: [0, y],
        blur: 0,
        spread: 0,
        color: INK,
    }
}

/// Muted secondary text.
pub fn soft(text: impl Into<String>) -> RichText {
    RichText::new(text.into())
        .font(nunito(14.0))
        .color(INK_SOFT)
}

/// Small letter-spaced caption ("NEXT UP", "STEP 1 OF 6").
pub fn caption(text: &str, color: Color32) -> RichText {
    RichText::new(text)
        .font(nunito_x(12.0))
        .extra_letter_spacing(1.2)
        .color(color)
}

/// Frame with fill, 2.5 px ink stroke, rounded corners and a hard (unblurred) shadow.
pub fn sticker<R>(
    ui: &mut Ui,
    fill: Color32,
    radius_px: u8,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    sticker_with(ui, fill, radius_px, Margin::symmetric(20, 18), 5, add)
}

/// [`sticker`] with explicit inner margin and shadow height. Fills the available width.
pub fn sticker_with<R>(
    ui: &mut Ui,
    fill: Color32,
    radius_px: u8,
    margin: Margin,
    shadow_y: i8,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(theme::STROKE_W, INK))
        .corner_radius(CornerRadius::same(radius_px))
        .inner_margin(margin)
        .shadow(hard_shadow(shadow_y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // `vertical` resets the layout: columns pass a justified one, which would stretch text.
            ui.vertical(add).inner
        })
}

/// White card with an optional Fredoka title.
pub fn card<R>(
    ui: &mut Ui,
    title: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    sticker(ui, PAPER, 22, |ui| {
        ui.spacing_mut().item_spacing.y = 11.0;
        if let Some(t) = title {
            ui.label(RichText::new(t).font(fredoka(17.0)));
        }
        add(ui)
    })
}

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Coral,
    Mint,
    White,
    Ghost,
}

pub struct FunButton<'a> {
    label: &'a str,
    kind: Kind,
    size: f32,
    pad: Vec2,
    shadow: f32,
    enabled: bool,
    min_w: f32,
}

impl<'a> FunButton<'a> {
    pub fn new(label: &'a str, kind: Kind) -> Self {
        let (size, pad, shadow) = match kind {
            Kind::Ghost => (15.0, egui::vec2(8.0, 6.0), 0.0),
            _ => (17.0, egui::vec2(22.0, 11.0), 4.0),
        };
        Self {
            label,
            kind,
            size,
            pad,
            shadow,
            enabled: true,
            min_w: 0.0,
        }
    }

    /// Stretch the button to at least this width.
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_w = w;
        self
    }

    /// Smaller secondary button (15 px text, 3 px shadow).
    pub fn small(mut self) -> Self {
        if self.kind != Kind::Ghost {
            self.size = 15.0;
            self.pad = egui::vec2(18.0, 8.0);
            self.shadow = 3.0;
        }
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Total size including the shadow strip, for manual placement.
    pub fn measure(&self, ui: &Ui) -> Vec2 {
        let g = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), fredoka(self.size), INK);
        let s = g.size() + self.pad * 2.0 + egui::vec2(0.0, self.shadow);
        egui::vec2(s.x.max(self.min_w), s.y)
    }
}

impl Widget for FunButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let enabled = self.enabled && ui.is_enabled();
        let size = self.measure(ui);
        let (rect, resp) = ui.allocate_exact_size(
            size,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, self.label));
        let hovered = resp.hovered() && enabled;
        let pressed = enabled && resp.is_pointer_button_down_on();
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
        let body_h = size.y - self.shadow;
        // Squish: pressed buttons sit 3 px lower with a 1 px shadow.
        let (dy, sh) = if pressed {
            (self.shadow - 1.0, 1.0)
        } else {
            (0.0, self.shadow)
        };
        let body = Rect::from_min_size(rect.min + egui::vec2(0.0, dy), egui::vec2(size.x, body_h));
        let g = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), fredoka(self.size), INK);
        let p = ui.painter();
        if self.kind == Kind::Ghost {
            let col = if hovered { INK } else { INK_SOFT };
            let g = p.layout_no_wrap(self.label.to_owned(), fredoka(self.size), fade(col));
            p.galley(body.center() - g.size() / 2.0, g, fade(col));
            return resp;
        }
        let fill = match self.kind {
            Kind::Coral => CORAL,
            Kind::Mint => MINT,
            _ => PAPER,
        };
        let fill = if hovered {
            fill.lerp_to_gamma(Color32::WHITE, 0.25)
        } else {
            fill
        };
        let r = radius(body_h / 2.0);
        if sh > 0.0 {
            p.rect_filled(body.translate(egui::vec2(0.0, sh)), r, fade(INK));
        }
        p.rect(
            body,
            r,
            fade(fill),
            Stroke::new(theme::STROKE_W, fade(INK)),
            StrokeKind::Inside,
        );
        p.galley(body.center() - g.size() / 2.0, g, fade(INK));
        resp
    }
}

/// Pill button (the common case).
pub fn fun_button(ui: &mut Ui, label: &str, kind: Kind) -> Response {
    ui.add(FunButton::new(label, kind))
}

pub fn fun_button_if(ui: &mut Ui, enabled: bool, label: &str, kind: Kind) -> Response {
    ui.add(FunButton::new(label, kind).enabled(enabled))
}

pub fn small_button(ui: &mut Ui, label: &str, kind: Kind) -> Response {
    ui.add(FunButton::new(label, kind).small())
}

pub fn small_button_if(ui: &mut Ui, enabled: bool, label: &str, kind: Kind) -> Response {
    ui.add(FunButton::new(label, kind).small().enabled(enabled))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
}

/// Round coral transport button with a painted play triangle or pause bars.
pub fn round_button(ui: &mut Ui, icon: Icon, enabled: bool) -> Response {
    let d = 46.0;
    let sh = 3.0;
    let enabled = enabled && ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(d, d + sh),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let label = if icon == Icon::Play { "Play" } else { "Pause" };
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let hovered = resp.hovered() && enabled;
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let pressed = enabled && resp.is_pointer_button_down_on();
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let (dy, s) = if pressed { (sh - 1.0, 1.0) } else { (0.0, sh) };
    let c = Pos2::new(rect.center().x, rect.min.y + d / 2.0 + dy);
    let p = ui.painter();
    p.circle_filled(c + egui::vec2(0.0, s), d / 2.0, fade(INK));
    let fill = if hovered {
        CORAL.lerp_to_gamma(Color32::WHITE, 0.25)
    } else {
        CORAL
    };
    p.circle(
        c,
        d / 2.0 - 1.25,
        fade(fill),
        Stroke::new(theme::STROKE_W, fade(INK)),
    );
    let stroke = Stroke::new(1.5, fade(INK));
    match icon {
        Icon::Play => {
            // 18x18 viewBox triangle, centred on the button.
            let o = c - egui::vec2(9.0, 9.0);
            let pts = [(5.0, 3.2), (5.0, 14.8), (15.0, 9.0)]
                .iter()
                .map(|&(x, y)| o + egui::vec2(x, y))
                .collect();
            p.add(Shape::convex_polygon(pts, fade(Color32::WHITE), stroke));
        }
        Icon::Pause => {
            for x in [3.5, 10.5] {
                let r = Rect::from_min_size(
                    c - egui::vec2(9.0, 9.0) + egui::vec2(x, 3.0),
                    egui::vec2(4.0, 12.0),
                );
                p.rect(
                    r,
                    radius(1.2),
                    fade(Color32::WHITE),
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
    }
    resp
}

/// The two-note logo (24x24 viewBox scaled to `size`).
pub fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let k = size / 24.0;
    let pt = |x: f32, y: f32| rect.min + egui::vec2(x * k, y * k);
    let p = ui.painter();
    p.add(Shape::line(
        vec![pt(9.0, 18.0), pt(9.0, 5.0), pt(20.0, 3.0), pt(20.0, 16.0)],
        PathStroke::new((2.5 * k).max(1.5), INK),
    ));
    for (x, y, c) in [(6.5, 18.0, CORAL), (17.5, 16.0, MINT)] {
        p.circle(pt(x, y), 3.0 * k, c, Stroke::new((2.0 * k).max(1.2), INK));
    }
}

// ------------------------------------------------------------------ pills

/// Status chip with an optional coloured dot.
pub fn pill(ui: &mut Ui, text: &str, fill: Color32, dot: Option<Color32>) -> Response {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), fredoka(13.0), INK);
    let dot_w = if dot.is_some() { 10.0 + 7.0 } else { 0.0 };
    let body = egui::vec2(g.size().x + dot_w + 24.0, g.size().y + 10.0);
    let (rect, resp) = ui.allocate_exact_size(body + egui::vec2(0.0, 3.0), Sense::hover());
    let r = Rect::from_min_size(rect.min, body);
    let p = ui.painter();
    let cr = radius(body.y / 2.0);
    p.rect_filled(r.translate(egui::vec2(0.0, 3.0)), cr, INK);
    p.rect(r, cr, fill, Stroke::new(2.0, INK), StrokeKind::Inside);
    let mut x = r.min.x + 12.0;
    if let Some(d) = dot {
        p.circle(
            Pos2::new(x + 5.0, r.center().y),
            4.25,
            d,
            Stroke::new(1.5, INK),
        );
        x += 17.0;
    }
    p.galley(Pos2::new(x, r.center().y - g.size().y / 2.0), g, INK);
    resp
}

// ----------------------------------------------------------------- stones

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StoneState {
    Done,
    Current,
    Todo,
}

fn star_points(center: Pos2, scale: f32) -> Vec<Pos2> {
    const PTS: [(f32, f32); 10] = [
        (12.0, 2.5),
        (14.9, 8.5),
        (21.5, 9.3),
        (16.6, 13.8),
        (17.9, 20.3),
        (12.0, 17.0),
        (6.1, 20.3),
        (7.4, 13.8),
        (2.5, 9.3),
        (9.1, 8.5),
    ];
    PTS.iter()
        .map(|&(x, y)| center + egui::vec2(x - 12.0, y - 11.5) * scale)
        .collect()
}

fn paint_star(p: &egui::Painter, center: Pos2, size: f32) {
    let pts = star_points(center, size / 24.0);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, SUN);
    for q in &pts {
        mesh.colored_vertex(*q, SUN);
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::closed_line(pts, PathStroke::new(1.6, INK)));
}

/// One stop on the journey rail. `accent` is coral (host) or mint (guest) for the current step.
pub fn stone(ui: &mut Ui, state: StoneState, n: usize, label: &str, accent: Color32) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 44.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let hovered = resp.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let c = Pos2::new(rect.min.x + 22.0, rect.center().y);
    let p = ui.painter();
    let text_pos = |g: &std::sync::Arc<egui::Galley>| {
        Pos2::new(rect.min.x + 56.0, rect.center().y - g.size().y / 2.0)
    };
    match state {
        StoneState::Done => {
            p.circle_filled(c + egui::vec2(0.0, 3.0), 18.0, INK);
            p.circle(c, 16.75, MINT, Stroke::new(theme::STROKE_W, INK));
            paint_star(p, c, 20.0);
            let g = p.layout_no_wrap(label.to_owned(), nunito_x(15.0), INK);
            p.galley(text_pos(&g), g, INK);
        }
        StoneState::Current => {
            p.circle_filled(c + egui::vec2(0.0, 4.0), 22.0, INK);
            p.circle(c, 20.75, accent, Stroke::new(theme::STROKE_W, INK));
            p.text(
                c,
                Align2::CENTER_CENTER,
                n.to_string(),
                fredoka_bold(20.0),
                INK,
            );
            let g = p.layout_no_wrap(label.to_owned(), fredoka(17.0), INK);
            p.galley(text_pos(&g), g, INK);
        }
        StoneState::Todo => {
            let pts: Vec<Pos2> = (0..=48)
                .map(|k| {
                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                    c + egui::vec2(a.cos(), a.sin()) * 16.75
                })
                .collect();
            p.circle_filled(c, 16.75, PAPER);
            p.add(Shape::Vec(Shape::dashed_line(
                &pts,
                Stroke::new(theme::STROKE_W, DASHED),
                5.0,
                4.0,
            )));
            p.text(
                c,
                Align2::CENTER_CENTER,
                n.to_string(),
                fredoka_bold(16.0),
                INK_SOFT,
            );
            let col = if hovered { INK } else { INK_SOFT };
            let g = p.layout_no_wrap(label.to_owned(), nunito_x(15.0), col);
            p.galley(text_pos(&g), g, col);
        }
    }
    resp
}

/// Dashed vertical connector painted behind the stones. Call before adding them, then
/// [`finish_rail`] after; `slot` reserves the paint order.
pub fn rail_slot(ui: &Ui) -> egui::layers::ShapeIdx {
    ui.painter().add(Shape::Noop)
}

pub fn finish_rail(ui: &Ui, slot: egui::layers::ShapeIdx, x: f32, y0: f32, y1: f32) {
    let line = Shape::dashed_line(
        &[Pos2::new(x, y0), Pos2::new(x, y1)],
        Stroke::new(3.0, DISABLED),
        6.0,
        5.0,
    );
    ui.painter().set(slot, Shape::Vec(line));
}

// -------------------------------------------------------------- check row

/// Mint tick circle (ok) or sun "!" circle (needs attention) with bold text.
pub fn check_row(ui: &mut Ui, ok: bool, text: &str) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
        let c = rect.center();
        let p = ui.painter();
        p.circle(c, 10.0, if ok { MINT } else { SUN }, Stroke::new(2.0, INK));
        if ok {
            let pts = [(6.5, 11.5), (9.5, 14.5), (15.5, 8.0)]
                .iter()
                .map(|&(x, y)| rect.min + egui::vec2(x, y))
                .collect();
            p.add(Shape::line(pts, PathStroke::new(2.2, INK)));
        } else {
            p.line_segment(
                [c + egui::vec2(0.0, -5.0), c + egui::vec2(0.0, 1.0)],
                Stroke::new(2.4, INK),
            );
            p.circle_filled(c + egui::vec2(0.0, 4.6), 1.4, INK);
        }
        ui.add(egui::Label::new(RichText::new(text).font(nunito_x(14.0))).wrap());
    });
}

// ----------------------------------------------------------------- bubble

/// Mascot + "NEXT UP" speech sticker. Returns true when the optional button was clicked.
pub fn bubble(
    ui: &mut Ui,
    role: Mode,
    mood: Mood,
    bob: f32,
    sentence: &str,
    button: Option<(&str, Kind)>,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        mascot(ui, role, mood, 84.0, bob);
        sticker_with(ui, LILAC_TINT, 22, Margin::symmetric(20, 14), 5, |ui| {
            let avail = ui.available_width();
            let btn = button.map(|(l, k)| FunButton::new(l, k));
            let btn_size = btn.as_ref().map(|b| b.measure(ui));
            let text_w = match btn_size {
                Some(s) => (avail - s.x - 14.0).max(120.0),
                None => avail,
            };
            let cap = ui
                .painter()
                .layout_job(egui::text::LayoutJob::single_section(
                    "NEXT UP".into(),
                    egui::TextFormat {
                        font_id: nunito_x(12.0),
                        color: INK_TINT,
                        extra_letter_spacing: 1.2,
                        ..Default::default()
                    },
                ));
            let sent = ui
                .painter()
                .layout(sentence.to_owned(), fredoka(20.0), INK, text_w);
            let text_h = cap.size().y + 2.0 + sent.size().y;
            let h = text_h.max(btn_size.map_or(0.0, |s| s.y));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, h), Sense::hover());
            let p = ui.painter();
            let top = rect.min.y + (h - text_h) / 2.0;
            let cap_h = cap.size().y;
            p.galley(Pos2::new(rect.min.x, top), cap, INK_TINT);
            p.galley(Pos2::new(rect.min.x, top + cap_h + 2.0), sent, INK);
            if let (Some(b), Some(s)) = (btn, btn_size) {
                let r = Rect::from_min_size(
                    Pos2::new(rect.max.x - s.x, rect.min.y + (h - s.y) / 2.0),
                    s,
                );
                clicked = ui
                    .scope_builder(UiBuilder::new().max_rect(r), |ui| ui.add(b))
                    .inner
                    .clicked();
            }
        });
    });
    clicked
}

// ------------------------------------------------------------ progress/meter

/// Draggable progress track. Returns the response and the fraction under the pointer while
/// the user presses or drags it (only if `interactive`).
pub fn progress_track(
    ui: &mut Ui,
    width: f32,
    frac: f32,
    fill: Color32,
    interactive: bool,
) -> (Response, Option<f32>) {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(width.max(40.0), 22.0),
        if interactive {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    if interactive && resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 14.0));
    let p = ui.painter();
    p.rect_filled(track, radius(7.0), SUN_TINT);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let x = track.min.x + track.width() * f;
        let fill_rect = Rect::from_min_max(track.min, Pos2::new(x, track.max.y));
        p.with_clip_rect(track).rect_filled(
            fill_rect.expand2(egui::vec2(0.0, 0.0)),
            radius(7.0),
            fill,
        );
        if f < 1.0 {
            p.line_segment(
                [Pos2::new(x, track.min.y), Pos2::new(x, track.max.y)],
                Stroke::new(2.0, INK),
            );
        }
    }
    p.rect_stroke(
        track,
        radius(7.0),
        Stroke::new(2.0, INK),
        StrokeKind::Inside,
    );
    let new = if interactive && (resp.is_pointer_button_down_on() || resp.drag_stopped()) {
        resp.interact_pointer_pos()
            .map(|pos| ((pos.x - track.min.x) / track.width()).clamp(0.0, 1.0))
    } else {
        None
    };
    (resp, new)
}

/// Level meter (0 dBFS right, -60 dBFS left) with a dB read-out.
pub fn meter(ui: &mut Ui, peak: f32) {
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 86.0).clamp(60.0, 260.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 14.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, radius(7.0), PAPER);
        let db = 20.0 * peak.max(1e-6).log10();
        let f = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        if f > 0.02 {
            let mut fill = rect;
            fill.set_width(rect.width() * f);
            let c = if db > -1.0 {
                CORAL
            } else if db > -12.0 {
                SUN
            } else {
                MINT
            };
            p.with_clip_rect(rect).rect_filled(fill, radius(7.0), c);
        }
        p.rect_stroke(rect, radius(7.0), Stroke::new(2.0, INK), StrokeKind::Inside);
        ui.label(
            RichText::new(if peak < 1e-6 {
                "-inf dB".to_string()
            } else {
                format!("{db:.0} dB")
            })
            .font(nunito_x(12.5))
            .color(INK_SOFT),
        );
    });
}

/// Rounded cream text field with a coloured border.
pub fn text_field(
    ui: &mut Ui,
    width: f32,
    buf: &mut dyn egui::TextBuffer,
    hint: &str,
    border: Color32,
    border_w: f32,
) -> Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 10.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            Frame::new()
                .fill(theme::CREAM)
                .stroke(Stroke::new(border_w, border))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::symmetric(14, 11))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::TextEdit::singleline(buf)
                            .frame(Frame::NONE)
                            .hint_text(RichText::new(hint).color(OFFLINE.gamma_multiply(1.0)))
                            .font(nunito_x(15.0))
                            .desired_width(f32::INFINITY),
                    )
                })
                .inner
        },
    )
    .inner
}
