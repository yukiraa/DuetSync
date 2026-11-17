//! Host / guest TCP session: handshake, encrypted framing, heartbeats, reconnection.
//! Each live connection uses one reader and one writer thread (blocking I/O + timeouts).

use super::clocksync::{ClockSync, Estimate};
use super::crypto::{self, Sealer};
use super::protocol::{Handshake, Msg, MAX_FRAME, PROTO_VERSION};
use crate::clock::now_us;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const HEARTBEAT: Duration = Duration::from_millis(500);
pub const PEER_TIMEOUT_US: i64 = 5_000_000;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_HANDSHAKES: usize = 4;
const DIR_C2S: u8 = 1;
const DIR_S2C: u8 = 2;

// ---------- framing ----------

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> Result<(), String> {
    if payload.len() > MAX_FRAME {
        return Err("frame too large".into());
    }
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf).map_err(|e| format!("send: {e}"))
}

/// Read one frame. `Ok(None)` on read timeout before any byte of a frame arrived.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut len = [0u8; 4];
    match r.read(&mut len[..1]) {
        Ok(0) => return Err("connection closed".into()),
        Ok(_) => {}
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(None)
        }
        Err(e) => return Err(format!("receive: {e}")),
    }
    read_exact_patient(r, &mut len[1..])?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(format!("oversized frame ({n} bytes) rejected"));
    }
    let mut buf = vec![0u8; n];
    read_exact_patient(r, &mut buf)?;
    Ok(Some(buf))
}

/// read_exact that tolerates read timeouts mid-frame (bounded to ~5 s).
fn read_exact_patient<R: Read>(r: &mut R, mut buf: &mut [u8]) -> Result<(), String> {
    let deadline = now_us() + 5_000_000;
    while !buf.is_empty() {
        match r.read(buf) {
            Ok(0) => return Err("connection closed".into()),
            Ok(n) => buf = &mut buf[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                if now_us() > deadline {
                    return Err("timeout inside frame".into());
                }
            }
            Err(e) => return Err(format!("receive: {e}")),
        }
    }
    Ok(())
}

fn send_hs<W: Write>(w: &mut W, h: &Handshake) -> Result<(), String> {
    write_frame(w, &serde_json::to_vec(h).map_err(|e| e.to_string())?)
}

fn recv_hs<R: Read>(r: &mut R) -> Result<Handshake, String> {
    let deadline = now_us() + HANDSHAKE_TIMEOUT.as_micros() as i64;
    loop {
        if let Some(f) = read_frame(r)? {
            return serde_json::from_slice(&f).map_err(|_| "malformed handshake".to_string());
        }
        if now_us() > deadline {
            return Err("handshake timeout".into());
        }
    }
}

// ---------- events ----------

#[derive(Debug, Clone)]
pub enum Event {
    Connected(String),
    Disconnected(String),
    AuthFailed(String),
    Msg(Msg),
    Info(String),
}

#[derive(Default)]
pub struct LinkStats {
    pub rx_frames: AtomicU64,
    pub tx_frames: AtomicU64,
    pub last_rx_us: std::sync::atomic::AtomicI64,
    pub connected: AtomicBool,
    pub reconnects: AtomicU64,
}

enum Out {
    Msg(Msg),
    Pong { seq: u32, t1: i64, t2: i64 },
    Close,
}

