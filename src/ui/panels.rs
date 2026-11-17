//! Chrome around the journey: top bar, rail, transport, welcome, diagnostics, dialogs.

use super::fun::{
    card, check_row, finish_rail, fun_button, logo, pill, progress_track, rail_slot, round_button,
    small_button, soft, sticker_with, stone, FunButton, Icon, Kind, StoneState,
};
use super::mascot::Mood;
use super::steps;
use super::theme::{
    self, fmt_time, fredoka, fredoka_bold, nunito_x, CORAL, CORAL_TINT, ERR_TEXT, INK, INK_SOFT,
    MINT, MINT_TINT, OFFLINE, PAPER, SUN,
};
use super::{Gui, View};
use crate::audio::DeviceInfo;
use crate::config::Mode;
use eframe::egui::{self, Align, Color32, Layout, Margin, RichText, Stroke};

pub fn device_combo(
    ui: &mut egui::Ui,
    id: &str,
    cur: &mut Option<String>,
    devs: &[DeviceInfo],
) -> bool {
    let sel = match cur {
        None => "System default".to_string(),
        Some(c) => devs
            .iter()
            .find(|d| &d.id == c)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "(unavailable device)".into()),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width((ui.available_width() - 8.0).max(120.0))
        .selected_text(sel)
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(cur, None, "System default").changed();
            for d in devs {
                changed |= ui
                    .selectable_value(cur, Some(d.id.clone()), d.name.as_str())
                    .changed();
            }
        });
    changed
}

pub fn input_combo(
    ui: &mut egui::Ui,
    id: &str,
    cur: &mut Option<String>,
    names: &[String],
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(240.0)
        .selected_text(cur.clone().unwrap_or_else(|| "- not used -".into()))
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(cur, None, "- not used -").changed();
            for n in names {
                changed |= ui
                    .selectable_value(cur, Some(n.clone()), n.as_str())
                    .changed();
            }
        });
    changed
}

/// Small pill that is mint while "on".
pub fn toggle_chip(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    small_button(ui, label, if on { Kind::Mint } else { Kind::White })
}

/// Label + value row for a two-column grid.
pub fn value(ui: &mut egui::Ui, label: &str, v: impl Into<String>) {
    ui.label(soft(label));
    ui.label(RichText::new(v.into()).font(nunito_x(14.0)));
    ui.end_row();
}

// ------------------------------------------------------------------ top bar

