use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{accept_hdr, Error as WsError, Message, WebSocket};

const MAX_REQUEST: usize = 8 * 1024;
const MAX_CLIENTS: usize = 4;
const PING_US: i64 = 1_000_000;
const COOLDOWN_US: i64 = 4_000_000;
const STABLE_US: i64 = 1_000_000;
const STORM_US: i64 = 30_000_000;
const PAGE: &str = include_str!("../../assets/video/player.html");

#[derive(Clone, Debug)]
pub struct ClientInfo {
    pub role: String,
    pub state: String,
    pub last_pos_s: f64,
    pub error_ms: Option<f64>,
    pub seek_lead_ms: f64,
    pub corrections: u64,
    pub rtt_ms: f64,
    /// The browser blocks sound until the user clicks the page once.
    pub needs_click: bool,
}

pub struct VideoBridge {
    inner: Arc<Inner>,
    accept_thread: Option<JoinHandle<()>>,
}

struct Inner {
    port: u16,
    stop: AtomicBool,
    clients: Mutex<Vec<Arc<Client>>>,
}

struct Client {
    role: Role,
    tx: mpsc::Sender<String>,
    data: Mutex<ClientData>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Player,
    Obs,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Obs => "obs",
        }
    }
}

struct ClientData {
    state: String,
    needs_click: bool,
    last_pos_s: f64,
    report_local_us: i64,
    offset_us: Option<i64>,
    rtt_us: Option<i64>,
    samples: VecDeque<(i64, i64)>, // (round trip, page minus local offset)
    start_local_us: Option<i64>,
    from_s: f64,
    error_ms: Option<f64>,
    policy: CorrectionPolicy,
}

impl ClientData {
    fn new() -> Self {
        Self {
            state: "unstarted".to_owned(),
            needs_click: false,
            last_pos_s: 0.0,
            report_local_us: 0,
            offset_us: None,
            rtt_us: None,
            samples: VecDeque::new(),
            start_local_us: None,
            from_s: 0.0,
            error_ms: None,
            policy: CorrectionPolicy::new(),
        }
    }
}

/// All timing decisions consume distinct, timestamped player reports, never timer ticks.
struct CorrectionPolicy {
    seek_lead_s: f64,
    start_lead_ms: f64,
    corrections: u64,
    start_us: Option<i64>,
    startup_pending: bool,
    seeking: bool,
    resumed_us: Option<i64>,
    playing_since_us: Option<i64>,
    last_report_us: Option<i64>,
    cooldown_until_us: i64,
    outside_reports: u8,
    recent: VecDeque<i64>,
    storm_until_us: i64,
}

impl CorrectionPolicy {
    fn new() -> Self {
        Self {
            seek_lead_s: 0.12,
            start_lead_ms: 120.0,
            corrections: 0,
            start_us: None,
            startup_pending: false,
            seeking: false,
            resumed_us: None,
            playing_since_us: None,
            last_report_us: None,
            cooldown_until_us: i64::MIN,
            outside_reports: 0,
            recent: VecDeque::new(),
            storm_until_us: i64::MIN,
        }
    }

    fn cancel_learning(&mut self) {
        self.startup_pending = false;
        self.seeking = false;
        self.resumed_us = None;
        self.playing_since_us = None;
        self.outside_reports = 0;
    }

    fn play_at(&mut self, start_us: i64) {
        self.cancel_learning();
        self.start_us = Some(start_us);
        self.startup_pending = true;
        self.last_report_us = None;
    }

    fn threshold_ms(&self, now_us: i64) -> f64 {
        if now_us < self.storm_until_us {
            120.0
        } else {
            60.0
        }
    }

