//! Strict parser for the limit lines of `claude -p "/usage"`.
//!
//! Recognised shape (Claude Code 2.1.x):
//! ```text
//! Current session: 82% used · resets Nov 10, 4:10pm (Europe/London)
//! Current week (all models): 28% used · resets Nov 15, 6am (Europe/London)
//! Current week (Fable): 0% used · resets Nov 15, 6am (Europe/London)
//! ```
//! Anything else is reported back as unrecognised. A missing limit is never
//! turned into 0%.

use std::sync::LazyLock;

use aum_core::{Provenance, ProviderError, UsageWindow, WindowKind};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use serde::Deserialize;

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^Current (session|week)(?: \(([^)]+)\))?: (\d{1,3})% used · resets (.+?) \(([A-Za-z0-9_+\-/]+)\)\s*$",
    )
    .expect("valid regex")
});

static RESET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Z][a-z]{2}) (\d{1,2}), (\d{1,2})(?::(\d{2}))?(am|pm)$").expect("valid regex")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub windows: Vec<UsageWindow>,
    pub unrecognized: Vec<String>,
}

/// Parse limit lines. Returns an error only when no limit line at all could
/// be read; partially readable output is returned with `unrecognized` set.
pub fn parse_usage(text: &str, now: DateTime<Utc>) -> Result<Parsed, ProviderError> {
    let mut windows = Vec::new();
    let mut unrecognized = Vec::new();

    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Current "))
    {
        match parse_line(line, now) {
            Some(w) => windows.push(w),
            None => unrecognized.push(line.to_owned()),
        }
    }

    if windows.is_empty() {
        return Err(ProviderError::Parse(if unrecognized.is_empty() {
            "no usage limit lines found".to_owned()
        } else {
            // Quote the first one: it holds only a percentage, a reset time and a time zone, and
            // seeing it is the fastest way to learn what changed (another OS, locale or version).
            format!(
                "{} limit line(s) in an unrecognised format, first: \"{}\"",
                unrecognized.len(),
                aum_core::sanitize_label(&unrecognized[0], 120)
            )
        }));
    }
    Ok(Parsed {
        windows,
        unrecognized,
    })
}

fn parse_line(line: &str, now: DateTime<Utc>) -> Option<UsageWindow> {
    let c = LINE.captures(line)?;
    let kind = match (&c[1], c.get(2).map(|m| m.as_str())) {
        ("session", None) => WindowKind::Session,
        ("week", None | Some("all models")) => WindowKind::Weekly,
        ("week", Some(model)) => WindowKind::WeeklyModel(model.to_owned()),
        _ => return None,
    };
    let percent: u32 = c[3].parse().ok()?;
    if percent > 100 {
        return None;
    }
    let tz: Tz = c[5].parse().ok()?;
    let resets_at = parse_reset(&c[4], tz, now)?;
    Some(UsageWindow::new(
        kind,
        percent,
        resets_at,
        Provenance::Official,
    ))
}

/// "Nov 10, 4:10pm" / "Nov 15, 6am" in `tz`, with the year inferred.
///
/// Reset times are always in the near future, so the year is the current
/// local year unless that would put the date more than a day in the past
/// (e.g. "Jan 2" seen on Dec 30), in which case it is the next year.
pub fn parse_reset(text: &str, tz: Tz, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let c = RESET.captures(text)?;
    let month = month_number(&c[1])?;
    let day: u32 = c[2].parse().ok()?;
    let hour12: u32 = c[3].parse().ok()?;
    let minute: u32 = c.get(4).map_or(Some(0), |m| m.as_str().parse().ok())?;
    if !(1..=12).contains(&hour12) || minute > 59 {
        return None;
    }
    let hour = match (&c[5], hour12) {
        ("am", 12) => 0,
        ("am", h) => h,
        (_, 12) => 12,
        (_, h) => h + 12,
    };

    let local_year = now.with_timezone(&tz).year();
    let build = |year: i32| -> Option<DateTime<Utc>> {
        let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, 0)?;
        // Ambiguous (DST fall-back): take the earlier; skipped (spring-forward): reject.
        let local = tz.from_local_datetime(&naive).earliest()?;
        Some(local.with_timezone(&Utc))
    };

    let this_year = build(local_year)?;
    if this_year < now - Duration::days(1) {
        build(local_year + 1)
    } else {
        Some(this_year)
    }
}