pub fn top_bar(g: &mut Gui, ui: &mut egui::Ui) {
    let mut switch = false;
    ui.horizontal(|ui| {
        logo(ui, 26.0);
        ui.label(RichText::new("DuetSync").font(fredoka_bold(24.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let app = &g.app;
            if fun_button(ui, "Switch role", Kind::Ghost).clicked() {
                switch = true;
            }
            let host = app.cfg.mode == Mode::Host;
            if host {
                let (t, fill, dot) = if app.obs_connected && !app.cfg.ledger.applied.is_empty() {
                    ("OBS synced", PAPER, MINT)
                } else if app.obs_connected {
                    ("OBS connected", PAPER, MINT)
                } else {
                    ("OBS offline", PAPER, OFFLINE)
                };
                pill(ui, t, fill, Some(dot));
            }
            if !host {
                if let Some(e) = app.clock_estimate() {
                    pill(
                        ui,
                        &format!("Clock +-{:.1} ms", e.uncertainty_us / 1000.0),
                        PAPER,
                        Some(if e.uncertainty_us < 10_000.0 {
                            MINT
                        } else {
                            SUN
                        }),
                    );
                }
            }
            let (t, dot) = if app.connected() {
                ("Partner here", MINT)
            } else if app.host.is_some() || app.guest.is_some() {
                ("Waiting for partner", SUN)
            } else {
                ("Not connected", OFFLINE)
            };
            pill(ui, t, PAPER, Some(dot));
            pill(
                ui,
                if host { "Host" } else { "Guest" },
                if host { CORAL_TINT } else { MINT_TINT },
                None,
            );
        });
    });
    if switch {
        g.switch_role();
    }
}

// --------------------------------------------------------------------- rail

pub fn rail(g: &mut Gui, ui: &mut egui::Ui, done: &[bool]) {
    let mode = g.app.cfg.mode;
    let list = steps::steps(mode);
    let accent = if mode == Mode::Host { CORAL } else { MINT };
    let h = ui.available_height();
    let gap = if h < 470.0 { 6.0 } else { 16.0 };
    let selected = (g.view == View::Journey).then_some(g.step);
    let mut clicked = None;
    let mut diag = false;
    egui::Frame::new()
        .fill(PAPER)
        .stroke(Stroke::new(theme::STROKE_W, INK))
        .corner_radius(egui::CornerRadius::same(26))
        .inner_margin(Margin::same(22))
        .shadow(egui::Shadow {
            offset: [0, 5],
            blur: 0,
            spread: 0,
            color: INK,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height((h - 44.0 - 14.0).max(100.0));
            ui.spacing_mut().item_spacing.y = gap;
            ui.label(RichText::new("Your journey").font(fredoka_bold(19.0)));
            let slot = rail_slot(ui);
            let mut ys: Vec<(f32, f32)> = vec![];
            for (i, step) in list.iter().enumerate() {
                let state = if selected == Some(i) {
                    StoneState::Current
                } else if done[i] {
                    StoneState::Done
                } else {
                    StoneState::Todo
                };
                let r = stone(ui, state, i + 1, step.title(), accent);
                ys.push((r.rect.min.x + 22.0, r.rect.center().y));
                if r.clicked() {
                    clicked = Some(i);
                }
            }
            if let (Some(a), Some(b)) = (ys.first(), ys.last()) {
                finish_rail(ui, slot, a.0, a.1, b.1);
            }
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                if fun_button(ui, "Diagnostics", Kind::Ghost).clicked() {
                    diag = true;
                }
            });
        });
    if let Some(i) = clicked {
        g.step = i;
        g.view = View::Journey;
    }
    if diag {
        g.view = if g.view == View::Diagnostics {
            View::Journey
        } else {
            View::Diagnostics
        };
    }
}

// ---------------------------------------------------------------- transport

enum Tail {
    Countdown(String),
    Badge(String, Color32),
    Hint(String),
}

