//! Application controller: owns session, audio, calibration, playback and OBS state.
//! The UI calls methods here and renders the public fields; nothing here draws.

use crate::audio::wasapi::{list_devices, CaptureHandle, RenderHandle, RenderMsg, RingSource};
use crate::audio::{process, CaptureSource, DeviceInfo};
use crate::calibration::stats::Sample;
use crate::calibration::*;
use crate::clock::now_us;
use crate::config::{Config, GuestAudio, Mode};
use crate::diagnostics::{Log, ProcStats, Sampler};
use crate::network::clocksync::Estimate;
use crate::network::crypto;
use crate::network::protocol::GuestCalibReport;
use crate::network::session::{Event, Guest, GuestConfig, Host, HostConfig};
use crate::network::Msg;
use crate::obs::compensation::{plan, OffsetChange, SourceLatency};
use crate::obs::{ApplyResult, InputState, ObsCmd, ObsEvent, ObsWorker};
use crate::playback::core::{Command, State};
use crate::playback::track::probe;
use crate::playback::{Feeder, TrackInfo};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

type Res<T> = Result<T, String>;

/// Calibration run state (host side).
pub struct HostCalib {
    pub plan: CalibrationPlan,
    rec_rx: Receiver<Res<Recording>>,
    rec: Option<Res<Recording>>,
    report: Option<GuestCalibReport>,
    report_deadline_us: i64,
}

