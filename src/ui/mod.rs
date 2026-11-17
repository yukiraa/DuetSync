//! Native egui/eframe (OpenGL via glow, no browser engine) user interface.
//!
//! A guided "journey": a welcome screen picks the role, then a rail of steps, a speech
//! bubble that says what to click next, the step's cards, and a transport bar.

mod fun;
mod mascot;
mod panels;
mod steps;
pub mod theme;

use crate::app::App;
use crate::config::{Config, Mode};
use eframe::egui::{self, Margin};
use fun::soft;
use fun::{bubble, fun_button, Kind};
use std::time::Duration;
use steps::Step;
use theme::CREAM;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Journey,
    Diagnostics,
}

/// `DUETSYNC_PREVIEW=welcome|host:N|guest:N`: start on that view (navigation only).
#[derive(Clone, Copy)]
enum Preview {
    Welcome,
    Step(Mode, usize),
}

fn parse_preview(v: &str) -> Option<Preview> {
    let v = v.trim().to_ascii_lowercase();
    if v == "welcome" {
        return Some(Preview::Welcome);
    }
    let (role, n) = v.split_once(':')?;
    let n: usize = n.trim().parse().ok()?;
    match role {
        "host" => Some(Preview::Step(Mode::Host, n)),
        "guest" => Some(Preview::Step(Mode::Guest, n)),
        _ => None,
    }
}

pub struct Gui {
    pub app: App,
    pub seek_drag: Option<f64>,
    pub view: View,
    /// Viewed step, 0-based.
    pub step: usize,
    pub advanced: bool,
    pub details: bool,
    pub copied_until: f64,
    prev_done: Vec<bool>,
    need_init: bool,
}

impl Gui {
    fn new(app: App, preview: Option<Preview>) -> Self {
        let mut g = Gui {
            app,
            seek_drag: None,
            view: View::Journey,
            step: 0,
            advanced: false,
            details: false,
            copied_until: 0.0,
            prev_done: vec![],
            need_init: true,
        };
        match preview {
            Some(Preview::Welcome) => g.app.cfg.onboarded = false,
            Some(Preview::Step(mode, n)) => {
                g.app.cfg.mode = mode;
                g.app.cfg.onboarded = true;
                let last = steps::steps(mode).len() - 1;
                g.step = n.saturating_sub(1).min(last);
                g.need_init = false;
                // `DUETSYNC_PREVIEW_YT=1`: show the YouTube variants (host side only).
                if mode == Mode::Host && std::env::var_os("DUETSYNC_PREVIEW_YT").is_some() {
                    g.app.set_youtube("dQw4w9WgXcQ");
                }
                // `DUETSYNC_PREVIEW_JOIN=<invite>`: the guest starts joining that invite
                // (used to screenshot the connection help).
                if mode == Mode::Guest {
                    if let Ok(invite) = std::env::var("DUETSYNC_PREVIEW_JOIN") {
                        g.app.join_input = invite;
                        g.app.join();
                    }
                }
            }
            None => {}
        }
        g
    }

    fn reset_journey(&mut self) {
        self.view = View::Journey;
        self.step = 0;
        self.advanced = false;
        self.details = false;
        self.prev_done.clear();
        self.need_init = true;
        self.app.confirmed_steps.clear();
        self.app.guest_source_ok = false;
        self.app.routing_ok = false;
    }

    /// Welcome screen: remember the role and start the journey.
    pub fn choose_role(&mut self, mode: Mode) {
        self.reset_journey();
        self.app.cfg.mode = mode;
        self.app.cfg.onboarded = true;
        self.app.save();
    }

    /// "Switch role": stop everything and go back to the welcome screen.
    pub fn switch_role(&mut self) {
        self.app.stop();
        self.app.stop_session();
        self.app.cfg.onboarded = false;
        self.app.save();
        self.reset_journey();
    }
}

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DuetSync")
            .with_inner_size([1080.0, 780.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    let preview = std::env::var("DUETSYNC_PREVIEW")
        .ok()
        .and_then(|v| parse_preview(&v));
    eframe::run_native(
        "DuetSync",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let app = if preview.is_some() {
                // Preview never writes the config.
                let (cfg, _) = Config::load();
                App::with_config(cfg, false)
            } else {
                App::new()
            };
            Ok(Box::new(Gui::new(app, preview)))
        }),
    )
    .map_err(|e| e.to_string())
}

impl eframe::App for Gui {
    /// Runs before every `ui` pass and also while the window is minimized (when `ui` is
    /// skipped), so session messages such as a guest's Play are still handled.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let active = self.app.tick();
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        let next = if minimized {
            Duration::from_secs(1)
        } else if active {
            Duration::from_millis(66)
        } else {
            Duration::from_millis(500)
        };
        ctx.request_repaint_after(next);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Drag & drop a WAV onto the window.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            self.app.load_track(p);
        }

        if !self.app.cfg.onboarded {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(CREAM))
                .show(ui, |ui| panels::welcome(self, ui));
        } else {
            self.journey(ui);
        }
        panels::dialogs(self, &ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.shutdown();
    }
}

impl Gui {
    /// Completion flags per step and the index of the first incomplete one. Also advances the
    /// viewed step when it has just become done.
    fn progress(&mut self) -> (Vec<bool>, usize) {
        let mode = self.app.cfg.mode;
        let list = steps::steps(mode);
        let done: Vec<bool> = list
            .iter()
            .enumerate()
            .map(|(i, s)| {
                *s != Step::Sing
                    && (steps::raw_done(&self.app, *s)
                        || self.app.confirmed_steps.contains(&(i as u8 + 1)))
            })
            .collect();
        let first = done.iter().position(|d| !d).unwrap_or(list.len() - 1);
        if self.prev_done.len() != done.len() {
            self.prev_done = done.clone();
        }
        if self.need_init {
            self.need_init = false;
            self.step = first;
        } else if self.view == View::Journey
            && self.step < done.len()
            && !self.prev_done[self.step]
            && done[self.step]
        {
            self.step = first;
        }
        self.prev_done = done.clone();
        self.step = self.step.min(list.len() - 1);
        (done, first)
    }

