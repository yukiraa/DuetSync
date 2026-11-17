//! Session tests over real localhost sockets: auth, expiry, heartbeat clock sync,
//! host disconnect, guest reconnect, bounded frames, version mismatch.

use duetsync::clock::now_us;
use duetsync::network::crypto::{random_bytes, Sealer};
use duetsync::network::protocol::{Handshake, MAX_FRAME};
use duetsync::network::session::{read_frame, write_frame};
use duetsync::network::*;
use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn host(secret: [u8; 16], deadline_s: i64) -> Host {
    Host::start(HostConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        secret,
        join_deadline_us: now_us() + deadline_s * 1_000_000,
    })
    .unwrap()
}

fn wait_event<F: Fn(&Event) -> bool>(
    rx: &std::sync::mpsc::Receiver<Event>,
    secs: u64,
    f: F,
) -> Option<Event> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Ok(e) = rx.recv_timeout(Duration::from_millis(50)) {
            if f(&e) {
                return Some(e);
            }
        }
    }
    None
}

#[test]
fn connect_exchange_and_clock_sync() {
    let secret = random_bytes::<16>();
    let h = host(secret, 60);
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret,
    })
    .unwrap();
    assert!(wait_event(&h.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(h.send(Msg::Notice("hello".into())));
    let got = wait_event(&g.events, 3, |e| matches!(e, Event::Msg(Msg::Notice(_))));
    assert!(matches!(got, Some(Event::Msg(Msg::Notice(s))) if s == "hello"));
    assert!(g.send(Msg::Stop));
    assert!(wait_event(&h.events, 3, |e| matches!(e, Event::Msg(Msg::Stop))).is_some());
    // Same machine, same clock: estimated offset must be ~0 within its uncertainty.
    std::thread::sleep(Duration::from_millis(2600));
    let est = g.clock().expect("clock estimate after heartbeats");
    assert!(est.offset_us.abs() <= est.uncertainty_us + 500.0, "{est:?}");
    assert!(est.rtt_min_us < 50_000.0);
}

#[test]
fn invalid_code_rejected() {
    let h = host(random_bytes::<16>(), 60);
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret: random_bytes::<16>(),
    })
    .unwrap();
    let e = wait_event(&g.events, 5, |e| matches!(e, Event::AuthFailed(_)));
    assert!(matches!(e, Some(Event::AuthFailed(s)) if s.contains("invalid join code")));
    assert!(wait_event(&h.events, 3, |e| matches!(e, Event::AuthFailed(_))).is_some());
    assert!(!h.connected());
}

#[test]
fn expired_code_rejected() {
    let secret = random_bytes::<16>();
    let h = host(secret, -1);
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret,
    })
    .unwrap();
    let e = wait_event(&g.events, 5, |e| matches!(e, Event::AuthFailed(_)));
    assert!(
        matches!(&e, Some(Event::AuthFailed(s)) if s.contains("expired")),
        "{e:?}"
    );
}

#[test]
fn pending_handshakes_are_capped_and_a_later_guest_can_connect() {
    let secret = random_bytes::<16>();
    let h = host(secret, 60);
    let _slow: Vec<TcpStream> = (0..10)
        .map(|_| TcpStream::connect(h.local_addr).expect("open slow client"))
        .collect();

    let end = Instant::now() + Duration::from_secs(3);
    let mut rejected = 0;
    while Instant::now() < end && rejected < 6 {
        if let Ok(Event::AuthFailed(reason)) = h.events.recv_timeout(Duration::from_millis(50)) {
            if reason == "too many pending connections" {
                rejected += 1;
            }
        }
    }
    // Six immediate rejections prove only four of the ten can be consuming handshake slots.
    assert_eq!(rejected, 6);

    // The four admitted clients time out after the five-second handshake deadline.
    std::thread::sleep(Duration::from_secs(6));
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret,
    })
    .unwrap();
    assert!(wait_event(&h.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
}

#[test]
fn expired_join_code_rejects_new_guest_but_allows_resume() {
    let secret = random_bytes::<16>();
    let h = host(secret, 1);
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret,
    })
    .unwrap();
    assert!(wait_event(&h.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 5, |e| matches!(e, Event::Connected(_))).is_some());

    std::thread::sleep(Duration::from_millis(1200));
    let new_guest = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret,
    })
    .unwrap();
    let rejected = wait_event(&new_guest.events, 5, |e| matches!(e, Event::AuthFailed(_)));
    assert!(matches!(rejected, Some(Event::AuthFailed(reason)) if reason.contains("expired")));

    assert!(h.send(Msg::Bye));
    assert!(wait_event(&h.events, 8, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 8, |e| matches!(e, Event::Connected(_))).is_some());
}

