//! macOS implementations. Written without access to a Mac: type-checked for the macOS targets in
//! CI and unit-tested for their text output, but not yet run on a real machine.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use aum_store::Rect;

use crate::mac_text::{LAUNCH_AGENT_LABEL, launch_agent_plist, notification_script};

/// `handle` is the `NSView` pointer winit reports for the window. Must run on the main thread.
pub fn set_opacity(handle: isize, percent: u8) -> bool {
    use objc2_app_kit::NSView;
    if handle == 0 {
        return false;
    }
    // SAFETY: the pointer is the NSView of a live window owned by this process (the shell reads
    // it from winit just before calling), and the shell only calls this on the main thread.
    let view: &NSView = unsafe { &*(handle as *const NSView) };
    let Some(window) = view.window() else {
        return false;
    };
    window.setAlphaValue(f64::from(percent.clamp(30, 100)) / 100.0);
    true
}

/// Not implemented on macOS: the borderless window keeps square corners.
pub fn round_corners(_handle: isize) -> bool {
    false
}

/// The shell asks winit for the monitor layout on macOS instead.
pub fn monitors() -> Vec<Rect> {
    Vec::new()
}

fn launch_agent_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCH_AGENT_LABEL}.plist")),
    )
}

pub fn autostart_enabled() -> bool {
    launch_agent_path().is_some_and(|p| p.is_file())
}

pub fn set_autostart(enabled: bool, exe: &Path) -> Result<(), String> {
    let path = launch_agent_path().ok_or("HOME is not set")?;
    if enabled {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, launch_agent_plist(&exe.display().to_string()))
            .map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Uses `osascript`, which needs no signing or entitlements. The notification is attributed to
/// "Script Editor" and macOS asks the user to allow it the first time.
pub fn toast(title: &str, body: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(notification_script(title, body))
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .next()
            .unwrap_or("osascript failed")
            .to_owned())
    }
}
