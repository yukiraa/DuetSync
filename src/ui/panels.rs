//! Panel contents. Every control calls a real `App` method; no placeholder data.

use super::theme::{self, card, dot, fmt_time, meter, value};
use super::{Gui, Tab};
use crate::audio::{CaptureSource, DeviceInfo};
use crate::calibration::CalibrationStatus;
use crate::clock::now_us;
use crate::config::{GuestAudio, Mode};
use crate::obs::client::{OFFSET_MAX_MS, OFFSET_MIN_MS, VIDEO_DELAY_MAX_MS};
use crate::obs::routing_warnings;
use crate::playback::core::State;
use eframe::egui::{self, RichText};

fn weak(s: impl Into<String>) -> RichText {
    RichText::new(s.into()).color(theme::WEAK)
}

fn device_combo(
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
        .width(ui.available_width() - 6.0)
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

fn input_combo(ui: &mut egui::Ui, id: &str, cur: &mut Option<String>, names: &[String]) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(220.0)
        .selected_text(cur.clone().unwrap_or_else(|| "— not used —".into()))
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(cur, None, "— not used —").changed();
            for n in names {
                changed |= ui
                    .selectable_value(cur, Some(n.clone()), n.as_str())
                    .changed();
            }
        });
    changed
}

pub fn top_bar(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("DuetSync")
                .size(20.0)
                .strong()
                .color(theme::ACCENT),
        );
        ui.label(weak("Sing together, anywhere."));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut mode = app.cfg.mode;
            ui.selectable_value(&mut mode, Mode::Guest, "Guest");
            ui.selectable_value(&mut mode, Mode::Host, "Host");
            if mode != app.cfg.mode {
                app.stop();
                app.stop_session();
                app.cfg.mode = mode;
                app.save();
            }
            ui.add_space(16.0);
            if app.cfg.mode == Mode::Host {
                let (c, t) = if app.obs_connected {
                    (theme::OK, "OBS")
                } else {
                    (theme::WEAK, "OBS offline")
                };
                dot(ui, c, t);
            } else {
                match app.clock_estimate() {
                    Some(e) => dot(
                        ui,
                        if e.uncertainty_us < 10_000.0 {
                            theme::OK
                        } else {
                            theme::WARN
                        },
                        &format!("clock ±{:.1} ms", e.uncertainty_us / 1000.0),
                    ),
                    None => dot(ui, theme::WEAK, "clock not synced"),
                }
            }
            let (c, t) = if app.connected() {
                (theme::OK, "partner connected")
            } else if app.host.is_some() || app.guest.is_some() {
                (theme::WARN, "waiting for partner")
            } else {
                (theme::WEAK, "no session")
            };
            dot(ui, c, t);
        });
    });
}

pub fn transport(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    let host = app.cfg.mode == Mode::Host;
    let rate = app.track_rate();
    let total = app.track.as_ref().map(|t| t.frames).unwrap_or(0) as f64;
    ui.horizontal(|ui| {
        let has_track = app.track.is_some();
        if host {
            if app.playing() {
                if ui.button("Pause").clicked() {
                    app.pause();
                }
            } else if ui
                .add_enabled(has_track, egui::Button::new("Play"))
                .on_hover_text("Starts after the countdown on both computers")
                .clicked()
            {
                let from = app.paused_at;
                let cd = app.cfg.countdown_s;
                app.host_play(from, cd);
            }
            if ui
                .add_enabled(has_track, egui::Button::new("Stop"))
                .clicked()
            {
                app.stop();
            }
        } else {
            ui.label(weak("Host controls playback"));
        }
        let pos = g.seek_drag.unwrap_or_else(|| app.position_frames());
        let mut p = pos;
        let slider = ui.add_enabled(
            host && has_track,
            egui::Slider::new(&mut p, 0.0..=total.max(1.0))
                .show_value(false)
                .trailing_fill(true),
        );
        if slider.dragged() {
            g.seek_drag = Some(p);
        }
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            g.seek_drag = None;
            app.seek(p as u64);
        }
        ui.label(
            RichText::new(format!(
                "{} / {}",
                fmt_time(pos, rate),
                fmt_time(total, rate)
            ))
            .monospace(),
        );
        if let Some(c) = app.countdown_s() {
            ui.label(
                RichText::new(format!("starting in {c:.1} s"))
                    .color(theme::WARN)
                    .strong(),
            );
        }
        if let Some((e, unc)) = app.sync_error_ms() {
            let c = if e.abs() <= 20.0 {
                theme::OK
            } else {
                theme::WARN
            };
            ui.label(RichText::new(format!("guest timeline {e:+.1} ms (±{unc:.1})")).color(c));
        }
    });
    if let Some((ok, msg)) = &app.status {
        ui.label(RichText::new(msg).color(if *ok { theme::WEAK } else { theme::ERR }));
    }
    if let Some(b) = &app.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(weak(b.clone()));
        });
    }
}

