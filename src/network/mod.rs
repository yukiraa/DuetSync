//! Host/guest session networking (LAN / private VPN such as Tailscale).

pub mod clocksync;
pub mod crypto;
pub mod protocol;
pub mod session;

pub use protocol::{Msg, DEFAULT_PORT};
pub use session::{Event, Guest, GuestConfig, Host, HostConfig};

/// Parse an invite of the form `host:port#JOIN-CODE` (either part may be pasted alone).
pub fn parse_invite(s: &str) -> (Option<String>, Option<String>) {
    let s = s.trim();
    match s.split_once('#') {
        Some((a, c)) => (
            Some(a.trim().to_string()).filter(|x| !x.is_empty()),
            Some(c.trim().to_string()).filter(|x| !x.is_empty()),
        ),
        None if crypto::decode_code(s).is_ok() => (None, Some(s.to_string())),
        None => (Some(s.to_string()).filter(|x| !x.is_empty()), None),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn invite_parsing() {
        let (a, c) = super::parse_invite("100.64.0.2:47800#ABCDE-FGHJK");
        assert_eq!(a.as_deref(), Some("100.64.0.2:47800"));
        assert_eq!(c.as_deref(), Some("ABCDE-FGHJK"));
        let (a, c) = super::parse_invite("192.168.1.5");
        assert_eq!(a.as_deref(), Some("192.168.1.5"));
        assert!(c.is_none());
    }
}
