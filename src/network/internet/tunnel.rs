//! Hole-punched UDP tunnel carrying the session's TCP byte stream.
//!
//! The session itself (handshake, AEAD frames, heartbeats) is unchanged: on the guest a
//! local TCP "bridge" listener stands in for the host; on the host the tunnel connects to the
//! host's own listener on 127.0.0.1. Tunnel packets carry a truncated HMAC so stray or forged
//! UDP is dropped; the payload is already end-to-end encrypted by the session layer.
//!
//! Reliability: per-epoch byte stream, cumulative ACK, sliding window, RTO retransmission.
//! Each new TCP connection on the guest starts a new epoch (RESET), so reconnects are clean.

use crate::network::crypto::mac;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const T_PUNCH: u8 = 1;
pub const T_PUNCH_ACK: u8 = 2;
pub const T_DATA: u8 = 3;
pub const T_ACK: u8 = 4;
pub const T_RESET: u8 = 5;
pub const T_CLOSE: u8 = 6;

const HDR: usize = 24;
const TAG: usize = 8;
const MSS: usize = 1100;
const WINDOW: usize = 48;
pub const DEAD_AFTER: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub kind: u8,
    pub sid: u64,
    pub epoch: u32,
    pub seq: u32,
    pub ack: u32,
    pub payload: Vec<u8>,
}

pub fn tunnel_key(secret: &[u8; 16]) -> [u8; 32] {
    mac(secret, b"DuetSync-udp-v1", &[])
}

pub fn encode(key: &[u8; 32], p: &Packet) -> Vec<u8> {
    let mut b = Vec::with_capacity(HDR + p.payload.len() + TAG);
    b.extend_from_slice(b"DS");
    b.push(p.kind);
    b.push(0);
    b.extend_from_slice(&p.sid.to_be_bytes());
    b.extend_from_slice(&p.epoch.to_be_bytes());
    b.extend_from_slice(&p.seq.to_be_bytes());
    b.extend_from_slice(&p.ack.to_be_bytes());
    b.extend_from_slice(&p.payload);
    let t = mac(key, b"DS-pkt", &[&b]);
    b.extend_from_slice(&t[..TAG]);
    b
}

pub fn decode(key: &[u8; 32], b: &[u8]) -> Option<Packet> {
    if b.len() < HDR + TAG || &b[..2] != b"DS" {
        return None;
    }
    let (body, tag) = b.split_at(b.len() - TAG);
    let t = mac(key, b"DS-pkt", &[body]);
    if t[..TAG].iter().zip(tag).fold(0u8, |a, (x, y)| a | (x ^ y)) != 0 {
        return None;
    }
    let u32_at = |i: usize| u32::from_be_bytes([body[i], body[i + 1], body[i + 2], body[i + 3]]);
    Some(Packet {
        kind: body[2],
        sid: u64::from_be_bytes(body[4..12].try_into().ok()?),
        epoch: u32_at(12),
        seq: u32_at(16),
        ack: u32_at(20),
        payload: body[HDR..].to_vec(),
    })
}

struct Seg {
    seq: u32,
    data: Vec<u8>,
    sent_at: Instant,
    tries: u32,
}

/// Reliable, ordered byte stream for one epoch. Pure state machine (no I/O), unit-tested.
pub struct Stream {
    pub epoch: u32,
    next_seq: u32,
    unacked: VecDeque<Seg>,
    pub pending: VecDeque<u8>,
    expected: u32,
    ooo: BTreeMap<u32, Vec<u8>>,
    pub deliver: VecDeque<u8>,
    srtt_ms: f64,
    pub ack_due: bool,
}

impl Stream {
    pub fn new(epoch: u32) -> Self {
        Self {
            epoch,
            next_seq: 0,
            unacked: VecDeque::new(),
            pending: VecDeque::new(),
            expected: 0,
            ooo: BTreeMap::new(),
            deliver: VecDeque::new(),
            srtt_ms: 150.0,
            ack_due: false,
        }
    }

    fn rto(&self, tries: u32) -> Duration {
        let base = (self.srtt_ms * 2.0).clamp(60.0, 1000.0);
        Duration::from_millis((base * (1u64 << tries.min(4)) as f64).min(3000.0) as u64)
    }

    /// Bytes accepted from the local TCP side (bounded by the caller).
    pub fn room(&self) -> usize {
        (WINDOW * MSS * 2).saturating_sub(self.pending.len())
    }