pub fn session(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    card(ui, "Session", |ui| {
        if app.cfg.mode == Mode::Host {
            if app.host.is_none() {
                ui.horizontal(|ui| {
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut app.cfg.listen_port).range(1024..=65535));
                    ui.label("Code valid");
                    ui.add(
                        egui::DragValue::new(&mut app.cfg.join_code_minutes)
                            .range(1..=1440)
                            .suffix(" min"),
                    );
                });
                if ui.button("Create session").clicked() {
                    app.start_host();
                    app.save();
                }
                ui.label(weak(
                    "LAN or private VPN (e.g. Tailscale). Do not port-forward to the internet.",
                ));
            } else {
                if let Some(code) = app.join_code.clone() {
                    ui.label(weak(
                        "Send your partner one of these invites (private chat only):",
                    ));
                    for h in app.invite_hosts.clone() {
                        let invite = format!("{h}#{code}");
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&invite).monospace().size(12.0));
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(invite.clone());
                            }
                        });
                    }
                    let left = (app.code_expires_us - now_us()) / 60_000_000;
                    ui.label(weak(if left >= 0 {
                        format!(
                            "New joins allowed for {left} more min (reconnects always allowed)."
                        )
                    } else {
                        "Code expired for new joins.".into()
                    }));
                }
                ui.horizontal(|ui| {
                    if ui.button("New code").clicked() {
                        app.regenerate_code();
                    }
                    if ui.button("End session").clicked() {
                        app.stop_session();
                    }
                });
            }
        } else if app.guest.is_none() {
            ui.label("Invite from host");
            ui.add(
                egui::TextEdit::singleline(&mut app.join_input)
                    .hint_text("100.64.1.2:47800#XXXXX-XXXXX-...")
                    .desired_width(f32::INFINITY),
            );
            if ui.button("Join session").clicked() {
                app.join();
                app.save();
            }
        } else {
            ui.label(if app.connected() {
                format!("Connected to {}", app.peer.clone().unwrap_or_default())
            } else {
                "Connecting / reconnecting...".into()
            });
            if ui.button("Leave session").clicked() {
                app.stop_session();
            }
        }
        if let Some(e) = app.clock_estimate() {
            egui::Grid::new("clk").num_columns(2).show(ui, |ui| {
                value(
                    ui,
                    "Round trip",
                    format!(
                        "{:.1} ms (min {:.1})",
                        e.rtt_median_us / 1000.0,
                        e.rtt_min_us / 1000.0
                    ),
                );
                value(
                    ui,
                    "Clock uncertainty",
                    format!("±{:.1} ms", e.uncertainty_us / 1000.0),
                );
                value(ui, "Clock drift", format!("{:+.1} ppm", e.drift_ppm));
            });
        } else if let Some(r) = &app.remote {
            egui::Grid::new("clk").num_columns(2).show(ui, |ui| {
                value(
                    ui,
                    "Round trip",
                    format!("{:.1} ms", r.rtt_us as f64 / 1000.0),
                );
                value(
                    ui,
                    "Guest clock",
                    format!("±{:.1} ms", r.clock_uncertainty_us as f64 / 1000.0),
                );
            });
        }
    });
}

