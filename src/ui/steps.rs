//! The guided journey: step list, completion rules, "next up" sentences and step contents.

use super::fun::{
    caption, card, check_row, fun_button, fun_button_if, meter, pill, small_button,
    small_button_if, soft, sticker, text_field, FunButton, Kind,
};
use super::mascot::{bob_offset, mascot, mascot_faded, Mood};
use super::panels::{device_combo, input_combo, toggle_chip, value};
use super::theme::{
    self, fmt_time, fredoka, fredoka_bold, INK, INK_SOFT, INK_TINT, LILAC_TINT, MINT, PAPER, SUN,
    SUN_TINT,
};
use super::Gui;
use crate::app::App;
use crate::audio::CaptureSource;
use crate::calibration::CalibrationStatus;
use crate::clock::now_us;
use crate::config::{GuestAudio, Mode};
use crate::obs::client::{OFFSET_MAX_MS, OFFSET_MIN_MS, VIDEO_DELAY_MAX_MS};
use crate::obs::routing_warnings;
use crate::playback::core::State;
use eframe::egui::{self, RichText, Sense, Shape, Stroke, Ui};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    HostConnect,
    HostAudio,
    Song,
    Measure,
    Obs,
    Sing,
    GuestJoin,
    GuestAudio,
    GuestReady,
}

const HOST_STEPS: [Step; 6] = [
    Step::HostConnect,
    Step::HostAudio,
    Step::Song,
    Step::Measure,
    Step::Obs,
    Step::Sing,
];
const GUEST_STEPS: [Step; 5] = [
    Step::GuestJoin,
    Step::GuestAudio,
    Step::Song,
    Step::GuestReady,
    Step::Sing,
];

pub fn steps(mode: Mode) -> &'static [Step] {
    match mode {
        Mode::Host => &HOST_STEPS,
        Mode::Guest => &GUEST_STEPS,
    }
}

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::HostConnect => "Find your partner",
            Step::HostAudio | Step::GuestAudio => "Set up audio",
            Step::Song => "Load the song",
            Step::Measure => "Measure the delay",
            Step::Obs => "Send to OBS",
            Step::Sing => "Sing!",
            Step::GuestJoin => "Join your host",
            Step::GuestReady => "Get ready to measure",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Step::HostConnect => "Your partner needs this invite to join. Works on the same Wi-Fi, or over Tailscale if you live far apart.",
            Step::HostAudio => "Pick your headphones and microphone, and tell DuetSync where your partner's voice comes out.",
            Step::GuestAudio => "Pick your headphones, then check that the test sound can reach your microphone.",
            Step::Song => "You both load the exact same instrumental .wav file.",
            Step::Measure => "We play little test sounds on your partner's PC and time how late they reach you through Discord. Takes about 15 seconds. Shhh while it runs!",
            Step::Obs => "Connect OBS, then send the measured delay so your recording lines up.",
            Step::Sing => "Everything is ready. Time to sing!",
            Step::GuestJoin => "Your host made a session and sent you an invite. It looks like 100.64.1.2:47800#ABCDE-...",
            Step::GuestReady => "Your host measures how late your voice arrives through Discord. Get Discord ready first.",
        }
    }

    pub fn has_advanced(self) -> bool {
        matches!(
            self,
            Step::HostConnect | Step::Measure | Step::Obs | Step::Sing
        )
    }
}

/// Raw completion from the app state (confirmation by "Next" is added by the caller).
pub fn raw_done(app: &App, step: Step) -> bool {
    match step {
        Step::HostConnect => app.connected(),
        Step::HostAudio => app.guest_source_ok,
        Step::Song => {
            app.track.is_some() && (!app.connected() || app.track_matches() == Some(true))
        }
        Step::Measure => app
            .last_calib
            .as_ref()
            .is_some_and(|o| o.result.status != CalibrationStatus::Invalid),
        Step::Obs => {
            !app.cfg.ledger.applied.is_empty() && app.last_apply.iter().all(|r| r.verified())
        }
        Step::Sing => false,
        Step::GuestJoin => app.connected() && app.clock_estimate().is_some(),
        Step::GuestAudio => app.routing_ok,
        Step::GuestReady => app.guest_calib_note.is_some(),
    }
}

/// An unfinished earlier step that blocks the viewed step's main action, if any.
/// Only then does the bubble offer "Go to step N" (and its sentence says why).
pub fn blocker(app: &App, step: Step) -> Option<Step> {
    match step {
        Step::Measure if !app.connected() => Some(Step::HostConnect),
        Step::Obs
            if app.last_calib.is_none()
                && app.cfg.manual_guest_latency_ms == 0.0
                && app.obs_connected =>
        {
            Some(Step::Measure)
        }
        Step::Sing if app.track.is_none() => Some(Step::Song),
        Step::GuestReady if !app.connected() => Some(Step::GuestJoin),
        _ => None,
    }
}

/// Mood of the bubble mascot for the viewed step.
pub fn bubble_mood(app: &App, step: Step, done: bool) -> Mood {
    if app.calib.is_some()
        || app
            .busy
            .as_deref()
            .is_some_and(|b| b.contains("calibration"))
    {
        return Mood::Listening;
    }
    match step {
        Step::HostConnect if !done => Mood::Waiting,
        Step::GuestJoin if !done => Mood::Listening,
        Step::GuestReady if !done => Mood::Listening,
        Step::Sing if app.playing() => Mood::Singing,
        _ => Mood::Happy,
    }
}

