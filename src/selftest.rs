//! `DuetSync.exe --selftest [--bench SECONDS]`: real single-machine hardware checks.
//! Plays calibration chirps on the default output with presentation-time scheduling and
//! captures this process via process loopback, measuring where they land. Prints honest
//! results (including failures); never substitutes simulated data.

pub fn run(args: &[String]) -> i32 {
    #[cfg(windows)]
    {
        imp::run(args)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("selftest requires Windows");
        2
    }
}

/// Headless end-to-end run of two real controllers (host + guest) on one PC over
/// localhost. Test signals and both players are routed to a virtual cable so nothing is
/// audible. Exercises session, clock sync, calibration and scheduled playback.
#[cfg(windows)]
mod e2e {
    use crate::app::App;
    use crate::audio::wasapi::list_devices;
    use crate::calibration::CalibrationStatus;
    use crate::config::{Config, GuestAudio, Mode};
    use std::time::{Duration, Instant};

    fn find(devs: &[crate::audio::DeviceInfo], needle: &str) -> Option<String> {
        devs.iter()
            .find(|d| d.name.to_lowercase().contains(&needle.to_lowercase()))
            .map(|d| d.id.clone())
    }

    fn pump(
        h: &mut App,
        g: &mut App,
        secs: f64,
        mut until: impl FnMut(&App, &App) -> bool,
    ) -> bool {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            h.tick();
            g.tick();
            if until(h, g) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        false
    }

    fn write_test_wav(path: &std::path::Path) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..44_100 * 20 {
            let v = ((i as f32 * 220.0 * std::f32::consts::TAU / 44_100.0).sin() * 3000.0) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
    }

    pub fn run(args: &[String]) -> i32 {
        println!(
            "DuetSync {} end-to-end (host + guest on this PC)",
            crate::APP_VERSION
        );
        let ins = list_devices(true).unwrap_or_default();
        let outs = list_devices(false).unwrap_or_default();
        let cable_in_name = args
            .iter()
            .position(|a| a == "--cable")
            .and_then(|i| args.get(i + 1).cloned())
            .unwrap_or_else(|| "CABLE".into());
        let (Some(cable_play), Some(cable_rec)) = (
            find(&outs, &format!("{cable_in_name} Input")),
            find(&ins, &format!("{cable_in_name} Output")),
        ) else {
            println!(
                "SKIP: needs a virtual cable (\"CABLE Input\" output + \"CABLE Output\" input)"
            );
            return 2;
        };
        let port = 47_871;
        let host_cfg = Config {
            mode: Mode::Host,
            listen_port: port,
            output_device: Some(cable_play.clone()),
            guest_audio: GuestAudio::Input(Some(cable_rec.clone())),
            chirp_count: 8,
            chirp_spacing_ms: 1200.0,
            last_track: None,
            ..Default::default()
        };
        let mut host = App::with_config(host_cfg, false);
        host.start_host();
        let invite = format!(
            "127.0.0.1:{port}#{}",
            host.join_code.clone().unwrap_or_default()
        );
        let guest_cfg = Config {
            mode: Mode::Guest,
            output_device: Some(cable_play.clone()),
            calib_output_device: Some(cable_play.clone()),
            guest_reference_input: Some(cable_rec.clone()),
            use_guest_reference: !args.iter().any(|a| a == "--no-ref"),
            last_track: None,
            ..Default::default()
        };
        let mut guest = App::with_config(guest_cfg, false);
        guest.join_input = invite;
        guest.join();
        let mut code = 0;

        // 1. Session + clock sync
        let ok = pump(&mut host, &mut guest, 10.0, |h, g| {
            h.connected() && g.clock_estimate().is_some() && h.remote.is_some()
        });
        if !ok {
            println!(
                "FAIL session/clock: host log: {:?}",
                host.log.lines().collect::<Vec<_>>()
            );
            return 1;
        }
        let e = guest.clock_estimate().unwrap();
        println!(
            "OK session: clock offset {:.3} ms ±{:.3} ms, rtt {:.2} ms",
            e.offset_us / 1000.0,
            e.uncertainty_us / 1000.0,
            e.rtt_median_us / 1000.0
        );

        // 2. Calibration through the virtual cable
        host.start_calibration();
        let done = pump(&mut host, &mut guest, 30.0, |h, _| h.last_calib.is_some());
        match host.last_calib.as_ref().map(|o| &o.result) {
            Some(r) if done && r.status != CalibrationStatus::Invalid => println!(
                "OK calibration: {:?} latency {:.2} ms jitter {:.2} ms ±{:.2} ms ({}/{} signals, reference={})",
                r.status,
                r.median_ms.unwrap_or(f64::NAN),
                r.jitter_ms.unwrap_or(f64::NAN),
                r.uncertainty_ms.unwrap_or(f64::NAN),
                r.accepted,
                r.total,
                host.last_calib.as_ref().unwrap().reference
            ),
            other => {
                println!("FAIL calibration: {:?}", other.map(|r| &r.messages));
                for l in host.log.lines().chain(guest.log.lines()) {
                    println!("  {l}");
                }
                code = 1;
            }
        }

        // 3. Synchronized playback of the same file on both controllers
        let wav = std::env::temp_dir().join("duetsync-e2e.wav");
        write_test_wav(&wav);
        host.load_track(wav.clone());
        guest.load_track(wav.clone());
        let ok = pump(&mut host, &mut guest, 10.0, |h, _| {
            h.track_matches() == Some(true)
        });
        if !ok {
            println!("FAIL track verification");
            return 1;
        }
        println!("OK track identity verified (SHA-256) on both sides");
        host.host_play(0, 2.0);
        let mut sampler = crate::diagnostics::Sampler::default();
        let mut errs = Vec::new();
        let t0 = Instant::now();
        pump(&mut host, &mut guest, 14.0, |h, _| {
            if t0.elapsed().as_secs_f64() > 4.0 {
                if let Some((e, _)) = h.sync_error_ms() {
                    if errs.last() != Some(&e) {
                        errs.push(e);
                    }
                }
            }
            false
        });
        let s = sampler.sample();
        println!(
            "INFO resources while both players run (host+guest controllers in ONE process, 15 ms poll loop): CPU {:.2} % of all cores, working set {:.1} MB, private {:.1} MB",
            s.cpu_percent, s.working_set_mb, s.private_mb
        );
        host.stop();
        pump(&mut host, &mut guest, 1.0, |_, _| false);
        if errs.is_empty() {
            println!("FAIL playback: no guest position reports while playing");
            code = 1;
        } else {
            let worst = errs.iter().fold(0f64, |a, e| a.max(e.abs()));
            let mean = errs.iter().sum::<f64>() / errs.len() as f64;
            let mut abs: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
            abs.sort_by(f64::total_cmp);
            let p95 = abs[(abs.len() * 95 / 100).min(abs.len() - 1)];
            println!(
                "{} playback: guest-vs-host timeline over {} samples: mean {mean:+.2} ms, p95 {p95:.2} ms, worst {worst:.2} ms",
                if worst <= 20.0 { "OK" } else { "FAIL" },
                errs.len()
            );
            if worst > 20.0 {
                code = 1;
            }
        }
        let _ = std::fs::remove_file(wav);
        host.shutdown();
        guest.shutdown();
        println!("{}", if code == 0 { "E2E PASS" } else { "E2E FAIL" });
        code
    }
}