pub fn audio(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    card(ui, "Audio", |ui| {
        ui.label(weak("Instrumental output (your headphones)"));
        let mut out = app.cfg.output_device.clone();
        if device_combo(ui, "out", &mut out, &app.outputs) {
            app.change_output_device(out);
            app.save();
        }
        meter(ui, app.out_peak);
        ui.add_space(4.0);
        if app.cfg.mode == Mode::Host {
            ui.label(weak("Your microphone"));
            if device_combo(ui, "mic", &mut app.cfg.mic_device, &app.inputs) {
                app.monitor = None;
                app.save();
            }
            ui.label(weak("Guest voice source (Discord output)"));
            let mut kind = match &app.cfg.guest_audio {
                GuestAudio::DiscordProcess => 0,
                GuestAudio::OutputLoopback(_) => 1,
                GuestAudio::Input(_) => 2,
            };
            let before = kind;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut kind, 0, "Discord app");
                ui.selectable_value(&mut kind, 1, "Output device");
                ui.selectable_value(&mut kind, 2, "Input device");
            });
            if kind != before {
                app.cfg.guest_audio = match kind {
                    0 => GuestAudio::DiscordProcess,
                    1 => GuestAudio::OutputLoopback(None),
                    _ => GuestAudio::Input(None),
                };
                app.save();
            }
            let (outs, ins) = (app.outputs.clone(), app.inputs.clone());
            match &mut app.cfg.guest_audio {
                GuestAudio::DiscordProcess => {
                    ui.label(weak(match app.discord.first() {
                        Some(d) => format!("Found {} (pid {})", d.name, d.pid),
                        None => "Discord not detected (start it, then Refresh)".into(),
                    }));
                }
                GuestAudio::OutputLoopback(id) => {
                    device_combo(ui, "gout", id, &outs);
                }
                GuestAudio::Input(id) => {
                    device_combo(ui, "gin", id, &ins);
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Test guest source").clicked() {
                    app.test_guest_capture();
                }
                let mon = app.monitor.is_some();
                if ui.selectable_label(mon, "Mic level").clicked() {
                    let src = CaptureSource::Input(app.cfg.mic_device.clone());
                    app.toggle_monitor(Some(src));
                }
            });
        } else {
            ui.label(weak("Calibration signal output"));
            if device_combo(ui, "cal", &mut app.cfg.calib_output_device, &app.outputs) {
                app.save();
            }
            ui.label(weak(
                "Speakers your mic can hear, or a virtual cable set as Discord's input.",
            ));
            ui.checkbox(
                &mut app.cfg.use_guest_reference,
                "Verify with my input (recommended)",
            );
            if app.cfg.use_guest_reference {
                device_combo(ui, "ref", &mut app.cfg.guest_reference_input, &app.inputs);
            }
            ui.horizontal(|ui| {
                if ui.button("Test routing").clicked() {
                    app.save();
                    app.test_routing();
                }
                let mon = app.monitor.is_some();
                if ui.selectable_label(mon, "Input level").clicked() {
                    let src = CaptureSource::Input(app.cfg.guest_reference_input.clone());
                    app.toggle_monitor(Some(src));
                }
            });
        }
        if app.monitor.is_some() {
            meter(ui, app.monitor_peak);
        }
        if ui.small_button("Refresh devices").clicked() {
            app.refresh_devices();
        }
    });
}

