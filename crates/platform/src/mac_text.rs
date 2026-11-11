//! Text-producing pieces of the macOS integration. They contain no macOS calls, so they are
//! compiled and unit-tested on every OS.

/// Label (and file name stem) of the per-user LaunchAgent.
pub const LAUNCH_AGENT_LABEL: &str = "io.github.agent-usage-monitor";

/// Escape text for use inside an AppleScript string literal: backslash and double quote are the
/// only characters with meaning there, and newlines are flattened to spaces.
pub fn applescript_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The `osascript` program text that shows a notification.
pub fn notification_script(title: &str, body: &str) -> String {
    format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title)
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A LaunchAgent property list that starts `exe` at login. `ProcessType` Interactive and no
/// KeepAlive: if the user quits the app it stays quit until the next login.
pub fn launch_agent_plist(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        exe = xml_escape(exe)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_strings_cannot_be_broken_out_of() {
        assert_eq!(
            applescript_escape(r#"say "hi" \ now"#),
            r#"say \"hi\" \\ now"#
        );
        assert_eq!(applescript_escape("a\nb\rc"), "a b c");
        assert_eq!(applescript_escape("tab\there\u{7}"), "tabhere");
        // An injection attempt stays inside the literal.
        let s = notification_script(r#"x" & (do shell script "id") & ""#, "body");
        assert!(s.contains(r#"\" & (do shell script \"id\") & \""#), "{s}");
        assert_eq!(s.matches("with title").count(), 1);
    }

    #[test]
    fn notification_script_shape() {
        assert_eq!(
            notification_script("Claude Code: Session at 80%", "Resets in 2h 00m."),
            r#"display notification "Resets in 2h 00m." with title "Claude Code: Session at 80%""#
        );
        assert_eq!(
            applescript_escape("Weekly (Fable) · ready"),
            "Weekly (Fable) · ready"
        );
    }

    #[test]
    fn launch_agent_plist_is_well_formed_and_escaped() {
        let p = launch_agent_plist(
            "/Applications/Agent Usage Monitor.app/Contents/MacOS/agent-usage-monitor",
        );
        assert!(p.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(p.contains(LAUNCH_AGENT_LABEL));
        assert!(p.contains("<string>/Applications/Agent Usage Monitor.app/Contents/MacOS/agent-usage-monitor</string>"));
        assert!(
            !p.contains("KeepAlive"),
            "quitting must stick until next login"
        );
        let evil = launch_agent_plist("/tmp/a&b</string><string>evil");
        assert!(evil.contains("a&amp;b&lt;/string&gt;&lt;string&gt;evil"));
        assert_eq!(
            evil.matches("<string>").count(),
            3,
            "label, exe and ProcessType only"
        );
    }
}