    /// Return the lead to add to the current shared timeline for a correction seek.
    fn report(&mut self, now_us: i64, state: &str, error_ms: f64) -> Option<f64> {
        if !error_ms.is_finite() || self.last_report_us.is_some_and(|last| now_us <= last) {
            return None;
        }
        // Missing reports cannot establish a full second of stable playback.
        if self
            .last_report_us
            .is_some_and(|last| now_us - last > 750_000)
        {
            self.playing_since_us = None;
            self.outside_reports = 0;
        }
        self.last_report_us = Some(now_us);
        let start_us = self.start_us?;
        if self.startup_pending && now_us > start_us + 2_000_000 {
            self.startup_pending = false;
        }
        if state != "playing" || now_us < start_us {
            self.playing_since_us = None;
            self.outside_reports = 0;
            if self.seeking {
                self.resumed_us = None;
            }
            return None;
        }
        let playing_since = *self.playing_since_us.get_or_insert(now_us);
        let stable = now_us - playing_since >= STABLE_US;
        if self.seeking {
            if self.resumed_us.is_none() {
                self.resumed_us = Some(now_us);
                self.cooldown_until_us = now_us + COOLDOWN_US;
            }
            if !stable {
                return None;
            }
            self.seek_lead_s = (self.seek_lead_s - error_ms / 1_000.0 * 0.8).clamp(0.0, 0.6);
            self.seeking = false;
        }
        if self.startup_pending {
            if stable && now_us >= start_us + STABLE_US {
                self.start_lead_ms = (self.start_lead_ms - error_ms * 0.8).clamp(0.0, 400.0);
                self.startup_pending = false;
            } else {
                return None;
            }
        }
        if error_ms.abs() > self.threshold_ms(now_us) {
            self.outside_reports = self.outside_reports.saturating_add(1);
        } else {
            self.outside_reports = 0;
        }
        if self.outside_reports < 2 || now_us < self.cooldown_until_us {
            return None;
        }
        self.outside_reports = 0;
        self.seeking = true;
        self.resumed_us = None;
        self.playing_since_us = None;
        self.corrections += 1;
        while self
            .recent
            .front()
            .is_some_and(|time| now_us - time > STORM_US)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now_us);
        if self.recent.len() >= 3 {
            self.storm_until_us = now_us + STORM_US;
        }
        Some(self.seek_lead_s)
    }
}

impl VideoBridge {
    pub fn start(port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let inner = Arc::new(Inner {
            port,
            stop: AtomicBool::new(false),
            clients: Mutex::new(Vec::new()),
        });
        let accept_inner = Arc::clone(&inner);
        let accept_thread = thread::spawn(move || accept_loop(listener, accept_inner));
        Ok(Self {
            inner,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    pub fn url(&self, role: &str) -> String {
        let role = if role == "obs" { "obs" } else { "player" };
        format!(
            "http://127.0.0.1:{}{}",
            self.port(),
            if role == "obs" { "?role=obs" } else { "" }
        )
    }

    pub fn load(&self, id: &str) {
        for client in self.snapshot_clients() {
            let mut data = client.data.lock().expect("video client data poisoned");
            data.policy = CorrectionPolicy::new();
            data.start_local_us = None;
            data.error_ms = None;
            send(&client, json!({"t":"load","videoId":id}));
        }
    }

    pub fn play_at(&self, start_local_us: i64, from_s: f64) {
        let cue = start_local_us - crate::clock::now_us() > 1_500_000;
        for client in self.snapshot_clients() {
            let page_ms = {
                let mut data = client.data.lock().expect("video client data poisoned");
                data.start_local_us = Some(start_local_us);
                data.from_s = from_s;
                data.error_ms = None;
                data.policy.play_at(start_local_us);
                (start_local_us + data.offset_us.unwrap_or(0)) as f64 / 1_000.0
                    - data.policy.start_lead_ms
            };
            if cue {
                send(&client, json!({"t":"cue","pos_s":from_s}));
            }
            send(
                &client,
                json!({"t":"play_at","page_ms":page_ms,"from_s":from_s}),
            );
        }
    }

    pub fn pause(&self) {
        self.cancel_learning(true);
        self.broadcast(json!({"t":"pause"}));
    }
    pub fn stop(&self) {
        self.cancel_learning(true);
        self.broadcast(json!({"t":"stop"}));
    }
    pub fn seek(&self, pos_s: f64) {
        self.cancel_learning(false);
        self.broadcast(json!({"t":"seek","pos_s":pos_s}));
    }

    pub fn clients(&self) -> Vec<ClientInfo> {
        self.snapshot_clients()
            .into_iter()
            .map(|client| {
                let data = client.data.lock().expect("video client data poisoned");
                ClientInfo {
                    role: client.role.as_str().to_owned(),
                    state: data.state.clone(),
                    last_pos_s: data.last_pos_s,
                    error_ms: data.error_ms,
                    seek_lead_ms: data.policy.seek_lead_s * 1_000.0,
                    corrections: data.policy.corrections,
                    rtt_ms: data.rtt_us.unwrap_or(0) as f64 / 1_000.0,
                    needs_click: data.needs_click,
                }
            })
            .collect()
    }

    pub fn position_at(&self, local_us: i64) -> Option<f64> {
        self.snapshot_clients()
            .into_iter()
            .find(|client| client.role == Role::Player)
            .map(|client| {
                let data = client.data.lock().expect("video client data poisoned");
                if data.state == "playing" {
                    data.last_pos_s + (local_us - data.report_local_us).max(0) as f64 / 1e6
                } else {
                    data.last_pos_s
                }
            })
    }

    fn broadcast(&self, value: Value) {
        for client in self.snapshot_clients() {
            send(&client, value.clone());
        }
    }
    fn cancel_learning(&self, clear_timeline: bool) {
        for client in self.snapshot_clients() {
            let mut data = client.data.lock().expect("video client data poisoned");
            data.policy.cancel_learning();
            if clear_timeline {
                data.start_local_us = None;
                data.policy.start_us = None;
            }
        }
    }
    fn snapshot_clients(&self) -> Vec<Arc<Client>> {
        self.inner
            .clients
            .lock()
            .expect("video clients poisoned")
            .clone()
    }
}

impl Drop for VideoBridge {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.inner.port));
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
    }
}