pub fn transport(g: &mut Gui, ui: &mut egui::Ui) {
    // Status / busy line above the pill.
    {
        let app = &g.app;
        if let Some(b) = &app.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(soft(b.clone()));
            });
        } else if let Some((ok, msg)) = &app.status {
            ui.add(
                egui::Label::new(RichText::new(msg).font(theme::nunito(13.0)).color(if *ok {
                    INK_SOFT
                } else {
                    ERR_TEXT
                }))
                .truncate(),
            );
        }
    }
    let host = g.app.cfg.mode == Mode::Host;
    let rate = g.app.track_rate();
    let yt = g.app.yt_mode;
    // A YouTube video's length is unknown: show the elapsed time only and no seeking.
    let total = if yt {
        0.0
    } else {
        g.app.track.as_ref().map(|t| t.frames).unwrap_or(0) as f64
    };
    let has_track = g.app.media_ready();
    let playing = g.app.playing();
    let pos = g.seek_drag.unwrap_or_else(|| g.app.position_frames());
    let countdown = g.app.countdown_s();
    let sync = g.app.sync_error_ms();
    let time_s = if yt {
        fmt_time(pos, rate)
    } else {
        format!("{} / {}", fmt_time(pos, rate), fmt_time(total, rate))
    };
    let tail = if let Some(c) = countdown {
        Tail::Countdown(format!("Starting in {}...", c.ceil() as u32))
    } else if let Some((e, _)) = sync.filter(|_| host) {
        if e.abs() <= 20.0 {
            Tail::Badge(format!("In time! {e:+.1} ms"), MINT)
        } else {
            Tail::Badge(format!("Off by {e:+.1} ms"), SUN)
        }
    } else if host {
        Tail::Hint(if !has_track {
            if yt {
                "Choose a video in step 3 to play".into()
            } else {
                "Load a song in step 3 to play".into()
            }
        } else if playing {
            "Playing".into()
        } else {
            "Ready to play".into()
        })
    } else {
        Tail::Hint(String::new())
    };
    let mut want_seek: Option<f64> = None;
    let mut play_clicked = false;
    let mut pause_clicked = false;
    let mut stop_clicked = false;
    let mut resync_clicked = false;
    sticker_with(
        ui,
        PAPER,
        255,
        Margin {
            left: 10,
            right: 18,
            top: 9,
            bottom: 9,
        },
        3,
        |ui| {
            ui.horizontal(|ui| {
                let sp = 14.0;
                ui.spacing_mut().item_spacing.x = sp;
                let measure = |ui: &egui::Ui, s: &str, f: egui::FontId| {
                    ui.painter().layout_no_wrap(s.to_owned(), f, INK).size().x
                };
                let mut used = measure(ui, &time_s, nunito_x(14.0)) + sp;
                used += match &tail {
                    Tail::Countdown(s) => measure(ui, s, fredoka_bold(18.0)) + sp,
                    Tail::Badge(s, _) => measure(ui, s, fredoka(13.0)) + 24.0 + sp,
                    Tail::Hint(s) if s.is_empty() => 0.0,
                    Tail::Hint(s) => measure(ui, s, theme::nunito(13.0)) + sp,
                };
                if host {
                    if playing {
                        if round_button(ui, Icon::Pause, true).clicked() {
                            pause_clicked = true;
                        }
                        used += 46.0 + sp;
                    } else {
                        if round_button(ui, Icon::Play, has_track).clicked() {
                            play_clicked = true;
                        }
                        used += 46.0 + sp;
                    }
                    let stop = FunButton::new("Stop", Kind::White)
                        .small()
                        .enabled(has_track);
                    used += stop.measure(ui).x + sp;
                    if ui.add(stop).clicked() {
                        stop_clicked = true;
                    }
                    if playing {
                        used += 74.0 + sp;
                    }
                } else {
                    let t = "Your host presses play. Just sing along!";
                    ui.label(RichText::new(t).font(fredoka(15.0)).color(INK_SOFT));
                    used += measure(ui, t, fredoka(15.0)) + sp;
                }
                let w = (ui.available_width() - used).max(60.0);
                let frac = if total > 0.0 {
                    (pos / total) as f32
                } else {
                    0.0
                };
                let (resp, new) = progress_track(
                    ui,
                    w,
                    frac,
                    if host { CORAL } else { MINT },
                    host && has_track && !yt,
                );
                if let Some(f) = new {
                    let p = f as f64 * total;
                    if resp.dragged() {
                        want_seek = Some(-p - 1.0); // negative: dragging, not released
                    }
                    if resp.drag_stopped() || resp.clicked() {
                        want_seek = Some(p);
                    }
                }
                ui.label(RichText::new(&time_s).font(nunito_x(14.0)));
                match &tail {
                    Tail::Countdown(s) => {
                        ui.label(RichText::new(s).font(fredoka_bold(18.0)));
                    }
                    Tail::Badge(s, c) => {
                        pill(ui, s, *c, None);
                    }
                    Tail::Hint(s) if s.is_empty() => {}
                    Tail::Hint(s) => {
                        ui.label(soft(s.clone()));
                    }
                }
                if host && playing && fun_button(ui, "Resync", Kind::Ghost).clicked() {
                    resync_clicked = true;
                }
            });
        },
    );
    let app = &mut g.app;
    if play_clicked {
        let from = app.paused_at;
        let cd = app.cfg.countdown_s;
        app.host_play(from, cd);
    }
    if pause_clicked {
        app.pause();
    }
    if stop_clicked {
        app.stop();
    }
    if resync_clicked {
        app.resync();
    }
    match want_seek {
        Some(p) if p < 0.0 => g.seek_drag = Some(-p - 1.0),
        Some(p) => {
            g.seek_drag = None;
            g.app.seek(p as u64);
        }
        None => {}
    }
}

