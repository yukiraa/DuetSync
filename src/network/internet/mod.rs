//! Internet mode: join from anywhere with just the invite, nothing installed, no router
//! setup. Steps:
//! 1. Each PC learns its public UDP address from public STUN servers.
//! 2. Host and guest swap addresses (encrypted) through the ntfy.sh relay.
//! 3. Both send packets to each other at once, which opens both NATs ("hole punching").
//! 4. The existing encrypted session runs through that UDP path via a local TCP bridge.
//!
//! Very strict networks (some mobile and carrier-grade NAT) can't be punched. Then the
//! status says so plainly, and the session stays usable on the same Wi-Fi.

pub mod https;
pub mod signal;
pub mod stun;
pub mod tunnel;

use signal::{Channel, Record};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tunnel::*;

#[derive(Clone, Debug, PartialEq)]
pub enum NetStatus {
    Starting,
    /// Host: reachable through the internet relay.
    Ready,
    /// Guest: looking for the host.
    Searching,
    /// Both sides know each other; opening the path.
    Punching,
    Connected(SocketAddr),
    Failed(String),
}

impl NetStatus {
    pub fn text(&self) -> String {
        match self {
            NetStatus::Starting => "Getting ready for internet joining...".into(),
            NetStatus::Ready => "Ready: your partner can join from anywhere.".into(),
            NetStatus::Searching => "Looking for your host over the internet...".into(),
            NetStatus::Punching => "Found your host, opening the connection...".into(),
            NetStatus::Connected(_) => "Connected over the internet.".into(),
            NetStatus::Failed(e) => e.clone(),
        }
    }
}

struct Shared {
    stop: AtomicBool,
    status: Mutex<NetStatus>,
}

impl Shared {
    fn set(&self, s: NetStatus) {
        if let Ok(mut g) = self.status.lock() {
            *g = s;
        }
    }
}

fn local_candidates(port: u16) -> Vec<SocketAddr> {
    crate::app::local_addresses()
        .into_iter()
        .filter_map(|ip| ip.parse::<IpAddr>().ok())
        .filter(|ip| !ip.is_loopback())
        .map(|ip| SocketAddr::new(ip, port))
        .collect()
}

fn sleep_unless(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(50));
    }
}

fn random_sid() -> u64 {
    u64::from_be_bytes(crate::network::crypto::random_bytes::<8>()) | 1
}

// ------------------------------------------------------------------ host

pub struct InternetHost {
    shared: Arc<Shared>,
}

impl InternetHost {
    pub fn start(secret: [u8; 16], listen_port: u16) -> Self {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            status: Mutex::new(NetStatus::Starting),
        });
        let sh = shared.clone();
        let _ = thread::Builder::new()
            .name("duetsync-inet-host".into())
            .spawn(move || host_main(secret, listen_port, sh));
        Self { shared }
    }
    pub fn status(&self) -> NetStatus {
        self.shared
            .status
            .lock()
            .map(|g| g.clone())
            .unwrap_or(NetStatus::Starting)
    }
}

impl Drop for InternetHost {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

fn host_main(secret: [u8; 16], listen_port: u16, sh: Arc<Shared>) {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return sh.set(NetStatus::Failed(
            "Internet joining unavailable (no network socket).".into(),
        ));
    };
    let public = match stun::public_addr(&sock) {
        Ok(p) => p,
        Err(_) => {
            return sh.set(NetStatus::Failed(
                "No internet connection found: only partners on your Wi-Fi can join.".into(),
            ))
        }
    };
    let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
    let channel = Arc::new(Channel::new(&secret));
    let my_record = Record {
        role: "h".into(),
        sid: 0,
        public: Some(public),
        local: local_candidates(port),
    };
    // Publish (retry a few times).
    let mut published = false;
    for _ in 0..4 {
        if channel.publish(&my_record).is_ok() {
            published = true;
            break;
        }
        sleep_unless(&sh.stop, Duration::from_secs(2));
    }
    if !published {
        return sh.set(NetStatus::Failed(
            "Couldn't reach the joining helper (ntfy.sh): only partners on your Wi-Fi can join."
                .into(),
        ));
    }
    sh.set(NetStatus::Ready);

