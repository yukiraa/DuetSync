#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("{} {}", duetsync::APP_NAME, duetsync::APP_VERSION);
        return;
    }
    if args.iter().any(|a| a == "--print-default-config") {
        attach_console();
        let cfg = duetsync::config::Config::default();
        println!("{}", serde_json::to_string_pretty(&cfg).unwrap_or_default());
        return;
    }
    if args.iter().any(|a| a == "--selftest") {
        attach_console();
        let code = duetsync::selftest::run(&args);
        std::process::exit(code);
    }
    if let Err(e) = duetsync::ui::run() {
        eprintln!("DuetSync failed to start: {e}");
        std::process::exit(1);
    }
}

/// Release builds use the GUI subsystem; reattach to the parent console for CLI output.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows::Win32::System::Console::AttachConsole(u32::MAX).ok();
    }
}