// ------------------------------------------------------------------ welcome

fn bullets(ui: &mut egui::Ui, items: [&str; 3]) {
    ui.spacing_mut().item_spacing.y = 8.0;
    for t in items {
        check_row(ui, true, t);
    }
}

pub fn welcome(g: &mut Gui, ui: &mut egui::Ui) {
    let mut chosen = None;
    egui::Frame::new()
        .inner_margin(Margin::symmetric(28, 18))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                logo(ui, 28.0);
                ui.label(RichText::new("DuetSync").font(fredoka_bold(26.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("Sing together, anywhere.")
                            .font(fredoka(14.0))
                            .color(INK_SOFT),
                    );
                });
            });
        });
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(112.0 + 100.0 - 8.0, 112.0),
                    egui::Sense::hover(),
                );
                super::mascot::paint(
                    ui.painter(),
                    egui::Rect::from_min_size(rect.min, egui::vec2(112.0, 112.0)),
                    Mode::Host,
                    Mood::Happy,
                    0.0,
                    false,
                );
                super::mascot::paint(
                    ui.painter(),
                    egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + 104.0, rect.min.y + 12.0),
                        egui::vec2(100.0, 100.0),
                    ),
                    Mode::Guest,
                    Mood::Happy,
                    0.0,
                    false,
                );
                ui.label(RichText::new("Who's singing today?").font(fredoka_bold(46.0)));
                ui.label(
                    RichText::new("Pick your role. You can switch any time.")
                        .font(theme::nunito(16.0))
                        .color(INK_SOFT),
                );
                let total_w = 360.0 * 2.0 + 26.0;
                let pad = ((ui.available_width() - total_w) / 2.0).max(0.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 26.0;
                    ui.add_space(pad);
                    for (host, fill) in [(true, CORAL_TINT), (false, MINT_TINT)] {
                        ui.allocate_ui_with_layout(
                            egui::vec2(360.0, 10.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                sticker_with(ui, fill, 26, Margin::same(22), 5, |ui| {
                                    ui.spacing_mut().item_spacing.y = 12.0;
                                    ui.set_min_height(206.0);
                                    ui.label(
                                        RichText::new(if host {
                                            "I'm the host"
                                        } else {
                                            "I'm joining a friend"
                                        })
                                        .font(fredoka_bold(26.0)),
                                    );
                                    ui.label(
                                        RichText::new(if host {
                                            "I stream or record with OBS. My partner joins me."
                                        } else {
                                            "My partner sent me an invite. I just sing!"
                                        })
                                        .font(theme::nunito(15.0)),
                                    );
                                    bullets(
                                        ui,
                                        if host {
                                            [
                                                "Make an invite",
                                                "Measure the Discord delay",
                                                "Fix it in OBS with one click",
                                            ]
                                        } else {
                                            ["Paste the invite", "Load the same song", "Sing along"]
                                        },
                                    );
                                    let w = ui.available_width();
                                    let b = FunButton::new(
                                        if host {
                                            "Let's host!"
                                        } else {
                                            "Join the duet!"
                                        },
                                        if host { Kind::Coral } else { Kind::Mint },
                                    )
                                    .min_width(w);
                                    if ui.add(b).clicked() {
                                        chosen = Some(if host { Mode::Host } else { Mode::Guest });
                                    }
                                });
                            },
                        );
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("You both need Discord, headphones and the same .wav song file.")
                        .font(nunito_x(14.0))
                        .color(INK_SOFT),
                );
                ui.add_space(16.0);
            });
        });
    if let Some(m) = chosen {
        g.choose_role(m);
    }
}

