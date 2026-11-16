//! Minimal synchronous OBS WebSocket 5.x client (rpcVersion 1).

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use tungstenite::{Message, WebSocket};

pub const OFFSET_MIN_MS: i64 = -950;
pub const OFFSET_MAX_MS: i64 = 20_000;
/// OBS "Render Delay" filter (`gpu_delay`) limit.
pub const VIDEO_DELAY_MAX_MS: i64 = 500;
pub const VIDEO_FILTER_KIND: &str = "gpu_delay";
pub const VIDEO_FILTER_NAME: &str = "DuetSync Delay";

#[derive(Clone, Debug, PartialEq)]
pub struct InputInfo {
    pub name: String,
    pub kind: String,
}

pub struct ObsClient {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    pub obs_version: String,
    pub ws_version: String,
}

/// `base64(sha256(base64(sha256(password + salt)) + challenge))`
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    b64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

impl ObsClient {
    pub fn connect(host: &str, port: u16, password: Option<&str>) -> Result<Self, String> {
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("resolve {host}: {e}"))?
            .next()
            .ok_or("no address")?;
        if !addr.ip().is_loopback() && !is_private(addr.ip()) {
            return Err(
                "refusing to connect to OBS on a public address; use localhost or a private network"
                    .into(),
            );
        }
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| {
            format!("cannot reach OBS at {addr} ({e}). Is OBS running with WebSocket server enabled (Tools → WebSocket Server Settings)?")
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let _ = stream.set_nodelay(true);
        let (ws, _) = tungstenite::client(format!("ws://{host}:{port}/"), stream)
            .map_err(|e| format!("WebSocket handshake failed: {e}"))?;
        let mut c = Self {
            ws,
            next_id: 1,
            obs_version: String::new(),
            ws_version: String::new(),
        };
        let hello = c.read_op(0)?;
        let d = &hello["d"];
        c.ws_version = d["obsWebSocketVersion"].as_str().unwrap_or("?").to_string();
        let mut ident = json!({ "rpcVersion": 1, "eventSubscriptions": 0 });
        if let Some(auth) = d.get("authentication") {
            let pw = password.ok_or("OBS requires a WebSocket password")?;
            let (Some(ch), Some(salt)) = (auth["challenge"].as_str(), auth["salt"].as_str()) else {
                return Err("malformed OBS authentication challenge".into());
            };
            ident["authentication"] = json!(auth_string(pw, salt, ch));
        }
        c.send(json!({ "op": 1, "d": ident }))?;
        c.read_op(2).map_err(|e| {
            if e.contains("closed") {
                "OBS rejected the WebSocket password".to_string()
            } else {
                e
            }
        })?;
        if let Ok(v) = c.request("GetVersion", json!({})) {
            c.obs_version = v["obsVersion"].as_str().unwrap_or("?").to_string();
        }
        Ok(c)
    }

    fn send(&mut self, v: Value) -> Result<(), String> {
        self.ws
            .send(Message::Text(v.to_string().into()))
            .map_err(|e| format!("OBS send: {e}"))
    }