    pub fn on_data(&mut self, seq: u32, data: Vec<u8>) {
        self.ack_due = true;
        if seq < self.expected {
            return; // duplicate
        }
        if seq > self.expected {
            if self.ooo.len() < WINDOW * 2 {
                self.ooo.insert(seq, data);
            }
            return;
        }
        self.deliver.extend(data);
        self.expected += 1;
        while let Some(d) = self.ooo.remove(&self.expected) {
            self.deliver.extend(d);
            self.expected += 1;
        }
    }

    pub fn on_ack(&mut self, ack: u32, now: Instant) {
        while let Some(s) = self.unacked.front() {
            if s.seq >= ack {
                break;
            }
            if s.tries == 1 {
                let rtt = now.duration_since(s.sent_at).as_secs_f64() * 1000.0;
                self.srtt_ms += 0.2 * (rtt - self.srtt_ms);
            }
            self.unacked.pop_front();
        }
    }

    /// Packets to send now (new data within the window + due retransmissions + ack).
    pub fn outgoing(&mut self, now: Instant) -> Vec<(u8, u32, Vec<u8>)> {
        let mut out = Vec::new();
        while self.unacked.len() < WINDOW && !self.pending.is_empty() {
            let n = self.pending.len().min(MSS);
            let data: Vec<u8> = self.pending.drain(..n).collect();
            let seq = self.next_seq;
            self.next_seq += 1;
            out.push((T_DATA, seq, data.clone()));
            self.unacked.push_back(Seg {
                seq,
                data,
                sent_at: now,
                tries: 1,
            });
        }
        let rtos: Vec<Duration> = self.unacked.iter().map(|s| self.rto(s.tries)).collect();
        for (s, rto) in self.unacked.iter_mut().zip(rtos) {
            if now.duration_since(s.sent_at) >= rto {
                s.sent_at = now;
                s.tries += 1;
                out.push((T_DATA, s.seq, s.data.clone()));
            }
        }
        if self.ack_due || !out.is_empty() {
            self.ack_due = false;
        }
        out
    }

    pub fn ack_value(&self) -> u32 {
        self.expected
    }
}

/// A tunnel packet with its source, or raw non-tunnel bytes (e.g. STUN) with theirs.
pub type Received = Result<(Packet, SocketAddr), (Vec<u8>, SocketAddr)>;

/// UDP side of one tunnel endpoint plus its TCP bridge.
///
/// Event-driven in both directions so the tunnel adds no timer-tick latency (Windows timer
/// granularity is ~15.6 ms, which would otherwise skew clock sync):
/// - UDP -> TCP: the control thread blocks in `recv`, and the caller runs `service()` right
///   after each packet, which writes delivered bytes to TCP at once.
/// - TCP -> UDP: a reader thread blocks on the TCP socket and sends segments the moment bytes
///   arrive.
///
/// The control loop's 20 ms receive timeout only paces retransmissions and keepalives.
pub struct Pump {
    pub sock: UdpSocket,
    pub key: [u8; 32],
    pub sid: u64,
    pub peer: Option<SocketAddr>,
    pub last_rx: Instant,
    last_tx: Arc<Mutex<Instant>>,
    shared_peer: Arc<Mutex<Option<SocketAddr>>>,
    stream: Option<Arc<Mutex<Stream>>>,
    tcp: Option<TcpStream>,
    tcp_closed: Arc<AtomicBool>,
    /// Data may flow (guest: only after the host acknowledged this epoch's RESET).
    gate: Arc<AtomicBool>,
}

#[allow(clippy::too_many_arguments)]
fn send_raw(
    sock: &UdpSocket,
    key: &[u8; 32],
    sid: u64,
    to: SocketAddr,
    kind: u8,
    epoch: u32,
    seq: u32,
    ack: u32,
    payload: &[u8],
) {
    let p = Packet {
        kind,
        sid,
        epoch,
        seq,
        ack,
        payload: payload.to_vec(),
    };
    let _ = sock.send_to(&encode(key, &p), to);
}