    // Guest records arrive on the subscription thread -> punch targets for the pump.
    type Targets = Arc<Mutex<Vec<(u64, SocketAddr, Instant)>>>;
    let targets: Targets = Arc::new(Mutex::new(Vec::new()));
    {
        let (sh2, ch2, t2, rec) = (
            sh.clone(),
            channel.clone(),
            targets.clone(),
            my_record.clone(),
        );
        let _ = thread::Builder::new()
            .name("duetsync-inet-sub".into())
            .spawn(move || {
                let mut seen: Vec<u64> = Vec::new();
                let mut last_pub = Instant::now();
                while !sh2.stop.load(Ordering::Acquire) {
                    let _ = ch2.subscribe("all", &sh2.stop, |r| {
                        if r.role == "g" && !seen.contains(&r.sid) {
                            seen.push(r.sid);
                            let until = Instant::now() + Duration::from_secs(20);
                            if let Ok(mut t) = t2.lock() {
                                t.retain(|(_, _, u)| *u > Instant::now());
                                for a in r.public.iter().chain(r.local.iter()) {
                                    t.push((r.sid, *a, until));
                                }
                            }
                            // Answer so a guest that came later than our first post sees us.
                            if last_pub.elapsed() > Duration::from_secs(2) {
                                let _ = ch2.publish(&rec);
                                last_pub = Instant::now();
                            }
                        }
                        !sh2.stop.load(Ordering::Acquire)
                    });
                    sleep_unless(&sh2.stop, Duration::from_secs(3));
                }
            });
    }

    let key = tunnel_key(&secret);
    let mut pump = Pump::new(sock, key, 0);
    let mut last_punch = Instant::now() - Duration::from_secs(1);
    let mut last_stun = Instant::now();
    let mut stun_txid = [0u8; 12];
    while !sh.stop.load(Ordering::Acquire) {
        // Punch towards known guests so our NAT lets their packets in.
        if last_punch.elapsed() >= Duration::from_millis(100) {
            last_punch = Instant::now();
            let list: Vec<(u64, SocketAddr)> = targets
                .lock()
                .map(|mut t| {
                    t.retain(|(_, _, u)| *u > Instant::now());
                    t.iter().map(|(s, a, _)| (*s, *a)).collect()
                })
                .unwrap_or_default();
            for (sid, a) in list {
                let p = Packet {
                    kind: T_PUNCH,
                    sid,
                    epoch: 0,
                    seq: 0,
                    ack: 0,
                    payload: vec![],
                };
                let _ = pump.sock.send_to(&encode(&pump.key, &p), a);
            }
        }
        // Keep our public mapping alive while nobody is connected.
        if pump.peer.is_none() && last_stun.elapsed() >= Duration::from_secs(20) {
            last_stun = Instant::now();
            let _ = getrandom::fill(&mut stun_txid);
            if let Some(a) = std::net::ToSocketAddrs::to_socket_addrs(stun::SERVERS[0])
                .ok()
                .and_then(|mut i| i.find(|a| a.is_ipv4()))
            {
                let _ = pump.sock.send_to(&stun::request(&stun_txid), a);
            }
        }
        match pump.recv() {
            Some(Ok((p, from))) => match p.kind {
                T_PUNCH => {
                    if pump.sid != p.sid {
                        // A new guest attempt replaces the old tunnel.
                        pump.sid = p.sid;
                        pump.detach();
                    }
                    pump.set_peer(from);
                    pump.last_rx = Instant::now();
                    pump.send(from, T_PUNCH_ACK, 0, 0, 0, &[]);
                    sh.set(NetStatus::Connected(from));
                }
                _ if p.sid == pump.sid && pump.sid != 0 => {
                    pump.set_peer(from);
                    pump.last_rx = Instant::now();
                    match p.kind {
                        T_RESET => {
                            if pump.epoch() != Some(p.epoch) {
                                if let Ok(t) = TcpStream::connect(("127.0.0.1", listen_port)) {
                                    pump.attach(t, p.epoch, true);
                                }
                            }
                            pump.send(from, T_ACK, p.epoch, 0, 0, &[]);
                        }
                        T_CLOSE => {
                            if pump.epoch() == Some(p.epoch) {
                                pump.detach();
                            }
                        }
                        T_DATA | T_ACK => pump.on_stream_packet(&p),
                        _ => {}
                    }
                }
                _ => {}
            },
            Some(Err((raw, _))) => {
                if let Some(m) = stun::parse_response(&raw, &stun_txid) {
                    if Some(m) != my_record.public {
                        let mut r = my_record.clone();
                        r.public = Some(m);
                        let _ = channel.publish(&r);
                    }
                }
            }
            None => {}
        }
        if pump.peer.is_some() && !pump.service() {
            if let Some((peer, epoch)) = pump.peer.zip(pump.epoch()) {
                for _ in 0..3 {
                    pump.send(peer, T_CLOSE, epoch, 0, 0, &[]);
                }
            }
            pump.detach();
        }
        if pump.peer.is_some() && !pump.alive() {
            pump.clear_peer();
            pump.sid = 0;
            pump.detach();
            sh.set(NetStatus::Ready);
        }
    }
}