    fn journey(&mut self, ui: &mut egui::Ui) {
        let (done, _first) = self.progress();
        let mode = self.app.cfg.mode;
        let list = steps::steps(mode);
        let host = mode == Mode::Host;

        egui::Panel::top("top")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(CREAM)
                    .inner_margin(Margin::symmetric(24, 14)),
            )
            .show(ui, |ui| panels::top_bar(self, ui));
        egui::Panel::bottom("transport")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(CREAM).inner_margin(Margin {
                left: 24,
                right: 24,
                top: 8,
                bottom: 18,
            }))
            .show(ui, |ui| panels::transport(self, ui));
        egui::Panel::left("rail")
            .resizable(false)
            .show_separator_line(false)
            .exact_size(262.0 + 24.0 + 10.0)
            .frame(egui::Frame::new().fill(CREAM).inner_margin(Margin {
                left: 24,
                right: 10,
                top: 4,
                bottom: 0,
            }))
            .show(ui, |ui| panels::rail(self, ui, &done));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CREAM).inner_margin(Margin {
                left: 10,
                right: 24,
                top: 4,
                bottom: 0,
            }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 16.0;
                if self.view == View::Diagnostics {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if fun_button(ui, "Back to the journey", Kind::White).clicked() {
                                self.view = View::Journey;
                            }
                            panels::diagnostics(self, ui);
                            ui.add_space(8.0);
                        });
                    return;
                }
                let step = list[self.step];
                let viewed_done = done[self.step];

                // "Next up" bubble.
                let sentence = steps::bubble_sentence(&self.app, step, viewed_done);
                let mood = steps::bubble_mood(&self.app, step, viewed_done);
                let calibrating = self.app.calib.is_some()
                    || self
                        .app
                        .busy
                        .as_deref()
                        .is_some_and(|b| b.contains("calibration"));
                let bob = if calibrating {
                    mascot::bob_offset(ui)
                } else {
                    0.0
                };
                // Offer a jump only when an unfinished step blocks this one.
                let target = steps::blocker(&self.app, step)
                    .and_then(|b| list.iter().position(|s| *s == b))
                    .filter(|&i| !done[i]);
                let go = target.map(|i| {
                    (
                        format!("Go to step {}", i + 1),
                        if host { Kind::Coral } else { Kind::Mint },
                    )
                });
                let who = if host { Mode::Host } else { Mode::Guest };
                if bubble(
                    ui,
                    who,
                    mood,
                    bob,
                    &sentence,
                    go.as_ref().map(|(l, k)| (l.as_str(), *k)),
                ) {
                    if let Some(i) = target {
                        self.step = i;
                    }
                }

                // Footer: Back / Next.
                let mut nav = None;
                egui::Panel::bottom("nav")
                    .show_separator_line(false)
                    .frame(egui::Frame::new().inner_margin(Margin {
                        left: 0,
                        right: 0,
                        top: 6,
                        bottom: 14,
                    }))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if self.step > 0 && fun_button(ui, "Back", Kind::Ghost).clicked() {
                                nav = Some(self.step - 1);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if self.step + 1 < list.len() {
                                        let label =
                                            format!("Next: {}", list[self.step + 1].title());
                                        // The guest cannot do anything before joining.
                                        let can = !(step == Step::GuestJoin && !viewed_done);
                                        if fun::fun_button_if(ui, can, &label, Kind::White)
                                            .clicked()
                                        {
                                            nav = Some(self.step + 1);
                                        }
                                    }
                                },
                            );
                        });
                    });
                if let Some(to) = nav {
                    if to > self.step {
                        self.app.confirmed_steps.insert(self.step as u8 + 1);
                    }
                    self.step = to;
                    self.advanced = false;
                    self.details = false;
                }

                let avail_h = ui.available_height();
                let mut toggle_adv = false;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 6.0);
                        if step != Step::Sing {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(fun::caption(
                                    &format!("STEP {} OF {}", self.step + 1, list.len()),
                                    theme::INK_SOFT,
                                ));
                                ui.label(
                                    egui::RichText::new(step.title())
                                        .font(theme::fredoka_bold(32.0)),
                                );
                                ui.add(egui::Label::new(soft(step.blurb()).size(15.0)).wrap());
                            });
                        }
                        steps::content(self, ui, step, avail_h);
                        if step.has_advanced() {
                            let label = if self.advanced {
                                "- Hide advanced settings"
                            } else {
                                "+ Advanced settings"
                            };
                            if fun_button(ui, label, Kind::Ghost).clicked() {
                                toggle_adv = true;
                            }
                            if self.advanced {
                                steps::advanced(self, ui, step);
                            }
                        }
                        ui.add_space(8.0);
                    });
                if toggle_adv {
                    self.advanced = !self.advanced;
                }
            });
    }
}

/// Native Windows "Open file" dialog for WAV files.
#[cfg(windows)]
pub fn pick_wav() -> Option<std::path::PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::UI::Controls::Dialogs::*;
    let mut buf = vec![0u16; 1024];
    let filter: Vec<u16> = "WAV audio (*.wav)\0*.wav\0All files\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: windows::core::PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    unsafe {
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
            return Some(String::from_utf16_lossy(&buf[..n]).into());
        }
    }
    None
}

#[cfg(not(windows))]
pub fn pick_wav() -> Option<std::path::PathBuf> {
    None
}