fn accept_loop(listener: TcpListener, inner: Arc<Inner>) {
    while !inner.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if inner.clients.lock().expect("video clients poisoned").len() >= MAX_CLIENTS {
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                let current = Arc::clone(&inner);
                thread::spawn(move || handle_connection(stream, current));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(_) => break,
        }
    }
}

// tungstenite's handshake callback requires its HTTP response as the error value.
#[allow(clippy::result_large_err)]
fn handle_connection(mut stream: TcpStream, inner: Arc<Inner>) {
    let _ = stream.set_nonblocking(false);
    let request = match peek_request(&mut stream) {
        Some(request) => request,
        None => return,
    };
    let (method, target, headers) = match parse_request(&request) {
        Some(value) => value,
        None => {
            consume_request(&mut stream, request.len());
            reply(&mut stream, 400, "Bad Request", "");
            return;
        }
    };
    if !valid_origin(&headers, inner.port) {
        consume_request(&mut stream, request.len());
        reply(&mut stream, 403, "Forbidden", "");
        return;
    }
    if method != "GET" {
        consume_request(&mut stream, request.len());
        reply(&mut stream, 404, "Not Found", "");
        return;
    }
    if target == "/" || target == "/?role=obs" {
        consume_request(&mut stream, request.len());
        // Keep the embedded page's seek and play together at the advanced deadline.
        // This adapter lives here so the bridge owns the complete scheduling policy.
        let page = PAGE.replace(
            "player.seekTo(m.from_s,true);playTimer=setTimeout(()=>player.playVideo(),",
            "playTimer=setTimeout(()=>{player.seekTo(m.from_s,true);player.playVideo();},",
        );
        reply(&mut stream, 200, "OK", &page);
        return;
    }
    if !target.starts_with("/ws?")
        || !headers.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket")
        })
    {
        consume_request(&mut stream, request.len());
        reply(&mut stream, 404, "Not Found", "");
        return;
    }
    let Some(role) = role_from_target(target) else {
        consume_request(&mut stream, request.len());
        reply(&mut stream, 404, "Not Found", "");
        return;
    };
    let port = inner.port;
    let accepted_role = role;
    let websocket = match accept_hdr(stream, move |request: &Request, response: Response| {
        let path = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or_default();
        if role_from_target(path) != Some(accepted_role) || !request_headers_valid(request, port) {
            return Err(Response::builder()
                .status(403)
                .body(Some("Forbidden".to_owned()))
                .expect("valid rejection response"));
        }
        Ok(response)
    }) {
        Ok(socket) => socket,
        Err(_) => return,
    };
    let (tx, rx) = mpsc::channel();
    let client = Arc::new(Client {
        role,
        tx,
        data: Mutex::new(ClientData::new()),
    });
    {
        let mut clients = inner.clients.lock().expect("video clients poisoned");
        if clients.len() >= MAX_CLIENTS {
            return;
        }
        clients.push(Arc::clone(&client));
    }
    client_loop(websocket, rx, Arc::clone(&client), Arc::clone(&inner));
    let mut clients = inner.clients.lock().expect("video clients poisoned");
    clients.retain(|item| !Arc::ptr_eq(item, &client));
}

