//! A short, redacted report for bug reports ("Copy diagnostics" in Settings).
//!
//! It exists so that a problem like "unrecognised format on a Mac" can be diagnosed from one paste
//! instead of a round trip. It carries what the app itself knows (versions, the status of each
//! provider, which settings are on, the lines it could not read, its own recent log lines) and
//! **never** usage numbers, conversations, account details, project names or file paths. The
//! report is built here, as pure text; it only goes to the clipboard when the user asks, and is
//! never sent anywhere.

/// Reports longer than this are cut (a pasted wall of text helps nobody).
const MAX_REPORT_BYTES: usize = 8 * 1024;
const MAX_LINE_CHARS: usize = 240;
const MAX_UNRECOGNISED: usize = 3;
const MAX_LOG_LINES: usize = 15;

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    /// The connection status in the app's own words ("Connected", "Offline", ...).
    pub status: String,
    /// "Updated 3 min ago" or "Never updated".
    pub updated: String,
    /// How many limit windows were read at the last poll.
    pub windows_read: usize,
    /// First line of `<cli> --version`, or `None` when it could not be run.
    pub cli_version: Option<String>,
    /// Limit lines the parser could not read (a percentage, a reset time and a time zone).
    pub unrecognized: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SettingsInfo {
    pub refresh_minutes: u64,
    pub launch_at_login: bool,
    pub widget_visible: bool,
    pub widget_compact: bool,
    pub widget_always_on_top: bool,
    pub activity_enabled: bool,
    /// Only how many, never the names.
    pub wsl_distro_count: usize,
    pub update_check_daily: bool,
    pub update_include_beta: bool,
    /// Only whether a custom path is set, never the path.
    pub custom_claude_path: bool,
    pub custom_codex_path: bool,
}

#[derive(Debug, Clone)]
pub struct Input {
    pub app_version: String,
    pub os: String,
    pub created_utc: String,
    pub providers: Vec<ProviderInfo>,
    pub settings: SettingsInfo,
    /// "last check 2026-11-27T10:00:00Z, ok" or the error text.
    pub update_status: String,
    /// The app's own `diagnostics.log`, newest last.
    pub log_tail: String,
    /// The user's home folder, to be replaced by `~` wherever it appears.
    pub home_dir: Option<String>,
}

/// Remove what must not leave the computer: the home folder, user names inside paths, e-mail
/// addresses and control characters.
pub fn scrub(text: &str, home_dir: Option<&str>) -> String {
    let mut out: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect();
    if let Some(home) = home_dir.filter(|h| h.len() > 2) {
        // The same folder written with either kind of slash.
        let back = home.replace('/', "\\");
        let fwd = home.replace('\\', "/");
        for variant in [home.to_owned(), back, fwd] {
            out = out.replace(&variant, "~");
        }
    }
    out = scrub_profile_names(&out);
    scrub_emails(&out)
}

/// `C:\Users\<name>\...`, `/Users/<name>/...` and `/home/<name>/...` lose the name, even when the
/// home folder is not known (a log line may mention another account's folder).
fn scrub_profile_names(text: &str) -> String {
    let mut out = text.to_owned();
    // (marker, is the name allowed to contain spaces?) Windows names often do; Unix short names do not.
    for (marker, spaces_ok) in [("\\Users\\", true), ("/Users/", false), ("/home/", false)] {
        let mut from = 0;
        while let Some(rel) = out[from..].find(marker) {
            let start = from + rel + marker.len();
            let end = out[start..]
                .find(|c: char| {
                    if spaces_ok {
                        matches!(c, '\\' | '/' | '\n' | '"' | '\'')
                    } else {
                        matches!(c, '/' | '\\' | '\n' | '"' | '\'') || c.is_whitespace()
                    }
                })
                .map_or(out.len(), |e| start + e);
            let name = &out[start..end];
            let keep = name.is_empty()
                || name == "<user>"
                || ["Public", "Default", "Shared"].contains(&name);
            if keep {
                from = start;
            } else {
                out.replace_range(start..end, "<user>");
                from = start + "<user>".len();
            }
        }
    }
    out
}

