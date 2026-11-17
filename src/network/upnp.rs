//! Minimal UPnP Internet Gateway Device client: asks the home router to forward the
//! session port, so a partner on another network can join without installing anything.
//!
//! Only the session port (TCP) is mapped. The session is authenticated (128-bit join code,
//! mutual HMAC) and encrypted (ChaCha20-Poly1305), and the host caps unauthenticated
//! handshakes. The mapping has a lease and is removed when the session ends.
//! OBS and the video bridge stay on 127.0.0.1 and are never mapped.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

const SSDP_ADDR: &str = "239.255.255.250:1900";
const LEASE_S: u32 = 4 * 3600;
const IO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub struct Mapping {
    pub external_ip: Ipv4Addr,
    pub port: u16,
    pub local_ip: Ipv4Addr,
    control_url: String,
    service_type: String,
}

/// Why automatic setup did not give a reachable public address.
#[derive(Clone, Debug, PartialEq)]
pub enum UpnpError {
    /// No router answered (UPnP switched off, or not supported).
    NoRouter,
    /// The router refused the forwarding request.
    Refused(String),
    /// The router's own "public" address is private: the provider uses shared
    /// addresses (CGNAT), so nothing outside can reach this home directly.
    SharedAddress(Ipv4Addr),
    Other(String),
}

impl std::fmt::Display for UpnpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpnpError::NoRouter => write!(f, "router did not answer (UPnP off)"),
            UpnpError::Refused(r) => write!(f, "router refused: {r}"),
            UpnpError::SharedAddress(ip) => {
                write!(f, "provider shares addresses (router WAN {ip})")
            }
            UpnpError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// True for addresses that are not reachable from the internet.
pub fn is_non_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10 carrier-grade NAT
}

/// Read-only check: is there a UPnP router, and what public address does it report?
/// Changes nothing on the router.
pub fn probe() -> Result<Ipv4Addr, UpnpError> {
    let location = discover().ok_or(UpnpError::NoRouter)?;
    let (control_url, service_type) = find_service(&location)
        .map_err(|e| UpnpError::Other(format!("router description: {e}")))?;
    let ip = soap(&control_url, &service_type, "GetExternalIPAddress", "")
        .ok()
        .and_then(|b| tag(&b, "NewExternalIPAddress"))
        .and_then(|s| s.trim().parse::<Ipv4Addr>().ok())
        .ok_or_else(|| UpnpError::Other("router did not report its public address".into()))?;
    if is_non_public(ip) {
        return Err(UpnpError::SharedAddress(ip));
    }
    Ok(ip)
}

/// Discover the gateway, forward TCP `port` to this PC and return the public address.
/// Blocking (a few seconds at most); call from a worker thread.
pub fn open_port(port: u16) -> Result<Mapping, UpnpError> {
    let location = discover().ok_or(UpnpError::NoRouter)?;
    let (control_url, service_type) = find_service(&location)
        .map_err(|e| UpnpError::Other(format!("router description: {e}")))?;
    let router =
        url_host(&control_url).ok_or_else(|| UpnpError::Other("bad control URL".into()))?;
    let local_ip =
        local_ip_towards(router).ok_or_else(|| UpnpError::Other("no local address".into()))?;

    let external_ip = soap(&control_url, &service_type, "GetExternalIPAddress", "")
        .ok()
        .and_then(|b| tag(&b, "NewExternalIPAddress"))
        .and_then(|s| s.trim().parse::<Ipv4Addr>().ok())
        .ok_or_else(|| UpnpError::Other("router did not report its public address".into()))?;
    if is_non_public(external_ip) {
        return Err(UpnpError::SharedAddress(external_ip));
    }

    let add = |lease: u32| {
        let args = format!(
            "<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort>\
             <NewProtocol>TCP</NewProtocol><NewInternalPort>{port}</NewInternalPort>\
             <NewInternalClient>{local_ip}</NewInternalClient><NewEnabled>1</NewEnabled>\
             <NewPortMappingDescription>DuetSync</NewPortMappingDescription>\
             <NewLeaseDuration>{lease}</NewLeaseDuration>"
        );
        soap(&control_url, &service_type, "AddPortMapping", &args)
    };
    // Some routers only accept permanent leases (UPnP error 725).
    match add(LEASE_S) {
        Ok(_) => {}
        Err(e) if e.contains("725") || e.contains("OnlyPermanentLeases") => {
            add(0).map_err(UpnpError::Refused)?;
        }
        Err(e) => return Err(UpnpError::Refused(e)),
    }
    Ok(Mapping {
        external_ip,
        port,
        local_ip,
        control_url,
        service_type,
    })
}

impl Mapping {
    /// Remove the forwarding rule (best effort; the lease expires anyway).
    pub fn close(&self) {
        let args = format!(
            "<NewRemoteHost></NewRemoteHost><NewExternalPort>{}</NewExternalPort>\
             <NewProtocol>TCP</NewProtocol>",
            self.port
        );
        let _ = soap(
            &self.control_url,
            &self.service_type,
            "DeletePortMapping",
            &args,
        );
    }
}

