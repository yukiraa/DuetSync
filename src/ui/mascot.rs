//! The two mascots, painted with egui shapes (geometry from the 100x100 mockup SVGs).

use super::theme::{CORAL, INK, MINT, PAPER, PINK, SUN};
use crate::config::Mode;
use eframe::egui::{
    self,
    epaint::{CubicBezierShape, PathShape, PathStroke},
    Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    /// Open singing mouth.
    Happy,
    /// Sleepy lids, flat mouth, a "z".
    Waiting,
    /// Looking up, small "o" mouth.
    Listening,
    /// Closed happy eyes, big open mouth.
    Singing,
}

/// Vertical bob in points; only use it while playing or calibrating (the repaint is fast then).
pub fn bob_offset(ui: &Ui) -> f32 {
    (ui.input(|i| i.time) * 3.0).sin() as f32 * 3.0
}

/// Allocate a `size` square and paint a mascot into it.
pub fn mascot(ui: &mut Ui, role: Mode, mood: Mood, size: f32, bob: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    paint(ui.painter(), rect, role, mood, bob, false);
    resp
}

/// Same, washed out (used for the "waiting for your partner" placeholder).
pub fn mascot_faded(ui: &mut Ui, role: Mode, mood: Mood, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    paint(ui.painter(), rect, role, mood, 0.0, true);
    resp
}

struct Pen<'a> {
    painter: &'a Painter,
    o: Pos2,
    s: f32,
    faded: bool,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        egui::pos2(self.o.x + x * self.s, self.o.y + y * self.s)
    }

    fn c(&self, c: Color32) -> Color32 {
        if self.faded {
            c.lerp_to_gamma(PAPER, 0.5)
        } else {
            c
        }
    }

    fn w(&self, w: f32) -> f32 {
        (w * self.s).max(1.4)
    }

    fn circle(&self, cx: f32, cy: f32, r: f32, fill: Color32, stroke_w: f32) {
        let stroke = if stroke_w > 0.0 {
            Stroke::new(self.w(stroke_w), self.c(INK))
        } else {
            Stroke::NONE
        };
        self.painter
            .circle(self.pt(cx, cy), r * self.s, self.c(fill), stroke);
    }

    fn ellipse_points(&self, cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Pos2> {
        (0..32)
            .map(|k| {
                let a = k as f32 / 32.0 * std::f32::consts::TAU;
                self.pt(cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect()
    }

    fn ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32, fill: Color32) {
        let pts = self.ellipse_points(cx, cy, rx, ry);
        self.painter
            .add(PathShape::convex_polygon(pts, fill, Stroke::NONE));
    }

    fn poly(&self, pts: Vec<Pos2>, fill: Color32, stroke_w: f32) {
        let stroke = if stroke_w > 0.0 {
            PathStroke::new(self.w(stroke_w), self.c(INK))
        } else {
            PathStroke::NONE
        };
        self.painter.add(Shape::Path(PathShape {
            points: pts,
            closed: true,
            fill: self.c(fill),
            stroke,
        }));
    }

    /// Open stroked line with round caps.
    fn line(&self, pts: Vec<Pos2>, stroke_w: f32) {
        let w = self.w(stroke_w);
        let col = self.c(INK);
        if let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied()) {
            self.painter.circle_filled(a, w / 2.0, col);
            self.painter.circle_filled(b, w / 2.0, col);
        }
        self.painter.add(Shape::line(pts, PathStroke::new(w, col)));
    }

    fn quad(&self, a: (f32, f32), c: (f32, f32), b: (f32, f32)) -> Vec<Pos2> {
        (0..=16)
            .map(|k| {
                let t = k as f32 / 16.0;
                let u = 1.0 - t;
                self.pt(
                    u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
                    u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
                )
            })
            .collect()
    }

    fn cubic(&self, p: [(f32, f32); 4]) -> Vec<Pos2> {
        let pts = p.map(|(x, y)| self.pt(x, y));
        CubicBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, Stroke::NONE)
            .flatten(Some(0.1))
    }
}

