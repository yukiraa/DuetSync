//! STUN (RFC 5389) binding request: learn this socket's public address as seen from the
//! internet. Uses free public STUN servers; they only see the request.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

pub const SERVERS: [&str; 3] = [
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];
const MAGIC: u32 = 0x2112_A442;

pub fn request(txid: &[u8; 12]) -> [u8; 20] {
    let mut m = [0u8; 20];
    m[0..2].copy_from_slice(&1u16.to_be_bytes()); // Binding Request
    m[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    m[8..20].copy_from_slice(txid);
    m
}

/// Parse a Binding Success Response for `txid`.
pub fn parse_response(buf: &[u8], txid: &[u8; 12]) -> Option<SocketAddr> {
    if buf.len() < 20
        || buf[0..2] != [0x01, 0x01]
        || buf[4..8] != MAGIC.to_be_bytes()
        || buf[8..20] != txid[..]
    {
        return None;
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let attrs = buf.get(20..20 + len)?;
    let mut i = 0;
    let mut plain = None;
    while i + 4 <= attrs.len() {
        let t = u16::from_be_bytes([attrs[i], attrs[i + 1]]);
        let l = u16::from_be_bytes([attrs[i + 2], attrs[i + 3]]) as usize;
        let v = attrs.get(i + 4..i + 4 + l)?;
        if (t == 0x0020 || t == 0x0001) && l >= 8 && v[1] == 0x01 {
            let mut port = u16::from_be_bytes([v[2], v[3]]);
            let mut ip = [v[4], v[5], v[6], v[7]];
            if t == 0x0020 {
                port ^= (MAGIC >> 16) as u16;
                let m = MAGIC.to_be_bytes();
                for k in 0..4 {
                    ip[k] ^= m[k];
                }
                return Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port));
            }
            plain = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port));
        }
        i += 4 + ((l + 3) & !3);
    }
    plain
}

/// Ask STUN servers (in order) for `sock`'s public mapping. The socket must not be in use
/// by another reader. Restores a 50 ms read timeout afterwards.
pub fn public_addr(sock: &UdpSocket) -> Result<SocketAddr, String> {
    let mut last = String::from("no STUN server answered");
    for server in SERVERS {
        let Some(addr) = server
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.find(|a| a.is_ipv4()))
        else {
            last = format!("cannot resolve {server} (no internet?)");
            continue;
        };
        let mut txid = [0u8; 12];
        getrandom::fill(&mut txid).map_err(|e| e.to_string())?;
        let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
        for _ in 0..3 {
            let _ = sock.send_to(&request(&txid), addr);
            let until = Instant::now() + Duration::from_millis(700);
            let mut buf = [0u8; 512];
            while Instant::now() < until {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    if from == addr {
                        if let Some(m) = parse_response(&buf[..n], &txid) {
                            let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
                            return Ok(m);
                        }
                    }
                }
            }
        }
        last = format!("{server} did not answer");
    }
    let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xor_mapped_address() {
        let txid = [7u8; 12];
        // Response with XOR-MAPPED-ADDRESS 203.0.113.9:40001
        let ip = [203u8, 0, 113, 9];
        let m = MAGIC.to_be_bytes();
        let port = 40001u16 ^ (MAGIC >> 16) as u16;
        let mut r = vec![0x01, 0x01, 0, 12];
        r.extend_from_slice(&m);
        r.extend_from_slice(&txid);
        r.extend_from_slice(&[0x00, 0x20, 0, 8, 0, 0x01]);
        r.extend_from_slice(&port.to_be_bytes());
        for k in 0..4 {
            r.push(ip[k] ^ m[k]);
        }
        assert_eq!(
            parse_response(&r, &txid),
            Some("203.0.113.9:40001".parse().unwrap())
        );
        assert_eq!(parse_response(&r, &[8u8; 12]), None, "wrong transaction id");
        assert_eq!(request(&txid)[0..2], [0, 1]);
    }
}