#[cfg(windows)]
mod imp {
    use crate::audio::wasapi::{list_devices, CaptureHandle, RenderHandle};
    use crate::audio::CaptureSource;
    use crate::calibration::stats::Sample;
    use crate::calibration::*;
    use crate::clock::now_us;
    use crate::diagnostics::Sampler;
    use crate::playback::core::Command;
    use std::sync::Arc;

    pub fn run(args: &[String]) -> i32 {
        if args.iter().any(|a| a == "--e2e") {
            return super::e2e::run(args);
        }
        println!("DuetSync {} self-test", crate::APP_VERSION);
        let mut sampler = Sampler::default();
        for (label, cap) in [("Inputs", true), ("Outputs", false)] {
            match list_devices(cap) {
                Ok(d) => {
                    println!("{label}: {}", d.len());
                    for x in d {
                        println!("  - {}", x.name);
                    }
                }
                Err(e) => println!("{label}: ERROR {e}"),
            }
        }
        let render = match RenderHandle::start(None, crate::SAMPLE_RATE) {
            Ok(r) => r,
            Err(e) => {
                println!("FAIL render: {e}");
                return 1;
            }
        };
        println!(
            "Render: {} (stream latency {} ms)",
            render.description,
            render
                .shared
                .stream_latency_us
                .load(std::sync::atomic::Ordering::Relaxed) as f64
                / 1000.0
        );
        let mut code = 0;
        let pid = std::process::id();
        match CaptureHandle::start(CaptureSource::Process {
            pid,
            name: "DuetSync (self)".into(),
        }) {
            Ok(mut cap) => {
                println!("Capture: {}", cap.description);
                code |= chirp_test(&render, &mut cap, "process loopback");
            }
            Err(e) => {
                println!("FAIL process loopback capture: {e}");
                code = 1;
            }
        }
        match CaptureHandle::start(CaptureSource::OutputLoopback(None)) {
            Ok(mut cap) => {
                println!("Capture: {}", cap.description);
                code |= chirp_test(&render, &mut cap, "output-device loopback");
            }
            Err(e) => println!("SKIP output loopback: {e}"),
        }
        if args.iter().any(|a| a == "--cable-test") {
            let ins = list_devices(true).unwrap_or_default();
            let outs = list_devices(false).unwrap_or_default();
            let o = outs
                .iter()
                .find(|d| d.name.contains("CABLE Input"))
                .map(|d| d.id.clone());
            let i = ins
                .iter()
                .find(|d| d.name.contains("CABLE Output"))
                .map(|d| d.id.clone());
            if let (Some(o), Some(i)) = (o, i) {
                for round in 0..3 {
                    let r = RenderHandle::start(Some(o.clone()), crate::SAMPLE_RATE);
                    let c = CaptureHandle::start(CaptureSource::Input(Some(i.clone())));
                    match (r, c) {
                        (Ok(r), Ok(mut c)) => {
                            println!(
                                "Cable round {round}: {} -> {}",
                                r.description, c.description
                            );
                            code |= chirp_test(&r, &mut c, "virtual cable");
                        }
                        (r, c) => println!("cable open failed: {:?} {:?}", r.err(), c.err()),
                    }
                }
            }
        }
        if let Some(i) = args.iter().position(|a| a == "--bench") {
            let secs: u64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(30);
            println!("Benchmark: render stream open + idle for {secs} s ...");
            sampler.sample();
            std::thread::sleep(std::time::Duration::from_secs(secs));
            let s = sampler.sample();
            println!(
                "  process CPU {:.3} % (of all cores), working set {:.1} MB, private {:.1} MB",
                s.cpu_percent, s.working_set_mb, s.private_mb
            );
        }
        let s = sampler.sample();
        println!(
            "End: working set {:.1} MB, private {:.1} MB",
            s.working_set_mb, s.private_mb
        );
        println!(
            "{}",
            if code == 0 {
                "SELFTEST PASS"
            } else {
                "SELFTEST FAIL"
            }
        );
        code
    }

