//! Native helpers that winit/Slint do not offer: window opacity, rounded corners, monitor layout,
//! launch at login and notifications. One implementation per OS behind the same functions; on
//! platforms without one every function is a harmless no-op, so the workspace builds and tests
//! anywhere.

pub mod mac_text;

use aum_store::Rect;

/// Set whole-window opacity (30..=100 %). Returns false if the call failed.
/// `handle` is the native window handle (Windows: HWND, macOS: NSView pointer) as an integer.
pub fn set_opacity(handle: isize, percent: u8) -> bool {
    imp::set_opacity(handle, percent.clamp(30, 100))
}

/// Ask the OS for rounded corners (Windows 11 only; elsewhere returns false).
pub fn round_corners(handle: isize) -> bool {
    imp::round_corners(handle)
}

/// Work areas (excluding the taskbar) of all monitors in physical pixels, primary monitor first.
/// Empty if this platform module cannot tell (the shell then asks winit).
pub fn monitors() -> Vec<Rect> {
    imp::monitors()
}

/// `AppId` of installer/agent-usage-monitor.iss (a test keeps the two in step). Inno Setup names the
/// uninstall entry `{AppId}_is1`, which is how an installed copy is told from a portable one.
pub const INSTALLER_APP_ID: &str = "{441C2F18-5B58-4B7C-A95D-68D18A524F2C}";

/// The folder the setup program installed this app into, from its uninstall entry (Windows only;
/// `None` for the portable zip, other systems, or when it cannot be read).
pub fn installed_location() -> Option<String> {
    imp::installed_location()
}

/// Start `program` with `args`, detached so it keeps running after this process exits (used to hand
/// over to the installer during an update).
pub fn spawn_detached(program: &std::path::Path, args: &[&str]) -> Result<(), String> {
    imp::spawn_detached(program, args)
}

/// Is this app registered to start at login (per-user, no admin rights needed)?
pub fn autostart_enabled() -> bool {
    imp::autostart_enabled()
}

/// Register or remove the per-user login entry for `exe`.
pub fn set_autostart(enabled: bool, exe: &std::path::Path) -> Result<(), String> {
    imp::set_autostart(enabled, exe)
}

/// The bytes `clip.exe` needs for correct text: UTF-16 little endian with a byte-order mark.
pub fn utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Put text on the system clipboard through the OS's own command (`clip.exe` on Windows,
/// `pbcopy` on macOS), with no shell involved. Errors are returned, never panicked on.
#[cfg(any(windows, target_os = "macos"))]
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(windows)]
    let (program, bytes) = {
        let system32 = std::env::var_os("SystemRoot")
            .map(|r| {
                std::path::PathBuf::from(r)
                    .join("System32")
                    .join("clip.exe")
            })
            .filter(|p| p.is_file());
        (
            system32.unwrap_or_else(|| std::path::PathBuf::from("clip.exe")),
            utf16le_with_bom(text),
        )
    };
    #[cfg(target_os = "macos")]
    let (program, bytes) = (
        std::path::PathBuf::from("/usr/bin/pbcopy"),
        text.as_bytes().to_vec(),
    );

    let mut cmd = Command::new(&program);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // pbcopy assumes a legacy encoding unless told the text is UTF-8.
    #[cfg(target_os = "macos")]
    cmd.env("LC_ALL", "en_US.UTF-8");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(&bytes)
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} failed", program.display()))
    }
}

/// No clipboard command is known on this platform.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn copy_to_clipboard(_text: &str) -> Result<(), String> {
    Err("copying to the clipboard is only supported on Windows and macOS".to_owned())
}

/// Show a native toast notification. Errors are returned, never panicked on.
pub fn toast(title: &str, body: &str) -> Result<(), String> {
    imp::toast(title, body)
}