/// The "NEXT UP" sentence for the viewed step.
pub fn bubble_sentence(app: &App, step: Step, done: bool) -> String {
    let s = match step {
        Step::HostConnect => {
            if app.connected() {
                "Your partner is here! Next, set up audio."
            } else if app.host.is_some() {
                "Copy the invite and send it to your partner on Discord!"
            } else {
                "Press Create session to make an invite."
            }
        }
        Step::HostAudio => {
            if done {
                "Audio is ready! Next, load the song."
            } else {
                "Ask your partner to talk, then press Test partner's voice."
            }
        }
        Step::Song => {
            if app.track.is_none() {
                "Pick the instrumental .wav. Your partner needs the exact same file."
            } else if app.track_matches() == Some(false) {
                match app.cfg.mode {
                    Mode::Host => {
                        "Your partner's file is different. Send them the exact same .wav."
                    }
                    Mode::Guest => {
                        "Your host's file is different. Ask them for the exact same .wav."
                    }
                }
            } else if app.connected() && app.track_matches().is_none() {
                match app.cfg.mode {
                    Mode::Host => "Waiting for your partner to load the same song...",
                    Mode::Guest => "Waiting for your host to load the song...",
                }
            } else {
                match app.cfg.mode {
                    Mode::Host => "Song loaded! Next, measure the delay.",
                    Mode::Guest => "Song loaded! Next, get ready to measure.",
                }
            }
        }
        Step::Measure => {
            if app.calib.is_some() {
                "Shhh... listening to the test sounds."
            } else if done {
                "Got it! Now send the delay to OBS so the recording lines up."
            } else if !app.connected() {
                "Your partner is not connected yet. Go back to step 1."
            } else {
                "Press Measure delay, then keep quiet for about 15 seconds."
            }
        }
        Step::Obs => {
            if !app.obs_connected {
                "Connect OBS first, then pick which source is which."
            } else if done {
                "OBS is in sync! Time to sing."
            } else if blocker(app, step).is_some() {
                "Measure the delay first (step 4), or type it in Advanced settings."
            } else {
                "Press Apply to OBS to send the delay."
            }
        }
        Step::Sing if app.track.is_none() => "Load the song first (step 3), then come back here.",
        Step::Sing => match app.cfg.mode {
            Mode::Host => {
                if app.playing() {
                    "Sing your heart out!"
                } else {
                    "Press Play when you are both ready. Have fun!"
                }
            }
            Mode::Guest => "Your host presses play. Just sing along!",
        },
        Step::GuestJoin => {
            if app.guest.is_none() {
                "Paste the invite your host sent you, then press Join!"
            } else if !app.connected() {
                "Connecting to your host..."
            } else if done {
                "You are in! Next, set up audio."
            } else {
                "Connected! Syncing clocks..."
            }
        }
        Step::GuestAudio => {
            if done {
                "The test sound was heard! Next, load the song."
            } else {
                "Headphones will not reach your mic. Use speakers, or a virtual cable that Discord listens to, then press Test sound path."
            }
        }
        Step::GuestReady => {
            if done {
                "Measurement heard! Next, get ready to sing."
            } else if !app.connected() {
                "Join your host first (step 1)."
            } else {
                "Your host will start the measurement. Keep quiet when it starts!"
            }
        }
    };
    s.to_owned()
}

// ===================================================================== contents

/// Draw the content cards of `step`.
pub fn content(g: &mut Gui, ui: &mut Ui, step: Step, avail_h: f32) {
    match step {
        Step::HostConnect => host_connect(g, ui),
        Step::HostAudio => host_audio(g, ui),
        Step::Song => song(g, ui),
        Step::Measure => host_measure(g, ui),
        Step::Obs => obs(g, ui),
        Step::Sing => sing(g, ui, avail_h),
        Step::GuestJoin => guest_join(g, ui),
        Step::GuestAudio => guest_audio(g, ui),
        Step::GuestReady => guest_ready(g, ui),
    }
}

/// The collapsible "Advanced settings" body of `step`.
pub fn advanced(g: &mut Gui, ui: &mut Ui, step: Step) {
    match step {
        Step::HostConnect => advanced_connect(g, ui),
        Step::Measure => advanced_measure(g, ui),
        Step::Obs => advanced_obs(g, ui),
        Step::Sing => advanced_sing(g, ui),
        _ => {}
    }
}

fn row_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::nunito_x(14.0)));
}

// ------------------------------------------------------------ host: connect