pub fn calibrate(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    if app.cfg.mode == Mode::Guest {
        card(ui, "Calibration (guest)", |ui| {
            ui.label("The host starts calibration. Your computer then plays short test sweeps that travel through your Discord microphone path to the host.");
            ui.label(weak("Before calibrating in Discord: turn OFF Noise Suppression (Krisp) and Echo Cancellation, and set input sensitivity low enough that the sweeps are transmitted. Turn them back on afterwards if you like."));
            ui.label(weak("Playing the sweep into headphones does NOT reach your microphone: use speakers near the mic, or a virtual cable that Discord uses as input."));
            if let Some(n) = &app.guest_calib_note {
                ui.label(RichText::new(n).color(theme::ACCENT));
            }
            ui.horizontal(|ui| {
                ui.label("Test signal level");
                ui.add(egui::Slider::new(&mut app.cfg.chirp_amplitude, 0.05..=0.9));
            });
        });
        return;
    }
    card(ui, "Assisted calibration", |ui| {
        ui.label("Measures guest voice latency: guest-mic -> Discord -> this PC, using test sweeps scheduled on the shared clock.");
        ui.horizontal(|ui| {
            ui.label("Signals");
            ui.add(egui::DragValue::new(&mut app.cfg.chirp_count).range(4..=16));
            ui.label("Spacing");
            ui.add(
                egui::DragValue::new(&mut app.cfg.chirp_spacing_ms)
                    .range(800.0..=3000.0)
                    .suffix(" ms"),
            );
            ui.label("Level");
            ui.add(
                egui::DragValue::new(&mut app.cfg.chirp_amplitude)
                    .range(0.05..=0.9)
                    .speed(0.01),
            );
        });
        let can = app.can_calibrate();
        ui.horizontal(|ui| {
            let b = ui.add_enabled(can.is_ok(), egui::Button::new("Calibrate"));
            if b.clicked() {
                app.save();
                app.start_calibration();
            }
            if let Err(why) = &can {
                ui.label(weak(why.clone()));
            }
        });
        if let Some(p) = app.calibration_progress() {
            ui.add(egui::ProgressBar::new(p as f32).text("measuring... keep quiet"));
        }
    });
    ui.add_space(8.0);
    card(ui, "Result", |ui| match &app.last_calib {
        None => {
            ui.label(weak("No measurement yet."));
        }
        Some(o) => {
            let r = &o.result;
            let (c, s) = match r.status {
                CalibrationStatus::Valid => (theme::OK, "Valid"),
                CalibrationStatus::LowConfidence => (theme::WARN, "Low confidence"),
                CalibrationStatus::Invalid => (theme::ERR, "Rejected"),
            };
            ui.label(RichText::new(s).color(c).strong());
            egui::Grid::new("res").num_columns(2).show(ui, |ui| {
                value(
                    ui,
                    "Relative latency",
                    r.median_ms
                        .map(|m| format!("{m:.1} ms"))
                        .unwrap_or("—".into()),
                );
                value(
                    ui,
                    "Uncertainty (~95 %)",
                    r.uncertainty_ms
                        .map(|m| format!("±{m:.1} ms"))
                        .unwrap_or("—".into()),
                );
                value(
                    ui,
                    "Jitter",
                    r.jitter_ms
                        .map(|m| format!("{m:.1} ms"))
                        .unwrap_or("—".into()),
                );
                value(
                    ui,
                    "Range",
                    match (r.min_ms, r.max_ms) {
                        (Some(a), Some(b)) => format!("{a:.1} … {b:.1} ms"),
                        _ => "—".into(),
                    },
                );
                value(
                    ui,
                    "Signals accepted",
                    format!("{}/{} ({} outliers)", r.accepted, r.total, r.outliers),
                );
                value(ui, "Confidence", format!("{:.0} %", r.confidence * 100.0));
                value(
                    ui,
                    "Clock uncertainty",
                    format!("±{:.1} ms", r.clock_uncertainty_ms),
                );
                value(
                    ui,
                    "Reference",
                    if o.reference {
                        "guest input timestamps"
                    } else {
                        "scheduled playback times"
                    },
                );
            });
            for m in &r.messages {
                ui.label(RichText::new(format!("• {m}")).color(theme::WARN));
            }
            ui.collapsing("Per-signal results", |ui| {
                for (k, p) in r.per_chirp.iter().enumerate() {
                    ui.label(match p {
                        Ok(v) => format!("#{k}: {v:.2} ms"),
                        Err(e) => format!("#{k}: {e}"),
                    });
                }
            });
        }
    });
    ui.add_space(8.0);
    card(ui, "Guest latency used for compensation", |ui| {
        ui.checkbox(&mut app.use_measured, "Use measured value when valid");
        ui.horizontal(|ui| {
            ui.label("Manual latency");
            ui.add(
                egui::DragValue::new(&mut app.cfg.manual_guest_latency_ms)
                    .range(0.0..=2000.0)
                    .suffix(" ms"),
            );
            ui.label("Fine tune");
            ui.add(
                egui::DragValue::new(&mut app.cfg.fine_tune_ms)
                    .range(-200.0..=200.0)
                    .speed(0.5)
                    .suffix(" ms"),
            );
        });
        if let Some(l) = app.guest_latency_ms() {
            ui.label(RichText::new(format!("In use: {l:.1} ms")).strong());
        }
        for s in app.staleness() {
            ui.label(RichText::new(format!("• {s}")).color(theme::WEAK));
        }
    });
}