    fn read_op(&mut self, op: u64) -> Result<Value, String> {
        loop {
            let m = self.ws.read().map_err(|e| match e {
                tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
                    "OBS connection closed".to_string()
                }
                tungstenite::Error::Protocol(_) => "OBS connection closed (protocol)".to_string(),
                e => format!("OBS read: {e}"),
            })?;
            let text = match m {
                Message::Text(t) => t.to_string(),
                Message::Close(f) => {
                    return Err(format!(
                        "OBS connection closed{}",
                        f.map(|f| format!(": {} {}", u16::from(f.code), f.reason))
                            .unwrap_or_default()
                    ))
                }
                _ => continue,
            };
            let v: Value = serde_json::from_str(&text).map_err(|_| "bad JSON from OBS")?;
            if v["op"].as_u64() == Some(op) {
                return Ok(v);
            }
        }
    }

    /// Send a request and wait for its response; `Err` carries OBS's status comment.
    pub fn request(&mut self, kind: &str, data: Value) -> Result<Value, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.send(json!({
            "op": 6,
            "d": { "requestType": kind, "requestId": id, "requestData": data }
        }))?;
        loop {
            let v = self.read_op(7)?;
            let d = &v["d"];
            if d["requestId"].as_str() != Some(id.as_str()) {
                continue;
            }
            let st = &d["requestStatus"];
            if st["result"].as_bool() == Some(true) {
                return Ok(d.get("responseData").cloned().unwrap_or(json!({})));
            }
            return Err(format!(
                "{kind} failed (code {}): {}",
                st["code"],
                st["comment"].as_str().unwrap_or("no details")
            ));
        }
    }

    pub fn input_list(&mut self) -> Result<Vec<InputInfo>, String> {
        let v = self.request("GetInputList", json!({}))?;
        Ok(v["inputs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| InputInfo {
                        name: i["inputName"].as_str().unwrap_or("").to_string(),
                        kind: i["inputKind"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn sync_offset(&mut self, input: &str) -> Result<i64, String> {
        let v = self.request("GetInputAudioSyncOffset", json!({ "inputName": input }))?;
        v["inputAudioSyncOffset"]
            .as_i64()
            .ok_or_else(|| "missing inputAudioSyncOffset".into())
    }

    pub fn set_sync_offset(&mut self, input: &str, ms: i64) -> Result<(), String> {
        if !(OFFSET_MIN_MS..=OFFSET_MAX_MS).contains(&ms) {
            return Err(format!(
                "offset {ms} ms outside OBS range {OFFSET_MIN_MS}..{OFFSET_MAX_MS}"
            ));
        }
        self.request(
            "SetInputAudioSyncOffset",
            json!({ "inputName": input, "inputAudioSyncOffset": ms }),
        )
        .map(|_| ())
    }

    pub fn monitor_type(&mut self, input: &str) -> Result<String, String> {
        let v = self.request("GetInputAudioMonitorType", json!({ "inputName": input }))?;
        Ok(v["monitorType"].as_str().unwrap_or("").to_string())
    }

    pub fn muted(&mut self, input: &str) -> Result<bool, String> {
        let v = self.request("GetInputMute", json!({ "inputName": input }))?;
        Ok(v["inputMuted"].as_bool().unwrap_or(false))
    }

    /// Current DuetSync render-delay filter value on `source`, if present.
    pub fn video_delay(&mut self, source: &str) -> Result<Option<i64>, String> {
        match self.request(
            "GetSourceFilter",
            json!({ "sourceName": source, "filterName": VIDEO_FILTER_NAME }),
        ) {
            Ok(v) => Ok(Some(v["filterSettings"]["delay_ms"].as_i64().unwrap_or(0))),
            Err(e) if e.contains("code 600") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_video_delay(&mut self, source: &str, ms: i64) -> Result<(), String> {
        if !(0..=VIDEO_DELAY_MAX_MS).contains(&ms) {
            return Err(format!("video delay must be 0..{VIDEO_DELAY_MAX_MS} ms"));
        }
        let settings = json!({ "delay_ms": ms });
        if self.video_delay(source)?.is_some() {
            self.request(
                "SetSourceFilterSettings",
                json!({ "sourceName": source, "filterName": VIDEO_FILTER_NAME, "filterSettings": settings }),
            )?;
        } else {
            self.request(
                "CreateSourceFilter",
                json!({ "sourceName": source, "filterName": VIDEO_FILTER_NAME,
                        "filterKind": VIDEO_FILTER_KIND, "filterSettings": settings }),
            )?;
        }
        Ok(())
    }

    pub fn remove_video_delay(&mut self, source: &str) -> Result<(), String> {
        if self.video_delay(source)?.is_some() {
            self.request(
                "RemoveSourceFilter",
                json!({ "sourceName": source, "filterName": VIDEO_FILTER_NAME }),
            )?;
        }
        Ok(())
    }
}

fn is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            v.is_private()
                || v.is_link_local()
                || (v.octets()[0] == 100 && v.octets()[1] & 0xC0 == 64)
        }
        std::net::IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn auth_matches_protocol_example() {
        // Values from the obs-websocket protocol documentation example.
        let s = super::auth_string(
            "supersecretpassword",
            "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
            "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=",
        );
        assert_eq!(s, "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4=");
    }
}