#[test]
fn rotating_join_code_preserves_existing_guest_resume() {
    let old_secret = random_bytes::<16>();
    let new_secret = random_bytes::<16>();
    let h = host(old_secret, 60);
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret: old_secret,
    })
    .unwrap();
    assert!(wait_event(&h.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 5, |e| matches!(e, Event::Connected(_))).is_some());

    h.rotate(new_secret, now_us() + 60_000_000);
    assert!(h.send(Msg::Bye));
    assert!(wait_event(&h.events, 8, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 8, |e| matches!(e, Event::Connected(_))).is_some());

    let old_code_guest = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret: old_secret,
    })
    .unwrap();
    let rejected = wait_event(&old_code_guest.events, 5, |e| {
        matches!(e, Event::AuthFailed(_))
    });
    assert!(
        matches!(rejected, Some(Event::AuthFailed(reason)) if reason.contains("invalid join code"))
    );
}

#[test]
fn guest_reconnects_after_host_restart_and_host_disconnect_detected() {
    let secret = random_bytes::<16>();
    let h = host(secret, 60);
    let addr = h.local_addr;
    let g = Guest::start(GuestConfig {
        host: addr.to_string(),
        secret,
    })
    .unwrap();
    assert!(wait_event(&g.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    drop(h); // host goes away
    assert!(wait_event(&g.events, 8, |e| matches!(e, Event::Disconnected(_))).is_some());
    // Host comes back on the same port with the same secret; the guest must rejoin.
    let h2 = Host::start(HostConfig {
        bind: addr,
        secret,
        join_deadline_us: now_us() + 60_000_000,
    })
    .unwrap();
    assert!(wait_event(&h2.events, 12, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(wait_event(&g.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    assert!(
        g.stats()
            .reconnects
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 1
    );
}

#[test]
fn guest_disconnect_detected_by_host() {
    let secret = random_bytes::<16>();
    let h = host(secret, 60);
    let g = Guest::start(GuestConfig {
        host: h.local_addr.to_string(),
        secret,
    })
    .unwrap();
    assert!(wait_event(&h.events, 5, |e| matches!(e, Event::Connected(_))).is_some());
    drop(g);
    assert!(wait_event(&h.events, 8, |e| matches!(e, Event::Disconnected(_))).is_some());
    assert!(!h.connected());
}

#[test]
fn oversized_frame_and_version_mismatch() {
    let h = host(random_bytes::<16>(), 60);
    // Oversized frame header is rejected before allocation.
    let mut s = TcpStream::connect(h.local_addr).unwrap();
    s.write_all(&((MAX_FRAME as u32 + 1).to_be_bytes()))
        .unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    assert!(wait_event(
        &h.events,
        4,
        |e| matches!(e, Event::AuthFailed(m) if m.contains("oversized"))
    )
    .is_some());
    assert!(write_frame(&mut s, &vec![0u8; MAX_FRAME + 1]).is_err());

    // Version mismatch gets an explicit Reject.
    let mut s = TcpStream::connect(h.local_addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let hello = Handshake::Hello {
        proto: 999,
        app: "future".into(),
        nonce: "00".repeat(16),
    };
    write_frame(&mut s, &serde_json::to_vec(&hello).unwrap()).unwrap();
    let f = read_frame(&mut s).unwrap().unwrap();
    let r: Handshake = serde_json::from_slice(&f).unwrap();
    assert!(matches!(r, Handshake::Reject { reason } if reason.contains("version")));
    let _ = Sealer::new([0; 32], 1);
}

// ---------------------------------------------------------------- YouTube mode over a session

mod youtube {
    use duetsync::app::App;
    use duetsync::config::{Config, Mode};
    use serde_json::Value;
    use std::net::TcpStream;
    use std::time::{Duration, Instant};
    use tungstenite::{client, Message, WebSocket};

    fn pump(a: &mut App, b: &mut App, secs: f64, done: impl Fn(&App, &App) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            a.tick();
            b.tick();
            if done(a, b) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        false
    }

    fn fake_page(port: u16) -> WebSocket<TcpStream> {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        client(format!("ws://127.0.0.1:{port}/ws?role=player"), stream)
            .unwrap()
            .0
    }

    fn commands(page: &mut WebSocket<TcpStream>) -> Vec<Value> {
        let mut out = vec![];
        for _ in 0..30 {
            match page.read() {
                Ok(Message::Text(t)) => out.push(serde_json::from_str(t.as_str()).unwrap()),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        out
    }

    #[test]
    fn host_picks_video_guest_follows_and_playback_drives_both_pages() {
        std::env::set_var("DUETSYNC_NO_BROWSER", "1");
        let port = 47_883;
        let mut host = App::with_config(
            Config {
                mode: Mode::Host,
                listen_port: port,
                last_track: None,
                auto_sync: false,
                ..Default::default()
            },
            false,
        );
        host.start_host();
        let mut guest = App::with_config(
            Config {
                mode: Mode::Guest,
                last_track: None,
                auto_sync: false,
                ..Default::default()
            },
            false,
        );
        guest.join_input = format!("127.0.0.1:{port}#{}", host.join_code.clone().unwrap());
        guest.join();
        assert!(
            pump(&mut host, &mut guest, 15.0, |h, g| h.connected()
                && g.clock_estimate().is_some()),
            "session"
        );

        host.set_youtube("not a link");
        assert!(host.yt_id.is_none(), "invalid link rejected");
        host.set_youtube("https://youtu.be/dQw4w9WgXcQ?t=3");
        assert_eq!(host.yt_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert!(
            pump(&mut host, &mut guest, 10.0, |h, g| h.track_matches()
                == Some(true)
                && g.track_matches() == Some(true)),
            "video id matched on both sides"
        );
        assert!(guest.yt_mode && guest.yt_id.as_deref() == Some("dQw4w9WgXcQ"));

        // Playing needs the page open.
        host.host_play(0, 1.0);
        assert!(!host.playing(), "no video page connected yet");

        let hp = host.video_bridge().unwrap().port();
        let gp = guest.video_bridge().unwrap().port();
        let mut host_page = fake_page(hp);
        let mut guest_page = fake_page(gp);
        assert!(
            pump(&mut host, &mut guest, 5.0, |h, g| h
                .video_player()
                .is_some()
                && g.video_player().is_some()),
            "pages connected"
        );

        host.host_play(0, 1.0);
        assert!(host.playing());
        assert!(
            pump(&mut host, &mut guest, 5.0, |_, g| g.playing()),
            "guest follows Play"
        );
        for page in [&mut host_page, &mut guest_page] {
            let cmds = commands(page);
            assert!(
                cmds.iter()
                    .any(|c| c["t"] == "play_at" && c["from_s"].as_f64() == Some(0.0)),
                "page got play_at: {cmds:?}"
            );
        }

        host.pause();
        assert!(!host.playing());
        assert!(pump(&mut host, &mut guest, 5.0, |_, g| !g.playing()));
        assert!(commands(&mut guest_page).iter().any(|c| c["t"] == "pause"));

        host.shutdown();
        guest.shutdown();
    }
}