fn host_connect(g: &mut Gui, ui: &mut Ui) {
    let now = ui.input(|i| i.time);
    let app = &mut g.app;
    if app.host.is_none() {
        card(ui, Some("Make an invite"), |ui| {
            ui.label(soft(
                "Private on purpose: please do not port-forward this to the internet.",
            ));
            ui.horizontal(|ui| {
                if fun_button(ui, "Create session", Kind::Coral).clicked() {
                    app.start_host();
                    app.save();
                }
                ui.label(soft(format!(
                    "Code valid {} min",
                    app.cfg.join_code_minutes
                )));
            });
        });
        return;
    }
    card(ui, Some("Your invite"), |ui| {
        if let Some(code) = app.join_code.clone() {
            let invites: Vec<String> = app
                .invite_hosts
                .iter()
                .map(|h| format!("{h}#{code}"))
                .collect();
            for (k, invite) in invites.iter().enumerate() {
                ui.horizontal(|ui| {
                    let copied = now < g.copied_until;
                    let btn = if k == 0 {
                        FunButton::new(if copied { "Copied!" } else { "Copy invite" }, Kind::Coral)
                    } else {
                        FunButton::new("Copy", Kind::White).small()
                    };
                    let w = ui.available_width() - btn.measure(ui).x - ui.spacing().item_spacing.x;
                    let mut shown: &str = invite;
                    text_field(ui, w, &mut shown, "", INK, 2.0);
                    if ui.add(btn).clicked() {
                        ui.ctx().copy_text(invite.clone());
                        g.copied_until = now + 1.6;
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(1700));
                    }
                });
            }
        }
        if app.connected() {
            ui.horizontal(|ui| {
                mascot(ui, Mode::Guest, Mood::Happy, 46.0, 0.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Your partner is here!").font(fredoka(17.0)));
                    ui.label(soft(app.peer.clone().unwrap_or_default()));
                });
            });
        } else {
            let left = (app.code_expires_us - now_us()) / 60_000_000;
            ui.horizontal(|ui| {
                mascot_faded(ui, Mode::Guest, Mood::Waiting, 46.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Waiting for your partner to join...").font(fredoka(17.0)),
                    );
                    ui.label(soft(if left >= 0 {
                        format!("The invite works for {left} more minutes. If they drop out, they hop back in by themselves.")
                    } else {
                        "The invite expired for new joins. Press New code to make a fresh one."
                            .into()
                    }));
                });
            });
        }
        ui.horizontal(|ui| {
            if small_button(ui, "New code", Kind::White).clicked() {
                app.regenerate_code();
            }
            if fun_button(ui, "End session", Kind::Ghost).clicked() {
                app.stop_session();
            }
        });
    });
}

fn advanced_connect(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Session settings"), |ui| {
        ui.horizontal(|ui| {
            row_label(ui, "Port");
            ui.add_enabled(
                app.host.is_none(),
                egui::DragValue::new(&mut app.cfg.listen_port).range(1024..=65535),
            );
            row_label(ui, "Code valid");
            ui.add(
                egui::DragValue::new(&mut app.cfg.join_code_minutes)
                    .range(1..=1440)
                    .suffix(" min"),
            );
        });
        ui.label(soft(
            "Round trip and clock details are in Diagnostics. The port can only change before a session starts.",
        ));
        if let Some(e) = app.clock_estimate() {
            ui.label(soft(format!(
                "Round trip {:.1} ms, clock +-{:.1} ms, drift {:+.1} ppm",
                e.rtt_median_us / 1000.0,
                e.uncertainty_us / 1000.0,
                e.drift_ppm
            )));
        } else if let Some(r) = &app.remote {
            ui.label(soft(format!(
                "Round trip {:.1} ms, guest clock +-{:.1} ms",
                r.rtt_us as f64 / 1000.0,
                r.clock_uncertainty_us as f64 / 1000.0
            )));
        }
    });
}

// -------------------------------------------------------------- host: audio

fn host_audio(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    ui.columns(2, |cols| {
        card(&mut cols[0], Some("Your headphones"), |ui| {
            ui.label(soft("Where you hear the music."));
            let mut out = app.cfg.output_device.clone();
            if device_combo(ui, "out", &mut out, &app.outputs) {
                app.change_output_device(out);
                app.save();
            }
            meter(ui, app.out_peak);
        });
        card(&mut cols[1], Some("Your microphone"), |ui| {
            ui.label(soft("The mic your viewers hear."));
            if device_combo(ui, "mic", &mut app.cfg.mic_device, &app.inputs) {
                app.monitor = None;
                app.save();
            }
            if toggle_chip(ui, "Mic level", app.monitor.is_some()).clicked() {
                let src = CaptureSource::Input(app.cfg.mic_device.clone());
                app.toggle_monitor(Some(src));
            }
            if app.monitor.is_some() {
                meter(ui, app.monitor_peak);
            }
        });
    });
    card(ui, Some("Your partner's voice"), |ui| {
        ui.label(soft(
            "Where Discord plays your partner. DuetSync listens there to time the delay.",
        ));
        let mut kind = match &app.cfg.guest_audio {
            GuestAudio::DiscordProcess => 0,
            GuestAudio::OutputLoopback(_) => 1,
            GuestAudio::Input(_) => 2,
        };
        let before = kind;
        ui.horizontal(|ui| {
            for (k, label) in [
                (0, "Discord app"),
                (1, "Output device"),
                (2, "Input device"),
            ] {
                if toggle_chip(ui, label, kind == k).clicked() {
                    kind = k;
                }
            }
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
                ui.label(soft(match app.discord.first() {
                    Some(d) => format!("Found {} (pid {})", d.name, d.pid),
                    None => "Discord not found. Start it, then press Refresh devices.".into(),
                }));
            }
            GuestAudio::OutputLoopback(id) => {
                row_label(ui, "Output device");
                device_combo(ui, "gout", id, &outs);
            }
            GuestAudio::Input(id) => {
                row_label(ui, "Input device");
                device_combo(ui, "gin", id, &ins);
            }
        }
        ui.horizontal(|ui| {
            if fun_button(ui, "Test partner's voice", Kind::Coral).clicked() {
                app.test_guest_capture();
            }
            if fun_button(ui, "Refresh devices", Kind::Ghost).clicked() {
                app.refresh_devices();
            }
        });
        if app.guest_source_ok {
            check_row(ui, true, "Your partner's voice was heard");
        } else {
            ui.label(soft(
                "Not tested yet. Ask your partner to talk, then press the button.",
            ));
        }
    });
}

// -------------------------------------------------------------------- song