fn scrub_emails(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let local = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
    let domain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' {
            let mut s = out.chars().count();
            let mut start = s;
            let produced: Vec<char> = out.chars().collect();
            while start > 0 && local(produced[start - 1]) {
                start -= 1;
            }
            let mut end = i + 1;
            while end < chars.len() && domain(chars[end]) {
                end += 1;
            }
            let dom: String = chars[i + 1..end].iter().collect();
            if start < s && dom.contains('.') && !dom.starts_with('.') && !dom.ends_with('.') {
                s = start;
                out = produced[..s].iter().collect();
                out.push_str("<email>");
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn cap(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        line.to_owned()
    } else {
        line.chars().take(MAX_LINE_CHARS).collect::<String>() + "…"
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

pub fn build_report(input: &Input) -> String {
    let home = input.home_dir.as_deref();
    let clean = |t: &str| cap(&scrub(t, home));
    let mut lines: Vec<String> = vec![
        "Agent Usage Monitor diagnostics".into(),
        "(no usage numbers, conversations, accounts or file paths are included)".into(),
        format!("Version: {}", clean(&input.app_version)),
        format!("System: {}", clean(&input.os)),
        format!("Created: {}", clean(&input.created_utc)),
        String::new(),
        "Providers".into(),
    ];
    for p in &input.providers {
        if !p.enabled {
            lines.push(format!("- {}: disabled", clean(&p.name)));
            continue;
        }
        let version = p
            .cli_version
            .as_deref()
            .map_or_else(|| "could not be run".to_owned(), clean);
        lines.push(clean(&format!(
            "- {}: enabled; status: {}; {}; limit windows read: {}; version: {}",
            p.name, p.status, p.updated, p.windows_read, version
        )));
        if !p.unrecognized.is_empty() {
            lines.push(format!(
                "  {} line(s) could not be read; the first {}:",
                p.unrecognized.len(),
                p.unrecognized.len().min(MAX_UNRECOGNISED)
            ));
            for l in p.unrecognized.iter().take(MAX_UNRECOGNISED) {
                lines.push(format!("    \"{}\"", clean(l)));
            }
        }
    }
    let s = &input.settings;
    lines.push(String::new());
    lines.push("Settings".into());
    lines.push(format!("- Refresh every {} min", s.refresh_minutes));
    lines.push(format!(
        "- widget: visible {}, compact {}, always on top {}; launch at login: {}",
        yes_no(s.widget_visible),
        yes_no(s.widget_compact),
        yes_no(s.widget_always_on_top),
        yes_no(s.launch_at_login)
    ));
    lines.push(format!(
        "- session activity: {}; WSL distributions: {}",
        if s.activity_enabled { "on" } else { "off" },
        s.wsl_distro_count
    ));
    lines.push(format!(
        "- update check daily: {}; include beta: {}",
        yes_no(s.update_check_daily),
        yes_no(s.update_include_beta)
    ));
    lines.push(format!(
        "- custom claude path: {}; custom codex path: {}",
        yes_no(s.custom_claude_path),
        yes_no(s.custom_codex_path)
    ));
    lines.push(String::new());
    lines.push(format!("Update check: {}", clean(&input.update_status)));
    lines.push(String::new());
    lines.push("Recent log".into());
    let log: Vec<&str> = input
        .log_tail
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if log.is_empty() {
        lines.push("(empty)".into());
    } else {
        let skip = log.len().saturating_sub(MAX_LOG_LINES);
        lines.extend(log[skip..].iter().map(|l| clean(l)));
    }
    let mut report = lines.join("\n");
    if report.len() > MAX_REPORT_BYTES {
        // Cut at a character boundary; the newest (last) lines matter most, so trim the front of
        // the log section only if it ever gets this long.
        let mut cut = report.len() - MAX_REPORT_BYTES;
        while !report.is_char_boundary(cut) {
            cut += 1;
        }
        report = format!("(earlier lines cut)\n{}", &report[cut..]);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Input {
        Input {
            app_version: "0.11.4".into(),
            os: "macOS aarch64".into(),
            created_utc: "2026-11-27T12:00:00Z".into(),
            providers: vec![
                ProviderInfo {
                    name: "Claude Code".into(),
                    enabled: true,
                    status: "Connected".into(),
                    updated: "Updated 1 min ago".into(),
                    windows_read: 2,
                    cli_version: Some("2.1.330 (Claude Code)".into()),
                    unrecognized: vec![],
                },
                ProviderInfo {
                    name: "Codex CLI".into(),
                    enabled: false,
                    status: "Unknown".into(),
                    updated: "Never updated".into(),
                    windows_read: 0,
                    cli_version: None,
                    unrecognized: vec![],
                },
            ],
            settings: SettingsInfo {
                refresh_minutes: 5,
                launch_at_login: false,
                widget_visible: true,
                widget_compact: false,
                widget_always_on_top: true,
                activity_enabled: true,
                wsl_distro_count: 0,
                update_check_daily: false,
                update_include_beta: true,
                custom_claude_path: false,
                custom_codex_path: false,
            },
            update_status: "never checked".into(),
            log_tail: "2026-11-27T10:00:00Z display layout changed".into(),
            home_dir: Some("/Users/jane".into()),
        }
    }

    #[test]
    fn report_has_the_facts_needed_to_diagnose() {
        let r = build_report(&sample());
        for needle in [
            "0.11.4",
            "macOS aarch64",
            "2026-11-27T12:00:00Z",
            "Claude Code",
            "enabled",
            "Connected",
            "Updated 1 min ago",
            "windows read: 2",
            "2.1.330 (Claude Code)",
            "Codex CLI",
            "disabled",
            "Refresh every 5 min",
            "display layout changed",
            "never checked",
        ] {
            assert!(r.contains(needle), "missing {needle:?} in:\n{r}");
        }
    }

    #[test]
    fn unreadable_limit_lines_are_included_for_diagnosis_capped_and_counted() {
        let mut i = sample();
        i.providers[0].unrecognized = (1..=5)
            .map(|n| format!("Current session: {n}% used · resets Nov 26, 4pm (PST)"))
            .collect();
        let r = build_report(&i);
        assert!(r.contains("Current session: 1% used · resets Nov 26, 4pm (PST)"));
        assert!(r.contains("Current session: 3% used"));
        assert!(
            !r.contains("Current session: 4% used"),
            "only the first three"
        );
        assert!(r.contains("5 line(s) could not be read"));
    }

    #[test]
    fn nothing_private_leaves_in_the_report() {
        let mut i = sample();
        i.log_tail = "update failed for jane@example.com at /Users/jane/Library/x\nC:\\Users\\Jane Doe\\AppData\\y".into();
        i.providers[0].cli_version = Some("2.1.330\u{7} (Claude Code)".into());
        let r = build_report(&i);
        assert!(!r.contains("jane@example.com"), "{r}");
        assert!(!r.contains("/Users/jane"), "{r}");
        assert!(!r.to_lowercase().contains("jane doe"), "{r}");
        assert!(!r.contains('\u{7}'));
        assert!(r.contains("<email>") && r.contains("~/Library/x"));
        // Settings carry only yes/no and counts, never paths or names.
        i.settings.custom_claude_path = true;
        i.settings.wsl_distro_count = 2;
        let r = build_report(&i);
        assert!(r.contains("custom claude path: yes") && r.contains("WSL distributions: 2"));
    }

    #[test]
    fn scrub_handles_home_user_paths_emails_and_control_characters() {
        let home = Some("C:\\Users\\Jane Doe");
        assert_eq!(
            scrub("see C:\\Users\\Jane Doe\\x\\y.txt", home),
            "see ~\\x\\y.txt"
        );
        assert_eq!(scrub("see C:/Users/Jane Doe/x", home), "see ~/x");
        // Without a known home folder, user names inside well-known profile paths still go.
        assert_eq!(
            scrub("/Users/bob/Library/z", None),
            "/Users/<user>/Library/z"
        );
        assert_eq!(scrub("/home/bob/.claude", None), "/home/<user>/.claude");
        assert_eq!(
            scrub("C:\\Users\\bob\\AppData", None),
            "C:\\Users\\<user>\\AppData"
        );
        assert_eq!(
            scrub("mail a.b+c@sub.example.org now", None),
            "mail <email> now"
        );
        assert_eq!(
            scrub("2 @ 3 and foo@bar", None),
            "2 @ 3 and foo@bar",
            "not an address"
        );
        assert_eq!(scrub("a\u{1b}[31mb\u{7}", None), "a[31mb");
    }

    #[test]
    fn long_lines_and_long_logs_are_cut() {
        let mut i = sample();
        i.log_tail = (0..500)
            .map(|n| format!("line {n} {}", "x".repeat(400)))
            .collect::<Vec<_>>()
            .join("\n");
        let r = build_report(&i);
        assert!(r.len() <= MAX_REPORT_BYTES + 64, "{} bytes", r.len());
        assert!(r.contains("line 499"), "the newest log lines are kept");
        assert!(!r.contains("line 0 "), "the oldest are dropped");
        assert!(r.lines().all(|l| l.chars().count() <= MAX_LINE_CHARS + 20));
    }

    #[test]
    fn missing_pieces_are_reported_plainly() {
        let mut i = sample();
        i.providers[0].cli_version = None;
        i.providers[0].enabled = true;
        i.log_tail.clear();
        let r = build_report(&i);
        assert!(r.contains("version: could not be run"), "{r}");
        assert!(r.contains("Recent log") && r.contains("(empty)"));
    }
}