/// Map 30..=100 % to a layered-window alpha byte.
pub fn alpha_byte(percent: u8) -> u8 {
    ((percent.clamp(30, 100) as u32 * 255 + 50) / 100) as u8
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use super::{Rect, alpha_byte};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW,
        WS_EX_LAYERED,
    };

    pub fn set_opacity(hwnd: isize, percent: u8) -> bool {
        let hwnd = hwnd as HWND;
        // SAFETY: plain Win32 calls on a window handle owned by this process; a
        // stale or invalid handle makes the calls fail, which we report.
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if ex & WS_EX_LAYERED as isize == 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
            }
            SetLayeredWindowAttributes(hwnd, 0, alpha_byte(percent), LWA_ALPHA) != 0
        }
    }

    pub fn round_corners(hwnd: isize) -> bool {
        let pref: i32 = DWMWCP_ROUND;
        // SAFETY: `pref` outlives the call and the size matches its type.
        unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &pref as *const i32 as *const _,
                size_of::<i32>() as u32,
            ) == 0
        }
    }

    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
        // SAFETY: `data` is the `&mut Vec` passed by `monitors()` below and is only
        // used for the duration of EnumDisplayMonitors.
        let out = unsafe { &mut *(data as *mut Vec<(bool, Rect)>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(m, &mut info) } != 0 {
            let r = info.rcWork;
            out.push((
                info.dwFlags & 1 != 0,
                Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
            ));
        }
        TRUE
    }

    pub fn installed_location() -> Option<String> {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(format!(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}_is1",
                super::INSTALLER_APP_ID
            ))
            .and_then(|k| k.get_value::<String, _>("InstallLocation"))
            .ok()
    }

    pub fn spawn_detached(program: &std::path::Path, args: &[&str]) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        std::process::Command::new(program)
            .args(args)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = "AgentUsageMonitor";

    pub fn autostart_enabled() -> bool {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
            .is_ok()
    }

    pub fn set_autostart(enabled: bool, exe: &std::path::Path) -> Result<(), String> {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
            .map_err(|e| e.to_string())?;
        if enabled {
            // Quoted, so a path with spaces starts correctly.
            key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display()))
                .map_err(|e| e.to_string())
        } else {
            match key.delete_value(RUN_VALUE) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
    }

    /// App id the installer gives its Start Menu shortcut, so toasts carry our name and icon.
    const AUMID: &str = "AgentUsageMonitor.App";
    const SHORTCUT: &str = "Agent Usage Monitor.lnk";

    /// Toasts are only reliably shown for an app id Windows knows about. The installer
    /// registers ours through its Start Menu shortcut; without it (portable use) the
    /// PowerShell id is used and the toast is attributed to "Windows PowerShell".
    fn app_id() -> &'static str {
        use tauri_winrt_notification::Toast;
        let shortcut = std::env::var_os("APPDATA").map(|a| {
            std::path::PathBuf::from(a)
                .join(r"Microsoft\Windows\Start Menu\Programs")
                .join(SHORTCUT)
        });
        match shortcut {
            Some(p) if p.is_file() => AUMID,
            _ => Toast::POWERSHELL_APP_ID,
        }
    }

    pub fn toast(title: &str, body: &str) -> Result<(), String> {
        use tauri_winrt_notification::{Duration, Toast};
        Toast::new(app_id())
            .title(title)
            .text1(body)
            .duration(Duration::Short)
            .show()
            .map_err(|e| e.to_string())
    }

    pub fn monitors() -> Vec<Rect> {
        let mut found: Vec<(bool, Rect)> = Vec::new();
        // SAFETY: the callback only touches `found`, which outlives the call.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect),
                &mut found as *mut _ as LPARAM,
            );
        }
        found.sort_by_key(|(primary, _)| !*primary); // primary first (stable)
        found.into_iter().map(|(_, r)| r).collect()
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::Rect;
    pub fn set_opacity(_: isize, _: u8) -> bool {
        false
    }
    pub fn round_corners(_: isize) -> bool {
        false
    }
    pub fn monitors() -> Vec<Rect> {
        Vec::new()
    }
    pub fn installed_location() -> Option<String> {
        None
    }
    pub fn spawn_detached(program: &std::path::Path, args: &[&str]) -> Result<(), String> {
        std::process::Command::new(program)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub fn autostart_enabled() -> bool {
        false
    }
    pub fn set_autostart(_: bool, _: &std::path::Path) -> Result<(), String> {
        Err("launch at login is only supported on Windows".to_owned())
    }
    pub fn toast(_: &str, _: &str) -> Result<(), String> {
        Err("notifications are only supported on Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_for_windows_is_utf16le_with_a_byte_order_mark() {
        // clip.exe reads UTF-16 when it starts with a BOM; plain UTF-8 would garble non-ASCII text.
        assert_eq!(utf16le_with_bom(""), vec![0xFF, 0xFE]);
        assert_eq!(
            utf16le_with_bom("A·"),
            vec![0xFF, 0xFE, 0x41, 0x00, 0xB7, 0x00]
        );
        // A character outside the basic plane becomes a surrogate pair.
        assert_eq!(
            utf16le_with_bom("\u{1F600}"),
            vec![0xFF, 0xFE, 0x3D, 0xD8, 0x00, 0xDE]
        );
        // Newlines pass through untouched (clip.exe keeps them).
        assert_eq!(utf16le_with_bom("\n"), vec![0xFF, 0xFE, 0x0A, 0x00]);
    }

    /// Needs a desktop session (it replaces the clipboard): `cargo test -p aum-platform -- --ignored`.
    /// Windows and macOS only; read the clipboard back to compare.
    #[test]
    #[ignore = "replaces the clipboard"]
    fn copying_puts_the_text_on_the_clipboard() {
        copy_to_clipboard("diagnostics test · \u{1F600}\nsecond line").unwrap();
    }

    #[test]
    fn alpha_mapping() {
        assert_eq!(alpha_byte(100), 255);
        assert_eq!(alpha_byte(30), 77);
        assert_eq!(alpha_byte(0), 77, "floor applies");
        assert!(alpha_byte(50) > alpha_byte(40));
    }

    #[test]
    fn installer_app_id_matches_the_inno_script() {
        // The script writes the braces doubled ("{{") because "{" starts a constant in Inno.
        let iss = include_str!("../../../installer/agent-usage-monitor.iss");
        let wanted = format!("AppId={{{}", INSTALLER_APP_ID);
        assert!(
            iss.contains(&wanted),
            "AppId in the installer script is not {INSTALLER_APP_ID}"
        );
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn unsupported_platform_is_a_noop() {
        assert_eq!(installed_location(), None);
        assert!(!set_opacity(0, 80));
        assert!(monitors().is_empty());
        assert!(!autostart_enabled());
        assert!(set_autostart(true, std::path::Path::new("x")).is_err());
        assert!(toast("t", "b").is_err());
    }
}