fn song(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Your song"), |ui| {
        ui.horizontal(|ui| {
            if fun_button(ui, "Choose song (.wav)", Kind::Coral).clicked() {
                if let Some(p) = super::pick_wav() {
                    app.load_track(p);
                }
            }
            ui.label(soft("or drop a .wav on the window"));
        });
        if let Some(t) = &app.track {
            sticker(ui, SUN_TINT, 16, |ui| {
                ui.label(RichText::new(&t.name).font(fredoka(17.0)));
                ui.label(soft(format!(
                    "{}  -  {} Hz, {} ch",
                    fmt_time(t.frames as f64, t.sample_rate),
                    t.sample_rate,
                    t.channels
                )));
            });
        } else {
            ui.label(soft("No song yet. Both of you need the identical file."));
        }
        let partner = match app.cfg.mode {
            Mode::Host => app.guest_track_name.clone(),
            Mode::Guest => app.host_track.as_ref().map(|t| t.name.clone()),
        };
        let who = match app.cfg.mode {
            Mode::Host => "partner",
            Mode::Guest => "host",
        };
        if app.track.is_some() {
            match app.track_matches() {
                Some(true) => check_row(ui, true, &format!("Your {who} has the same file")),
                Some(false) => check_row(
                    ui,
                    false,
                    &format!(
                        "Your {who}'s file is different ({})",
                        partner.unwrap_or_default()
                    ),
                ),
                None if app.connected() => check_row(
                    ui,
                    false,
                    &format!("Waiting for your {who} to load the same song..."),
                ),
                None => {
                    ui.label(soft(format!(
                        "When your {who} joins, we check that you have the same file."
                    )));
                }
            }
        }
    });
}

// ------------------------------------------------------------ host: measure

fn host_measure(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    let details = &mut g.details;
    ui.columns(2, |cols| {
        card(&mut cols[0], Some("Before you start"), |ui| {
            check_row(
                ui,
                app.connected(),
                if app.connected() {
                    "Partner connected"
                } else {
                    "Partner not connected yet"
                },
            );
            match &app.remote {
                Some(r) => check_row(
                    ui,
                    true,
                    &format!(
                        "Clocks in sync (within {:.1} ms)",
                        r.clock_uncertainty_us as f64 / 1000.0
                    ),
                ),
                None => check_row(ui, false, "Waiting for the clocks to sync"),
            }
            check_row(
                ui,
                false,
                "Partner: turn off Krisp and Echo Cancellation in Discord",
            );
            let can = app.can_calibrate();
            let again = app.last_calib.is_some();
            ui.horizontal(|ui| {
                let (label, kind) = if again {
                    ("Measure again", Kind::White)
                } else {
                    ("Measure delay", Kind::Coral)
                };
                if fun_button_if(ui, can.is_ok(), label, kind).clicked() {
                    app.save();
                    app.start_calibration();
                }
            });
            if let Err(why) = &can {
                ui.label(soft(why.clone()));
            }
            if let Some(p) = app.calibration_progress() {
                ui.horizontal(|ui| {
                    let bob = bob_offset(ui);
                    mascot(ui, Mode::Host, Mood::Listening, 46.0, bob);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Shhh... measuring").font(fredoka(17.0)));
                        let w = ui.available_width();
                        super::fun::progress_track(ui, w, p as f32, theme::CORAL, false);
                    });
                });
            }
        });
        result_card(&mut cols[1], app, details);
    });
}

fn result_card(ui: &mut Ui, app: &mut App, details: &mut bool) {
    sticker(ui, SUN_TINT, 22, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        let Some(o) = &app.last_calib else {
            ui.label(RichText::new("No measurement yet").font(fredoka(17.0)));
            ui.label(soft(
                "Press Measure delay. We will show how late your partner's voice arrives.",
            ));
            return;
        };
        let r = &o.result;
        let (label, fill) = match r.status {
            CalibrationStatus::Valid => ("Great measurement", MINT),
            CalibrationStatus::LowConfidence => ("Not sure, measure again", SUN),
            CalibrationStatus::Invalid => ("Didn't work", theme::CORAL_TINT),
        };
        ui.horizontal_wrapped(|ui| {
            let big = r
                .median_ms
                .map(|m| format!("{m:.0} ms"))
                .unwrap_or_else(|| "-- ms".into());
            ui.label(RichText::new(big).font(fredoka_bold(48.0)));
            pill(ui, label, fill, None);
        });
        match (r.status, r.median_ms) {
            (CalibrationStatus::Invalid, _) | (_, None) => {
                ui.label(RichText::new(
                    "We could not hear the test sounds clearly. Check the list on the left and measure again.",
                ).font(theme::nunito(14.0)));
            }
            (_, Some(m)) => {
                ui.label(
                    RichText::new(format!(
                        "Your partner's voice arrives {m:.0} ms late. In step 5 we delay your mic and music by the same amount, so viewers hear you together."
                    ))
                    .font(theme::nunito(14.0)),
                );
            }
        }
        ui.horizontal(|ui| {
            mascot(ui, Mode::Host, Mood::Happy, 38.0, 0.0);
            mascot(ui, Mode::Guest, Mood::Happy, 38.0, 0.0);
            ui.label(
                RichText::new(format!("{} of {} test sounds heard", r.accepted, r.total))
                    .font(theme::nunito_x(13.0))
                    .color(INK_TINT),
            );
        });
        if let Some(l) = app.guest_latency_ms() {
            ui.label(soft(format!("In use for OBS: {l:.1} ms")));
        }
        for s in app.staleness() {
            ui.label(RichText::new(s).font(theme::nunito(12.5)).color(INK_SOFT));
        }
        if fun_button(
            ui,
            if *details {
                "Hide details"
            } else {
                "Show details"
            },
            Kind::Ghost,
        )
        .clicked()
        {
            *details = !*details;
        }
        if *details {
            let o = app.last_calib.as_ref().unwrap();
            let r = &o.result;
            egui::Grid::new("res").num_columns(2).show(ui, |ui| {
                let ms = |v: Option<f64>| v.map(|m| format!("{m:.1} ms")).unwrap_or("-".into());
                value(ui, "Relative latency", ms(r.median_ms));
                value(
                    ui,
                    "Uncertainty (~95 %)",
                    r.uncertainty_ms
                        .map(|m| format!("+-{m:.1} ms"))
                        .unwrap_or("-".into()),
                );
                value(ui, "Jitter", ms(r.jitter_ms));
                value(
                    ui,
                    "Range",
                    match (r.min_ms, r.max_ms) {
                        (Some(a), Some(b)) => format!("{a:.1} to {b:.1} ms"),
                        _ => "-".into(),
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
                    format!("+-{:.1} ms", r.clock_uncertainty_ms),
                );
                value(
                    ui,
                    "Reference",
                    if o.reference {
                        "guest input timestamps".to_string()
                    } else {
                        "scheduled playback times".to_string()
                    },
                );
            });
            for m in &r.messages {
                ui.label(RichText::new(format!("- {m}")).font(theme::nunito(13.0)));
            }
            ui.label(soft("Per signal:"));
            for (k, p) in r.per_chirp.iter().enumerate() {
                ui.label(soft(match p {
                    Ok(v) => format!("#{k}: {v:.2} ms"),
                    Err(e) => format!("#{k}: {e}"),
                }));
            }
        }
    });
}

