// No console window in release builds on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, target_os = "macos"))]
mod shell;

#[cfg(any(windows, target_os = "macos"))]
fn main() {
    shell::run();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("agent-usage-monitor supports Windows and macOS. Use `aum` for the headless CLI.");
    std::process::exit(1);
}