// ------------------------------------------------------------------ guest

pub struct InternetGuest {
    shared: Arc<Shared>,
    bridge: SocketAddr,
}

impl InternetGuest {
    pub fn start(secret: [u8; 16]) -> Result<Self, String> {
        let listener = bridge_listener().map_err(|e| e.to_string())?;
        let bridge = listener.local_addr().map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            status: Mutex::new(NetStatus::Searching),
        });
        let sh = shared.clone();
        thread::Builder::new()
            .name("duetsync-inet-guest".into())
            .spawn(move || guest_main(secret, listener, sh))
            .map_err(|e| e.to_string())?;
        Ok(Self { shared, bridge })
    }
    pub fn status(&self) -> NetStatus {
        self.shared
            .status
            .lock()
            .map(|g| g.clone())
            .unwrap_or(NetStatus::Searching)
    }
    /// Local address the session should connect to once `Connected`.
    pub fn bridge(&self) -> SocketAddr {
        self.bridge
    }
}

impl Drop for InternetGuest {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

fn guest_main(secret: [u8; 16], listener: std::net::TcpListener, sh: Arc<Shared>) {
    let channel = Arc::new(Channel::new(&secret));
    let key = tunnel_key(&secret);
    let mut found_host_ever = false;
    while !sh.stop.load(Ordering::Acquire) {
        sh.set(NetStatus::Searching);
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
            sh.set(NetStatus::Failed("No network socket available.".into()));
            sleep_unless(&sh.stop, Duration::from_secs(3));
            continue;
        };
        let public = match stun::public_addr(&sock) {
            Ok(p) => Some(p),
            Err(_) => {
                sh.set(NetStatus::Failed("No internet connection found.".into()));
                sleep_unless(&sh.stop, Duration::from_secs(3));
                continue;
            }
        };
        let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
        let sid = random_sid();
        let me = Record {
            role: "g".into(),
            sid,
            public,
            local: local_candidates(port),
        };
        if channel.publish(&me).is_err() {
            sh.set(NetStatus::Failed(
                "Couldn't reach the joining helper (ntfy.sh). Check your internet, retrying..."
                    .into(),
            ));
            sleep_unless(&sh.stop, Duration::from_secs(3));
            continue;
        }
        // Collect host addresses in the background.
        let hosts: Arc<Mutex<Vec<SocketAddr>>> = Arc::new(Mutex::new(Vec::new()));
        let sub_stop = Arc::new(AtomicBool::new(false));
        {
            let (ch, hs, st, root) = (channel.clone(), hosts.clone(), sub_stop.clone(), sh.clone());
            let _ = thread::Builder::new()
                .name("duetsync-inet-gsub".into())
                .spawn(move || {
                    while !st.load(Ordering::Acquire) && !root.stop.load(Ordering::Acquire) {
                        let _ = ch.subscribe("all", &st, |r| {
                            if r.role == "h" {
                                if let Ok(mut h) = hs.lock() {
                                    for a in r.public.iter().chain(r.local.iter()) {
                                        if !h.contains(a) {
                                            h.push(*a);
                                        }
                                    }
                                }
                            }
                            !st.load(Ordering::Acquire)
                        });
                        sleep_unless(&st, Duration::from_secs(2));
                    }
                });
        }
        let mut pump = Pump::new(sock, key, sid);
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut last_punch = Instant::now() - Duration::from_secs(1);
        let mut last_pub = Instant::now();
        while pump.peer.is_none() && Instant::now() < deadline && !sh.stop.load(Ordering::Acquire) {
            let targets: Vec<SocketAddr> = hosts.lock().map(|h| h.clone()).unwrap_or_default();
            if !targets.is_empty() {
                found_host_ever = true;
                sh.set(NetStatus::Punching);
            }
            if last_punch.elapsed() >= Duration::from_millis(100) {
                last_punch = Instant::now();
                for a in &targets {
                    pump.send(*a, T_PUNCH, 0, 0, 0, &[]);
                }
            }
            if last_pub.elapsed() >= Duration::from_secs(6) {
                last_pub = Instant::now();
                let _ = channel.publish(&me);
            }
            if let Some(Ok((p, from))) = pump.recv() {
                if p.sid == sid && (p.kind == T_PUNCH || p.kind == T_PUNCH_ACK) {
                    pump.set_peer(from);
                    pump.last_rx = Instant::now();
                    if p.kind == T_PUNCH {
                        pump.send(from, T_PUNCH_ACK, 0, 0, 0, &[]);
                    }
                }
            }
        }
        sub_stop.store(true, Ordering::Release);
        let Some(peer) = pump.peer else {
            sh.set(NetStatus::Failed(if found_host_ever {
                "Found your host, but your two internet connections can't reach each other directly (strict network). Try another Wi-Fi or a phone hotspot on one side.".into()
            } else {
                "Can't find your host. Is DuetSync open on their PC with a session created? Ask them for a fresh invite.".into()
            }));
            sleep_unless(&sh.stop, Duration::from_secs(3));
            continue;
        };
        sh.set(NetStatus::Connected(peer));
        guest_pump(&mut pump, &listener, &sh);
        if !sh.stop.load(Ordering::Acquire) {
            sh.set(NetStatus::Failed(
                "Internet connection to your host was lost, reconnecting...".into(),
            ));
            sleep_unless(&sh.stop, Duration::from_secs(1));
        }
    }
}