pub fn obs(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    card(ui, "OBS WebSocket", |ui| {
        ui.horizontal(|ui| {
            ui.label("Host");
            ui.add(egui::TextEdit::singleline(&mut app.cfg.obs_host).desired_width(110.0));
            ui.label("Port");
            ui.add(egui::DragValue::new(&mut app.cfg.obs_port));
            ui.label("Password");
            ui.add(
                egui::TextEdit::singleline(&mut app.obs_password)
                    .password(true)
                    .desired_width(130.0),
            );
        });
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut app.cfg.remember_obs_password,
                "Remember password (stored locally)",
            );
            if app.obs_connected {
                if ui.button("Refresh").clicked() {
                    app.obs_refresh();
                }
                if ui.button("Disconnect").clicked() {
                    app.obs.send(crate::obs::ObsCmd::Disconnect);
                }
            } else if ui
                .add_enabled(!app.obs_busy, egui::Button::new("Connect OBS"))
                .clicked()
            {
                app.save();
                app.obs_connect();
            }
            if app.obs_busy {
                ui.spinner();
            }
        });
        if app.obs_connected {
            ui.label(weak(app.obs_version.clone()));
        } else {
            ui.label(weak(
                "OBS -> Tools -> WebSocket Server Settings -> Enable. Keep it on localhost.",
            ));
        }
    });
    if !app.obs_connected {
        return;
    }
    ui.add_space(8.0);
    let names: Vec<String> = app
        .obs_inputs
        .iter()
        .filter(|i| i.is_audio())
        .map(|i| i.name.clone())
        .collect();
    let all_names: Vec<String> = app.obs_inputs.iter().map(|i| i.name.clone()).collect();
    card(ui, "Audio sources and latency", |ui| {
        let mut changed = false;
        let guest_l = app.guest_latency_ms();
        egui::Grid::new("roles")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label(weak("Role"));
                ui.label(weak("OBS input"));
                ui.label(weak("Latency"));
                ui.label(weak("Current offset"));
                ui.end_row();
                let cur = app.current_offsets();
                let show = |ui: &mut egui::Ui, n: &Option<String>| {
                    ui.label(
                        RichText::new(
                            n.as_ref()
                                .and_then(|n| cur.get(n))
                                .map(|o| format!("{o} ms"))
                                .unwrap_or("—".into()),
                        )
                        .monospace(),
                    );
                };
                ui.label("Your mic");
                changed |= input_combo(ui, "r_mic", &mut app.cfg.obs_host_mic, &names);
                ui.add(
                    egui::DragValue::new(&mut app.cfg.host_mic_latency_ms)
                        .range(-500.0..=2000.0)
                        .suffix(" ms"),
                );
                show(ui, &app.cfg.obs_host_mic);
                ui.end_row();
                ui.label("Instrumental");
                changed |= input_combo(ui, "r_mus", &mut app.cfg.obs_music, &names);
                ui.add(
                    egui::DragValue::new(&mut app.cfg.music_latency_ms)
                        .range(-500.0..=2000.0)
                        .suffix(" ms"),
                );
                show(ui, &app.cfg.obs_music);
                ui.end_row();
                ui.label("Guest (Discord)");
                changed |= input_combo(ui, "r_gst", &mut app.cfg.obs_guest, &names);
                ui.label(
                    RichText::new(guest_l.map(|l| format!("{l:.1} ms")).unwrap_or("—".into()))
                        .monospace(),
                );
                show(ui, &app.cfg.obs_guest);
                ui.end_row();
            });
        ui.horizontal(|ui| {
            ui.label("Safety margin");
            ui.add(
                egui::DragValue::new(&mut app.cfg.margin_ms)
                    .range(0.0..=500.0)
                    .suffix(" ms"),
            );
            ui.label(weak(
                "added to every source; absorbs small latency changes, does not track them",
            ));
        });
        if changed {
            app.save();
        }
        let comp: Vec<_> =
            crate::obs::compensation::compensation(&app.source_latencies(), app.cfg.margin_ms);
        if !comp.is_empty() {
            ui.add_space(4.0);
            ui.label(weak("Compensation (target − source latency):"));
            for (n, c) in &comp {
                ui.label(RichText::new(format!("  {n}: +{c:.0} ms")).monospace());
            }
        }
        for w in routing_warnings(
            &app.obs_inputs,
            app.cfg.obs_host_mic.as_deref(),
            app.cfg.obs_guest.as_deref(),
            app.cfg.obs_music.as_deref(),
        ) {
            ui.label(RichText::new(format!("Warning: {w}")).color(theme::WARN));
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !app.obs_busy && !comp.is_empty(),
                    egui::Button::new("Apply compensation…"),
                )
                .clicked()
            {
                app.prepare_apply();
            }
            let n = app.cfg.ledger.baseline.len();
            if ui
                .add_enabled(
                    !app.obs_busy && n > 0,
                    egui::Button::new(format!("Restore original offsets ({n})")),
                )
                .clicked()
            {
                app.confirm_restore = true;
            }
        });
        if !app.last_apply.is_empty() {
            for r in &app.last_apply {
                let (c, t) = match &r.readback {
                    Ok(v) if *v == r.requested => {
                        (theme::OK, format!("{}: {} ms verified", r.input, v))
                    }
                    Ok(v) => (
                        theme::ERR,
                        format!(
                            "FAILED {}: requested {} ms, OBS reports {v} ms",
                            r.input, r.requested
                        ),
                    ),
                    Err(e) => (theme::ERR, format!("FAILED {}: {e}", r.input)),
                };
                ui.label(RichText::new(t).color(c));
            }
        }
        ui.label(weak(format!("OBS accepts offsets from {OFFSET_MIN_MS} to {OFFSET_MAX_MS} ms. Monitor your own voice outside OBS (headphones / Discord), not through the delayed OBS mix.")));
    });
    ui.add_space(8.0);
    card(ui, "Avatar video delay (manual)", |ui| {
        ui.label(weak("Adds an OBS Render Delay filter named \"DuetSync Delay\". Video latency is not measured automatically: match lips to audio by eye. Uses GPU frame buffering up to 500 ms."));
        let v = &mut app.cfg.video;
        let mut actions: Vec<(String, Option<i64>)> = vec![];
        egui::Grid::new("video").num_columns(4).show(ui, |ui| {
            for (label, src, ms) in [
                ("Host avatar", &mut v.host_source, &mut v.host_ms),
                ("Guest avatar", &mut v.guest_source, &mut v.guest_ms),
            ] {
                ui.label(label);
                input_combo(ui, label, src, &all_names);
                ui.add(
                    egui::DragValue::new(ms)
                        .range(0..=VIDEO_DELAY_MAX_MS)
                        .suffix(" ms"),
                );
                ui.horizontal(|ui| {
                    if let Some(s) = src.clone() {
                        if ui.button("Apply").clicked() {
                            actions.push((s.clone(), Some(*ms)));
                        }
                        if ui.button("Remove").clicked() {
                            actions.push((s, None));
                        }
                    }
                });
                ui.end_row();
            }
        });
        for (s, ms) in actions {
            match ms {
                Some(m) => app.set_video_delay(s, m),
                None => app.remove_video_delay(s),
            }
            app.save();
        }
        for (s, ms) in &app.video_state {
            ui.label(weak(format!(
                "{s}: {}",
                ms.map(|m| format!("{m} ms")).unwrap_or("no filter".into())
            )));
        }
    });
}