fn peek_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = vec![0; MAX_REQUEST];
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let read = stream.peek(&mut buf).ok()?;
        if let Some(end) = buf[..read]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            buf.truncate(end + 4);
            return Some(buf);
        }
        if read == MAX_REQUEST || Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn consume_request(stream: &mut TcpStream, len: usize) {
    let mut request = vec![0; len];
    let _ = stream.read_exact(&mut request);
}

type ParsedRequest<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

fn parse_request(bytes: &[u8]) -> Option<ParsedRequest<'_>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?;
    let target = first.next()?;
    if first.next()? != "HTTP/1.1" {
        return None;
    }
    let headers = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()))
        })
        .collect();
    Some((method, target, headers))
}

fn valid_origin(headers: &[(impl AsRef<str>, impl AsRef<str>)], port: u16) -> bool {
    let host = headers
        .iter()
        .find(|(key, _)| key.as_ref().eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.as_ref());
    let Some(host) = host else {
        return false;
    };
    if !valid_host(host, port) {
        return false;
    }
    match headers
        .iter()
        .find(|(key, _)| key.as_ref().eq_ignore_ascii_case("origin"))
        .map(|(_, value)| value.as_ref())
    {
        None => true,
        Some(origin) => origin == format!("http://{host}"),
    }
}

fn request_headers_valid(request: &Request, port: u16) -> bool {
    let host = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok());
    let Some(host) = host else {
        return false;
    };
    if !valid_host(host, port) {
        return false;
    }
    request
        .headers()
        .get("origin")
        .map(|value| value.to_str().ok() == Some(format!("http://{host}").as_str()))
        .unwrap_or(true)
}

fn valid_host(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn role_from_target(target: &str) -> Option<Role> {
    match target {
        "/ws?role=player" => Some(Role::Player),
        "/ws?role=obs" => Some(Role::Obs),
        _ => None,
    }
}

fn reply(stream: &mut TcpStream, code: u16, text: &str, body: &str) {
    let content_type = if code == 200 {
        "text/html; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    let response = format!("HTTP/1.1 {code} {text}\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes());
}

fn client_loop(
    mut socket: WebSocket<TcpStream>,
    rx: mpsc::Receiver<String>,
    client: Arc<Client>,
    inner: Arc<Inner>,
) {
    socket.set_config(|config| {
        config.max_message_size = Some(MAX_REQUEST);
        config.max_frame_size = Some(MAX_REQUEST);
    });
    let _ = socket
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(100)));
    let mut next_ping = crate::clock::now_us();
    let mut ping_id = 0u64;
    while !inner.stop.load(Ordering::Acquire) {
        while let Ok(command) = rx.try_recv() {
            if socket.send(Message::Text(command.into())).is_err() {
                return;
            }
        }
        let now = crate::clock::now_us();
        if now >= next_ping {
            ping_id = ping_id.wrapping_add(1);
            let ping = json!({"t":"ping","id":ping_id,"t1":now}).to_string();
            if socket.send(Message::Text(ping.into())).is_err() {
                return;
            }
            next_ping = now + PING_US;
        }
        match socket.read() {
            Ok(Message::Text(text)) => handle_message(&client, &text),
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(WsError::Io(error))
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut => {
            }
            Err(_) => return,
        }
    }
    let _ = socket.close(None);
}