/// Spawn reader+writer for an authenticated connection. Returns the outgoing sender.
#[allow(clippy::too_many_arguments)]
fn run_link(
    stream: TcpStream,
    tx_seal: Sealer,
    rx_seal: Sealer,
    events: SyncSender<Event>,
    stats: Arc<LinkStats>,
    clock: Option<Arc<Mutex<ClockSync>>>,
    ping: bool,
) -> Result<(SyncSender<Out>, thread::JoinHandle<String>), String> {
    let (otx, orx) = sync_channel::<Out>(256);
    let mut wstream = stream.try_clone().map_err(|e| e.to_string())?;
    let mut rstream = stream;
    let _ = rstream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = rstream.set_nodelay(true);
    let wstats = stats.clone();
    let writer = thread::Builder::new()
        .name("duetsync-net-w".into())
        .spawn(move || writer_loop(&mut wstream, tx_seal, orx, wstats, ping))
        .map_err(|e| e.to_string())?;
    let otx2 = otx.clone();
    let reader = thread::Builder::new()
        .name("duetsync-net-r".into())
        .spawn(move || {
            let reason = reader_loop(&mut rstream, rx_seal, &events, &stats, &otx2, clock);
            let _ = otx2.try_send(Out::Close);
            let _ = rstream.shutdown(Shutdown::Both);
            let _ = writer.join();
            stats.connected.store(false, Ordering::Release);
            reason
        })
        .map_err(|e| e.to_string())?;
    Ok((otx, reader))
}

fn writer_loop(
    s: &mut TcpStream,
    mut seal: Sealer,
    rx: Receiver<Out>,
    stats: Arc<LinkStats>,
    ping: bool,
) {
    let mut seq = 0u32;
    let mut next_ping = now_us();
    loop {
        let wait = if ping {
            Duration::from_micros((next_ping - now_us()).max(0) as u64)
        } else {
            HEARTBEAT
        };
        let item = match rx.recv_timeout(wait) {
            Ok(Out::Close) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(o) => Some(o),
            Err(RecvTimeoutError::Timeout) => None,
        };
        let msg = match item {
            Some(Out::Msg(m)) => m,
            Some(Out::Pong { seq, t1, t2 }) => Msg::Pong {
                seq,
                t1,
                t2,
                t3: now_us(), // stamped as late as possible, right before the write
            },
            Some(Out::Close) => unreachable!(),
            None if ping => {
                seq = seq.wrapping_add(1);
                // Burst the first pings so a fresh join gets a clock estimate in ~0.5 s,
                // then settle to the normal heartbeat.
                let interval = if seq <= 8 {
                    100_000
                } else {
                    HEARTBEAT.as_micros() as i64
                };
                next_ping = now_us() + interval;
                Msg::Ping { seq, t1: now_us() }
            }
            None => continue,
        };
        let pt = serde_json::to_vec(&msg).unwrap_or_default();
        if write_frame(s, &seal.seal(&pt)).is_err() {
            break;
        }
        stats.tx_frames.fetch_add(1, Ordering::Relaxed);
        if msg == Msg::Bye {
            break;
        }
    }
    let _ = s.shutdown(Shutdown::Both);
}

fn reader_loop(
    s: &mut TcpStream,
    mut seal: Sealer,
    events: &SyncSender<Event>,
    stats: &LinkStats,
    out: &SyncSender<Out>,
    clock: Option<Arc<Mutex<ClockSync>>>,
) -> String {
    stats.last_rx_us.store(now_us(), Ordering::Relaxed);
    loop {
        let frame = match read_frame(s) {
            Ok(Some(f)) => f,
            Ok(None) => {
                if now_us() - stats.last_rx_us.load(Ordering::Relaxed) > PEER_TIMEOUT_US {
                    return "peer timed out (no heartbeat for 5 s)".into();
                }
                continue;
            }
            Err(e) => return e,
        };
        let t_rx = now_us();
        let pt = match seal.open(&frame) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let Ok(msg) = serde_json::from_slice::<Msg>(&pt) else {
            return "malformed message".into();
        };
        stats.last_rx_us.store(t_rx, Ordering::Relaxed);
        stats.rx_frames.fetch_add(1, Ordering::Relaxed);
        match msg {
            Msg::Ping { seq, t1 } => {
                let _ = out.try_send(Out::Pong { seq, t1, t2: t_rx });
            }
            Msg::Pong { t1, t2, t3, .. } => {
                if let Some(c) = &clock {
                    if let Ok(mut c) = c.lock() {
                        c.add(t1, t2, t3, t_rx);
                    }
                }
            }
            Msg::Bye => return "peer closed the session".into(),
            m => {
                if events.try_send(Event::Msg(m)).is_err() {
                    // UI not draining: drop rather than grow unbounded.
                }
            }
        }
    }
}