pub fn player(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    card(ui, "Instrumental", |ui| {
        ui.horizontal(|ui| {
            if ui.button("Load WAV…").clicked() {
                if let Some(p) = super::pick_wav() {
                    app.load_track(p);
                }
            }
            ui.label(weak("or drop a .wav file on the window"));
        });
        match &app.track {
            Some(t) => {
                egui::Grid::new("trk").num_columns(2).show(ui, |ui| {
                    value(ui, "File", t.name.clone());
                    value(ui, "Length", fmt_time(t.frames as f64, t.sample_rate));
                    value(
                        ui,
                        "Format",
                        format!("{} Hz, {} ch", t.sample_rate, t.channels),
                    );
                    value(ui, "SHA-256", format!("{}…", &t.sha256[..16]));
                });
            }
            None => {
                ui.label(weak(
                    "No instrumental loaded. Both performers need the identical file.",
                ));
            }
        }
        let partner = match app.cfg.mode {
            Mode::Host => app.guest_track_name.clone(),
            Mode::Guest => app.host_track.as_ref().map(|t| t.name.clone()),
        };
        match app.track_matches() {
            Some(true) => {
                ui.label(RichText::new("Partner has the identical file").color(theme::OK));
            }
            Some(false) => {
                ui.label(
                    RichText::new(format!(
                        "FAILED Partner's file differs ({})",
                        partner.unwrap_or_default()
                    ))
                    .color(theme::ERR),
                );
            }
            None if app.connected() => {
                ui.label(weak("Partner has not loaded a file yet"));
            }
            None => {}
        }
    });
    ui.add_space(8.0);
    card(ui, "Playback timing", |ui| {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Volume");
            changed |= ui
                .add(egui::Slider::new(&mut app.cfg.volume, 0.0..=1.5))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Output latency trim");
            changed |= ui.add(egui::DragValue::new(&mut app.cfg.output_latency_ms).range(-100.0..=500.0).suffix(" ms")).changed();
        })
        .response
        .on_hover_text("Extra latency of your headphones not reported by Windows (e.g. Bluetooth ~ 150–250 ms). Positive values start the music earlier.");
        changed |= ui
            .checkbox(
                &mut app.cfg.drift_correction,
                "Gradual drift correction (±300 ppm resampling)",
            )
            .changed();
        if app.cfg.mode == Mode::Host {
            ui.horizontal(|ui| {
                ui.label("Countdown");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut app.cfg.countdown_s)
                            .range(2.0..=15.0)
                            .suffix(" s"),
                    )
                    .changed();
                if ui
                    .add_enabled(app.playing(), egui::Button::new("Resync now"))
                    .clicked()
                {
                    app.resync();
                }
            });
        }
        if changed {
            // Positive trim must make audio audible earlier: the core adds it to presentation time.
            app.apply_render_settings();
            app.save();
        }
        if let Some(r) = &app.render {
            let st = r.shared.state();
            egui::Grid::new("pb").num_columns(2).show(ui, |ui| {
                value(ui, "State", format!("{st:?}"));
                value(
                    ui,
                    "Local drift",
                    format!(
                        "{:+.2} ms",
                        r.shared.drift_us.load(std::sync::atomic::Ordering::Relaxed) as f64
                            / 1000.0
                    ),
                );
                value(
                    ui,
                    "Underruns",
                    r.shared
                        .underruns
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .to_string(),
                );
                value(ui, "Output", r.description.clone());
            });
            if st == State::Playing
                && app.cfg.mode == Mode::Host
                && app.connected()
                && app.sync_error_ms().is_none()
            {
                ui.label(weak("Waiting for guest playback report..."));
            }
        }
    });
}

