//! The settings window's form, as plain data. Converts to and from [`Settings`] and
//! owns all validation, so the UI layer is just field copying.

use aum_core::sanitize_label;
use aum_providers::claude::activity::MAX_WSL_DISTROS;
use aum_store::{ActivitySettings, NotifyConfig, Settings, Theme};

/// Dropdown presets for the refresh interval, in minutes. The dropdown's last entry is "Custom".
pub const PRESET_MINUTES: [i32; 4] = [1, 5, 15, 30];
/// Dropdown index meaning "a value that is not one of the presets".
pub const CUSTOM_INDEX: i32 = PRESET_MINUTES.len() as i32;
/// Longest refresh interval offered: 24 hours.
pub const MAX_MINUTES: i32 = 1440;

/// Dropdown index for a number of minutes: the matching preset, or [`CUSTOM_INDEX`].
pub fn preset_index(minutes: i32) -> i32 {
    PRESET_MINUTES
        .iter()
        .position(|m| *m == minutes)
        .map_or(CUSTOM_INDEX, |i| i as i32)
}

/// Short text for an interval: "7 min", "1h 30m", or "90 s" for a value that is not whole minutes.
pub fn interval_label(secs: u64) -> String {
    let secs = secs.clamp(60, 86_400);
    if secs % 60 != 0 {
        return format!("{secs} s");
    }
    match (secs / 3600, (secs % 3600) / 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Whole minutes shown for a stored interval (rounded, and kept inside the allowed range).
pub fn minutes_of(secs: u64) -> i32 {
    (((secs.clamp(60, 86_400) + 30) / 60) as i32).clamp(1, MAX_MINUTES)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub claude_enabled: bool,
    pub cli_path: String,
    /// Refresh interval in whole minutes, 1 to 1440. The dropdown and the number box both edit this.
    pub interval_minutes: i32,
    pub launch_at_login: bool,
    /// Check GitHub for a newer version about once a day (off until the user says yes).
    pub update_check: bool,
    pub update_beta: bool,
    pub widget_visible: bool,
    pub on_top: bool,
    pub compact: bool,
    pub theme_index: i32,
    pub opacity: i32,
    pub notify_enabled: bool,
    pub warn: i32,
    pub critical: i32,
    pub exhausted: bool,
    pub reset: bool,
    pub errors: bool,
    pub codex_enabled: bool,
    pub codex_cli_path: String,
    pub codex_notify_enabled: bool,
    pub activity_enabled: bool,
    pub activity_alert_needs_input: bool,
    pub activity_alert_finished: bool,
    pub activity_alert_min_secs: i32,
    pub activity_finished_minutes: i32,
    /// Comma-separated WSL distribution names (Windows only).
    pub wsl_distros: String,
    pub wsl_claude_path: String,
}

/// "Ubuntu, Debian" to `["Ubuntu", "Debian"]`: split on commas, semicolons and newlines, strip control
/// characters, drop blanks and case-insensitive duplicates, and cap the count (each distro adds a
/// process start to every activity poll).
pub fn parse_distros(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split([',', ';', '\n']) {
        let name = sanitize_label(part, 64);
        if !name.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            out.push(name);
        }
    }
    out.truncate(MAX_WSL_DISTROS);
    out
}

impl Form {
    /// `launch_at_login` is passed separately because the registry, not the settings
    /// file, is the source of truth for it.
    pub fn from_settings(s: &Settings, launch_at_login: bool) -> Self {
        let (warn, critical) = s.claude_notify.levels();
        Self {
            claude_enabled: s.claude_enabled,
            cli_path: s.claude_cli_path.clone().unwrap_or_default(),
            interval_minutes: minutes_of(s.poll_interval_secs),
            launch_at_login,
            update_check: s.update.check_daily,
            update_beta: s.update.include_beta,
            widget_visible: s.widget_visible,
            on_top: s.widget_always_on_top,
            compact: s.widget_compact,
            theme_index: i32::from(s.widget_theme == Theme::Light),
            opacity: i32::from(s.opacity_percent()),
            notify_enabled: s.claude_notify.enabled,
            warn: i32::from(warn),
            critical: i32::from(critical),
            exhausted: s.claude_notify.exhausted,
            reset: s.claude_notify.reset,
            errors: s.claude_notify.errors,
            codex_enabled: s.codex_enabled,
            codex_cli_path: s.codex_cli_path.clone().unwrap_or_default(),
            codex_notify_enabled: s.codex_notify.enabled,
            activity_enabled: s.activity.enabled,
            activity_alert_needs_input: s.activity.alert_needs_input,
            activity_alert_finished: s.activity.alert_finished,
            activity_alert_min_secs: s.activity.alert_min_secs.min(3600) as i32,
            activity_finished_minutes: s.activity.finished_minutes.clamp(1, 120) as i32,
            wsl_distros: s.activity.wsl_distros.join(", "),
            wsl_claude_path: s.activity.wsl_claude_path.clone().unwrap_or_default(),
        }
    }

    /// Validate and write into `s`. Invalid input is corrected, never stored:
    /// the interval is clamped to 1 to 1440 minutes, opacity is clamped, the critical level is
    /// kept above the warn level, and a blank CLI path means "find it automatically".
    pub fn apply_to(&self, s: &mut Settings) {
        s.claude_enabled = self.claude_enabled;
        let path = self.cli_path.trim();
        s.claude_cli_path = (!path.is_empty()).then(|| path.to_owned());
        // Keep a hand-edited value (say 90 s) exactly as it is unless the user actually changed the
        // minutes, so touching another control never silently rounds it.
        let minutes = self.interval_minutes.clamp(1, MAX_MINUTES);
        if minutes != minutes_of(s.poll_interval_secs) {
            s.poll_interval_secs = u64::from(minutes.unsigned_abs()) * 60;
        }
        s.launch_at_login = self.launch_at_login;
        // Touching the switch is an answer to the one-time question too.
        if self.update_check != s.update.check_daily {
            s.update.prompted = true;
        }
        s.update.check_daily = self.update_check;
        s.update.include_beta = self.update_beta;
        s.widget_visible = self.widget_visible;
        s.widget_always_on_top = self.on_top;
        s.widget_compact = self.compact;
        s.widget_theme = if self.theme_index == 1 {
            Theme::Light
        } else {
            Theme::Dark
        };
        s.widget_opacity_percent = self.opacity.clamp(30, 100) as u8;
        let probe = NotifyConfig {
            warn_percent: self.warn.clamp(0, 255) as u8,
            critical_percent: self.critical.clamp(0, 255) as u8,
            ..NotifyConfig::default()
        };
        let (warn, critical) = probe.levels();
        s.claude_notify = NotifyConfig {
            enabled: self.notify_enabled,
            warn_percent: warn,
            critical_percent: critical,
            exhausted: self.exhausted,
            reset: self.reset,
            errors: self.errors,
        };
        s.codex_enabled = self.codex_enabled;
        let codex_path = self.codex_cli_path.trim();
        s.codex_cli_path = (!codex_path.is_empty()).then(|| codex_path.to_owned());
        let wsl_path = self.wsl_claude_path.trim();
        s.activity = ActivitySettings {
            enabled: self.activity_enabled,
            finished_minutes: self.activity_finished_minutes.clamp(1, 120) as u32,
            alert_needs_input: self.activity_alert_needs_input,
            alert_finished: self.activity_alert_finished,
            alert_min_secs: self.activity_alert_min_secs.clamp(0, 3600) as u32,
            wsl_distros: parse_distros(&self.wsl_distros),
            wsl_claude_path: (!wsl_path.is_empty()).then(|| wsl_path.to_owned()),
        };
        // One set of alert levels serves every provider; each provider has its own on/off switch.
        s.codex_notify = NotifyConfig {
            enabled: self.codex_notify_enabled,
            ..s.claude_notify.clone()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_unchanged() {
        let s = Settings::default();
        let mut out = s.clone();
        Form::from_settings(&s, false).apply_to(&mut out);
        assert_eq!(out, s);
    }

    #[test]
    fn customised_settings_round_trip() {
        let s = Settings {
            claude_enabled: false,
            claude_cli_path: Some("C:\\tools\\claude.exe".into()),
            poll_interval_secs: 900,
            widget_visible: false,
            widget_compact: true,
            widget_opacity_percent: 60,
            widget_theme: Theme::Light,
            claude_notify: NotifyConfig {
                warn_percent: 60,
                critical_percent: 85,
                errors: false,
                ..NotifyConfig::default()
            },
            ..Settings::default()
        };
        let f = Form::from_settings(&s, true);
        assert_eq!(
            (
                f.interval_minutes,
                f.theme_index,
                f.opacity,
                f.warn,
                f.critical
            ),
            (15, 1, 60, 60, 85)
        );
        let mut out = Settings::default();
        f.apply_to(&mut out);
        assert_eq!(out.claude_cli_path, s.claude_cli_path);
        assert_eq!(out.claude_notify, s.claude_notify);
        assert_eq!(
            (
                out.poll_interval_secs,
                out.widget_theme,
                out.widget_opacity_percent
            ),
            (900, Theme::Light, 60)
        );
        assert!(out.launch_at_login);
    }

    #[test]
    fn blank_cli_path_means_automatic() {
        let mut f = Form::from_settings(&Settings::default(), false);
        f.cli_path = "   ".into();
        let mut s = Settings {
            claude_cli_path: Some("x".into()),
            ..Settings::default()
        };
        f.apply_to(&mut s);
        assert_eq!(s.claude_cli_path, None);
        f.cli_path = "  C:\\a b\\claude.exe  ".into();
        f.apply_to(&mut s);
        assert_eq!(s.claude_cli_path.as_deref(), Some("C:\\a b\\claude.exe"));
    }

    #[test]
    fn invalid_values_are_corrected_not_stored() {
        let mut f = Form::from_settings(&Settings::default(), false);
        f.interval_minutes = 99_999;
        f.opacity = 0;
        f.warn = 95;
        f.critical = 10;
        let mut s = Settings::default();
        f.apply_to(&mut s);
        assert_eq!(s.poll_interval_secs, 86_400, "capped at 24 hours");
        assert_eq!(s.widget_opacity_percent, 30);
        let (w, c) = s.claude_notify.levels();
        assert!(w < c, "{w} < {c}");
        assert!(s.claude_notify.warn_percent < s.claude_notify.critical_percent);

        f.interval_minutes = -5;
        f.opacity = 500;
        f.apply_to(&mut s);
        assert_eq!(s.poll_interval_secs, 60);
        assert_eq!(s.widget_opacity_percent, 100);
    }

    #[test]
    fn presets_and_custom_values_are_told_apart() {
        for (m, i) in [
            (1, 0),
            (5, 1),
            (15, 2),
            (30, 3),
            (2, CUSTOM_INDEX),
            (7, CUSTOM_INDEX),
            (60, CUSTOM_INDEX),
            (1440, CUSTOM_INDEX),
        ] {
            assert_eq!(preset_index(m), i, "{m} minutes");
        }
    }

    #[test]
    fn stored_seconds_show_as_rounded_minutes_inside_the_allowed_range() {
        for (secs, m) in [
            (10, 1),
            (59, 1),
            (60, 1),
            (89, 1),
            (90, 2),
            (400, 7),
            (420, 7),
            (900, 15),
            (86_400, 1440),
            (999_999, 1440),
        ] {
            assert_eq!(minutes_of(secs), m, "{secs} s");
        }
    }

    #[test]
    fn interval_labels() {
        for (secs, label) in [
            (60, "1 min"),
            (420, "7 min"),
            (3540, "59 min"),
            (3600, "1h"),
            (5400, "1h 30m"),
            (86_400, "24h"),
            (90, "90 s"),
            (5, "1 min"),
        ] {
            assert_eq!(interval_label(secs), label, "{secs}");
        }
    }

    #[test]
    fn a_custom_number_of_minutes_is_stored_as_seconds() {
        let mut f = Form::from_settings(&Settings::default(), false);
        let mut s = Settings::default();
        for (minutes, secs) in [
            (7, 420),
            (1, 60),
            (45, 2700),
            (1440, 86_400),
            (0, 60),
            (-9, 60),
            (5000, 86_400),
        ] {
            f.interval_minutes = minutes;
            f.apply_to(&mut s);
            assert_eq!(s.poll_interval_secs, secs, "{minutes} min");
        }
        // And it comes back out as the same number of minutes.
        f.interval_minutes = 7;
        f.apply_to(&mut s);
        assert_eq!(Form::from_settings(&s, false).interval_minutes, 7);
    }

    #[test]
    fn an_untouched_form_keeps_a_hand_edited_interval_exactly() {
        let mut s = Settings {
            poll_interval_secs: 90,
            ..Settings::default()
        };
        let f = Form::from_settings(&s, false);
        assert_eq!(f.interval_minutes, 2);
        f.apply_to(&mut s);
        assert_eq!(
            s.poll_interval_secs, 90,
            "changing another control must not round 90 s to 120 s"
        );
        // But an actual edit takes effect.
        let mut edited = f.clone();
        edited.interval_minutes = 3;
        edited.apply_to(&mut s);
        assert_eq!(s.poll_interval_secs, 180);
    }

    #[test]
    fn codex_settings_round_trip_and_share_alert_levels() {
        let s = Settings {
            codex_enabled: true,
            codex_cli_path: Some("C:\\npm\\codex.cmd".into()),
            claude_notify: NotifyConfig {
                warn_percent: 60,
                critical_percent: 80,
                ..NotifyConfig::default()
            },
            codex_notify: NotifyConfig {
                enabled: false,
                warn_percent: 60,
                critical_percent: 80,
                ..NotifyConfig::default()
            },
            ..Settings::default()
        };
        let f = Form::from_settings(&s, false);
        assert!(f.codex_enabled && !f.codex_notify_enabled);
        let mut out = Settings::default();
        f.apply_to(&mut out);
        assert!(out.codex_enabled);
        assert_eq!(out.codex_cli_path, s.codex_cli_path);
        assert_eq!(
            out.codex_notify, s.codex_notify,
            "codex alerts: own switch, shared levels"
        );

        let mut blank = f.clone();
        blank.codex_cli_path = "  ".into();
        blank.apply_to(&mut out);
        assert_eq!(out.codex_cli_path, None);
    }

    #[test]
    fn activity_defaults_round_trip() {
        let s = Settings::default();
        let f = Form::from_settings(&s, false);
        assert!(f.activity_enabled && !f.activity_alert_needs_input && !f.activity_alert_finished);
        assert_eq!(
            (f.activity_alert_min_secs, f.activity_finished_minutes),
            (60, 10)
        );
        let mut out = Settings::default();
        f.apply_to(&mut out);
        assert_eq!(out, s);
    }

    #[test]
    fn activity_settings_round_trip_with_wsl() {
        let s = Settings {
            activity: ActivitySettings {
                enabled: false,
                finished_minutes: 30,
                alert_needs_input: true,
                alert_finished: true,
                alert_min_secs: 120,
                wsl_distros: vec!["Ubuntu".into(), "Debian".into()],
                wsl_claude_path: Some("/home/me/.local/bin/claude".into()),
            },
            ..Settings::default()
        };
        let f = Form::from_settings(&s, false);
        assert_eq!(f.wsl_distros, "Ubuntu, Debian");
        let mut out = Settings::default();
        f.apply_to(&mut out);
        assert_eq!(out.activity, s.activity);
    }

    #[test]
    fn distro_lists_are_cleaned() {
        assert_eq!(
            parse_distros(" Ubuntu ,debian;  ,UBUNTU\nKali"),
            vec!["Ubuntu", "debian", "Kali"]
        );
        assert_eq!(
            parse_distros("My Distro"),
            vec!["My Distro"],
            "names with spaces are kept"
        );
        assert!(parse_distros("  ,; ").is_empty());
        assert_eq!(parse_distros("a,b,c,d,e,f").len(), MAX_WSL_DISTROS);
        assert_eq!(parse_distros("bad\u{7}name"), vec!["badname"]);
        assert_eq!(parse_distros(&"x".repeat(200))[0].len(), 64);
    }

    #[test]
    fn activity_numbers_are_clamped() {
        let mut f = Form::from_settings(&Settings::default(), false);
        f.activity_alert_min_secs = -5;
        f.activity_finished_minutes = 100_000;
        let mut s = Settings::default();
        f.apply_to(&mut s);
        assert_eq!(
            (s.activity.alert_min_secs, s.activity.finished_minutes),
            (0, 120)
        );
        f.activity_alert_min_secs = 999_999;
        f.activity_finished_minutes = -3;
        f.apply_to(&mut s);
        assert_eq!(
            (s.activity.alert_min_secs, s.activity.finished_minutes),
            (3600, 1)
        );
    }

    #[test]
    fn switching_the_update_check_answers_the_one_time_question() {
        let mut s = Settings::default();
        assert!(!s.update.check_daily && !s.update.prompted);
        let mut f = Form::from_settings(&s, false);
        f.update_beta = false;
        f.apply_to(&mut s);
        assert!(
            !s.update.prompted,
            "changing only the beta box is not an answer"
        );
        assert!(!s.update.include_beta);
        f.update_check = true;
        f.apply_to(&mut s);
        assert!(s.update.check_daily && s.update.prompted);
    }
}