// ---------- host ----------

pub struct HostConfig {
    pub bind: SocketAddr,
    pub secret: [u8; 16],
    /// Local time after which *new* joins are refused (reconnects stay allowed).
    pub join_deadline_us: i64,
}

struct HostShared {
    out: Mutex<Option<SyncSender<Out>>>,
    stop: AtomicBool,
    deadline: std::sync::atomic::AtomicI64,
    secret: Mutex<[u8; 16]>,
    resume: Mutex<Option<[u8; 32]>>,
    pending_handshakes: AtomicUsize,
    stats: Arc<LinkStats>,
    conn_seq: AtomicU64,
    current: AtomicU64,
}

struct HandshakeGuard<'a> {
    pending_handshakes: &'a AtomicUsize,
}

impl Drop for HandshakeGuard<'_> {
    fn drop(&mut self) {
        self.pending_handshakes.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Host {
    shared: Arc<HostShared>,
    pub events: Receiver<Event>,
    pub local_addr: SocketAddr,
    thread: Option<thread::JoinHandle<()>>,
}

impl Host {
    pub fn start(cfg: HostConfig) -> Result<Self, String> {
        let listener = TcpListener::bind(cfg.bind)
            .map_err(|e| format!("cannot listen on {}: {e}", cfg.bind))?;
        let local_addr = listener.local_addr().map_err(|e| e.to_string())?;
        let (etx, erx) = sync_channel::<Event>(512);
        let shared = Arc::new(HostShared {
            out: Mutex::new(None),
            stop: AtomicBool::new(false),
            deadline: std::sync::atomic::AtomicI64::new(cfg.join_deadline_us),
            secret: Mutex::new(cfg.secret),
            resume: Mutex::new(None),
            pending_handshakes: AtomicUsize::new(0),
            stats: Arc::new(LinkStats::default()),
            conn_seq: AtomicU64::new(0),
            current: AtomicU64::new(0),
        });
        let sh = shared.clone();
        let thread = thread::Builder::new()
            .name("duetsync-host".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if sh.stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = conn else { continue };
                    if sh
                        .pending_handshakes
                        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                            (n < MAX_PENDING_HANDSHAKES).then_some(n + 1)
                        })
                        .is_err()
                    {
                        let _ = stream.shutdown(Shutdown::Both);
                        let _ =
                            etx.try_send(Event::AuthFailed("too many pending connections".into()));
                        continue;
                    }
                    let sh2 = sh.clone();
                    let etx2 = etx.clone();
                    if thread::Builder::new()
                        .name("duetsync-host-conn".into())
                        .spawn(move || host_conn(stream, sh2, etx2))
                        .is_err()
                    {
                        sh.pending_handshakes.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            events: erx,
            local_addr,
            thread: Some(thread),
        })
    }

    pub fn send(&self, m: Msg) -> bool {
        let g = self.shared.out.lock().unwrap();
        g.as_ref()
            .is_some_and(|tx| tx.try_send(Out::Msg(m)).is_ok())
    }

    pub fn connected(&self) -> bool {
        self.shared.stats.connected.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> &LinkStats {
        &self.shared.stats
    }

    /// Replace the join secret and deadline (invalidates the old code for new joins).
    pub fn rotate(&self, secret: [u8; 16], deadline_us: i64) {
        *self.shared.secret.lock().unwrap() = secret;
        self.shared.deadline.store(deadline_us, Ordering::Release);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.send(Msg::Bye);
        if let Some(tx) = self.shared.out.lock().unwrap().take() {
            let _ = tx.try_send(Out::Close);
        }
        // Wake the blocking accept().
        let mut wake = self.local_addr;
        if wake.ip().is_unspecified() {
            wake.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
        }
        let _ = TcpStream::connect_timeout(&wake, Duration::from_millis(300));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn host_conn(mut s: TcpStream, sh: Arc<HostShared>, events: SyncSender<Event>) {
    // Counts only the unauthenticated phase; released right after the handshake so a
    // long-lived (or stale, not yet timed-out) link never blocks new joins.
    let handshake_guard = HandshakeGuard {
        pending_handshakes: &sh.pending_handshakes,
    };
    if sh.stop.load(Ordering::Acquire) {
        return;
    }
    let peer = s
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let join_secret = *sh.secret.lock().unwrap();
    let resume_secret = *sh.resume.lock().unwrap();
    let res = (|| -> Result<(Sealer, Sealer, [u8; 32]), String> {
        let Handshake::Hello { proto, app, nonce } = recv_hs(&mut s)? else {
            return Err("expected Hello".into());
        };
        if proto != PROTO_VERSION {
            let reason = format!(
                "protocol version mismatch (host v{PROTO_VERSION} / {}, guest v{proto} / {app})",
                crate::APP_VERSION
            );
            let _ = send_hs(
                &mut s,
                &Handshake::Reject {
                    reason: reason.clone(),
                },
            );
            return Err(reason);
        }
        let cn = crypto::unhex(&nonce)?;
        if cn.len() != 16 {
            return Err("bad nonce".into());
        }
        let sn = crypto::random_bytes::<16>();
        send_hs(
            &mut s,
            &Handshake::Challenge {
                proto: PROTO_VERSION,
                app: crate::APP_VERSION.into(),
                nonce: crypto::hex(&sn),
            },
        )?;
        let Handshake::Auth { mac } = recv_hs(&mut s)? else {
            return Err("expected Auth".into());
        };
        let tag = crypto::unhex(&mac)?;
        // Check both credentials before choosing one. The resume credential remains
        // valid after the public join code expires or is rotated.
        let join_matches = crypto::verify(&join_secret, b"DuetSync-auth-c", &[&cn, &sn], &tag);
        let resume_matches = resume_secret
            .as_ref()
            .is_some_and(|resume| crypto::verify(resume, b"DuetSync-auth-c", &[&cn, &sn], &tag));
        if !(join_matches || resume_matches) {
            thread::sleep(Duration::from_millis(500)); // slow down guessing
            let _ = send_hs(
                &mut s,
                &Handshake::Reject {
                    reason: "invalid join code".into(),
                },
            );
            return Err("invalid join code".into());
        }
        let joined = resume_matches;
        if !joined && now_us() > sh.deadline.load(Ordering::Acquire) {
            let _ = send_hs(
                &mut s,
                &Handshake::Reject {
                    reason: "join code expired — ask the host for a new one".into(),
                },
            );
            return Err("expired join code used".into());
        }
        let credential: &[u8] = if resume_matches {
            resume_secret.as_ref().unwrap()
        } else {
            &join_secret
        };
        let smac = crypto::mac(credential, b"DuetSync-auth-s", &[&cn, &sn]);
        send_hs(
            &mut s,
            &Handshake::AuthOk {
                mac: crypto::hex(&smac),
            },
        )?;
        let resume = crypto::mac(credential, b"DuetSync-resume", &[&cn, &sn]);
        let (c2s, s2c) = crypto::session_keys(credential, &cn, &sn);
        Ok((Sealer::new(s2c, DIR_S2C), Sealer::new(c2s, DIR_C2S), resume))
    })();
    let (tx_seal, rx_seal, resume) = match res {
        Ok(v) => v,
        Err(e) => {
            let _ = events.try_send(Event::AuthFailed(format!("{peer}: {e}")));
            return;
        }
    };
    drop(handshake_guard);
    *sh.resume.lock().unwrap() = Some(resume);
    // A new authenticated connection replaces any stale one (guest reconnect).
    if let Some(old) = sh.out.lock().unwrap().take() {
        let _ = old.try_send(Out::Close);
        sh.stats.reconnects.fetch_add(1, Ordering::Relaxed);
    }
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let Ok((otx, reader)) = run_link(
        s,
        tx_seal,
        rx_seal,
        events.clone(),
        sh.stats.clone(),
        None,
        false,
    ) else {
        return;
    };
    let id = sh.conn_seq.fetch_add(1, Ordering::Relaxed) + 1;
    sh.stats.connected.store(true, Ordering::Release);
    *sh.out.lock().unwrap() = Some(otx.clone());
    // Host dropped while this link was being set up: close it (drop() may have missed it).
    if sh.stop.load(Ordering::Acquire) {
        let _ = otx.try_send(Out::Msg(Msg::Bye));
        let _ = otx.try_send(Out::Close);
    }
    sh.current.store(id, Ordering::Release);
    let _ = events.try_send(Event::Connected(peer.clone()));
    let reason = reader
        .join()
        .unwrap_or_else(|_| "link thread panicked".into());
    if sh.current.load(Ordering::Acquire) == id {
        *sh.out.lock().unwrap() = None;
        sh.stats.connected.store(false, Ordering::Release);
        let _ = events.try_send(Event::Disconnected(format!("{peer}: {reason}")));
    } else {
        // Replaced by a newer connection; the link stats belong to that one now.
        sh.stats.connected.store(true, Ordering::Release);
    }
}

// ---------- guest ----------

pub struct GuestConfig {
    pub host: String,
    pub secret: [u8; 16],
}

struct GuestShared {
    out: Mutex<Option<SyncSender<Out>>>,
    stop: AtomicBool,
    stats: Arc<LinkStats>,
    clock: Arc<Mutex<ClockSync>>,
    attempts: AtomicU64,
    last_error: Mutex<Option<String>>,
    started_us: i64,
    extra: Mutex<Vec<SocketAddr>>,
}

/// What the guest's connection loop is doing, for the join screen.
#[derive(Clone, Debug, Default)]
pub struct JoinStatus {
    pub host: String,
    /// Failed connection attempts since the last success.
    pub failed_attempts: u64,
    /// Raw error of the last failed attempt (None after a success).
    pub last_error: Option<String>,
    pub seconds: f64,
}

pub struct Guest {
    shared: Arc<GuestShared>,
    pub events: Receiver<Event>,
    host: String,
    thread: Option<thread::JoinHandle<()>>,
}

impl Guest {
    /// `cfg.host` may hold several comma-separated addresses, or be empty (internet-only
    /// join: the tunnel's local bridge is added later with `set_extra`).
    pub fn start(cfg: GuestConfig) -> Result<Self, String> {
        let mut addrs = Vec::new();
        let mut bad = None;
        for part in cfg.host.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match resolve(part) {
                Ok(a) => addrs.push(a),
                Err(e) => bad = Some(e),
            }
        }
        if addrs.is_empty() {
            if let Some(e) = bad {
                return Err(e);
            }
        }
        let host_label = if addrs.is_empty() {
            "your host over the internet".to_string()
        } else {
            addrs
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (etx, erx) = sync_channel::<Event>(512);
        let shared = Arc::new(GuestShared {
            out: Mutex::new(None),
            stop: AtomicBool::new(false),
            stats: Arc::new(LinkStats::default()),
            clock: Arc::new(Mutex::new(ClockSync::default())),
            attempts: AtomicU64::new(0),
            last_error: Mutex::new(None),
            started_us: now_us(),
            extra: Mutex::new(Vec::new()),
        });
        let sh = shared.clone();
        let thread = thread::Builder::new()
            .name("duetsync-guest".into())
            .spawn(move || guest_loop(addrs, cfg.secret, sh, etx))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            events: erx,
            host: host_label,
            thread: Some(thread),
        })
    }

    /// Extra addresses tried first (e.g. the internet tunnel's local bridge once it is up).
    pub fn set_extra(&self, addrs: Vec<SocketAddr>) {
        if let Ok(mut g) = self.shared.extra.lock() {
            if *g != addrs {
                *g = addrs;
            }
        }
    }

    pub fn send(&self, m: Msg) -> bool {
        let g = self.shared.out.lock().unwrap();
        g.as_ref()
            .is_some_and(|tx| tx.try_send(Out::Msg(m)).is_ok())
    }

    pub fn connected(&self) -> bool {
        self.shared.stats.connected.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> &LinkStats {
        &self.shared.stats
    }

    pub fn clock(&self) -> Option<Estimate> {
        self.shared.clock.lock().ok()?.estimate()
    }

    pub fn join_status(&self) -> JoinStatus {
        JoinStatus {
            host: self.host.clone(),
            failed_attempts: self.shared.attempts.load(Ordering::Relaxed),
            last_error: self.shared.last_error.lock().ok().and_then(|g| g.clone()),
            seconds: (now_us() - self.shared.started_us) as f64 / 1e6,
        }
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.send(Msg::Bye);
        thread::sleep(Duration::from_millis(50));
        if let Some(tx) = self.shared.out.lock().unwrap().take() {
            let _ = tx.try_send(Out::Close);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn resolve(host: &str) -> Result<SocketAddr, String> {
    let host = host.trim();
    if let Ok(a) = host.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, super::protocol::DEFAULT_PORT));
    }
    // Host name with or without ":port".
    let h = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{}", super::protocol::DEFAULT_PORT)
    };
    h.to_socket_addrs()
        .map_err(|e| format!("cannot resolve '{host}': {e}"))?
        .next()
        .ok_or_else(|| format!("no address for '{host}'"))
}

