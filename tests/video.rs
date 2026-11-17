use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use duetsync::clock::now_us;
use duetsync::video::{should_correct, VideoBridge};
use serde_json::{json, Value};
use tungstenite::{client, Message, WebSocket};

fn http(port: u16, host: &str, origin: Option<&str>, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect http");
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(origin) = origin {
        request.push_str(&format!("Origin: {origin}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("write http request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read http response");
    response
}

#[test]
fn http_only_serves_the_embedded_loopback_page() {
    let bridge = VideoBridge::start(0).expect("start bridge");
    let port = bridge.port();
    let host = format!("127.0.0.1:{port}");
    let root = http(port, &host, Some(&format!("http://{host}")), "/");
    assert!(root.starts_with("HTTP/1.1 200"), "{root}");
    assert!(root.contains("youtube.com/iframe_api"));
    assert!(root.contains("Cache-Control: no-store"));
    assert!(root
        .contains("playTimer=setTimeout(()=>{player.seekTo(m.from_s,true);player.playVideo();},"));
    assert!(http(port, &host, None, "/nope").starts_with("HTTP/1.1 404"));
    assert!(http(port, "example.test:80", None, "/").starts_with("HTTP/1.1 403"));
    assert!(http(port, &host, Some("https://example.test"), "/").starts_with("HTTP/1.1 403"));
    assert_eq!(bridge.url("player"), format!("http://127.0.0.1:{port}"));
    assert_eq!(
        bridge.url("obs"),
        format!("http://127.0.0.1:{port}?role=obs")
    );
}

fn player(port: u16) -> WebSocket<TcpStream> {
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect websocket");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("set timeout");
    let (socket, _) =
        client(format!("ws://127.0.0.1:{port}/ws?role=player"), stream).expect("websocket client");
    socket
}

fn reply_to_ping(socket: &mut WebSocket<TcpStream>, offset_ms: f64) {
    loop {
        let message = socket.read().expect("read ping");
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).expect("json from bridge");
        if value["t"] == "ping" {
            let pong = json!({"t":"pong", "id":value["id"], "t1":value["t1"], "t2":now_us() as f64 / 1_000.0 + offset_ms});
            socket
                .send(Message::Text(pong.to_string().into()))
                .expect("send pong");
            return;
        }
    }
}

#[test]
fn websocket_clock_maps_play_deadline_to_page_time() {
    let bridge = VideoBridge::start(0).expect("start bridge");
    let mut socket = player(bridge.port());
    reply_to_ping(&mut socket, 12_345.0);
    thread::sleep(Duration::from_millis(50));
    let info = bridge.clients();
    assert_eq!(info.len(), 1);
    assert!(
        (info[0].rtt_ms >= 0.0) && (info[0].rtt_ms < 200.0),
        "{:?}",
        info[0]
    );

    let start = now_us() + 2_000_000;
    bridge.play_at(start, 30.0);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut cue = false;
    let mut target = None;
    while Instant::now() < deadline && target.is_none() {
        let Message::Text(text) = socket.read().expect("bridge command") else {
            continue;
        };
        let command: Value = serde_json::from_str(&text).expect("command json");
        match command["t"].as_str() {
            Some("cue") => {
                cue = true;
                assert_eq!(command["pos_s"], 30.0);
            }
            Some("play_at") => target = command["page_ms"].as_f64(),
            _ => {}
        }
    }
    assert!(cue, "play_at farther than 1.5 seconds must cue first");
    let expected = start as f64 / 1_000.0 + 12_345.0 - 120.0;
    assert!(
        (target.expect("play_at command") - expected).abs() < 2.0,
        "target={target:?}, expected={expected}"
    );
}

#[test]
fn drift_policy_seeks_large_error_once_per_cooldown() {
    assert!(should_correct(200.0, "playing", 4_000_000, 0));
    assert!(!should_correct(20.0, "playing", 4_000_000, 0));
    assert!(!should_correct(200.0, "playing", 4_000_000, 1));

    let bridge = VideoBridge::start(0).expect("start bridge");
    let mut socket = player(bridge.port());
    reply_to_ping(&mut socket, 0.0);
    let start = now_us() - 3_000_000;
    bridge.play_at(start, 10.0);
    let reported_at = now_us();
    let report = |at: i64| json!({"t":"state","state":"playing","pos_s":10.2 + (at-start) as f64 / 1e6,"page_ms":at as f64 / 1_000.0});
    socket
        .send(Message::Text(report(reported_at).to_string().into()))
        .expect("state");
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        if let Ok(Message::Text(text)) = socket.read() {
            assert_ne!(
                serde_json::from_str::<Value>(&text).expect("command json")["t"],
                "seek",
                "one report must not trigger correction"
            );
        }
    }
    socket
        .send(Message::Text(report(now_us()).to_string().into()))
        .expect("second state");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seeks = 0;
    while Instant::now() < deadline {
        match socket.read() {
            Ok(Message::Text(text))
                if serde_json::from_str::<Value>(&text).expect("command json")["t"] == "seek" =>
            {
                seeks += 1;
                break;
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    assert_eq!(seeks, 1, "large drift should seek");
    let info = bridge.clients();
    assert_eq!(info[0].corrections, 1);
    assert_eq!(info[0].seek_lead_ms, 120.0);
    assert!((info[0].error_ms.expect("reported error") - 200.0).abs() < 1.0);
    let until = Instant::now() + Duration::from_millis(900);
    while Instant::now() < until {
        if let Ok(Message::Text(text)) = socket.read() {
            assert_ne!(
                serde_json::from_str::<Value>(&text).expect("command json")["t"],
                "seek",
                "cooldown sent another seek"
            );
        }
    }
}

#[test]
fn fifth_client_is_refused_and_drop_releases_port() {
    let bridge = VideoBridge::start(0).expect("start bridge");
    let port = bridge.port();
    let mut clients = Vec::new();
    for _ in 0..4 {
        clients.push(player(port));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while bridge.clients().len() != 4 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(bridge.clients().len(), 4);
    let fifth = TcpStream::connect(("127.0.0.1", port)).expect("connect fifth socket");
    assert!(client(format!("ws://127.0.0.1:{port}/ws?role=player"), fifth).is_err());
    drop(clients);
    drop(bridge);
    thread::sleep(Duration::from_millis(50));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}