pub fn paint(painter: &Painter, rect: Rect, role: Mode, mood: Mood, bob: f32, faded: bool) {
    let s = rect.width().min(rect.height()) / 100.0;
    let pen = Pen {
        painter,
        o: egui::pos2(rect.min.x, rect.min.y + bob),
        s,
        faded,
    };
    let host = role == Mode::Host;
    let body_col = if host { CORAL } else { MINT };

    // Body: four cubic segments flattened into one closed, convex path.
    let segs: [[(f32, f32); 4]; 4] = if host {
        [
            [(50.0, 10.0), (76.0, 9.0), (92.0, 28.0), (91.0, 53.0)],
            [(91.0, 53.0), (90.0, 79.0), (72.0, 94.0), (49.0, 93.0)],
            [(49.0, 93.0), (25.0, 92.0), (8.0, 77.0), (9.0, 52.0)],
            [(9.0, 52.0), (10.0, 27.0), (25.0, 11.0), (50.0, 10.0)],
        ]
    } else {
        [
            [(50.0, 14.0), (72.0, 14.0), (86.0, 30.0), (87.0, 52.0)],
            [(87.0, 52.0), (88.0, 76.0), (76.0, 94.0), (50.0, 94.0)],
            [(50.0, 94.0), (24.0, 94.0), (12.0, 76.0), (13.0, 52.0)],
            [(13.0, 52.0), (14.0, 30.0), (28.0, 14.0), (50.0, 14.0)],
        ]
    };
    let mut body = Vec::new();
    for seg in segs {
        let mut pts = pen.cubic(seg);
        if !body.is_empty() {
            pts.remove(0);
        }
        body.extend(pts);
    }
    body.pop(); // last point repeats the first
    pen.poly(body, body_col, 3.0);

    // Accessory: music note (host) or sprout with a bud (guest).
    if host {
        pen.line(
            vec![
                pen.pt(78.0, 18.0),
                pen.pt(78.0, 6.0),
                pen.pt(86.0, 4.0),
                pen.pt(86.0, 16.0),
            ],
            2.5,
        );
        pen.circle(75.5, 18.0, 3.0, INK, 0.0);
        pen.circle(83.5, 16.0, 3.0, INK, 0.0);
    } else {
        pen.line(
            pen.cubic([(50.0, 14.0), (50.0, 6.0), (56.0, 2.0), (63.0, 4.0)]),
            3.0,
        );
        pen.circle(64.0, 4.5, 3.5, SUN, 2.0);
    }

    if !faded {
        let cheek = Color32::from_white_alpha(115);
        pen.ellipse(27.0, 62.0, 6.0, 4.0, cheek);
        pen.ellipse(73.0, 62.0, 6.0, 4.0, cheek);
    }

    if mood == Mood::Singing {
        pen.line(pen.quad((27.0, 46.0), (37.0, 38.0), (47.0, 46.0)), 3.0);
        pen.line(pen.quad((53.0, 46.0), (63.0, 38.0), (73.0, 46.0)), 3.0);
        pen.ellipse(50.0, 68.0, 9.0, 10.0, pen.c(INK));
        pen.ellipse(50.0, 73.0, 5.0, 3.5, pen.c(PINK));
        return;
    }

    // White eyes.
    pen.circle(37.0, 46.0, 11.0, PAPER, 2.5);
    pen.circle(63.0, 46.0, 11.0, PAPER, 2.5);

    // Pupils (and glints) depend on the mood.
    let (px, py, glint) = match (mood, host) {
        (Mood::Listening, _) => (0.0, -4.0, true),
        (Mood::Waiting, true) => (2.0, 4.0, false),
        (Mood::Waiting, false) => (4.0, 2.0, false),
        (_, true) => (1.0, 1.0, true),
        (_, false) => (-1.0, 1.0, true),
    };
    for ex in [37.0, 63.0] {
        pen.circle(ex + px, 46.0 + py, 5.0, INK, 0.0);
        if glint && !faded {
            pen.circle(ex + px - 2.0, 46.0 + py - 2.0, 1.6, PAPER, 0.0);
        }
    }

    match mood {
        Mood::Happy => {
            let mouth = pen.quad((38.0, 64.0), (50.0, 80.0), (62.0, 64.0));
            pen.poly(mouth, INK, 2.5);
            let tongue = pen.quad((44.0, 71.0), (50.0, 76.0), (56.0, 71.0));
            pen.poly(tongue, PINK, 0.0);
        }
        Mood::Waiting => {
            // Sleepy lids: the top half of each eye, in the body colour.
            for ex in [37.0f32, 63.0] {
                let mut lid: Vec<Pos2> = (0..=16)
                    .map(|k| {
                        let a = std::f32::consts::PI * (1.0 - k as f32 / 16.0);
                        pen.pt(ex + 11.0 * a.cos(), 46.0 - 11.0 * a.sin())
                    })
                    .collect();
                lid.dedup();
                pen.poly(lid, body_col, 2.5);
            }
            pen.line(pen.quad((43.0, 68.0), (50.0, 70.0), (57.0, 68.0)), 3.0);
            if !faded {
                pen.line(
                    vec![
                        pen.pt(86.0, 24.0),
                        pen.pt(92.0, 24.0),
                        pen.pt(86.0, 31.0),
                        pen.pt(92.0, 31.0),
                    ],
                    2.0,
                );
            }
        }
        Mood::Listening => {
            pen.ellipse(50.0, 68.0, 5.0, 6.0, pen.c(INK));
        }
        Mood::Singing => {}
    }
}