fn guest_handshake(
    s: &mut TcpStream,
    credential: &[u8],
) -> Result<(Sealer, Sealer, [u8; 32]), String> {
    let cn = crypto::random_bytes::<16>();
    send_hs(
        s,
        &Handshake::Hello {
            proto: PROTO_VERSION,
            app: crate::APP_VERSION.into(),
            nonce: crypto::hex(&cn),
        },
    )?;
    let sn = match recv_hs(s)? {
        Handshake::Challenge { nonce, .. } => crypto::unhex(&nonce)?,
        Handshake::Reject { reason } => return Err(reason),
        _ => return Err("unexpected handshake".into()),
    };
    let mac = crypto::mac(credential, b"DuetSync-auth-c", &[&cn, &sn]);
    send_hs(
        s,
        &Handshake::Auth {
            mac: crypto::hex(&mac),
        },
    )?;
    match recv_hs(s)? {
        Handshake::AuthOk { mac } => {
            if !crypto::verify(
                credential,
                b"DuetSync-auth-s",
                &[&cn, &sn],
                &crypto::unhex(&mac)?,
            ) {
                return Err("host failed to prove the join code (wrong host?)".into());
            }
        }
        Handshake::Reject { reason } => return Err(reason),
        _ => return Err("unexpected handshake".into()),
    }
    let resume = crypto::mac(credential, b"DuetSync-resume", &[&cn, &sn]);
    let (c2s, s2c) = crypto::session_keys(credential, &cn, &sn);
    Ok((Sealer::new(c2s, DIR_C2S), Sealer::new(s2c, DIR_S2C), resume))
}

