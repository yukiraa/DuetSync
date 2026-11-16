//! Native egui/eframe (OpenGL via glow, no browser engine) user interface.

mod panels;
pub mod theme;

use crate::app::App;
use crate::config::Mode;
use eframe::egui;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Calibrate,
    Obs,
    Player,
    Diagnostics,
}

pub struct Gui {
    pub app: App,
    pub tab: Tab,
    pub seek_drag: Option<f64>,
}

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DuetSync")
            .with_inner_size([1060.0, 720.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DuetSync",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(Gui {
                app: App::new(),
                tab: Tab::Calibrate,
                seek_drag: None,
            }))
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

        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| panels::top_bar(self, ui));
        egui::Panel::bottom("transport")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| panels::transport(self, ui));
        egui::Panel::left("left")
            .resizable(false)
            .exact_size(330.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    panels::session(self, ui);
                    ui.add_space(8.0);
                    panels::audio(self, ui);
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let host = self.app.cfg.mode == Mode::Host;
                    for (t, label) in [
                        (Tab::Calibrate, "Calibrate"),
                        (Tab::Obs, "OBS compensation"),
                        (Tab::Player, "Player"),
                        (Tab::Diagnostics, "Diagnostics"),
                    ] {
                        if t == Tab::Obs && !host {
                            continue;
                        }
                        ui.selectable_value(&mut self.tab, t, label);
                    }
                });
                if self.app.cfg.mode == Mode::Guest && self.tab == Tab::Obs {
                    self.tab = Tab::Calibrate;
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Calibrate => panels::calibrate(self, ui),
                    Tab::Obs => panels::obs(self, ui),
                    Tab::Player => panels::player(self, ui),
                    Tab::Diagnostics => panels::diagnostics(self, ui),
                });
            });
        panels::dialogs(self, &ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.shutdown();
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