pub fn diagnostics(g: &mut Gui, ui: &mut egui::Ui) {
    let app = &mut g.app;
    card(ui, "Resources (this process, measured)", |ui| {
        egui::Grid::new("res").num_columns(2).show(ui, |ui| {
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
    ui.add_space(8.0);
    card(ui, "Network", |ui| {
        ui.label(RichText::new(app.network_summary()).monospace().size(12.0));
    });
    ui.add_space(8.0);
    card(ui, "Audio", |ui| {
        ui.label(RichText::new(app.audio_summary()).monospace().size(12.0));
    });
    ui.add_space(8.0);
    card(ui, "Log", |ui| {
        if ui.button("Export diagnostics").clicked() {
            app.export_diagnostics();
        }
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for l in app.log.lines() {
                    ui.label(RichText::new(l).monospace().size(11.5).color(theme::WEAK));
                }
            });
    });
    let _ = Tab::Diagnostics;
}

pub fn dialogs(g: &mut Gui, ctx: &egui::Context) {
    let app = &mut g.app;
    if app.pending_apply.is_some() {
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("confirm_apply")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(
                RichText::new("Change OBS sync offsets?")
                    .strong()
                    .size(15.0),
            );
            let p = app.pending_apply.as_ref().unwrap();
            egui::Grid::new("chg").num_columns(4).show(ui, |ui| {
                for h in ["Input", "Original", "Now", "New"] {
                    ui.label(weak(h));
                }
                ui.end_row();
                for c in &p.changes {
                    ui.label(&c.input);
                    ui.label(format!("{} ms", c.baseline_ms));
                    ui.label(format!("{} ms", c.current_ms));
                    ui.label(RichText::new(format!("{} ms", c.target_ms)).strong());
                    ui.end_row();
                }
            });
            for n in &p.notes {
                ui.label(RichText::new(n).color(theme::WARN));
            }
            ui.label(weak(
                "Original values are saved; \"Restore original offsets\" puts them back.",
            ));
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
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
        egui::Modal::new(egui::Id::unique("confirm_restore")).show(ctx, |ui| {
            ui.label(RichText::new("Restore original OBS offsets?").strong());
            for (k, v) in &app.cfg.ledger.baseline {
                ui.label(format!("{k} -> {v} ms"));
            }
            ui.horizontal(|ui| {
                if ui.button("Restore").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
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