enum Credential {
    Join([u8; 16]),
    Resume([u8; 32]),
}

impl Credential {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Join(secret) => secret,
            Self::Resume(secret) => secret,
        }
    }
}

fn guest_loop(
    addrs: Vec<SocketAddr>,
    secret: [u8; 16],
    sh: Arc<GuestShared>,
    events: SyncSender<Event>,
) {
    let mut backoff = Duration::from_millis(500);
    let mut first = true;
    let join_secret = secret;
    let mut credential = Credential::Join(secret);
    while !sh.stop.load(Ordering::Acquire) {
        // Tunnel bridge (if up) first, then the invite's direct addresses.
        let mut candidates: Vec<SocketAddr> =
            sh.extra.lock().map(|g| g.clone()).unwrap_or_default();
        candidates.extend(addrs.iter().copied());
        if candidates.is_empty() {
            // Internet-only join: wait for the tunnel to come up (not a failure).
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        let attempt = (|| -> Result<_, String> {
            let mut last = String::new();
            for addr in &candidates {
                let timeout = if addr.ip().is_loopback() {
                    Duration::from_millis(500)
                } else {
                    Duration::from_secs(2)
                };
                let mut s = match TcpStream::connect_timeout(addr, timeout) {
                    Ok(s) => s,
                    Err(e) => {
                        last = format!("cannot reach host {addr}: {e}");
                        continue;
                    }
                };
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
                match guest_handshake(&mut s, credential.as_slice()) {
                    Ok((tx_seal, rx_seal, resume)) => return Ok((s, tx_seal, rx_seal, resume)),
                    // Auth answers are definitive, whichever path delivered them.
                    Err(e) if e.contains("join code") || e.contains("version") => return Err(e),
                    Err(e) => last = e,
                }
            }
            Err(last)
        })();
        match attempt {
            Ok((s, tx_seal, rx_seal, resume)) => {
                let via = match s.peer_addr() {
                    Ok(a) if a.ip().is_loopback() => "your host (over the internet)".to_string(),
                    Ok(a) => a.to_string(),
                    Err(_) => "your host".to_string(),
                };
                credential = Credential::Resume(resume);
                backoff = Duration::from_millis(500);
                sh.attempts.store(0, Ordering::Relaxed);
                if let Ok(mut g) = sh.last_error.lock() {
                    *g = None;
                }
                if let Ok(mut c) = sh.clock.lock() {
                    c.clear(); // a new path may have different delay
                }
                match run_link(
                    s,
                    tx_seal,
                    rx_seal,
                    events.clone(),
                    sh.stats.clone(),
                    Some(sh.clock.clone()),
                    true,
                ) {
                    Ok((otx, reader)) => {
                        sh.stats.connected.store(true, Ordering::Release);
                        if !first {
                            sh.stats.reconnects.fetch_add(1, Ordering::Relaxed);
                        }
                        first = false;
                        *sh.out.lock().unwrap() = Some(otx.clone());
                        if sh.stop.load(Ordering::Acquire) {
                            let _ = otx.try_send(Out::Msg(Msg::Bye));
                            let _ = otx.try_send(Out::Close);
                        }
                        let _ = events.try_send(Event::Connected(via.clone()));
                        let reason = reader.join().unwrap_or_else(|_| "link panicked".into());
                        *sh.out.lock().unwrap() = None;
                        let _ = events.try_send(Event::Disconnected(reason));
                    }
                    Err(e) => {
                        let _ = events.try_send(Event::Disconnected(e));
                    }
                }
            }
            Err(e) => {
                // A restarted host has no in-memory resume credential. Retry its current
                // public code once; ordinary reconnects keep using the resume credential.
                if matches!(&credential, Credential::Resume(_)) && e.contains("invalid join code") {
                    credential = Credential::Join(join_secret);
                    continue;
                }
                let fatal = e.contains("join code") || e.contains("version mismatch");
                sh.attempts.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut g) = sh.last_error.lock() {
                    *g = Some(e.clone());
                }
                let _ = events.try_send(if fatal {
                    Event::AuthFailed(e)
                } else {
                    Event::Info(e)
                });
                if fatal {
                    return;
                }
            }
        }
        // Sleep with early exit on stop.
        let until = now_us() + backoff.as_micros() as i64;
        while now_us() < until && !sh.stop.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(100));
        }
        // Retry quickly: a joining user is waiting on screen.
        backoff = (backoff * 2).min(Duration::from_secs(2));
    }
}