impl Pump {
    pub fn new(sock: UdpSocket, key: [u8; 32], sid: u64) -> Self {
        let _ = sock.set_read_timeout(Some(Duration::from_millis(20)));
        Self {
            sock,
            key,
            sid,
            peer: None,
            last_rx: Instant::now(),
            last_tx: Arc::new(Mutex::new(Instant::now())),
            shared_peer: Arc::new(Mutex::new(None)),
            stream: None,
            tcp: None,
            tcp_closed: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn set_peer(&mut self, a: SocketAddr) {
        self.peer = Some(a);
        if let Ok(mut g) = self.shared_peer.lock() {
            *g = Some(a);
        }
    }

    pub fn clear_peer(&mut self) {
        self.peer = None;
        if let Ok(mut g) = self.shared_peer.lock() {
            *g = None;
        }
    }

    pub fn epoch(&self) -> Option<u32> {
        self.stream
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| s.epoch))
    }

    pub fn attached(&self) -> bool {
        self.tcp.is_some()
    }

    /// Start carrying `tcp` as stream `epoch`. `open` = data may flow immediately.
    pub fn attach(&mut self, tcp: TcpStream, epoch: u32, open: bool) {
        self.detach();
        let _ = tcp.set_nonblocking(false);
        let _ = tcp.set_nodelay(true);
        let Ok(mut reader) = tcp.try_clone() else {
            return;
        };
        let Ok(sock) = self.sock.try_clone() else {
            return;
        };
        let stream = Arc::new(Mutex::new(Stream::new(epoch)));
        let closed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(AtomicBool::new(open));
        let (key, sid) = (self.key, self.sid);
        let (st, cl, gt, peer, last_tx) = (
            stream.clone(),
            closed.clone(),
            gate.clone(),
            self.shared_peer.clone(),
            self.last_tx.clone(),
        );
        let _ = std::thread::Builder::new()
            .name("duetsync-tunnel-r".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    // Back-pressure: wait while the window and queue are full.
                    loop {
                        let full = st.lock().map(|s| s.room() < n).unwrap_or(false);
                        if !full {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    let Ok(mut s) = st.lock() else { break };
                    s.pending.extend(&buf[..n]);
                    let to = peer.lock().ok().and_then(|g| *g);
                    if let (Some(to), true) = (to, gt.load(Ordering::Acquire)) {
                        let pkts = s.outgoing(Instant::now());
                        let (ep, ack) = (s.epoch, s.ack_value());
                        drop(s);
                        for (kind, seq, data) in pkts {
                            send_raw(&sock, &key, sid, to, kind, ep, seq, ack, &data);
                        }
                        if let Ok(mut t) = last_tx.lock() {
                            *t = Instant::now();
                        }
                    }
                }
                cl.store(true, Ordering::Release);
            });
        self.stream = Some(stream);
        self.tcp = Some(tcp);
        self.tcp_closed = closed;
        self.gate = gate;
    }

    pub fn open_gate(&self) {
        self.gate.store(true, Ordering::Release);
    }

    /// Stop carrying the current TCP connection (its reader thread exits).
    pub fn detach(&mut self) {
        if let Some(t) = self.tcp.take() {
            let _ = t.shutdown(Shutdown::Both);
        }
        self.stream = None;
    }

    pub fn send(
        &mut self,
        to: SocketAddr,
        kind: u8,
        epoch: u32,
        seq: u32,
        ack: u32,
        payload: &[u8],
    ) {
        send_raw(
            &self.sock, &self.key, self.sid, to, kind, epoch, seq, ack, payload,
        );
        if let Ok(mut t) = self.last_tx.lock() {
            *t = Instant::now();
        }
    }

    /// Receive one datagram (20 ms timeout). Returns the decoded packet and its source, or
    /// raw non-tunnel bytes (e.g. STUN) for the caller.
    pub fn recv(&mut self) -> Option<Received> {
        let mut buf = [0u8; 2048];
        match self.sock.recv_from(&mut buf) {
            Ok((n, from)) => Some(match decode(&self.key, &buf[..n]) {
                Some(p) => Ok((p, from)),
                None => Err((buf[..n].to_vec(), from)),
            }),
            Err(_) => None,
        }
    }

    /// Feed a stream packet (DATA/ACK) for the current epoch.
    pub fn on_stream_packet(&mut self, p: &Packet) {
        let now = Instant::now();
        if let Some(st) = &self.stream {
            if let Ok(mut s) = st.lock() {
                if s.epoch == p.epoch {
                    s.on_ack(p.ack, now);
                    if p.kind == T_DATA {
                        s.on_data(p.seq, p.payload.clone());
                    }
                }
            }
        }
    }

