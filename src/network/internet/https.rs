//! Tiny HTTPS client on top of Windows' own WinHTTP (system TLS, system proxy settings).
//! Used only for the signalling relay; no TLS library is bundled.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

struct H(*mut c_void);
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

const MAX_BODY: usize = 1 << 20;

unsafe fn open_request(
    host: &str,
    method: &str,
    path: &str,
    timeout_ms: i32,
) -> Result<(H, H, H), String> {
    let ses = H(WinHttpOpen(
        w!("DuetSync"),
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        PCWSTR::null(),
        PCWSTR::null(),
        0,
    ));
    if ses.0.is_null() {
        return Err("WinHttpOpen failed".into());
    }
    let _ = WinHttpSetTimeouts(ses.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms);
    let con = H(WinHttpConnect(
        ses.0,
        &HSTRING::from(host),
        INTERNET_DEFAULT_HTTPS_PORT,
        0,
    ));
    if con.0.is_null() {
        return Err(format!("cannot connect to {host}"));
    }
    let req = H(WinHttpOpenRequest(
        con.0,
        &HSTRING::from(method),
        &HSTRING::from(path),
        PCWSTR::null(),
        PCWSTR::null(),
        std::ptr::null(),
        WINHTTP_FLAG_SECURE,
    ));
    if req.0.is_null() {
        return Err("WinHttpOpenRequest failed".into());
    }
    Ok((ses, con, req))
}

unsafe fn send(req: &H, body: Option<&[u8]>) -> Result<u32, String> {
    let len = body.map(|b| b.len()).unwrap_or(0) as u32;
    WinHttpSendRequest(
        req.0,
        None,
        body.map(|b| b.as_ptr() as *const c_void),
        len,
        len,
        0,
    )
    .map_err(|e| format!("send: {e}"))?;
    WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("receive: {e}"))?;
    let (mut code, mut size, mut idx) = (0u32, 4u32, 0u32);
    WinHttpQueryHeaders(
        req.0,
        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
        PCWSTR::null(),
        Some(&mut code as *mut u32 as *mut c_void),
        &mut size,
        &mut idx,
    )
    .map_err(|e| format!("status: {e}"))?;
    Ok(code)
}

/// One request; returns (status, body).
pub fn request(
    method: &str,
    host: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout_ms: i32,
) -> Result<(u32, Vec<u8>), String> {
    unsafe {
        let (_s, _c, req) = open_request(host, method, path, timeout_ms)?;
        let code = send(&req, body)?;
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let mut n = 0u32;
            WinHttpReadData(
                req.0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut n,
            )
            .map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
            if out.len() > MAX_BODY {
                return Err("response too large".into());
            }
        }
        Ok((code, out))
    }
}

/// Streaming GET: calls `on_line` for each complete line until the server closes, an
/// error occurs, `stop` is set (checked between reads) or `on_line` returns false.
pub fn stream_lines(
    host: &str,
    path: &str,
    timeout_ms: i32,
    stop: &AtomicBool,
    mut on_line: impl FnMut(&str) -> bool,
) -> Result<(), String> {
    unsafe {
        let (_s, _c, req) = open_request(host, "GET", path, timeout_ms)?;
        let code = send(&req, None)?;
        if code != 200 {
            return Err(format!("HTTP {code}"));
        }
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        while !stop.load(Ordering::Acquire) {
            // Wait for *some* data, then read exactly that much: a plain blocking read would
            // wait to fill the whole buffer, which a slow event stream never does.
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail).map_err(|e| format!("wait: {e}"))?;
            if avail == 0 {
                return Ok(()); // stream ended
            }
            let want = (avail as usize).min(buf.len()) as u32;
            let mut n = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, want, &mut n)
                .map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Ok(());
            }
            pending.extend_from_slice(&buf[..n as usize]);
            while let Some(i) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=i).collect();
                let text = String::from_utf8_lossy(&line);
                if !on_line(text.trim()) {
                    return Ok(());
                }
            }
            if pending.len() > MAX_BODY {
                return Err("line too long".into());
            }
        }
        Ok(())
    }
}
