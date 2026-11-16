//! Join codes, mutual HMAC authentication and ChaCha20-Poly1305 framing keys.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;
const B32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS random generator unavailable");
    b
}

/// Human-friendly join code for a 128-bit secret: 26 Crockford base32 chars, grouped.
pub fn encode_code(secret: &[u8; 16]) -> String {
    let mut bits: u128 = u128::from_be_bytes(*secret);
    let mut chars = [0u8; 26];
    for c in chars.iter_mut().rev() {
        *c = B32[(bits & 31) as usize];
        bits >>= 5;
    }
    let s = String::from_utf8(chars.to_vec()).unwrap();
    s.as_bytes()
        .chunks(5)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn decode_code(code: &str) -> Result<[u8; 16], String> {
    let mut v: u128 = 0;
    let mut n = 0;
    for ch in code.chars().filter(|c| c.is_ascii_alphanumeric()) {
        let c = match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        } as u8;
        let d = B32
            .iter()
            .position(|&b| b == c)
            .ok_or_else(|| format!("invalid character '{ch}' in join code"))?;
        if n == 0 && d > 7 {
            return Err("invalid join code".into());
        }
        v = (v << 5) | d as u128;
        n += 1;
    }
    if n != 26 {
        return Err(format!("join code must have 26 characters (got {n})"));
    }
    Ok(v.to_be_bytes())
}

pub fn mac(key: &[u8], label: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("any key length");
    m.update(label);
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

pub fn verify(key: &[u8], label: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("any key length");
    m.update(label);
    for p in parts {
        m.update(p);
    }
    m.verify_slice(tag).is_ok()
}

/// One direction of an encrypted channel with a strictly increasing counter nonce,
/// which also provides replay and reordering protection.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
    dir: u8,
}

impl Sealer {
    pub fn new(key: [u8; 32], dir: u8) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(&key)),
            counter: 0,
            dir,
        }
    }

    fn nonce(&self) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[0] = self.dir;
        n[4..].copy_from_slice(&self.counter.to_be_bytes());
        n
    }

    pub fn seal(&mut self, msg: &[u8]) -> Vec<u8> {
        let n = self.nonce();
        self.counter += 1;
        self.cipher
            .encrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg,
                    aad: &[self.dir],
                },
            )
            .expect("encryption cannot fail")
    }

    pub fn open(&mut self, ct: &[u8]) -> Result<Vec<u8>, String> {
        let n = self.nonce();
        let pt = self
            .cipher
            .decrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg: ct,
                    aad: &[self.dir],
                },
            )
            .map_err(|_| "message authentication failed".to_string())?;
        self.counter += 1;
        Ok(pt)
    }
}

pub fn session_keys(secret: &[u8], cn: &[u8], sn: &[u8]) -> ([u8; 32], [u8; 32]) {
    (
        mac(secret, b"DuetSync-key-c2s", &[cn, sn]),
        mac(secret, b"DuetSync-key-s2c", &[cn, sn]),
    )
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) || s.len() > 256 {
        return Err("bad hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "bad hex".to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_roundtrip() {
        for _ in 0..50 {
            let s = random_bytes::<16>();
            let c = encode_code(&s);
            assert_eq!(c.len(), 26 + 5);
            assert_eq!(decode_code(&c).unwrap(), s);
            assert_eq!(decode_code(&c.to_lowercase().replace('-', " ")).unwrap(), s);
        }
        assert!(decode_code("ABC").is_err());
    }

    #[test]
    fn aead_rejects_tamper_and_replay() {
        let k = random_bytes::<32>();
        let mut a = Sealer::new(k, 1);
        let mut b = Sealer::new(k, 1);
        let c1 = a.seal(b"hello");
        let c2 = a.seal(b"world");
        assert_eq!(b.open(&c1).unwrap(), b"hello");
        assert!(b.open(&c1).is_err(), "replay must fail");
        let mut bad = c2.clone();
        bad[0] ^= 1;
        assert!(b.open(&bad).is_err());
        assert_eq!(b.open(&c2).unwrap(), b"world");
    }
}
