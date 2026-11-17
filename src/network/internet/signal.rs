//! Rendezvous through the free ntfy.sh relay: host and guest post their addresses to a
//! topic derived from the join secret. Each post is encrypted with ChaCha20-Poly1305 under a
//! key derived from the secret, so the relay (and anyone guessing the topic) learns nothing
//! but ciphertext. The relay sees both PCs' IP addresses, as any web server would.

use super::https;
use crate::network::crypto::{hex, mac, random_bytes};
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;

pub const RELAY_HOST: &str = "ntfy.sh";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Record {
    /// "h" host or "g" guest.
    pub role: String,
    /// Tunnel session id (random per guest attempt; host uses 0).
    pub sid: u64,
    pub public: Option<SocketAddr>,
    pub local: Vec<SocketAddr>,
}

pub struct Channel {
    topic: String,
    key: [u8; 32],
}

impl Channel {
    pub fn new(secret: &[u8; 16]) -> Self {
        let t = mac(secret, b"DuetSync-topic-v1", &[]);
        Channel {
            topic: format!("duetsync-{}", &hex(&t)[..32]),
            key: mac(secret, b"DuetSync-signal-v1", &[]),
        }
    }

    pub fn seal(&self, r: &Record) -> String {
        let nonce = random_bytes::<12>();
        let c = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let ct = c
            .encrypt(
                Nonce::from_slice(&nonce),
                serde_json::to_vec(r).unwrap_or_default().as_slice(),
            )
            .expect("encrypt");
        let mut all = nonce.to_vec();
        all.extend_from_slice(&ct);
        base64::engine::general_purpose::STANDARD.encode(all)
    }

    pub fn open(&self, text: &str) -> Option<Record> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .ok()?;
        if raw.len() < 12 + 16 {
            return None;
        }
        let c = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let pt = c.decrypt(Nonce::from_slice(&raw[..12]), &raw[12..]).ok()?;
        serde_json::from_slice(&pt).ok()
    }

    pub fn publish(&self, r: &Record) -> Result<(), String> {
        let body = self.seal(r);
        let (code, _) = https::request(
            "POST",
            RELAY_HOST,
            &format!("/{}", self.topic),
            Some(body.as_bytes()),
            8000,
        )?;
        if code == 200 {
            Ok(())
        } else if code == 429 {
            Err("relay busy (rate limited), retrying".into())
        } else {
            Err(format!("relay answered HTTP {code}"))
        }
    }

    /// Stream records posted in the last `since` (e.g. "10m") and from now on. Returns when
    /// the stream ends, `stop` is set, or `on_record` returns false.
    pub fn subscribe(
        &self,
        since: &str,
        stop: &AtomicBool,
        mut on_record: impl FnMut(Record) -> bool,
    ) -> Result<(), String> {
        https::stream_lines(
            RELAY_HOST,
            &format!("/{}/json?since={since}", self.topic),
            70_000,
            stop,
            |line| {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    return true;
                };
                if v["event"].as_str() != Some("message") {
                    return true;
                }
                match v["message"].as_str().and_then(|m| self.open(m)) {
                    Some(r) => on_record(r),
                    None => true, // not ours / tampered
                }
            },
        )
    }
}

/// Diagnostic: post a record and read it back through both poll and stream APIs.
pub fn relay_selftest() -> Result<String, String> {
    let secret = random_bytes::<16>();
    let ch = Channel::new(&secret);
    let rec = Record {
        role: "h".into(),
        sid: 7,
        public: None,
        local: vec![],
    };
    ch.publish(&rec)?;
    let (code, body) = https::request(
        "GET",
        RELAY_HOST,
        &format!("/{}/json?poll=1&since=all", ch.topic),
        None,
        8000,
    )?;
    let text = String::from_utf8_lossy(&body).to_string();
    let polled = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["message"].as_str().and_then(|m| ch.open(m)))
        .count();
    let mut raw_stream = Vec::new();
    let stop = AtomicBool::new(false);
    let t0 = std::time::Instant::now();
    let r = https::stream_lines(
        RELAY_HOST,
        &format!("/{}/json?since=all", ch.topic),
        15_000,
        &stop,
        |line| {
            raw_stream.push(line.chars().take(120).collect::<String>());
            raw_stream.len() < 3 && t0.elapsed().as_secs() < 10
        },
    );
    Ok(format!(
        "poll HTTP {code}, {polled} decrypted, raw poll {:?}\nstream {r:?} after {:.1}s, lines {raw_stream:?}",
        text.chars().take(200).collect::<String>(),
        t0.elapsed().as_secs_f64()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_roundtrip_and_reject_wrong_key() {
        let a = Channel::new(&[1u8; 16]);
        let b = Channel::new(&[2u8; 16]);
        assert_ne!(a.topic, b.topic);
        let r = Record {
            role: "g".into(),
            sid: 42,
            public: Some("203.0.113.9:40001".parse().unwrap()),
            local: vec!["192.168.1.5:5000".parse().unwrap()],
        };
        let s = a.seal(&r);
        assert_eq!(a.open(&s), Some(r));
        assert_eq!(b.open(&s), None);
        assert!(!s.contains("203.0.113.9"), "addresses are encrypted");
    }
}
