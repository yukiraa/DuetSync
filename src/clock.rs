//! Monotonic microsecond clock shared with WASAPI timestamps (both are QPC based).

#[cfg(windows)]
fn qpc_freq() -> i64 {
    use std::sync::OnceLock;
    static F: OnceLock<i64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    })
}

/// Monotonic time in microseconds (QueryPerformanceCounter based on Windows).
#[cfg(windows)]
pub fn now_us() -> i64 {
    let mut c = 0i64;
    unsafe {
        let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut c);
    }
    let f = qpc_freq();
    ((c as i128 * 1_000_000) / f as i128) as i64
}

#[cfg(not(windows))]
pub fn now_us() -> i64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_micros() as i64 + 1_000_000
}

/// Convert a WASAPI QPC position (100 ns units) into `now_us` timebase.
pub fn qpc100ns_to_us(v: u64) -> i64 {
    (v / 10) as i64
}

/// Sleep until `target_us` without busy waiting (coarse sleep, short final sleep).
pub fn sleep_until(target_us: i64) {
    loop {
        let d = target_us - now_us();
        if d <= 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_micros(d.min(50_000) as u64));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn monotonic() {
        let a = super::now_us();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = super::now_us();
        assert!(b - a >= 4_000, "{a} {b}");
    }
}