    fn chirp_test(render: &RenderHandle, cap: &mut CaptureHandle, label: &str) -> i32 {
        let plan = CalibrationPlan::new(now_us() + 800_000, 6, 1000.0);
        let tpl: Arc<[f32]> = plan.chirp.generate(crate::SAMPLE_RATE).into();
        for &t in &plan.times_host_us {
            if let Err(e) = render.cmd(Command::Clip {
                at_us: t,
                samples: tpl.clone(),
            }) {
                println!("FAIL schedule: {e}");
                return 1;
            }
        }
        let rec = match cap.record_until(plan.end_host_us()) {
            Ok(r) => r,
            Err(e) => {
                println!("FAIL {label} record: {e}");
                return 1;
            }
        };
        println!(
            "  recorded {:.2} s, effective rate {:.2} Hz",
            rec.samples.len() as f64 / crate::SAMPLE_RATE as f64,
            rec.rate_hz
        );
        let refs: Vec<Option<i64>> = plan.times_host_us.iter().map(|&t| Some(t)).collect();
        let found = locate_chirps(
            &rec,
            &refs,
            &plan.chirp,
            plan.max_latency_ms,
            &DetectorParams::default(),
        );
        let per: Vec<Result<Sample, String>> = found
            .into_iter()
            .zip(&refs)
            .map(|(r, rf)| {
                r.map(|(t, d)| Sample {
                    latency_ms: (t - rf.unwrap()) as f64 / 1000.0,
                    ncc: d.ncc,
                    clipped: d.clipped_samples > 0,
                })
            })
            .collect();
        for (k, p) in per.iter().enumerate() {
            match p {
                Ok(s) => println!("  chirp {k}: {:+.2} ms (ncc {:.2})", s.latency_ms, s.ncc),
                Err(e) => println!("  chirp {k}: {e}"),
            }
        }
        let r = analyze(per, 0.0, 4);
        match (r.status, r.median_ms, r.jitter_ms) {
            (CalibrationStatus::Invalid, _, _) => {
                println!("FAIL {label}: {:?}", r.messages);
                1
            }
            (st, Some(m), Some(j)) => {
                println!(
                    "OK {label}: scheduled-presentation → capture-timestamp = {m:+.2} ms, jitter {j:.2} ms, confidence {:.2} ({st:?})",
                    r.confidence
                );
                0
            }
            _ => 1,
        }
    }
}
