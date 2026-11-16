//! Process discovery (Discord detection) via Toolhelp snapshots.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
}

pub fn list_processes() -> Vec<ProcInfo> {
    let mut v = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return v;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            v.push(ProcInfo {
                pid: e.th32ProcessID,
                parent: e.th32ParentProcessID,
                name: String::from_utf16_lossy(&e.szExeFile[..len]),
            });
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    v
}

const DISCORD_NAMES: [&str; 3] = ["discord.exe", "discordptb.exe", "discordcanary.exe"];

/// Root Discord processes (whose parent is not itself a Discord process). Capturing the
/// root with "include process tree" covers the child process that actually renders audio.
pub fn find_discord(procs: &[ProcInfo]) -> Vec<ProcInfo> {
    let is_d = |n: &str| DISCORD_NAMES.iter().any(|d| n.eq_ignore_ascii_case(d));
    let pids: Vec<u32> = procs
        .iter()
        .filter(|p| is_d(&p.name))
        .map(|p| p.pid)
        .collect();
    procs
        .iter()
        .filter(|p| is_d(&p.name) && !pids.contains(&p.parent))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_root_discord() {
        let p = |pid, parent, n: &str| ProcInfo {
            pid,
            parent,
            name: n.into(),
        };
        let procs = vec![
            p(10, 1, "explorer.exe"),
            p(20, 10, "Discord.exe"),
            p(21, 20, "Discord.exe"),
            p(22, 20, "Discord.exe"),
            p(30, 10, "obs64.exe"),
        ];
        let d = find_discord(&procs);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].pid, 20);
        assert!(!list_processes().is_empty());
    }
}