fn handle_message(client: &Client, text: &str) {
    if text.len() > MAX_REQUEST {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let Some(kind) = value.get("t").and_then(Value::as_str) else {
        return;
    };
    let now = crate::clock::now_us();
    let mut data = client.data.lock().expect("video client data poisoned");
    match kind {
        "pong" => {
            let Some(t1) = value.get("t1").and_then(Value::as_i64) else {
                return;
            };
            let Some(t2) = value.get("t2").and_then(Value::as_f64) else {
                return;
            };
            let rtt = now - t1;
            if !(0..=5_000_000).contains(&rtt) {
                return;
            }
            let offset = (t2 * 1_000.0).round() as i64 - (t1 + now) / 2;
            data.samples.push_back((rtt, offset));
            if data.samples.len() > 20 {
                let _ = data.samples.pop_front();
            }
            if let Some((rtt, offset)) = data.samples.iter().min_by_key(|(rtt, _)| *rtt).copied() {
                data.rtt_us = Some(rtt);
                data.offset_us = Some(offset);
            }
        }
        "state" => {
            let Some(state) = value.get("state").and_then(Value::as_str) else {
                return;
            };
            let Some(pos) = value
                .get("pos_s")
                .and_then(Value::as_f64)
                .filter(|pos| pos.is_finite())
            else {
                return;
            };
            data.state = state.to_owned();
            data.last_pos_s = pos;
            let offset_us = data.offset_us.unwrap_or(0);
            data.report_local_us = value
                .get("page_ms")
                .and_then(Value::as_f64)
                .map(|page| (page * 1_000.0).round() as i64 - offset_us)
                .unwrap_or(now);
            let Some(start) = data.start_local_us else {
                return;
            };
            // Use the position's own timestamp; packet transit cannot bias learning.
            let report_us = data.report_local_us;
            let desired = data.from_s + (report_us - start) as f64 / 1e6;
            let error_ms = (data.last_pos_s - desired) * 1_000.0;
            data.error_ms = Some(error_ms);
            if let Some(lead_s) = data.policy.report(report_us, state, error_ms) {
                let desired_now = data.from_s + (now - start) as f64 / 1e6;
                send(
                    client,
                    json!({"t":"seek","pos_s":(desired_now + lead_s).max(0.0)}),
                );
            }
        }
        "error" => {
            data.state = "error".to_owned();
            data.policy.cancel_learning();
        }
        "ready" | "unlocked" => data.needs_click = false,
        "needs_click" => data.needs_click = true,
        _ => {}
    }
}

/// Stateless eligibility helper retained for callers; reports use CorrectionPolicy.
pub fn should_correct(error_ms: f64, state: &str, now_us: i64, last_seek_us: i64) -> bool {
    error_ms.abs() > 60.0 && state == "playing" && now_us - last_seek_us >= COOLDOWN_US
}

fn send(client: &Client, value: Value) {
    let _ = client.tx.send(value.to_string());
}

#[cfg(test)]
mod tests {
    use super::{should_correct, CorrectionPolicy};
    #[test]
    fn correction_policy_has_threshold_and_cooldown() {
        assert!(should_correct(61.0, "playing", 4_000_000, 0));
        assert!(!should_correct(20.0, "playing", 4_000_000, 0));
        assert!(!should_correct(200.0, "buffering", 4_000_000, 0));
        assert!(!should_correct(200.0, "paused", 4_000_000, 0));
        assert!(!should_correct(200.0, "playing", 3_999_999, 0));
    }

    // Event-driven player model: seek positions are frozen until an exact resume
    // event, with independent seeded latency jitter and report noise.
    struct PlayerModel {
        policy: CorrectionPolicy,
        now_us: i64,
        pos_s: f64,
        resume_us: Option<i64>,
        drift: f64,
        seed: u64,
        seeks: Vec<i64>,
        errors: Vec<(i64, f64)>,
    }

    impl PlayerModel {
        fn new(seed: u64, drift: f64) -> Self {
            let mut policy = CorrectionPolicy::new();
            policy.play_at(0);
            Self {
                policy,
                now_us: 0,
                pos_s: 0.0,
                // First reproduce the measured uncompensated -150 ms cold start.
                resume_us: Some(150_000),
                drift,
                seed,
                seeks: Vec::new(),
                errors: Vec::new(),
            }
        }

        fn noise(&mut self) -> f64 {
            self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (self.seed >> 32) as f64 / u32::MAX as f64 * 2.0 - 1.0
        }

        fn report(&mut self) {
            let error_ms = (self.pos_s - self.now_us as f64 / 1e6) * 1_000.0 + self.noise() * 3.0;
            let state = if self.resume_us.is_some() {
                "buffering"
            } else {
                "playing"
            };
            let seek = self.policy.report(self.now_us, state, error_ms);
            if state == "playing" {
                self.errors.push((self.now_us, error_ms));
            } else {
                assert!(seek.is_none(), "seek during buffering");
            }
            if let Some(lead_s) = seek {
                self.seeks.push(self.now_us);
                self.pos_s = self.now_us as f64 / 1e6 + lead_s;
                self.resume_us = Some(self.now_us + 120_000 + (self.noise() * 20_000.0) as i64);
                assert!(self
                    .policy
                    .report(self.now_us + 1, "buffering", error_ms)
                    .is_none());
            }
        }

        fn run_until(&mut self, end_us: i64) {
            while self.now_us < end_us {
                let next_report = (self.now_us / 250_000 + 1) * 250_000;
                let next = self
                    .resume_us
                    .unwrap_or(next_report)
                    .min(next_report)
                    .min(end_us);
                if self.resume_us.is_none() {
                    self.pos_s += (next - self.now_us) as f64 / 1e6 * (1.0 + self.drift);
                }
                self.now_us = next;
                if self.resume_us == Some(next) {
                    self.resume_us = None;
                    self.report();
                } else if next == next_report {
                    self.report();
                }
            }
        }
    }

    #[test]
    fn simulated_seek_latency_converges_and_stays_within_40_ms() {
        let mut worst_error: f64 = 0.0;
        let mut latest_convergence = 0;
        let mut most_corrections = 0;
        for seed in 1..=64 {
            let mut player = PlayerModel::new(seed, 0.0);
            player.run_until(5_000_000);
            assert!((player.errors[0].1 + 150.0).abs() <= 3.0);
            assert!(!player.seeks.is_empty() && player.seeks.len() <= 3);
            let corrections = player.seeks.len();
            let converged = player.seeks.last().copied().unwrap() + 140_000;
            latest_convergence = latest_convergence.max(converged);
            most_corrections = most_corrections.max(corrections);
            player.run_until(converged + 60_000_000);
            assert_eq!(
                player.seeks.len(),
                corrections,
                "seed {seed}: further seeks"
            );
            for &(_, error) in player.errors.iter().filter(|(time, _)| *time >= converged) {
                worst_error = worst_error.max(error.abs());
                assert!(error.abs() <= 40.0, "seed {seed}: {error} ms");
            }
        }
        println!("64 seeds: convergence <= {:.3} s, corrections <= {most_corrections}, worst steady |error| = {worst_error:.3} ms over 60 s", latest_convergence as f64 / 1e6);
    }

    #[test]
    fn simulated_300_ppm_drift_needs_at_most_one_seek_per_minute() {
        let mut worst_per_minute = 0;
        let mut most_seeks = 0;
        for seed in 1..=64 {
            let mut player = PlayerModel::new(seed, 300e-6);
            player.run_until(5_000_000);
            let initial = player.seeks.len();
            player.run_until(605_000_000);
            let learned = &player.seeks[initial..];
            most_seeks = most_seeks.max(learned.len());
            for &time in learned {
                let count = learned
                    .iter()
                    .filter(|&&other| other >= time && other < time + 60_000_000)
                    .count();
                worst_per_minute = worst_per_minute.max(count);
                assert!(count <= 1, "seed {seed}: seek storm at {time}");
            }
        }
        println!("300 ppm, 64 seeds over 600 s after learning: <= {most_seeks} seeks total, <= {worst_per_minute} per sliding 60 s");
    }

    fn running_policy() -> CorrectionPolicy {
        let mut policy = CorrectionPolicy::new();
        policy.play_at(-3_000_000);
        policy
    }

    #[test]
    fn correction_requires_two_distinct_consecutive_playing_reports() {
        let mut policy = running_policy();
        for state in [
            "buffering",
            "ad",
            "paused",
            "ended",
            "cued",
            "unstarted",
            "error",
        ] {
            assert!(policy.report(0, "playing", -150.0).is_none());
            assert!(policy.report(250_000, state, -150.0).is_none());
            assert!(policy.report(500_000, "playing", -150.0).is_none());
            // Repeated processing of the same report cannot count as a second sample.
            assert!(policy.report(500_000, "playing", -150.0).is_none());
            policy = running_policy();
        }
        assert!(policy.report(0, "playing", -150.0).is_none());
        assert!(policy.report(250_000, "playing", 60.0).is_none());
        assert!(policy.report(500_000, "playing", -150.0).is_none());
        assert_eq!(policy.report(750_000, "playing", -150.0), Some(0.12));
    }

    #[test]
    fn learning_waits_for_stability_and_cooldown_starts_at_resume() {
        let mut policy = running_policy();
        assert!(policy.report(0, "playing", -150.0).is_none());
        assert!(policy.report(250_000, "playing", -150.0).is_some());
        for time in (500_000..=5_000_000).step_by(250_000) {
            assert!(policy.report(time, "buffering", -500.0).is_none());
        }
        for time in (5_250_000..=6_000_000).step_by(250_000) {
            assert!(policy.report(time, "playing", -100.0).is_none());
            assert_eq!(policy.seek_lead_s, 0.12);
        }
        assert!(policy.report(6_250_000, "playing", -100.0).is_none());
        assert!((policy.seek_lead_s - 0.2).abs() < 1e-12);
        for time in (6_500_000..9_250_000).step_by(250_000) {
            assert!(policy.report(time, "playing", -100.0).is_none());
        }
        assert_eq!(policy.report(9_250_000, "playing", -100.0), Some(0.2));
    }

    #[test]
    fn storm_guard_raises_threshold_for_30_seconds_after_third_correction() {
        let mut policy = running_policy();
        let mut third = None;
        for time in (0..=15_000_000).step_by(250_000) {
            if policy.report(time, "playing", -200.0).is_some() {
                assert!(policy.report(time + 1, "buffering", -200.0).is_none());
                if policy.corrections == 3 {
                    third = Some(time);
                    break;
                }
            }
        }
        let third = third.expect("three corrections in 30 seconds");
        assert_eq!(policy.threshold_ms(third), 120.0);
        for time in ((third + 250_000)..(third + 30_000_000)).step_by(250_000) {
            assert!(policy.report(time, "playing", -100.0).is_none());
        }
        assert_eq!(policy.corrections, 3);
        assert_eq!(policy.threshold_ms(third + 30_000_000), 60.0);
        assert!(policy
            .report(third + 30_000_000, "playing", -100.0)
            .is_none());
        assert!(policy
            .report(third + 30_250_000, "playing", -100.0)
            .is_some());
    }

    #[test]
    fn startup_lead_learns_150_ms_latency_and_persists_between_starts() {
        let mut policy = CorrectionPolicy::new();
        let mut residuals = Vec::new();
        for start in [1_000_000, 10_000_000, 20_000_000] {
            policy.play_at(start);
            // Seek and play execute at start - lead; the frozen player resumes 150 ms later.
            let resume = start - (policy.start_lead_ms * 1_000.0).round() as i64 + 150_000;
            let residual_ms = (start - resume) as f64 / 1_000.0;
            residuals.push(residual_ms);
            for time in (resume..=start + 2_000_000).step_by(250_000) {
                assert!(policy.report(time, "playing", residual_ms).is_none());
            }
            assert!(!policy.startup_pending);
            assert_eq!(policy.corrections, 0);
        }
        assert!((policy.start_lead_ms - 149.76).abs() < 0.001);
        println!(
            "150 ms startup latency: residuals {residuals:?} ms; learned lead {:.3} ms",
            policy.start_lead_ms
        );
    }

    #[test]
    fn bridge_keeps_leads_per_client_until_load_and_uses_them_in_commands() {
        use super::*;
        let (tx, rx) = mpsc::channel();
        let first = Arc::new(Client {
            role: Role::Player,
            tx,
            data: Mutex::new(ClientData::new()),
        });
        let (tx, other_rx) = mpsc::channel();
        let second = Arc::new(Client {
            role: Role::Obs,
            tx,
            data: Mutex::new(ClientData::new()),
        });
        {
            let mut data = first.data.lock().unwrap();
            data.policy.seek_lead_s = 0.3;
            data.policy.start_lead_ms = 222.0;
            data.offset_us = Some(12_345_000);
        }
        let bridge = VideoBridge {
            inner: Arc::new(Inner {
                port: 0,
                stop: AtomicBool::new(false),
                clients: Mutex::new(vec![Arc::clone(&first), second]),
            }),
            accept_thread: None,
        };
        let start = crate::clock::now_us() - 3_000_000;
        bridge.play_at(start, 30.0);
        let command: Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(command["from_s"], 30.0);
        assert_eq!(
            command["page_ms"],
            start as f64 / 1_000.0 + 12_345.0 - 222.0
        );
        let other: Value = serde_json::from_str(&other_rx.recv().unwrap()).unwrap();
        assert_eq!(other["page_ms"], start as f64 / 1_000.0 - 120.0);
        for _ in 0..2 {
            let now = crate::clock::now_us();
            handle_message(&first, &json!({"t":"state", "state":"playing", "pos_s":30.2 + (now-start) as f64 / 1e6, "page_ms":now as f64 / 1_000.0 + 12_345.0}).to_string());
        }
        let correction: Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(correction["t"], "seek");
        let now = crate::clock::now_us();
        let lead = correction["pos_s"].as_f64().unwrap() - (30.0 + (now - start) as f64 / 1e6);
        assert!(
            (lead - 0.3).abs() < 0.01,
            "correction must include client lead: {lead}"
        );
        assert_eq!(bridge.clients()[0].corrections, 1);
        assert_eq!(bridge.clients()[1].corrections, 0);
        bridge.pause();
        bridge.seek(10.0);
        bridge.play_at(start, 30.0);
        assert_eq!(bridge.clients()[0].seek_lead_ms, 300.0);
        assert_eq!(first.data.lock().unwrap().policy.start_lead_ms, 222.0);
        bridge.load("dQw4w9WgXcQ");
        for info in bridge.clients() {
            assert_eq!(info.seek_lead_ms, 120.0);
            assert_eq!(info.corrections, 0);
            assert_eq!(info.error_ms, None);
        }
        let data = first.data.lock().unwrap();
        assert_eq!(data.policy.start_lead_ms, 120.0);
        assert_eq!(data.start_local_us, None);
    }

    #[test]
    fn lead_clamps_and_interrupted_playback_cannot_train_on_stale_reports() {
        let mut policy = running_policy();
        policy.seeking = true;
        for time in (0..=1_000_000).step_by(250_000) {
            policy.report(time, "playing", -1_000.0);
        }
        assert_eq!(policy.seek_lead_s, 0.6);
        policy.seeking = true;
        policy.resumed_us = None;
        policy.playing_since_us = None;
        for time in (1_250_000..=2_000_000).step_by(250_000) {
            policy.report(time, "playing", 1_000.0);
        }
        policy.report(2_250_000, "buffering", 1_000.0);
        policy.report(2_500_000, "playing", 1_000.0);
        assert_eq!(policy.seek_lead_s, 0.6);
        // A gap also restarts the stable observation window.
        policy.report(4_000_000, "playing", 1_000.0);
        assert_eq!(policy.seek_lead_s, 0.6);
        for time in (4_250_000..=5_000_000).step_by(250_000) {
            policy.report(time, "playing", 1_000.0);
        }
        assert_eq!(policy.seek_lead_s, 0.0);
        policy.play_at(10_000_000);
        assert_eq!(policy.seek_lead_s, 0.0, "play_at must retain learning");
        for time in (10_000_000..=11_000_000).step_by(250_000) {
            policy.report(time, "playing", -1_000.0);
        }
        assert_eq!(policy.start_lead_ms, 400.0);
        policy.play_at(20_000_000);
        for time in (20_000_000..=21_000_000).step_by(250_000) {
            policy.report(time, "playing", 1_000.0);
        }
        assert_eq!(policy.start_lead_ms, 0.0);
    }
}