pub struct CalibOutcome {
    pub result: CalibrationResult,
    pub at_us: i64,
    pub rtt_median_us: Option<i64>,
    pub reference: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlayInfo {
    /// Protocol generation (host's Play/Pause messages).
    pub gen: u32,
    /// Local ring/feeder generation (changes on every local re-seek).
    pub local_gen: u32,
    pub start_host_us: i64,
    pub from: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RemoteStatus {
    pub state: String,
    pub pos_frames: f64,
    pub at_host_us: i64,
    pub clock_uncertainty_us: i64,
    pub rtt_us: i64,
    pub drift_ms: f64,
    pub received_us: i64,
}

pub struct PendingApply {
    pub changes: Vec<OffsetChange>,
    pub notes: Vec<String>,
}

pub struct App {
    persist: bool,
    pub cfg: Config,
    pub log: Log,
    sampler: Sampler,
    pub stats: ProcStats,
    last_tick_us: i64,
    last_stats_us: i64,
    pub status: Option<(bool, String)>,

    // devices
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub discord: Vec<process::ProcInfo>,

    // session
    pub host: Option<Host>,
    pub guest: Option<Guest>,
    secret: Option<[u8; 16]>,
    pub join_code: Option<String>,
    pub code_expires_us: i64,
    pub peer: Option<String>,
    pub join_input: String,
    pub invite_hosts: Vec<String>,

    // audio
    pub render: Option<RenderHandle>,
    calib_render: Option<(RenderHandle, i64)>,
    pub monitor: Option<CaptureHandle>,
    pub monitor_peak: f32,
    pub out_peak: f32,
    worker_rx: Option<Receiver<WorkerMsg>>,
    pub busy: Option<String>,

    // track & playback
    pub track: Option<TrackInfo>,
    pub track_path: Option<PathBuf>,
    feeder: Option<Feeder>,
    probe_rx: Option<Receiver<Res<(PathBuf, TrackInfo)>>>,
    pub host_track: Option<TrackInfo>,
    pub guest_track_sha: Option<String>,
    pub guest_track_name: Option<String>,
    pub play: Option<PlayInfo>,
    gen: u32,
    local_gen: u32,
    pub paused_at: u64,
    pub remote: Option<RemoteStatus>,

    // calibration
    pub calib: Option<HostCalib>,
    analysis_rx: Option<Receiver<CalibOutcome>>,
    guest_calib_rx: Option<Receiver<GuestCalibReport>>,
    pub last_calib: Option<CalibOutcome>,
    pub use_measured: bool,
    pub guest_calib_note: Option<String>,

    // OBS
    pub obs: ObsWorker,
    pub obs_connected: bool,
    pub obs_version: String,
    pub obs_inputs: Vec<InputState>,
    pub obs_password: String,
    pub obs_busy: bool,
    pub pending_apply: Option<PendingApply>,
    applying: Option<Vec<OffsetChange>>,
    pub last_apply: Vec<ApplyResult>,
    pub video_state: BTreeMap<String, Option<i64>>,
    pub confirm_restore: bool,
}

enum WorkerMsg {
    Info(bool, String),
}

impl App {
    pub fn new() -> Self {
        let (cfg, warn) = Config::load();
        let mut app = Self::with_config(cfg, true);
        if let Some(w) = warn {
            app.log.push(w);
        }
        app
    }

    /// `persist = false` keeps the config in memory only (headless tests).
    pub fn with_config(cfg: Config, persist: bool) -> Self {
        let log = Log::default();
        let mut app = Self {
            persist,
            obs_password: cfg.obs_password.clone().unwrap_or_default(),
            cfg,
            log,
            sampler: Sampler::default(),
            stats: ProcStats::default(),
            last_tick_us: 0,
            last_stats_us: 0,
            status: None,
            inputs: vec![],
            outputs: vec![],
            discord: vec![],
            host: None,
            guest: None,
            secret: None,
            join_code: None,
            code_expires_us: 0,
            peer: None,
            join_input: String::new(),
            invite_hosts: vec![],
            render: None,
            calib_render: None,
            monitor: None,
            monitor_peak: 0.0,
            out_peak: 0.0,
            worker_rx: None,
            busy: None,
            track: None,
            track_path: None,
            feeder: None,
            probe_rx: None,
            host_track: None,
            guest_track_sha: None,
            guest_track_name: None,
            play: None,
            gen: 0,
            local_gen: 0,
            paused_at: 0,
            remote: None,
            calib: None,
            analysis_rx: None,
            guest_calib_rx: None,
            last_calib: None,
            use_measured: true,
            guest_calib_note: None,
            obs: ObsWorker::start(),
            obs_connected: false,
            obs_version: String::new(),
            obs_inputs: vec![],
            obs_busy: false,
            pending_apply: None,
            applying: None,
            last_apply: vec![],
            video_state: BTreeMap::new(),
            confirm_restore: false,
        };
        app.refresh_devices();
        app.join_input = app.cfg.host_address.clone();
        if let Some(p) = app.cfg.last_track.clone() {
            if p.exists() {
                app.load_track(p);
            }
        }
        app
    }

    fn info(&mut self, ok: bool, msg: impl Into<String>) {
        let m = msg.into();
        self.log
            .push(if ok { m.clone() } else { format!("ERROR: {m}") });
        self.status = Some((ok, m));
    }

    pub fn save(&mut self) {
        self.cfg.obs_password = if self.cfg.remember_obs_password && !self.obs_password.is_empty() {
            Some(self.obs_password.clone())
        } else {
            None
        };
        if !self.persist {
            return;
        }
        if let Err(e) = self.cfg.save() {
            self.log.push(format!("config save failed: {e}"));
        }
    }

    pub fn refresh_devices(&mut self) {
        match list_devices(true) {
            Ok(d) => self.inputs = d,
            Err(e) => self.info(false, format!("input devices: {e}")),
        }
        match list_devices(false) {
            Ok(d) => self.outputs = d,
            Err(e) => self.info(false, format!("output devices: {e}")),
        }
        self.discord = process::find_discord(&process::list_processes());
    }

    pub fn device_name(&self, id: &Option<String>, capture: bool) -> String {
        match id {
            None => "System default".into(),
            Some(id) => (if capture { &self.inputs } else { &self.outputs })
                .iter()
                .find(|d| &d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "(unavailable device)".into()),
        }
    }

    // ---------------- periodic ----------------

    /// Poll all channels. Call once per UI frame. Returns true when something is active
    /// that warrants a faster repaint.
    pub fn tick(&mut self) -> bool {
        let now = now_us();
        self.poll_network();
        self.poll_obs();
        self.poll_workers();
        self.poll_calibration(now);
        if let Some(r) = &self.render {
            self.out_peak = self.out_peak.max(r.shared.take_peak()) * 0.85;
            if let Some(e) = r.error() {
                self.render = None;
                self.feeder = None;
                self.info(false, format!("playback stopped: {e}"));
            }
        }
        if let Some(m) = &self.monitor {
            self.monitor_peak = self.monitor_peak.max(m.take_peak()) * 0.85;
            if let Some(e) = m.error() {
                self.monitor = None;
                self.info(false, e);
            }
        }
        if let Some((_, until)) = &self.calib_render {
            if now > *until {
                self.calib_render = None;
            }
        }
        if now - self.last_tick_us >= 1_000_000 {
            self.last_tick_us = now;
            self.every_second(now);
        }
        if now - self.last_stats_us >= 2_000_000 {
            self.last_stats_us = now;
            self.stats = self.sampler.sample();
        }
        self.playing()
            || self.calib.is_some()
            || self.monitor.is_some()
            || self.busy.is_some()
            || self.countdown_s().is_some()
    }

    fn every_second(&mut self, now: i64) {
        self.handle_reseek();
        if self.cfg.mode == Mode::Guest {
            if let (Some(g), Some(est)) = (&self.guest, self.clock_estimate()) {
                if let (Some(p), Some(r)) = (self.play, &self.render) {
                    let _ = r.cmd(Command::Retime {
                        gen: p.local_gen,
                        start_us: est.to_local(p.start_host_us),
                    });
                }
                let (state, pos, drift) = match &self.render {
                    Some(r) => (
                        format!("{:?}", r.shared.state()),
                        r.shared.position_at(now, r.rate as f64),
                        r.shared.drift_us.load(Ordering::Relaxed) as f64 / 1000.0,
                    ),
                    None => ("NoOutput".into(), 0.0, 0.0),
                };
                g.send(Msg::GuestStatus {
                    state,
                    pos_frames: pos,
                    at_host_us: est.to_host(now),
                    clock_uncertainty_us: est.uncertainty_us as i64,
                    rtt_us: est.rtt_median_us as i64,
                    drift_ms: drift,
                });
            }
        }
        if let Some(r) = &self.render {
            if r.shared.state() == State::Ended && self.play.is_some() {
                self.play = None;
                self.paused_at = 0;
                self.log.push("track ended");
            }
        }
    }

    // ---------------- session ----------------

    pub fn start_host(&mut self) {
        self.stop_session();
        let secret = crypto::random_bytes::<16>();
        let deadline = now_us() + self.cfg.join_code_minutes as i64 * 60_000_000;
        let bind = format!("0.0.0.0:{}", self.cfg.listen_port);
        match Host::start(HostConfig {
            bind: bind.parse().unwrap(),
            secret,
            join_deadline_us: deadline,
        }) {
            Ok(h) => {
                self.secret = Some(secret);
                self.join_code = Some(crypto::encode_code(&secret));
                self.code_expires_us = deadline;
                self.invite_hosts = local_addresses()
                    .into_iter()
                    .map(|ip| format!("{ip}:{}", h.local_addr.port()))
                    .collect();
                self.host = Some(h);
                self.info(
                    true,
                    format!("session open on port {}", self.cfg.listen_port),
                );
            }
            Err(e) => self.info(false, e),
        }
    }

    pub fn regenerate_code(&mut self) {
        if let Some(h) = &self.host {
            let secret = crypto::random_bytes::<16>();
            let deadline = now_us() + self.cfg.join_code_minutes as i64 * 60_000_000;
            h.rotate(secret, deadline);
            self.secret = Some(secret);
            self.join_code = Some(crypto::encode_code(&secret));
            self.code_expires_us = deadline;
            self.info(
                true,
                "new join code generated; the old one no longer works for new joins",
            );
        }
    }

    pub fn join(&mut self) {
        self.stop_session();
        let (addr, code) = crate::network::parse_invite(&self.join_input);
        let Some(addr) = addr else {
            return self.info(false, "enter the host address (ip:port#CODE)");
        };
        let Some(code) = code else {
            return self.info(false, "the invite is missing the join code after '#'");
        };
        let secret = match crypto::decode_code(&code) {
            Ok(s) => s,
            Err(e) => return self.info(false, e),
        };
        match Guest::start(GuestConfig {
            host: addr.clone(),
            secret,
        }) {
            Ok(g) => {
                self.cfg.host_address = format!("{addr}#{code}");
                self.guest = Some(g);
                self.info(true, format!("connecting to {addr} ..."));
            }
            Err(e) => self.info(false, e),
        }
    }

    pub fn stop_session(&mut self) {
        if self.host.take().is_some() || self.guest.take().is_some() {
            self.log.push("session closed");
        }
        self.peer = None;
        self.join_code = None;
        self.secret = None;
        self.remote = None;
        self.guest_track_sha = None;
        self.host_track = None;
    }

    pub fn connected(&self) -> bool {
        self.host.as_ref().is_some_and(|h| h.connected())
            || self.guest.as_ref().is_some_and(|g| g.connected())
    }

    pub fn clock_estimate(&self) -> Option<Estimate> {
        self.guest.as_ref().and_then(|g| g.clock())
    }

    fn send(&self, m: Msg) -> bool {
        if let Some(h) = &self.host {
            h.send(m)
        } else if let Some(g) = &self.guest {
            g.send(m)
        } else {
            false
        }
    }

    fn poll_network(&mut self) {
        let mut evs = Vec::new();
        if let Some(h) = &self.host {
            evs.extend(h.events.try_iter());
        }
        if let Some(g) = &self.guest {
            evs.extend(g.events.try_iter());
        }
        for e in evs {
            match e {
                Event::Connected(p) => {
                    self.info(true, format!("connected: {p}"));
                    self.peer = Some(p);
                    if self.cfg.mode == Mode::Host {
                        self.send(Msg::Track(self.track.clone()));
                    } else {
                        self.send_track_status();
                    }
                }
                Event::Disconnected(r) => {
                    self.peer = None;
                    self.info(false, format!("disconnected: {r}"));
                }
                Event::AuthFailed(r) => {
                    self.info(false, format!("join refused: {r}"));
                    if self.cfg.mode == Mode::Guest {
                        self.guest = None;
                    }
                }
                Event::Info(s) => self.log.push(s),
                Event::Msg(m) => self.on_msg(m),
            }
        }
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Track(t) => {
                self.host_track = t;
                self.send_track_status();
            }
            Msg::TrackStatus { sha256, name } => {
                self.guest_track_sha = sha256;
                self.guest_track_name = name;
            }
            Msg::Play {
                gen,
                start_host_us,
                from_frame,
            } => self.guest_play(gen, start_host_us, from_frame),
            Msg::Pause { gen, at_frame } => {
                if self.play.is_some_and(|p| p.gen == gen) {
                    if let Some(r) = &self.render {
                        let _ = r.cmd(Command::Pause);
                    }
                    self.play = None;
                    self.paused_at = at_frame;
                }
            }
            Msg::Stop => self.local_stop(),
            Msg::CalibStart(plan) => self.guest_calibrate(plan),
            Msg::CalibReport(r) => {
                if let Some(c) = &mut self.calib {
                    c.report = Some(r);
                }
            }
            Msg::GuestStatus {
                state,
                pos_frames,
                at_host_us,
                clock_uncertainty_us,
                rtt_us,
                drift_ms,
            } => {
                self.remote = Some(RemoteStatus {
                    state,
                    pos_frames,
                    at_host_us,
                    clock_uncertainty_us,
                    rtt_us,
                    drift_ms,
                    received_us: now_us(),
                });
            }
            Msg::Notice(s) => self.info(true, format!("peer: {s}")),
            Msg::Ping { .. } | Msg::Pong { .. } | Msg::Bye => {}
        }
    }

    // ---------------- track & playback ----------------

    pub fn load_track(&mut self, path: PathBuf) {
        let (tx, rx) = channel();
        self.busy = Some("verifying instrumental (SHA-256)...".into());
        std::thread::spawn(move || {
            let _ = tx.send(probe(&path).map(|i| (path, i)));
        });
        self.probe_rx = Some(rx);
    }

    fn on_probed(&mut self, path: PathBuf, info: TrackInfo) {
        self.local_stop();
        self.cfg.last_track = Some(path.clone());
        self.log.push(format!(
            "loaded {} ({:.1} s, {} Hz, sha256 {}…)",
            info.name,
            info.duration_s(),
            info.sample_rate,
            &info.sha256[..12]
        ));
        self.track = Some(info);
        self.track_path = Some(path);
        if let Err(e) = self.attach_track() {
            self.info(false, e);
        }
        if self.cfg.mode == Mode::Host {
            self.send(Msg::Track(self.track.clone()));
        } else {
            self.send_track_status();
        }
    }

    fn send_track_status(&self) {
        if self.guest.is_some() {
            self.send(Msg::TrackStatus {
                sha256: self.track.as_ref().map(|t| t.sha256.clone()),
                name: self.track.as_ref().map(|t| t.name.clone()),
            });
        }
    }

    /// (Re)open the output at the track's rate and hand it a fresh feeder ring.
    fn attach_track(&mut self) -> Res<()> {
        let (Some(info), Some(path)) = (self.track.clone(), self.track_path.clone()) else {
            return Ok(());
        };
        self.ensure_render(info.sample_rate)?;
        let (feeder, cons, eof) = Feeder::open(&path, info)?;
        self.render
            .as_ref()
            .unwrap()
            .send(RenderMsg::Source(Some(RingSource { cons, eof_gen: eof })))?;
        self.feeder = Some(feeder);
        Ok(())
    }

    fn ensure_render(&mut self, rate: u32) -> Res<()> {
        let ok = self
            .render
            .as_ref()
            .is_some_and(|r| r.rate == rate && r.shared.alive.load(Ordering::Acquire));
        if ok {
            return Ok(());
        }
        self.render = None;
        self.feeder = None;
        let r = RenderHandle::start(self.cfg.output_device.clone(), rate)?;
        self.log.push(format!("output: {}", r.description));
        let _ = r.cmd(Command::SetGain {
            gain: self.cfg.volume,
        });
        let _ = r.cmd(Command::SetOutputLatency {
            us: (self.cfg.output_latency_ms * 1000.0) as i64,
        });
        let _ = r.cmd(Command::SetDriftCorrection {
            enabled: self.cfg.drift_correction,
        });
        self.render = Some(r);
        Ok(())
    }

    pub fn change_output_device(&mut self, id: Option<String>) {
        self.cfg.output_device = id;
        self.local_stop();
        self.render = None;
        self.feeder = None;
        if let Err(e) = self.attach_track() {
            self.info(false, e);
        }
    }

    pub fn apply_render_settings(&mut self) {
        if let Some(r) = &self.render {
            let _ = r.cmd(Command::SetGain {
                gain: self.cfg.volume,
            });
            let _ = r.cmd(Command::SetOutputLatency {
                us: (self.cfg.output_latency_ms * 1000.0) as i64,
            });
            let _ = r.cmd(Command::SetDriftCorrection {
                enabled: self.cfg.drift_correction,
            });
        }
    }

    pub fn track_matches(&self) -> Option<bool> {
        let mine = self.track.as_ref()?.sha256.clone();
        match self.cfg.mode {
            Mode::Host => self.guest_track_sha.as_ref().map(|s| *s == mine),
            Mode::Guest => self.host_track.as_ref().map(|t| t.sha256 == mine),
        }
    }

    pub fn playing(&self) -> bool {
        self.play.is_some()
    }

    pub fn position_frames(&self) -> f64 {
        match (&self.render, self.play) {
            (Some(r), Some(_)) => r.shared.position_at(now_us(), r.rate as f64),
            _ => self.paused_at as f64,
        }
    }

    pub fn countdown_s(&self) -> Option<f64> {
        let p = self.play?;
        let start_local = match self.cfg.mode {
            Mode::Host => p.start_host_us,
            Mode::Guest => self.clock_estimate()?.to_local(p.start_host_us),
        };
        let d = (start_local - now_us()) as f64 / 1e6;
        (d > 0.0).then_some(d)
    }

    /// Host: schedule playback from `from` after the countdown (also used for seek/resume).
    pub fn host_play(&mut self, from: u64, countdown_s: f64) {
        if self.cfg.mode != Mode::Host {
            return;
        }
        if self.track.is_none() {
            return self.info(false, "load an instrumental first");
        }
        if self.connected() && self.track_matches() != Some(true) {
            return self.info(
                false,
                "guest's instrumental does not match (or is not loaded)",
            );
        }
        if let Err(e) = self.attach_if_needed() {
            return self.info(false, e);
        }
        self.gen = self.gen.wrapping_add(1);
        let start = now_us() + (countdown_s * 1e6) as i64;
        let (local_gen, s2, f2) = self.start_local(start, from);
        self.play = Some(PlayInfo {
            gen: self.gen,
            local_gen,
            start_host_us: s2,
            from: f2,
        });
        self.send(Msg::Play {
            gen: self.gen,
            start_host_us: start,
            from_frame: from,
        });
    }

    fn attach_if_needed(&mut self) -> Res<()> {
        let alive = self
            .render
            .as_ref()
            .is_some_and(|r| r.shared.alive.load(Ordering::Acquire));
        if !alive || self.feeder.is_none() {
            self.attach_track()?;
        }
        Ok(())
    }

    /// Start the local player so track frame `from` is audible at `start_local_us`.
    /// A start less than 300 ms away is re-anchored 500 ms ahead (same musical timeline).
    /// Returns (local ring generation, effective start, effective from).
    fn start_local(&mut self, start_local_us: i64, from: u64) -> (u32, i64, u64) {
        self.local_gen = self.local_gen.wrapping_add(1);
        let gen = self.local_gen;
        let now = now_us();
        let (start_local_us, from) = if start_local_us.saturating_sub(now) < 300_000 {
            let reanchored_start_us = now.saturating_add(500_000);
            let elapsed_us = reanchored_start_us.saturating_sub(start_local_us).max(0);
            let advance = (elapsed_us as u64).saturating_mul(self.track_rate() as u64) / 1_000_000;
            (reanchored_start_us, from.saturating_add(advance))
        } else {
            (start_local_us, from)
        };
        if let (Some(f), Some(r)) = (&self.feeder, &self.render) {
            f.seek(gen, from);
            let _ = r.cmd(Command::Play {
                gen,
                start_us: start_local_us,
                from,
            });
        }
        (gen, start_local_us, from)
    }

    /// The player could not skip far enough within its buffer: restart on the same
    /// timeline from the frame that should be playing now.
    fn handle_reseek(&mut self) {
        let Some(r) = &self.render else { return };
        if r.shared.reseek.swap(u64::MAX, Ordering::AcqRel) == u64::MAX {
            return;
        }
        let Some(p) = self.play else { return };
        let start_local = match self.cfg.mode {
            Mode::Host => p.start_host_us,
            Mode::Guest => match self.clock_estimate() {
                Some(e) => e.to_local(p.start_host_us),
                None => return,
            },
        };
        let (local_gen, s2, f2) = self.start_local(start_local, p.from);
        self.play = Some(PlayInfo {
            local_gen,
            start_host_us: p.start_host_us + (s2 - start_local),
            from: f2,
            ..p
        });
        self.log
            .push("player re-seeked to stay on the shared timeline");
    }

    fn guest_play(&mut self, gen: u32, start_host_us: i64, from: u64) {
        if self.track_matches() != Some(true) {
            self.send(Msg::Notice(
                "guest cannot play: instrumental missing or different".into(),
            ));
            return self.info(
                false,
                "host started playback but your instrumental does not match",
            );
        }
        let Some(est) = self.clock_estimate() else {
            self.send(Msg::Notice(
                "guest cannot play: clock not synchronized yet".into(),
            ));
            return self.info(false, "clock not synchronized yet");
        };
        if let Err(e) = self.attach_if_needed() {
            return self.info(false, e);
        }
        let start_local = est.to_local(start_host_us);
        let (local_gen, s2, f2) = self.start_local(start_local, from);
        // Keep the host-clock anchor consistent with the (possibly re-anchored) start, so
        // later Retime commands describe the same timeline.
        self.play = Some(PlayInfo {
            gen,
            local_gen,
            start_host_us: start_host_us + (s2 - start_local),
            from: f2,
        });
    }

    pub fn pause(&mut self) {
        let Some(p) = self.play else { return };
        let pos = self.position_frames() as u64;
        if let Some(r) = &self.render {
            let _ = r.cmd(Command::Pause);
        }
        self.play = None;
        self.paused_at = pos;
        if self.cfg.mode == Mode::Host {
            self.send(Msg::Pause {
                gen: p.gen,
                at_frame: pos,
            });
        }
    }

    pub fn stop(&mut self) {
        self.local_stop();
        if self.cfg.mode == Mode::Host {
            self.send(Msg::Stop);
        }
    }

    fn local_stop(&mut self) {
        if let Some(r) = &self.render {
            let _ = r.cmd(Command::Stop);
        }
        if let Some(f) = &self.feeder {
            f.idle();
        }
        self.play = None;
        self.paused_at = 0;
    }

    pub fn seek(&mut self, frame: u64) {
        if self.playing() {
            self.host_play(frame, 1.5);
        } else {
            self.paused_at = frame;
        }
    }

    pub fn resync(&mut self) {
        if self.cfg.mode == Mode::Host && self.playing() {
            let pos = self.position_frames() as u64;
            self.host_play(pos + self.track_rate() as u64 * 2, 2.0);
        }
    }

    pub fn track_rate(&self) -> u32 {
        self.track
            .as_ref()
            .map(|t| t.sample_rate)
            .unwrap_or(crate::SAMPLE_RATE)
    }

    /// Host: measured guest-vs-host timeline difference (ms) and its uncertainty.
    pub fn sync_error_ms(&self) -> Option<(f64, f64)> {
        let r = self.remote.as_ref()?;
        if now_us() - r.received_us > 3_000_000 || !r.state.contains("Playing") {
            return None;
        }
        let render = self.render.as_ref()?;
        if render.shared.state() != State::Playing {
            return None;
        }
        let rate = render.rate as f64;
        let host_pos = render.shared.position_at(r.at_host_us, rate);
        Some((
            (r.pos_frames - host_pos) / rate * 1000.0,
            r.clock_uncertainty_us as f64 / 1000.0,
        ))
    }

    // ---------------- capture helpers ----------------

    pub fn guest_capture_source(&self) -> Res<CaptureSource> {
        Ok(match &self.cfg.guest_audio {
            GuestAudio::DiscordProcess => {
                let d = process::find_discord(&process::list_processes());
                let p = d
                    .first()
                    .ok_or("Discord is not running (or use an output/input device source)")?;
                CaptureSource::Process {
                    pid: p.pid,
                    name: p.name.clone(),
                }
            }
            GuestAudio::OutputLoopback(id) => CaptureSource::OutputLoopback(id.clone()),
            GuestAudio::Input(id) => CaptureSource::Input(id.clone()),
        })
    }

    pub fn toggle_monitor(&mut self, src: Option<CaptureSource>) {
        if self.monitor.take().is_some() {
            return;
        }
        let Some(src) = src else { return };
        match CaptureHandle::start(src) {
            Ok(c) => {
                self.log.push(format!("level monitor: {}", c.description));
                self.monitor = Some(c);
            }
            Err(e) => self.info(false, e),
        }
    }

    /// Guest: play one chirp on the calibration output and look for it on the reference
    /// input — verifies the routing before a real calibration.
    pub fn test_routing(&mut self) {
        let chirp = ChirpSpec {
            amplitude: self.cfg.chirp_amplitude,
            ..Default::default()
        };
        let src = CaptureSource::Input(self.cfg.guest_reference_input.clone());
        let at = now_us() + 700_000;
        let sent = self.calibration_render(5_000_000).and_then(|render| {
            let samples: Arc<[f32]> = chirp.generate(render.rate).into();
            render.cmd(Command::Clip { at_us: at, samples })
        });
        if let Err(e) = sent {
            return self.info(false, e);
        }
        let (tx, rx) = channel();
        self.busy = Some("testing calibration routing...".into());
        std::thread::spawn(move || {
            let r = (|| -> Res<String> {
                let mut cap = CaptureHandle::start(src)?;
                let rec = cap.record_until(at + 900_000)?;
                let found =
                    locate_chirps(&rec, &[Some(at)], &chirp, 600.0, &DetectorParams::default());
                match &found[0] {
                    Ok((t, d)) => Ok(format!(
                        "routing OK: test signal reached the input {:+.1} ms after playback (correlation {:.2}{})",
                        (t - at) as f64 / 1000.0,
                        d.ncc,
                        if d.clipped_samples > 0 { ", CLIPPED: lower volume" } else { "" }
                    )),
                    Err(e) => Err(format!("routing FAILED: test signal not heard on the input ({e})")),
                }
            })();
            let _ = tx.send(match r {
                Ok(s) => WorkerMsg::Info(true, s),
                Err(e) => WorkerMsg::Info(false, e),
            });
        });
        self.worker_rx = Some(rx);
    }

    /// Host: capture the guest-audio source briefly and report its level.
    pub fn test_guest_capture(&mut self) {
        let src = match self.guest_capture_source() {
            Ok(s) => s,
            Err(e) => return self.info(false, e),
        };
        let (tx, rx) = channel();
        self.busy = Some("listening to guest audio for 3 s...".into());
        std::thread::spawn(move || {
            let r = (|| -> Res<String> {
                let mut cap = CaptureHandle::start(src)?;
                let desc = cap.description.clone();
                let rec = cap.record_until(now_us() + 3_000_000)?;
                let peak = rec.samples.iter().fold(0f32, |a, v| a.max(v.abs()));
                let db = 20.0 * peak.max(1e-9).log10();
                Ok(if peak < 1e-4 {
                    format!("{desc}: silence for 3 s — ask the guest to speak, or check the source")
                } else {
                    format!("{desc}: audio present, peak {db:.1} dBFS")
                })
            })();
            let _ = tx.send(match r {
                Ok(s) => WorkerMsg::Info(true, s),
                Err(e) => WorkerMsg::Info(false, e),
            });
        });
        self.worker_rx = Some(rx);
    }

    fn poll_workers(&mut self) {
        if let Some(rx) = &self.probe_rx {
            if let Ok(r) = rx.try_recv() {
                self.probe_rx = None;
                self.busy = None;
                match r {
                    Ok((p, i)) => self.on_probed(p, i),
                    Err(e) => self.info(false, e),
                }
            }
        }
        if let Some(rx) = &self.worker_rx {
            if let Ok(WorkerMsg::Info(ok, s)) = rx.try_recv() {
                self.worker_rx = None;
                self.busy = None;
                self.info(ok, s);
            }
        }
        if let Some(rx) = &self.guest_calib_rx {
            if let Ok(rep) = rx.try_recv() {
                self.guest_calib_rx = None;
                self.busy = None;
                let detected = rep.mic_times_host_us.iter().filter(|t| t.is_some()).count();
                self.guest_calib_note = Some(if rep.reference_capture {
                    format!(
                        "your input heard {detected}/{} test signals",
                        rep.mic_times_host_us.len()
                    )
                } else {
                    "test signals played (no reference input)".into()
                });
                self.send(Msg::CalibReport(rep));
            }
        }
    }

    // ---------------- calibration ----------------

    fn calibration_render(&mut self, keep_us: i64) -> Res<&RenderHandle> {
        let same = self.cfg.calib_output_device == self.cfg.output_device;
        if same {
            if self.render.is_none() {
                self.ensure_render(self.track_rate())?;
                let _ = self.attach_track();
            }
            return self.render.as_ref().ok_or_else(|| "no output".to_string());
        }
        let until = now_us() + keep_us;
        let reuse = self
            .calib_render
            .as_ref()
            .is_some_and(|(r, _)| r.shared.alive.load(Ordering::Acquire));
        if !reuse {
            let r = RenderHandle::start(self.cfg.calib_output_device.clone(), crate::SAMPLE_RATE)?;
            self.log
                .push(format!("calibration output: {}", r.description));
            self.calib_render = Some((r, until));
        }
        let (r, u) = self.calib_render.as_mut().unwrap();
        *u = (*u).max(until);
        Ok(r)
    }

    pub fn can_calibrate(&self) -> Result<(), String> {
        if self.cfg.mode != Mode::Host {
            return Err("calibration is started by the host".into());
        }
        if !self.connected() {
            return Err("guest not connected".into());
        }
        if self.remote.is_none() {
            return Err("waiting for guest clock sync".into());
        }
        if self.calib.is_some() {
            return Err("calibration running".into());
        }
        if self.playing() {
            return Err("stop playback first".into());
        }
        Ok(())
    }

    pub fn start_calibration(&mut self) {
        if let Err(e) = self.can_calibrate() {
            return self.info(false, e);
        }
        let src = match self.guest_capture_source() {
            Ok(s) => s,
            Err(e) => return self.info(false, e),
        };
        let mut plan = CalibrationPlan::new(
            now_us() + 2_500_000,
            self.cfg.chirp_count,
            self.cfg.chirp_spacing_ms,
        );
        plan.chirp.amplitude = self.cfg.chirp_amplitude;
        let end = plan.end_host_us() + 300_000;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = CaptureHandle::start(src).and_then(|mut c| c.record_until(end));
            let _ = tx.send(r); // capture handle dropped here: temporary capture stops
        });
        self.send(Msg::CalibStart(plan.clone()));
        self.log.push(format!(
            "calibration: {} test signals, {:.1} s",
            plan.times_host_us.len(),
            (end - now_us()) as f64 / 1e6
        ));
        self.calib = Some(HostCalib {
            plan,
            rec_rx: rx,
            rec: None,
            report: None,
            report_deadline_us: end + 6_000_000,
        });
    }

    pub fn calibration_progress(&self) -> Option<f64> {
        let c = self.calib.as_ref()?;
        let start = c.plan.times_host_us[0] - 2_500_000;
        let end = c.plan.end_host_us();
        Some(((now_us() - start) as f64 / (end - start) as f64).clamp(0.0, 1.0))
    }

    fn poll_calibration(&mut self, now: i64) {
        if let Some(rx) = &self.analysis_rx {
            if let Ok(o) = rx.try_recv() {
                self.analysis_rx = None;
                let r = &o.result;
                let msg = match r.median_ms {
                    Some(m) => format!(
                        "calibration {:?}: guest latency {m:.1} ms (jitter {:.1} ms, ±{:.1} ms, {}/{} signals)",
                        r.status,
                        r.jitter_ms.unwrap_or(0.0),
                        r.uncertainty_ms.unwrap_or(0.0),
                        r.accepted,
                        r.total
                    ),
                    None => format!("calibration failed: {}", r.messages.join("; ")),
                };
                let ok = r.status != CalibrationStatus::Invalid;
                self.last_calib = Some(o);
                self.info(ok, msg);
            }
        }
        let Some(c) = &mut self.calib else { return };
        if c.rec.is_none() {
            if let Ok(r) = c.rec_rx.try_recv() {
                c.rec = Some(r);
            }
        }
        let ready = c.rec.is_some() && (c.report.is_some() || now > c.report_deadline_us);
        if !ready {
            return;
        }
        let c = self.calib.take().unwrap();
        let rec = match c.rec.unwrap() {
            Ok(r) => r,
            Err(e) => return self.info(false, format!("calibration capture failed: {e}")),
        };
        let Some(report) = c.report else {
            return self.info(
                false,
                "guest did not report calibration results (connection?)",
            );
        };
        for e in &report.errors {
            self.log.push(format!("guest: {e}"));
        }
        let plan = c.plan;
        let rtt = self.remote.as_ref().map(|r| r.rtt_us);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let refs: Vec<Option<i64>> = if report.reference_capture {
                report.mic_times_host_us.clone()
            } else {
                plan.times_host_us.iter().map(|&t| Some(t)).collect()
            };
            let found = locate_chirps(
                &rec,
                &refs,
                &plan.chirp,
                plan.max_latency_ms,
                &DetectorParams::default(),
            );
            let per = found
                .into_iter()
                .zip(&refs)
                .map(|(r, rf)| match (r, rf) {
                    (Ok((t, d)), Some(rf)) => Ok(Sample {
                        latency_ms: (t - rf) as f64 / 1000.0,
                        ncc: d.ncc,
                        clipped: d.clipped_samples > 0,
                    }),
                    (Err(e), Some(_)) => Err(format!("not found in Discord audio: {e}")),
                    (_, None) => Err("not heard by guest's reference input".into()),
                })
                .collect();
            let mut result = analyze(per, report.clock_uncertainty_us as f64 / 1000.0, 4);
            if result.status == CalibrationStatus::Invalid && result.accepted == 0 {
                result.messages.push(
                    "Check: guest's Discord noise suppression / Krisp and echo cancellation OFF during calibration, input sensitivity not gating, correct guest-audio source on the host".into(),
                );
            }
            let _ = tx.send(CalibOutcome {
                result,
                at_us: now_us(),
                rtt_median_us: rtt,
                reference: report.reference_capture,
            });
        });
        self.analysis_rx = Some(rx);
    }

    fn guest_calibrate(&mut self, plan: CalibrationPlan) {
        let mut errors = Vec::new();
        let Some(est) = self.clock_estimate() else {
            self.send(Msg::CalibReport(GuestCalibReport {
                mic_times_host_us: vec![None; plan.times_host_us.len()],
                reference_capture: false,
                clock_uncertainty_us: 0,
                errors: vec!["guest clock not synchronized".into()],
            }));
            return;
        };
        let local: Vec<i64> = plan
            .times_host_us
            .iter()
            .map(|&t| est.to_local(t))
            .collect();
        let chirp = ChirpSpec {
            amplitude: self.cfg.chirp_amplitude,
            ..plan.chirp.clone()
        };
        let end_local = est.to_local(plan.end_host_us());
        match self.calibration_render(end_local - now_us() + 1_000_000) {
            Ok(r) => {
                let samples: Arc<[f32]> = chirp.generate(r.rate).into();
                for &t in &local {
                    if let Err(e) = r.cmd(Command::Clip {
                        at_us: t,
                        samples: samples.clone(),
                    }) {
                        errors.push(e);
                        break;
                    }
                }
            }
            Err(e) => errors.push(format!("calibration output: {e}")),
        }
        self.guest_calib_note = Some("calibration running: keep quiet...".into());
        let reference = self.cfg.use_guest_reference;
        let src = CaptureSource::Input(self.cfg.guest_reference_input.clone());
        let (tx, rx) = channel();
        self.busy = Some("calibration: playing test signals".into());
        std::thread::spawn(move || {
            let n = local.len();
            let mut rep = GuestCalibReport {
                mic_times_host_us: vec![None; n],
                reference_capture: reference,
                clock_uncertainty_us: est.uncertainty_us as i64,
                errors,
            };
            if reference {
                match CaptureHandle::start(src).and_then(|mut c| c.record_until(end_local)) {
                    Ok(rec) => {
                        let refs: Vec<Option<i64>> = local.iter().map(|&t| Some(t)).collect();
                        let found =
                            locate_chirps(&rec, &refs, &chirp, 300.0, &DetectorParams::default());
                        for (k, f) in found.into_iter().enumerate() {
                            match f {
                                Ok((t, _)) => rep.mic_times_host_us[k] = Some(est.to_host(t)),
                                Err(e) => {
                                    rep.errors.push(format!("signal {k} at guest input: {e}"))
                                }
                            }
                        }
                    }
                    Err(e) => {
                        rep.errors.push(format!("reference input: {e}"));
                    }
                }
            } else {
                crate::clock::sleep_until(end_local);
            }
            let _ = tx.send(rep);
        });
        self.guest_calib_rx = Some(rx);
    }

    /// Guest latency used for compensation (measured median or manual) + fine tune.
    pub fn guest_latency_ms(&self) -> Option<f64> {
        let base = if self.use_measured {
            self.last_calib
                .as_ref()
                .filter(|o| o.result.status != CalibrationStatus::Invalid)
                .and_then(|o| o.result.median_ms)
                .unwrap_or(self.cfg.manual_guest_latency_ms)
        } else {
            self.cfg.manual_guest_latency_ms
        };
        Some(base + self.cfg.fine_tune_ms)
    }

    /// Reasons the current measurement may be outdated.
    pub fn staleness(&self) -> Vec<String> {
        let mut v = Vec::new();
        let Some(o) = &self.last_calib else {
            return v;
        };
        let age = (now_us() - o.at_us) / 60_000_000;
        if age >= 10 {
            v.push(format!(
                "measured {age} min ago; Discord latency drifts, recalibrate between songs"
            ));
        }
        if let (Some(a), Some(r)) = (o.rtt_median_us, &self.remote) {
            if (r.rtt_us - a).abs() > 20_000 {
                v.push(format!(
                    "network round trip changed {:.0} -> {:.0} ms since calibration (not proof, but recalibrate)",
                    a as f64 / 1000.0,
                    r.rtt_us as f64 / 1000.0
                ));
            }
        }
        v.push(
            "DuetSync cannot observe Discord's latency during a song: compensation is static"
                .into(),
        );
        v
    }

    // ---------------- OBS ----------------

    pub fn obs_connect(&mut self) {
        self.obs_busy = true;
        self.obs.send(ObsCmd::Connect {
            host: self.cfg.obs_host.clone(),
            port: self.cfg.obs_port,
            password: (!self.obs_password.is_empty()).then(|| self.obs_password.clone()),
        });
    }

    pub fn obs_refresh(&mut self) {
        self.obs_busy = true;
        self.obs.send(ObsCmd::Refresh);
    }

    fn poll_obs(&mut self) {
        while let Ok(e) = self.obs.events.try_recv() {
            match e {
                ObsEvent::Connected { obs, ws } => {
                    self.obs_connected = true;
                    self.obs_version = format!("OBS {obs}, obs-websocket {ws}");
                    self.info(true, format!("connected to {}", self.obs_version));
                }
                ObsEvent::Disconnected(r) => {
                    self.obs_busy = false;
                    self.obs_connected = false;
                    self.applying = None;
                    self.info(false, format!("OBS: {r}"));
                }
                ObsEvent::Error(r) => {
                    self.obs_busy = false;
                    self.applying = None;
                    self.info(false, format!("OBS: {r}"));
                }
                ObsEvent::Inputs(i) => {
                    self.obs_busy = false;
                    self.obs_inputs = i;
                }
                ObsEvent::Offsets { restore, results } => {
                    self.obs_busy = false;
                    let ok = results.iter().all(|r| r.verified());
                    if restore {
                        for r in results.iter().filter(|r| r.verified()) {
                            self.cfg.ledger.forget(&r.input);
                        }
                    } else if let Some(ch) = self.applying.take() {
                        for c in &ch {
                            if results.iter().any(|r| r.input == c.input && r.verified()) {
                                self.cfg.ledger.record(c);
                            }
                        }
                    }
                    self.save();
                    let what = if restore { "restored" } else { "applied" };
                    self.info(
                        ok,
                        if ok {
                            format!("OBS offsets {what} and verified ({} inputs)", results.len())
                        } else {
                            format!("some OBS offsets were NOT {what}: see the OBS tab")
                        },
                    );
                    self.last_apply = results;
                }
                ObsEvent::VideoDelay { source, ms } => {
                    self.obs_busy = false;
                    self.info(
                        true,
                        match ms {
                            Some(v) => format!("video delay on '{source}': {v} ms"),
                            None => format!("video delay removed from '{source}'"),
                        },
                    );
                    self.video_state.insert(source, ms);
                }
            }
        }
    }

    pub fn source_latencies(&self) -> Vec<SourceLatency> {
        let mut v = Vec::new();
        if let Some(n) = &self.cfg.obs_host_mic {
            v.push(SourceLatency {
                input: n.clone(),
                latency_ms: self.cfg.host_mic_latency_ms,
            });
        }
        if let Some(n) = &self.cfg.obs_music {
            v.push(SourceLatency {
                input: n.clone(),
                latency_ms: self.cfg.music_latency_ms,
            });
        }
        if let (Some(n), Some(l)) = (&self.cfg.obs_guest, self.guest_latency_ms()) {
            v.push(SourceLatency {
                input: n.clone(),
                latency_ms: l,
            });
        }
        v
    }

    pub fn current_offsets(&self) -> BTreeMap<String, i64> {
        self.obs_inputs
            .iter()
            .filter_map(|i| i.offset_ms.map(|o| (i.name.clone(), o)))
            .collect()
    }

    /// Compute changes and open the confirmation dialog.
    pub fn prepare_apply(&mut self) {
        match plan(
            &self.source_latencies(),
            self.cfg.margin_ms,
            &self.current_offsets(),
            &self.cfg.ledger,
        ) {
            Ok((changes, notes)) => self.pending_apply = Some(PendingApply { changes, notes }),
            Err(e) => self.info(false, e),
        }
    }

    pub fn confirm_apply(&mut self) {
        let Some(p) = self.pending_apply.take() else {
            return;
        };
        self.obs_busy = true;
        self.obs.send(ObsCmd::SetOffsets {
            restore: false,
            targets: p
                .changes
                .iter()
                .map(|c| (c.input.clone(), c.target_ms))
                .collect(),
        });
        self.applying = Some(p.changes);
    }

    pub fn restore_offsets(&mut self) {
        let targets: Vec<(String, i64)> = self
            .cfg
            .ledger
            .baseline
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        if targets.is_empty() {
            return self.info(
                true,
                "nothing to restore: DuetSync has not changed any OBS offsets",
            );
        }
        self.obs_busy = true;
        self.obs.send(ObsCmd::SetOffsets {
            restore: true,
            targets,
        });
    }

    pub fn set_video_delay(&mut self, source: String, ms: i64) {
        self.obs_busy = true;
        self.obs.send(ObsCmd::SetVideoDelay { source, ms });
    }

    pub fn remove_video_delay(&mut self, source: String) {
        self.obs_busy = true;
        self.obs.send(ObsCmd::RemoveVideoDelay { source });
    }

    // ---------------- diagnostics ----------------

    pub fn export_diagnostics(&mut self) {
        let mut cfg = self.cfg.clone();
        cfg.obs_password = cfg.obs_password.map(|_| "<redacted>".into());
        cfg.host_address =
            cfg.host_address.split('#').next().unwrap_or("").to_string() + "#<redacted>";
        let mut sections: Vec<(&str, String)> = vec![
            (
                "Process",
                format!(
                    "CPU {:.2} %, working set {:.1} MB, private {:.1} MB",
                    self.stats.cpu_percent, self.stats.working_set_mb, self.stats.private_mb
                ),
            ),
            ("Network", self.network_summary()),
            ("Audio", self.audio_summary()),
            (
                "Calibration",
                self.last_calib
                    .as_ref()
                    .map(|o| serde_json::to_string_pretty(&o.result).unwrap_or_default())
                    .unwrap_or_else(|| "none".into()),
            ),
            (
                "OBS inputs",
                self.obs_inputs
                    .iter()
                    .map(|i| {
                        format!(
                            "{} [{}] offset {:?} monitor {} muted {}",
                            i.name, i.kind, i.offset_ms, i.monitor, i.muted
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "Config",
                serde_json::to_string_pretty(&cfg).unwrap_or_default(),
            ),
        ];
        sections.push((
            "Log",
            self.log.lines().cloned().collect::<Vec<_>>().join("\n"),
        ));
        let dir = crate::config::config_path()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("duetsync-diagnostics-{secs}.txt"));
        match crate::diagnostics::export(&path, &sections) {
            Ok(()) => self.info(true, format!("diagnostics saved to {}", path.display())),
            Err(e) => self.info(false, e),
        }
    }

    pub fn network_summary(&self) -> String {
        let mut s = String::new();
        let stats = self
            .host
            .as_ref()
            .map(|h| h.stats())
            .or(self.guest.as_ref().map(|g| g.stats()));
        if let Some(st) = stats {
            s += &format!(
                "connected {}, frames rx {} tx {}, reconnects {}\n",
                st.connected.load(Ordering::Relaxed),
                st.rx_frames.load(Ordering::Relaxed),
                st.tx_frames.load(Ordering::Relaxed),
                st.reconnects.load(Ordering::Relaxed)
            );
        } else {
            s += "no session\n";
        }
        if let Some(e) = self.clock_estimate() {
            s += &format!(
                "clock offset {:.1} ms ±{:.1} ms, drift {:.1} ppm, rtt min {:.1} / median {:.1} ms, samples {}\n",
                e.offset_us / 1000.0,
                e.uncertainty_us / 1000.0,
                e.drift_ppm,
                e.rtt_min_us / 1000.0,
                e.rtt_median_us / 1000.0,
                e.samples
            );
        }
        if let Some(r) = &self.remote {
            s += &format!(
                "guest: {} rtt {:.1} ms clock ±{:.1} ms drift {:.2} ms\n",
                r.state,
                r.rtt_us as f64 / 1000.0,
                r.clock_uncertainty_us as f64 / 1000.0,
                r.drift_ms
            );
        }
        s
    }

    pub fn audio_summary(&self) -> String {
        match &self.render {
            Some(r) => format!(
                "{} | state {:?} | drift {:.2} ms | underruns {} | resyncs {} | stream latency {:.1} ms",
                r.description,
                r.shared.state(),
                r.shared.drift_us.load(Ordering::Relaxed) as f64 / 1000.0,
                r.shared.underruns.load(Ordering::Relaxed),
                r.shared.resyncs.load(Ordering::Relaxed),
                r.shared.stream_latency_us.load(Ordering::Relaxed) as f64 / 1000.0
            ),
            None => "output closed".into(),
        }
    }

    pub fn shutdown(&mut self) {
        self.stop();
        self.save();
        self.stop_session();
        self.monitor = None;
        self.calib_render = None;
        self.feeder = None;
        self.render = None;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Candidate addresses for the invite. Uses UDP `connect` (no packet is sent) to learn
/// which local interface routes to LAN / Tailscale ranges.
pub fn local_addresses() -> Vec<String> {
    let mut v = Vec::new();
    for probe in ["192.168.0.1:9", "10.0.0.1:9", "100.100.100.100:9"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if s.connect(probe).is_ok() {
                if let Ok(a) = s.local_addr() {
                    let ip = a.ip().to_string();
                    if !a.ip().is_unspecified() && !v.contains(&ip) {
                        v.push(ip);
                    }
                }
            }
        }
    }
    if v.is_empty() {
        v.push("127.0.0.1".into());
    }
    v
}
