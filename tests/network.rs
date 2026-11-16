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