fn month_number(abbr: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS.iter().position(|m| *m == abbr).map(|i| i as u32 + 1)
}

/// The subset of `claude auth status` we read. Other fields (email, org) are
/// deliberately not deserialised, so they can never be logged.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    pub auth_method: Option<String>,
    pub subscription_type: Option<String>,
    pub login_expires_at: Option<DateTime<Utc>>,
}

pub fn parse_auth_status(json: &str) -> Result<AuthStatus, ProviderError> {
    serde_json::from_str(json)
        .map_err(|_| ProviderError::Parse("unreadable auth status".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: &str =
        include_str!("../../../../tests/fixtures/claude/usage/text_normal_28pct.txt");
    const HIGH: &str = include_str!("../../../../tests/fixtures/claude/usage/text_high_82pct.txt");
    const LOGGED_OUT: &str =
        include_str!("../../../../tests/fixtures/claude/usage/text_logged_out.txt");
    const AUTH: &str =
        include_str!("../../../../tests/fixtures/claude/usage/auth_status_team.json");

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn parses_real_normal_output() {
        let now = utc(2026, 11, 10, 17, 0);
        let p = parse_usage(NORMAL, now).unwrap();
        assert!(p.unrecognized.is_empty());
        assert_eq!(p.windows.len(), 3);
        let s = &p.windows[0];
        assert_eq!((s.kind.clone(), s.used_percent), (WindowKind::Session, 28));
        assert_eq!(s.resets_at, utc(2026, 11, 10, 21, 10)); // London = GMT in November
        assert_eq!(p.windows[1].kind, WindowKind::Weekly);
        assert_eq!(p.windows[1].resets_at, utc(2026, 11, 15, 6, 0));
        assert_eq!(p.windows[2].kind, WindowKind::WeeklyModel("Fable".into()));
        assert_eq!(p.windows[2].used_percent, 0);
        assert!(
            p.windows
                .iter()
                .all(|w| w.provenance == Provenance::Official)
        );
    }

    #[test]
    fn parses_high_usage_output() {
        let p = parse_usage(HIGH, utc(2026, 11, 10, 15, 0)).unwrap();
        assert_eq!(p.windows[0].used_percent, 82);
        assert_eq!(p.windows[0].resets_at, utc(2026, 11, 10, 16, 10));
    }

    #[test]
    fn logged_out_output_is_an_error_not_zero_percent() {
        let e = parse_usage(LOGGED_OUT, utc(2026, 11, 10, 17, 0)).unwrap_err();
        assert!(matches!(e, ProviderError::Parse(_)));
    }

    #[test]
    fn summer_time_offsets_are_applied() {
        let tz: Tz = "Europe/London".parse().unwrap();
        let now = utc(2026, 7, 1, 0, 0);
        assert_eq!(
            parse_reset("Jul 4, 6am", tz, now),
            Some(utc(2026, 7, 4, 5, 0))
        );
    }

    #[test]
    fn other_timezones() {
        let tz: Tz = "America/New_York".parse().unwrap();
        let now = utc(2026, 11, 10, 0, 0);
        assert_eq!(
            parse_reset("Nov 10, 9:10pm", tz, now),
            Some(utc(2026, 11, 11, 2, 10))
        );
    }

    #[test]
    fn year_rolls_over() {
        let tz: Tz = "Europe/London".parse().unwrap();
        let now = utc(2026, 12, 30, 12, 0);
        assert_eq!(
            parse_reset("Jan 2, 6am", tz, now),
            Some(utc(2027, 1, 2, 6, 0))
        );
        // Same-year future date is not bumped.
        assert_eq!(
            parse_reset("Dec 31, 6am", tz, now),
            Some(utc(2026, 12, 31, 6, 0))
        );
    }

    #[test]
    fn twelve_hour_clock_edges() {
        let tz: Tz = "UTC".parse().unwrap();
        let now = utc(2026, 11, 10, 0, 0);
        assert_eq!(
            parse_reset("Nov 10, 12am", tz, now),
            Some(utc(2026, 11, 10, 0, 0))
        );
        assert_eq!(
            parse_reset("Nov 10, 12:30pm", tz, now),
            Some(utc(2026, 11, 10, 12, 30))
        );
        assert_eq!(parse_reset("Nov 10, 13pm", tz, now), None);
        assert_eq!(parse_reset("Nov 31, 1pm", tz, now), None);
    }

    #[test]
    fn dst_gap_time_is_rejected_not_guessed() {
        let tz: Tz = "Europe/London".parse().unwrap();
        // 01:30 does not exist on 2027-03-28 in London.
        assert_eq!(
            parse_reset("Mar 28, 1:30am", tz, utc(2027, 3, 27, 0, 0)),
            None
        );
    }

    #[test]
    fn multiple_and_missing_model_lines() {
        let text = "Current session: 5% used · resets Nov 10, 9pm (UTC)\n\
                    Current week (all models): 6% used · resets Nov 15, 6am (UTC)\n\
                    Current week (Opus): 7% used · resets Nov 15, 6am (UTC)\n\
                    Current week (Sonnet): 8% used · resets Nov 15, 6am (UTC)\n";
        let p = parse_usage(text, utc(2026, 11, 10, 12, 0)).unwrap();
        assert_eq!(p.windows.len(), 4);
        let no_model = "Current session: 5% used · resets Nov 10, 9pm (UTC)\n";
        assert_eq!(
            parse_usage(no_model, utc(2026, 11, 10, 12, 0))
                .unwrap()
                .windows
                .len(),
            1
        );
    }

    #[test]
    fn exhausted_window() {
        let text = "Current session: 100% used · resets Nov 10, 9pm (UTC)\n";
        let p = parse_usage(text, utc(2026, 11, 10, 12, 0)).unwrap();
        assert!(p.windows[0].is_exhausted());
    }

    #[test]
    fn changed_wording_is_unrecognised_not_guessed() {
        let text = "Current session: 28 percent consumed, resets tomorrow\n";
        let e = parse_usage(text, utc(2026, 11, 10, 12, 0)).unwrap_err();
        assert!(matches!(e, ProviderError::Parse(_)));
        // One good line plus one odd line keeps the good one and flags the odd one.
        let mixed = "Current session: 5% used · resets Nov 10, 9pm (UTC)\n\
                     Current week (all models): lots used\n";
        let p = parse_usage(mixed, utc(2026, 11, 10, 12, 0)).unwrap();
        assert_eq!(p.windows.len(), 1);
        assert_eq!(p.unrecognized.len(), 1);
    }

    #[test]
    fn over_100_and_bad_timezone_are_unrecognised() {
        let text = "Current session: 140% used · resets Nov 10, 9pm (UTC)\n\
                    Current week (all models): 1% used · resets Nov 15, 6am (Mars/Base)\n";
        assert!(parse_usage(text, utc(2026, 11, 10, 12, 0)).is_err());
    }

    #[test]
    fn empty_and_garbage_input() {
        assert!(parse_usage("", utc(2026, 11, 10, 12, 0)).is_err());
        assert!(parse_usage("\u{0}\u{1}binary \n\n", utc(2026, 11, 10, 12, 0)).is_err());
    }

    #[test]
    fn auth_status_reads_only_needed_fields() {
        let a = parse_auth_status(AUTH).unwrap();
        assert!(a.logged_in);
        assert_eq!(a.auth_method.as_deref(), Some("claude.ai"));
        assert_eq!(a.subscription_type.as_deref(), Some("team"));
        assert!(a.login_expires_at.is_some());
        assert!(parse_auth_status("not json").is_err());
    }

    #[test]
    fn unrecognised_line_is_quoted_in_the_error_so_it_can_be_diagnosed() {
        // A limit line holds only a percentage, a reset time and a time zone: no personal data, so the
        // first unreadable one is shown (control characters removed, length capped).
        let text =
            "Current session: 35% used · resets Nov 26, 4pm (PST)\nCurrent week: 9% used\u{7}";
        let err = parse_usage(text, Utc::now()).unwrap_err().to_string();
        assert!(
            err.contains("2 limit line(s) in an unrecognised format"),
            "{err}"
        );
        assert!(
            err.contains("Current session: 35% used · resets Nov 26, 4pm (PST)"),
            "{err}"
        );
        assert!(!err.contains('\u{7}'));
        let long = format!("Current session: {}", "x".repeat(500));
        let err = parse_usage(&long, Utc::now()).unwrap_err().to_string();
        assert!(err.len() < 250, "capped: {}", err.len());
    }
}