fn advanced_measure(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Measurement settings"), |ui| {
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Test sounds");
            ui.add(egui::DragValue::new(&mut app.cfg.chirp_count).range(4..=16));
            row_label(ui, "Spacing");
            ui.add(
                egui::DragValue::new(&mut app.cfg.chirp_spacing_ms)
                    .range(800.0..=3000.0)
                    .suffix(" ms"),
            );
            row_label(ui, "Level");
            ui.add(
                egui::DragValue::new(&mut app.cfg.chirp_amplitude)
                    .range(0.05..=0.9)
                    .speed(0.01),
            );
        });
        ui.checkbox(
            &mut app.use_measured,
            "Use the measured value when it is good",
        );
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Manual latency");
            ui.add(
                egui::DragValue::new(&mut app.cfg.manual_guest_latency_ms)
                    .range(0.0..=2000.0)
                    .suffix(" ms"),
            );
            row_label(ui, "Fine tune");
            ui.add(
                egui::DragValue::new(&mut app.cfg.fine_tune_ms)
                    .range(-200.0..=200.0)
                    .speed(0.5)
                    .suffix(" ms"),
            );
        });
        if let Some(l) = app.guest_latency_ms() {
            ui.label(RichText::new(format!("In use: {l:.1} ms")).font(theme::nunito_x(14.0)));
        }
        for s in app.staleness() {
            ui.label(soft(s));
        }
    });
}

// ----------------------------------------------------------------- host: obs

fn obs(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Connect to OBS"), |ui| {
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Host");
            ui.add(egui::TextEdit::singleline(&mut app.cfg.obs_host).desired_width(120.0));
            row_label(ui, "Port");
            ui.add(egui::DragValue::new(&mut app.cfg.obs_port));
            row_label(ui, "Password");
            ui.add(
                egui::TextEdit::singleline(&mut app.obs_password)
                    .password(true)
                    .desired_width(140.0),
            );
        });
        ui.checkbox(
            &mut app.cfg.remember_obs_password,
            "Remember password (stored on this PC)",
        );
        ui.horizontal(|ui| {
            if app.obs_connected {
                if small_button(ui, "Refresh", Kind::White).clicked() {
                    app.obs_refresh();
                }
                if fun_button(ui, "Disconnect", Kind::Ghost).clicked() {
                    app.obs.send(crate::obs::ObsCmd::Disconnect);
                }
            } else if fun_button_if(ui, !app.obs_busy, "Connect OBS", Kind::Coral).clicked() {
                app.save();
                app.obs_connect();
            }
            if app.obs_busy {
                ui.spinner();
            }
        });
        ui.label(soft(if app.obs_connected {
            app.obs_version.clone()
        } else {
            "In OBS: Tools, WebSocket Server Settings, Enable. Keep it on this PC (localhost)."
                .into()
        }));
    });
    if !app.obs_connected {
        return;
    }
    let names: Vec<String> = app
        .obs_inputs
        .iter()
        .filter(|i| i.is_audio())
        .map(|i| i.name.clone())
        .collect();
    let comp = crate::obs::compensation::compensation(&app.source_latencies(), app.cfg.margin_ms);
    card(ui, Some("Which OBS source is which?"), |ui| {
        let mut changed = false;
        egui::Grid::new("roles")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                row_label(ui, "Your mic");
                changed |= input_combo(ui, "r_mic", &mut app.cfg.obs_host_mic, &names);
                ui.end_row();
                row_label(ui, "Instrumental");
                changed |= input_combo(ui, "r_mus", &mut app.cfg.obs_music, &names);
                ui.end_row();
                row_label(ui, "Partner (Discord)");
                changed |= input_combo(ui, "r_gst", &mut app.cfg.obs_guest, &names);
                ui.end_row();
            });
        if changed {
            app.save();
        }
        if comp.is_empty() {
            ui.label(soft("Pick at least one source to see what will change."));
        } else {
            ui.label(soft("What will change:"));
            for (n, c) in &comp {
                ui.label(
                    RichText::new(if *c < 0.5 {
                        format!("{n} stays as it is.")
                    } else {
                        format!("{n} will be delayed by {c:.0} ms.")
                    })
                    .font(theme::nunito_x(14.0)),
                );
            }
        }
        for w in routing_warnings(
            &app.obs_inputs,
            app.cfg.obs_host_mic.as_deref(),
            app.cfg.obs_guest.as_deref(),
            app.cfg.obs_music.as_deref(),
        ) {
            check_row(ui, false, &w);
        }
        ui.horizontal(|ui| {
            if fun_button_if(
                ui,
                !app.obs_busy && !comp.is_empty(),
                "Apply to OBS",
                Kind::Coral,
            )
            .clicked()
            {
                app.prepare_apply();
            }
            let n = app.cfg.ledger.baseline.len();
            if small_button_if(ui, !app.obs_busy && n > 0, "Restore original", Kind::White)
                .clicked()
            {
                app.confirm_restore = true;
            }
        });
        for r in &app.last_apply {
            match &r.readback {
                Ok(v) if *v == r.requested => {
                    check_row(ui, true, &format!("{}: {} ms, checked in OBS", r.input, v))
                }
                Ok(v) => check_row(
                    ui,
                    false,
                    &format!(
                        "FAILED {}: asked for {} ms, OBS says {v} ms",
                        r.input, r.requested
                    ),
                ),
                Err(e) => check_row(ui, false, &format!("FAILED {}: {e}", r.input)),
            }
        }
        ui.label(soft(format!(
            "OBS accepts offsets from {OFFSET_MIN_MS} to {OFFSET_MAX_MS} ms. Listen to your own voice outside OBS (headphones or Discord), not through the delayed OBS mix."
        )));
    });
}

