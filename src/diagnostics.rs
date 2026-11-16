//! Process resource sampling, bounded event log and diagnostics export.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcStats {
    /// Percent of total machine CPU capacity (Task Manager style).
    pub cpu_percent: f64,
    pub working_set_mb: f64,
    pub private_mb: f64,
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn ft(f: FILETIME) -> u64 {
        ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
    }

    /// (kernel+user time in 100 ns units)
    pub fn cpu_time() -> u64 {
        let (mut c, mut e, mut k, mut u) = Default::default();
        unsafe {
            if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_ok() {
                return ft(k) + ft(u);
            }
        }
        0
    }

    pub fn memory() -> (f64, f64) {
        let mut m = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        unsafe {
            if K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                m.cb,
            )
            .as_bool()
            {
                return (
                    m.WorkingSetSize as f64 / 1048576.0,
                    m.PrivateUsage as f64 / 1048576.0,
                );
            }
        }
        (0.0, 0.0)
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn cpu_time() -> u64 {
        0
    }
    pub fn memory() -> (f64, f64) {
        (0.0, 0.0)
    }
}

pub struct Sampler {
    last_cpu: u64,
    last_wall: i64,
    cpus: f64,
    pub last: ProcStats,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            last_cpu: imp::cpu_time(),
            last_wall: crate::clock::now_us(),
            cpus: std::thread::available_parallelism()
                .map(|n| n.get() as f64)
                .unwrap_or(1.0),
            last: ProcStats::default(),
        }
    }
}

impl Sampler {
    /// Sample CPU since the previous call and current memory.
    pub fn sample(&mut self) -> ProcStats {
        let cpu = imp::cpu_time();
        let wall = crate::clock::now_us();
        let dw = (wall - self.last_wall).max(1) as f64 * 10.0; // µs → 100 ns
        let pct = (cpu.saturating_sub(self.last_cpu)) as f64 / dw / self.cpus * 100.0;
        self.last_cpu = cpu;
        self.last_wall = wall;
        let (ws, pv) = imp::memory();
        self.last = ProcStats {
            cpu_percent: pct,
            working_set_mb: ws,
            private_mb: pv,
        };
        self.last
    }
}

/// Bounded in-memory log (never contains secrets: callers must not log codes/passwords).
pub struct Log {
    lines: VecDeque<String>,
    cap: usize,
    t0: i64,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            lines: VecDeque::with_capacity(400),
            cap: 400,
            t0: crate::clock::now_us(),
        }
    }
}

impl Log {
    pub fn push(&mut self, s: impl Into<String>) {
        if self.lines.len() == self.cap {
            self.lines.pop_front();
        }
        let t = (crate::clock::now_us() - self.t0) as f64 / 1e6;
        self.lines.push_back(format!("[{t:8.2}s] {}", s.into()));
    }
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &String> {
        self.lines.iter()
    }
}

pub fn export(path: &std::path::Path, sections: &[(&str, String)]) -> Result<(), String> {
    let mut s = format!(
        "DuetSync {} diagnostics\nOS: {} {}\n\n",
        crate::APP_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    for (h, body) in sections {
        s.push_str(&format!("== {h} ==\n{body}\n\n"));
    }
    std::fs::write(path, s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sampler_reports_memory() {
        let mut s = super::Sampler::default();
        let st = s.sample();
        assert!(st.working_set_mb > 0.1);
        assert!(st.cpu_percent >= 0.0);
        let mut log = super::Log::default();
        for i in 0..1000 {
            log.push(format!("{i}"));
        }
        assert_eq!(log.lines().count(), 400);
    }
}