fn guest_pump(pump: &mut Pump, listener: &std::net::TcpListener, sh: &Shared) {
    let mut epoch = 0u32;
    let mut reset_acked = false;
    let mut last_reset = Instant::now();
    while !sh.stop.load(Ordering::Acquire) && pump.alive() {
        // A new session connection starts a new stream epoch.
        if let Ok((tcp, _)) = listener.accept() {
            epoch += 1;
            pump.attach(tcp, epoch, false);
            reset_acked = false;
            last_reset = Instant::now() - Duration::from_secs(1);
        }
        let peer = pump.peer.expect("connected");
        if pump.attached() && !reset_acked && last_reset.elapsed() >= Duration::from_millis(200) {
            last_reset = Instant::now();
            pump.send(peer, T_RESET, epoch, 0, 0, &[]);
        }
        if let Some(Ok((p, from))) = pump.recv() {
            if p.sid == pump.sid {
                pump.set_peer(from);
                pump.last_rx = Instant::now();
                match p.kind {
                    T_PUNCH => pump.send(from, T_PUNCH_ACK, 0, 0, 0, &[]),
                    T_ACK if p.epoch == epoch && !reset_acked && pump.attached() => {
                        reset_acked = true;
                        pump.open_gate();
                        pump.on_stream_packet(&p);
                    }
                    T_CLOSE if p.epoch == epoch => pump.detach(),
                    T_DATA | T_ACK => pump.on_stream_packet(&p),
                    _ => {}
                }
            }
        }
        // Only move data once the host has opened its side for this epoch.
        if reset_acked && !pump.service() {
            for _ in 0..3 {
                pump.send(peer, T_CLOSE, epoch, 0, 0, &[]);
            }
            pump.detach();
            reset_acked = false;
        } else if !reset_acked && pump.last_tx_age() >= Duration::from_secs(1) {
            pump.send(peer, T_ACK, 0, 0, 0, &[]); // keepalive before any session
        }
    }
    pump.detach();
}