fn advanced_obs(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Latencies and safety margin"), |ui| {
        let guest_l = app.guest_latency_ms();
        let cur = app.current_offsets();
        let show = |ui: &mut Ui, n: &Option<String>| {
            ui.label(RichText::new(
                n.as_ref()
                    .and_then(|n| cur.get(n))
                    .map(|o| format!("{o} ms"))
                    .unwrap_or("-".into()),
            ));
        };
        egui::Grid::new("lat")
            .num_columns(3)
            .spacing([14.0, 8.0])
            .show(ui, |ui| {
                ui.label(soft("Source"));
                ui.label(soft("Latency"));
                ui.label(soft("Offset in OBS now"));
                ui.end_row();
                row_label(ui, "Your mic");
                ui.add(
                    egui::DragValue::new(&mut app.cfg.host_mic_latency_ms)
                        .range(-500.0..=2000.0)
                        .suffix(" ms"),
                );
                show(ui, &app.cfg.obs_host_mic);
                ui.end_row();
                row_label(ui, "Instrumental");
                ui.add(
                    egui::DragValue::new(&mut app.cfg.music_latency_ms)
                        .range(-500.0..=2000.0)
                        .suffix(" ms"),
                );
                show(ui, &app.cfg.obs_music);
                ui.end_row();
                row_label(ui, "Partner (Discord)");
                ui.label(guest_l.map(|l| format!("{l:.1} ms")).unwrap_or("-".into()));
                show(ui, &app.cfg.obs_guest);
                ui.end_row();
            });
        ui.horizontal(|ui| {
            row_label(ui, "Safety margin");
            ui.add(
                egui::DragValue::new(&mut app.cfg.margin_ms)
                    .range(0.0..=500.0)
                    .suffix(" ms"),
            );
        });
        ui.label(soft(
            "Added to every source. It absorbs small latency changes but does not follow them.",
        ));
    });
    if !app.obs_connected {
        return;
    }
    let all_names: Vec<String> = app.obs_inputs.iter().map(|i| i.name.clone()).collect();
    card(ui, Some("Avatar video delay (manual)"), |ui| {
        ui.label(soft("Adds an OBS Render Delay filter named \"DuetSync Delay\". Video latency is not measured: match lips to audio by eye. Uses GPU frame buffering up to 500 ms."));
        let v = &mut app.cfg.video;
        let mut actions: Vec<(String, Option<i64>)> = vec![];
        egui::Grid::new("video")
            .num_columns(4)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                for (label, src, ms) in [
                    ("Host avatar", &mut v.host_source, &mut v.host_ms),
                    ("Guest avatar", &mut v.guest_source, &mut v.guest_ms),
                ] {
                    row_label(ui, label);
                    input_combo(ui, label, src, &all_names);
                    ui.add(
                        egui::DragValue::new(ms)
                            .range(0..=VIDEO_DELAY_MAX_MS)
                            .suffix(" ms"),
                    );
                    ui.horizontal(|ui| {
                        if let Some(s) = src.clone() {
                            if small_button(ui, "Apply", Kind::White).clicked() {
                                actions.push((s.clone(), Some(*ms)));
                            }
                            if fun_button(ui, "Remove", Kind::Ghost).clicked() {
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
            ui.label(soft(format!(
                "{s}: {}",
                ms.map(|m| format!("{m} ms")).unwrap_or("no filter".into())
            )));
        }
    });
}

// ---------------------------------------------------------------------- sing

fn paint_note(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), Sense::hover());
    let k = 40.0 / 24.0;
    let pt = |x: f32, y: f32| rect.min + egui::vec2(x * k, y * k);
    let p = ui.painter();
    p.add(Shape::line(
        vec![pt(9.0, 18.0), pt(9.0, 5.0), pt(20.0, 3.0), pt(20.0, 16.0)],
        egui::epaint::PathStroke::new(2.5 * k * 0.6, INK),
    ));
    for (x, y) in [(6.5, 18.0), (17.5, 16.0)] {
        p.circle(pt(x, y), 3.0 * k, SUN, Stroke::new(2.0 * k * 0.6, INK));
    }
}

fn sing(g: &mut Gui, ui: &mut Ui, avail_h: f32) {
    // Small windows: smaller mascots and no extra sentence, so the three tiles stay visible.
    let compact = avail_h < 520.0;
    let stage_h = (avail_h - 168.0).max(0.0);
    let (host_px, guest_px) = if compact {
        (96.0, 90.0)
    } else {
        (150.0, 140.0)
    };
    let host = g.app.cfg.mode == Mode::Host;
    let playing = g.app.playing();
    let bob = if playing { bob_offset(ui) } else { 0.0 };
    {
        let app = &mut g.app;
        let name = app
            .track
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "Pick a song in step 3".into());
        let sync = app.sync_error_ms();
        egui::Frame::new()
            .fill(LILAC_TINT)
            .stroke(Stroke::new(theme::STROKE_W, INK))
            .corner_radius(egui::CornerRadius::same(26))
            .inner_margin(egui::Margin::same(22))
            .shadow(egui::Shadow {
                offset: [0, 5],
                blur: 0,
                spread: 0,
                color: INK,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(stage_h);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = if compact { 6.0 } else { 10.0 };
                    ui.label(caption("NOW SINGING", INK_TINT));
                    ui.add(
                        egui::Label::new(
                            RichText::new(name).font(fredoka_bold(if compact { 26.0 } else { 30.0 })),
                        )
                        .truncate(),
                    );
                    // Mascots with the badge between them.
                    let badge = if host {
                        match sync {
                            Some((e, _)) if e.abs() <= 20.0 => {
                                Some((format!("In time! {e:+.1} ms"), MINT))
                            }
                            Some((e, _)) => Some((format!("Off by {e:+.1} ms"), SUN)),
                            None if playing => Some(("Checking sync...".to_string(), PAPER)),
                            None => None,
                        }
                    } else if playing {
                        Some(("Playing along".to_string(), MINT))
                    } else {
                        None
                    };
                    let mid_w = 130.0;
                    let row_w = host_px + guest_px + mid_w + 48.0;
                    let pad = ((ui.available_width() - row_w) / 2.0).max(0.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 24.0;
                        ui.add_space(pad);
                        mascot(ui, Mode::Host, Mood::Singing, host_px, bob);
                        ui.allocate_ui_with_layout(
                            egui::vec2(mid_w, host_px),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                ui.add_space(if compact { 8.0 } else { 22.0 });
                                paint_note(ui);
                                if let Some((t, c)) = &badge {
                                    pill(ui, t, *c, None);
                                }
                            },
                        );
                        mascot(ui, Mode::Guest, Mood::Singing, guest_px, bob);
                    });
                    if !compact {
                        ui.add(
                            egui::Label::new(
                                RichText::new("You both hear the music at the same moment. Viewers hear you together in OBS.")
                                    .font(theme::nunito(15.0))
                                    .color(INK_TINT),
                            )
                            .wrap(),
                        );
                    }
                    if host && !playing {
                        let can = app.track.is_some()
                            && (!app.connected() || app.track_matches() == Some(true));
                        if ui
                            .add(FunButton::new("Play", Kind::Coral).min_width(160.0).enabled(can))
                            .clicked()
                        {
                            let from = app.paused_at;
                            let cd = app.cfg.countdown_s;
                            app.host_play(from, cd);
                        }
                        if !can && !compact {
                            ui.label(soft("Load the same song on both PCs to play."));
                        }
                    }
                });
            });
    }
    // Three small tiles.
    let app = &g.app;
    let partner = match app.cfg.mode {
        Mode::Host => {
            if app.sync_error_ms().is_some() {
                "Playing in time".to_string()
            } else if app.connected() {
                "Here and ready".into()
            } else {
                "Not connected".into()
            }
        }
        Mode::Guest => {
            if app.connected() {
                "Host is here".into()
            } else {
                "Not connected".into()
            }
        }
    };
    let (t2_cap, t2) = match app.cfg.mode {
        Mode::Host => {
            let applied = app
                .cfg
                .obs_host_mic
                .as_ref()
                .and_then(|n| app.cfg.ledger.applied.get(n))
                .or_else(|| {
                    app.cfg
                        .obs_music
                        .as_ref()
                        .and_then(|n| app.cfg.ledger.applied.get(n))
                });
            (
                "OBS DELAY",
                match applied {
                    Some(v) => format!("Mic & music +{v} ms"),
                    None => "Not sent yet".into(),
                },
            )
        }
        Mode::Guest => (
            "SONG",
            match app.track_matches() {
                Some(true) => "Same file as host".to_string(),
                Some(false) => "Different file!".to_string(),
                None => "Waiting for host".to_string(),
            },
        ),
    };
    let tip = match app.cfg.mode {
        Mode::Host => "Re-measure between songs",
        Mode::Guest => "Keep Discord open",
    };
    ui.columns(3, |cols| {
        for (col, (cap, text)) in
            cols.iter_mut()
                .zip([("PARTNER", partner), (t2_cap, t2), ("TIP", tip.to_string())])
        {
            super::fun::sticker_with(col, PAPER, 18, egui::Margin::symmetric(16, 12), 4, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(caption(cap, INK_SOFT));
                ui.add(egui::Label::new(RichText::new(text).font(fredoka(18.0))).wrap());
            });
        }
    });
}