fn discover() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(400)))
        .ok()?;
    let _ = sock.set_multicast_ttl_v4(2);
    for st in [
        "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
        "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
        "urn:schemas-upnp-org:service:WANIPConnection:1",
    ] {
        let req = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\n\r\n"
        );
        let _ = sock.send_to(req.as_bytes(), SSDP_ADDR);
    }
    let deadline = Instant::now() + Duration::from_millis(2500);
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        let Ok((n, _)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        if let Some(loc) = header(&text, "location") {
            if loc.starts_with("http://") {
                return Some(loc);
            }
        }
    }
    None
}

fn header(resp: &str, name: &str) -> Option<String> {
    resp.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

/// Text between the first `<name>`/`</name>` (namespace prefixes tolerated).
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = xml.find(&format!("{name}>"))? + name.len() + 1;
    let rest = &xml[open..];
    let close = rest.find("</")?;
    Some(rest[..close].trim().to_string())
}

fn find_service(location: &str) -> Result<(String, String), String> {
    let body = http(location, "GET", None, &[])?;
    let base = tag(&body, "URLBase").unwrap_or_else(|| location.to_string());
    for block in body.split("<service>").skip(1) {
        let Some(st) = tag(block, "serviceType") else {
            continue;
        };
        if st.contains("WANIPConnection") || st.contains("WANPPPConnection") {
            let ctrl = tag(block, "controlURL").ok_or("no controlURL")?;
            return Ok((join_url(&base, &ctrl), st));
        }
    }
    Err("router offers no WAN connection service".into())
}

fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") {
        return path.to_string();
    }
    let origin_end = base[7..].find('/').map(|i| i + 7).unwrap_or(base.len());
    let origin = &base[..origin_end];
    if path.starts_with('/') {
        format!("{origin}{path}")
    } else {
        format!("{origin}/{path}")
    }
}

fn url_host(url: &str) -> Option<SocketAddr> {
    let rest = url.strip_prefix("http://")?;
    let hostport = rest.split('/').next()?;
    let hp = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    hp.to_socket_addrs().ok()?.next()
}

fn local_ip_towards(router: SocketAddr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(router).ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v) => Some(v),
        IpAddr::V6(_) => None,
    }
}

fn soap(control_url: &str, service: &str, action: &str, args: &str) -> Result<String, String> {
    let body = format!(
        "<?xml version=\"1.0\"?>\r\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{service}\">{args}</u:{action}></s:Body></s:Envelope>"
    );
    let soap_action = format!("\"{service}#{action}\"");
    let resp = http(
        control_url,
        "POST",
        Some(&body),
        &[
            ("Content-Type", "text/xml; charset=\"utf-8\""),
            ("SOAPAction", &soap_action),
        ],
    )?;
    if resp.contains("Fault>") || resp.contains("<errorCode>") {
        let code = tag(&resp, "errorCode").unwrap_or_default();
        let desc = tag(&resp, "errorDescription").unwrap_or_default();
        return Err(format!("{action} error {code} {desc}"));
    }
    Ok(resp)
}

/// Tiny HTTP/1.1 client (router LAN only). Returns the body.
fn http(
    url: &str,
    method: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Result<String, String> {
    let addr = url_host(url).ok_or("bad URL")?;
    let rest = url.strip_prefix("http://").ok_or("bad URL")?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut s = TcpStream::connect_timeout(&addr, IO_TIMEOUT).map_err(|e| e.to_string())?;
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() > 256 * 1024 {
                    return Err("response too large".into());
                }
            }
            Err(_) if !out.is_empty() => break, // routers often keep the socket open
            Err(e) => return Err(e.to_string()),
        }
    }
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status_ok = head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200");
    if !status_ok && !body.contains("Fault>") {
        return Err(head.lines().next().unwrap_or("HTTP error").to_string());
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_helpers() {
        let ssdp = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
        assert_eq!(
            header(ssdp, "location").as_deref(),
            Some("http://192.168.1.1:5000/rootDesc.xml")
        );
        let xml = "<root><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/ctl/IPConn</controlURL></service></root>";
        assert_eq!(tag(xml, "controlURL").as_deref(), Some("/ctl/IPConn"));
        assert_eq!(
            join_url("http://192.168.1.1:5000/rootDesc.xml", "/ctl/IPConn"),
            "http://192.168.1.1:5000/ctl/IPConn"
        );
        assert_eq!(join_url("http://10.0.0.1/", "ctl"), "http://10.0.0.1/ctl");
        let resp = "<s:Envelope><s:Body><u:GetExternalIPAddressResponse><NewExternalIPAddress>203.0.113.7</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>";
        assert_eq!(
            tag(resp, "NewExternalIPAddress").as_deref(),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn shared_and_private_addresses_detected() {
        assert!(is_non_public("100.72.3.4".parse().unwrap()));
        assert!(is_non_public("10.1.2.3".parse().unwrap()));
        assert!(is_non_public("192.168.18.1".parse().unwrap()));
        assert!(!is_non_public("203.0.113.7".parse().unwrap()));
        assert!(!is_non_public("100.128.0.1".parse().unwrap()));
    }
}
