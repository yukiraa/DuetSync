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
const CORRECTION_US: i64 = 500_000;
const COOLDOWN_US: i64 = 3_000_000;
const PAGE: &str = include_str!("../../assets/video/player.html");

#[derive(Clone, Debug)]
pub struct ClientInfo {
    pub role: String,
    pub state: String,
    pub last_pos_s: f64,
    pub error_ms: Option<f64>,
    pub rtt_ms: f64,
    /// The browser blocks sound until the user clicks the page once.
    pub needs_click: bool,
}

pub struct VideoBridge {
    inner: Arc<Inner>,
    accept_thread: Option<JoinHandle<()>>,
    correction_thread: Option<JoinHandle<()>>,
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
    last_seek_us: i64,
    error_ms: Option<f64>,
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
            last_seek_us: i64::MIN / 2,
            error_ms: None,
        }
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
        let correction_inner = Arc::clone(&inner);
        let correction_thread = thread::spawn(move || correction_loop(correction_inner));
        Ok(Self {
            inner,
            accept_thread: Some(accept_thread),
            correction_thread: Some(correction_thread),
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
        self.broadcast(json!({"t":"load","videoId":id}));
    }

    pub fn play_at(&self, start_local_us: i64, from_s: f64) {
        let cue = start_local_us - crate::clock::now_us() > 1_500_000;
        for client in self.snapshot_clients() {
            let page_ms = {
                let mut data = client.data.lock().expect("video client data poisoned");
                data.start_local_us = Some(start_local_us);
                data.from_s = from_s;
                (start_local_us + data.offset_us.unwrap_or(0)) as f64 / 1_000.0
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
        self.broadcast(json!({"t":"pause"}));
    }
    pub fn stop(&self) {
        self.broadcast(json!({"t":"stop"}));
    }
    pub fn seek(&self, pos_s: f64) {
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
        if let Some(thread) = self.correction_thread.take() {
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
        reply(&mut stream, 200, "OK", PAGE);
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
            if let Some(state) = value.get("state").and_then(Value::as_str) {
                data.state = state.to_owned();
            }
            if let Some(pos) = value.get("pos_s").and_then(Value::as_f64) {
                data.last_pos_s = pos;
            }
            let offset_us = data.offset_us.unwrap_or(0);
            data.report_local_us = value
                .get("page_ms")
                .and_then(Value::as_f64)
                .map(|page| (page * 1_000.0).round() as i64 - offset_us)
                .unwrap_or(now);
        }
        "error" => {
            data.state = "error".to_owned();
        }
        "ready" | "unlocked" => data.needs_click = false,
        "needs_click" => data.needs_click = true,
        _ => {}
    }
}

fn correction_loop(inner: Arc<Inner>) {
    while !inner.stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_micros(CORRECTION_US as u64));
        let now = crate::clock::now_us();
        let clients = inner
            .clients
            .lock()
            .expect("video clients poisoned")
            .clone();
        for client in clients {
            let seek = {
                let mut data = client.data.lock().expect("video client data poisoned");
                let Some(start) = data.start_local_us else {
                    continue;
                };
                if data.state != "playing" {
                    continue;
                }
                let desired = data.from_s + (now - start) as f64 / 1e6;
                let reported = data.last_pos_s + (now - data.report_local_us).max(0) as f64 / 1e6;
                let error_ms = (reported - desired) * 1_000.0;
                data.error_ms = Some(error_ms);
                if should_correct(error_ms, &data.state, now, data.last_seek_us) {
                    data.last_seek_us = now;
                    Some(desired.max(0.0))
                } else {
                    None
                }
            };
            if let Some(pos_s) = seek {
                send(&client, json!({"t":"seek","pos_s":pos_s}));
            }
        }
    }
}

/// Pure drift correction policy, exposed for deterministic tests.
pub fn should_correct(error_ms: f64, state: &str, now_us: i64, last_seek_us: i64) -> bool {
    error_ms.abs() > 60.0
        && state != "buffering"
        && state != "ad"
        && now_us - last_seek_us >= COOLDOWN_US
}

fn send(client: &Client, value: Value) {
    let _ = client.tx.send(value.to_string());
}

#[cfg(test)]
mod tests {
    use super::should_correct;
    #[test]
    fn correction_policy_has_threshold_and_cooldown() {
        assert!(should_correct(61.0, "playing", 4_000_000, 0));
        assert!(!should_correct(20.0, "playing", 4_000_000, 0));
        assert!(!should_correct(200.0, "buffering", 4_000_000, 0));
        assert!(!should_correct(200.0, "playing", 2_999_999, 0));
    }
}