fn advanced_sing(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Playback"), |ui| {
        let mut changed = false;
        ui.horizontal(|ui| {
            row_label(ui, "Volume");
            changed |= ui
                .add(egui::Slider::new(&mut app.cfg.volume, 0.0..=1.5))
                .changed();
        });
        ui.horizontal(|ui| {
            row_label(ui, "Output latency trim");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut app.cfg.output_latency_ms)
                        .range(-100.0..=500.0)
                        .suffix(" ms"),
                )
                .on_hover_text("Extra latency of your headphones that Windows does not report (Bluetooth is often 150 to 250 ms). Positive values start the music earlier.")
                .changed();
        });
        changed |= ui
            .checkbox(
                &mut app.cfg.drift_correction,
                "Gradual drift correction (+-300 ppm resampling)",
            )
            .changed();
        if app.cfg.mode == Mode::Host {
            ui.horizontal(|ui| {
                row_label(ui, "Countdown");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut app.cfg.countdown_s)
                            .range(2.0..=15.0)
                            .suffix(" s"),
                    )
                    .changed();
                if small_button_if(ui, app.playing(), "Resync now", Kind::White).clicked() {
                    app.resync();
                }
            });
        }
        if changed {
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
                ui.label(soft("Waiting for your partner's playback report..."));
            }
        }
    });
}