    /// Deliver received bytes to TCP and send due packets (acks, retransmissions,
    /// keepalive). Returns false when the TCP side has closed.
    pub fn service(&mut self) -> bool {
        let Some(peer) = self.peer else { return true };
        let now = Instant::now();
        let mut out: Vec<(u8, u32, Vec<u8>)> = Vec::new();
        let mut deliver: Vec<u8> = Vec::new();
        let mut meta = None;
        if let Some(st) = &self.stream {
            if let Ok(mut s) = st.lock() {
                deliver.extend(s.deliver.drain(..));
                let ack_due = s.ack_due;
                if self.gate.load(Ordering::Acquire) {
                    out = s.outgoing(now);
                }
                if out.is_empty() && ack_due {
                    s.ack_due = false;
                    out.push((T_ACK, 0, Vec::new()));
                }
                meta = Some((s.epoch, s.ack_value()));
            }
        }
        let mut tcp_open = !self.tcp_closed.load(Ordering::Acquire) || self.tcp.is_none();
        if !deliver.is_empty() {
            if let Some(tcp) = self.tcp.as_mut() {
                if tcp.write_all(&deliver).is_err() {
                    tcp_open = false;
                }
            }
        }
        if let Some((epoch, ack)) = meta {
            for (kind, seq, data) in &out {
                self.send(peer, *kind, epoch, *seq, ack, data);
            }
        }
        if self.last_tx_age() >= Duration::from_secs(1) {
            let (epoch, ack) = meta.unwrap_or((0, 0));
            self.send(peer, T_ACK, epoch, 0, ack, &[]); // keepalive (keeps NAT mappings open)
        }
        tcp_open
    }

    pub fn last_tx_age(&self) -> Duration {
        self.last_tx.lock().map(|t| t.elapsed()).unwrap_or_default()
    }

    pub fn alive(&self) -> bool {
        self.last_rx.elapsed() < DEAD_AFTER
    }
}

/// Guest bridge listener on 127.0.0.1 (the session connects here instead of to the host).
pub fn bridge_listener() -> std::io::Result<TcpListener> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    l.set_nonblocking(true)?;
    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_roundtrip_and_tamper() {
        let k = tunnel_key(&[9u8; 16]);
        let p = Packet {
            kind: T_DATA,
            sid: 77,
            epoch: 3,
            seq: 12,
            ack: 5,
            payload: b"hello".to_vec(),
        };
        let mut b = encode(&k, &p);
        assert_eq!(decode(&k, &b), Some(p));
        b[HDR] ^= 1;
        assert_eq!(decode(&k, &b), None);
        assert_eq!(
            decode(&tunnel_key(&[1u8; 16]), &encode(&k, &decode_or(&k))),
            None
        );
    }

    fn decode_or(_k: &[u8; 32]) -> Packet {
        Packet {
            kind: T_ACK,
            sid: 1,
            epoch: 0,
            seq: 0,
            ack: 0,
            payload: vec![],
        }
    }

    /// Two streams over a lossy, reordering link deliver the exact byte sequence.
    #[test]
    fn stream_survives_loss_and_reordering() {
        let mut a = Stream::new(1);
        let mut b = Stream::new(1);
        let msg: Vec<u8> = (0..200_000u32).map(|i| (i * 31 % 251) as u8).collect();
        a.pending.extend(&msg);
        let mut rng = 12345u64;
        let mut rand = || {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as u32
        };
        let mut t = Instant::now();
        let mut in_flight: Vec<(u32, Vec<u8>)> = Vec::new();
        for _ in 0..20_000 {
            t += Duration::from_millis(10);
            for (_, seq, data) in a.outgoing(t) {
                if rand() % 100 >= 20 {
                    in_flight.push((seq, data)); // 20 % loss
                }
            }
            // deliver in shuffled order
            while !in_flight.is_empty() {
                let i = (rand() as usize) % in_flight.len();
                let (seq, data) = in_flight.swap_remove(i);
                b.on_data(seq, data);
            }
            if rand() % 100 >= 20 {
                a.on_ack(b.ack_value(), t);
            }
            if b.deliver.len() == msg.len() {
                break;
            }
        }
        let got: Vec<u8> = b.deliver.iter().copied().collect();
        assert_eq!(got.len(), msg.len());
        assert!(got == msg);
    }
}