// -------------------------------------------------------------- diagnostics

pub fn diagnostics(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    ui.label(RichText::new("Diagnostics").font(fredoka_bold(32.0)));
    ui.label(soft(
        "Numbers for troubleshooting. Nothing here needs to be touched to sing.",
    ));
    card(ui, Some("Resources (this process, measured)"), |ui| {
        egui::Grid::new("dres").num_columns(2).show(ui, |ui| {
            value(
                ui,
                "CPU",
                format!("{:.2} % of all cores", app.stats.cpu_percent),
            );
            value(
                ui,
                "Working set",
                format!("{:.1} MB", app.stats.working_set_mb),
            );
            value(
                ui,
                "Private bytes",
                format!("{:.1} MB", app.stats.private_mb),
            );
        });
    });
    card(ui, Some("Network"), |ui| {
        ui.label(RichText::new(app.network_summary()).monospace().size(12.0));
    });
    card(ui, Some("Audio"), |ui| {
        ui.label(RichText::new(app.audio_summary()).monospace().size(12.0));
    });
    card(ui, Some("Log"), |ui| {
        if small_button(ui, "Export diagnostics", Kind::White).clicked() {
            app.export_diagnostics();
        }
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for l in app.log.lines() {
                    ui.label(RichText::new(l).monospace().size(11.5).color(INK_SOFT));
                }
            });
    });
}

// ------------------------------------------------------------------ dialogs

fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(Stroke::new(theme::STROKE_W, INK))
        .corner_radius(egui::CornerRadius::same(22))
        .inner_margin(Margin::same(22))
        .shadow(egui::Shadow {
            offset: [0, 5],
            blur: 0,
            spread: 0,
            color: INK,
        })
}

pub fn dialogs(g: &mut Gui, ctx: &egui::Context) {
    let app = &mut g.app;
    if app.pending_apply.is_some() {
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("confirm_apply"))
            .frame(modal_frame())
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(RichText::new("Change OBS sync offsets?").font(fredoka_bold(22.0)));
                let p = app.pending_apply.as_ref().unwrap();
                egui::Grid::new("chg")
                    .num_columns(4)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        for h in ["Input", "Original", "Now", "New"] {
                            ui.label(soft(h));
                        }
                        ui.end_row();
                        for c in &p.changes {
                            ui.label(&c.input);
                            ui.label(format!("{} ms", c.baseline_ms));
                            ui.label(format!("{} ms", c.current_ms));
                            ui.label(
                                RichText::new(format!("{} ms", c.target_ms)).font(nunito_x(15.0)),
                            );
                            ui.end_row();
                        }
                    });
                for n in &p.notes {
                    check_row(ui, false, n);
                }
                ui.label(soft(
                    "Original values are saved. \"Restore original\" puts them back.",
                ));
                ui.horizontal(|ui| {
                    if fun_button(ui, "Apply", Kind::Coral).clicked() {
                        decision = Some(true);
                    }
                    if fun_button(ui, "Cancel", Kind::White).clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => app.confirm_apply(),
            Some(false) => app.pending_apply = None,
            None => {}
        }
    }
    if app.confirm_restore {
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("confirm_restore"))
            .frame(modal_frame())
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new("Restore original OBS offsets?").font(fredoka_bold(22.0)));
                for (k, v) in &app.cfg.ledger.baseline {
                    ui.label(format!("{k} -> {v} ms"));
                }
                ui.horizontal(|ui| {
                    if fun_button(ui, "Restore", Kind::Coral).clicked() {
                        decision = Some(true);
                    }
                    if fun_button(ui, "Cancel", Kind::White).clicked() {
                        decision = Some(false);
                    }
                });
            });
        if let Some(d) = decision {
            app.confirm_restore = false;
            if d {
                app.restore_offsets();
            }
        }
    }
}