// --------------------------------------------------------------- guest steps

fn guest_join(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    if app.guest.is_none() {
        card(ui, None, |ui| {
            ui.label(RichText::new("Invite from your host").font(fredoka(17.0)));
            ui.horizontal(|ui| {
                let btn = FunButton::new("Join", Kind::Mint).min_width(110.0);
                let w = ui.available_width() - btn.measure(ui).x - ui.spacing().item_spacing.x;
                let r = text_field(ui, w, &mut app.join_input, "Paste invite here", MINT, 3.0);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add(btn).clicked() || enter {
                    app.join();
                    app.save();
                }
            });
            ui.label(soft(
                "Can't connect? You both need the same Wi-Fi, or Tailscale on both PCs. If the invite is old, ask your host for a new one.",
            ));
        });
        return;
    }
    card(ui, Some("Your host"), |ui| {
        ui.horizontal(|ui| {
            let (mood, text) = if app.connected() {
                (Mood::Happy, "Connected!")
            } else {
                (Mood::Waiting, "Connecting...")
            };
            mascot(ui, Mode::Host, mood, 46.0, 0.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(text).font(fredoka(17.0)));
                match app.clock_estimate() {
                    Some(e) => ui.label(soft(format!(
                        "Clocks in sync (within {:.1} ms)",
                        e.uncertainty_us / 1000.0
                    ))),
                    None => ui.label(soft(if app.connected() {
                        "Syncing clocks..."
                    } else {
                        "If it takes long, ask your host for a new invite."
                    })),
                };
            });
        });
        if fun_button(ui, "Leave", Kind::Ghost).clicked() {
            app.stop_session();
        }
    });
}

fn guest_audio(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Your headphones"), |ui| {
        ui.label(soft("Where you hear the music."));
        let mut out = app.cfg.output_device.clone();
        if device_combo(ui, "out", &mut out, &app.outputs) {
            app.change_output_device(out);
            app.save();
        }
        meter(ui, app.out_peak);
    });
    card(ui, Some("Where the test sound plays"), |ui| {
        if device_combo(ui, "cal", &mut app.cfg.calib_output_device, &app.outputs) {
            app.save();
        }
        ui.label(soft(
            "Pick speakers that your microphone can hear, or a virtual cable that Discord uses as its microphone. Headphones will not work: the sound never reaches your mic.",
        ));
    });
    card(ui, Some("Check with my microphone"), |ui| {
        ui.checkbox(
            &mut app.cfg.use_guest_reference,
            "Check with my microphone (recommended)",
        );
        if app.cfg.use_guest_reference {
            device_combo(ui, "ref", &mut app.cfg.guest_reference_input, &app.inputs);
            if toggle_chip(ui, "Input level", app.monitor.is_some()).clicked() {
                let src = CaptureSource::Input(app.cfg.guest_reference_input.clone());
                app.toggle_monitor(Some(src));
            }
            if app.monitor.is_some() {
                meter(ui, app.monitor_peak);
            }
        }
        ui.horizontal(|ui| {
            if fun_button(ui, "Test sound path", Kind::Mint).clicked() {
                app.save();
                app.test_routing();
            }
            if fun_button(ui, "Refresh devices", Kind::Ghost).clicked() {
                app.refresh_devices();
            }
        });
        if app.routing_ok {
            check_row(ui, true, "The test sound was heard");
        } else {
            ui.label(soft(
                "Not tested yet. Press the button and listen for a short sweep.",
            ));
        }
    });
}

fn guest_ready(g: &mut Gui, ui: &mut Ui) {
    let app = &mut g.app;
    card(ui, Some("Discord checklist"), |ui| {
        check_row(ui, false, "Turn OFF noise suppression (Krisp) in Discord");
        check_row(ui, false, "Turn OFF echo cancellation in Discord");
        check_row(
            ui,
            false,
            "Set Discord's input sensitivity low, so the test sounds get through",
        );
        ui.label(soft("You can turn them back on after the measurement."));
        ui.horizontal(|ui| {
            row_label(ui, "Test sound level");
            ui.add(egui::Slider::new(&mut app.cfg.chirp_amplitude, 0.05..=0.9));
        });
        if let Some(n) = &app.guest_calib_note {
            check_row(ui, app.calib.is_none() && !n.contains("running"), n);
        }
    });
}
