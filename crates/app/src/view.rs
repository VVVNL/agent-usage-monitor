//! Turns provider states into text for the tray tooltip and menu.
//!
//! Rules: nothing is shown as current if its reset time has passed or the data
//! is stale, and an unavailable value is shown as unavailable.

use std::time::Duration;

use aum_core::{ConnectionStatus, ProviderState, UsageWindow, format_countdown};
use chrono::{DateTime, Utc};

use crate::icon::Level;

/// Windows truncates tray tooltips at 127 characters.
const TOOLTIP_MAX: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    pub tooltip: String,
    /// Short text for the macOS menu bar, next to the icon: the busiest session percentage, or a
    /// dash when there is no current value.
    pub title: String,
    /// Fill of the tray icon, 0..=100, or `None` when there is no usable value.
    pub icon_percent: Option<u8>,
    pub icon_level: Level,
    /// Dim the icon: data is stale or the provider is in error.
    pub icon_stale: bool,
    /// Session activity dot on the icon.
    pub badge: crate::icon::Badge,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    /// Connection line, e.g. "Connected" or "Not logged in".
    pub status: String,
    pub rows: Vec<String>,
    /// "Updated 3 min ago" / "Stale: updated 40 min ago" / "Never updated".
    pub updated: String,
    /// Session activity line ("2 working"), or `None` when activity is switched off.
    pub activity: Option<String>,
}

pub fn level_for(percent: u8) -> Level {
    match percent {
        0..=74 => Level::Ok,
        75..=89 => Level::Warn,
        _ => Level::Critical,
    }
}

pub fn status_text(s: &ConnectionStatus) -> String {
    match s {
        ConnectionStatus::Unknown => "Checking…".into(),
        ConnectionStatus::Connected => "Connected".into(),
        ConnectionStatus::NotLoggedIn => "Not logged in — run `claude` to sign in".into(),
        ConnectionStatus::AuthExpired => "Login expired — run `claude` to sign in again".into(),
        ConnectionStatus::RateLimited => "Rate limited — backing off".into(),
        ConnectionStatus::Offline => "Offline".into(),
        ConnectionStatus::Unsupported(why) => format!("Not available: {why}"),
        ConnectionStatus::Error(e) => format!("Error: {e}"),
    }
}

pub fn ago(d: Duration) -> String {
    let m = d.as_secs() / 60;
    match m {
        0 => "just now".into(),
        1..=59 => format!("{m} min ago"),
        _ => format!("{}h {:02}m ago", m / 60, m % 60),
    }
}

fn window_row(w: &UsageWindow, now: DateTime<Utc>) -> String {
    match w.time_to_reset(now) {
        Some(d) => format!(
            "{:<16} {:>3}%   resets in {}",
            w.kind.label(),
            w.used_percent,
            format_countdown(d)
        ),
        // The window has rolled over since this value was read: do not show it as current.
        None => format!("{:<16}  —    reset passed, refreshing", w.kind.label()),
    }
}

pub fn build(states: &[ProviderState], now: DateTime<Utc>, poll_interval: Duration) -> TrayModel {
    build_with_activity(
        states,
        &std::collections::HashMap::new(),
        now,
        poll_interval,
    )
}

/// `activity` has an entry for every provider whose session activity is switched on.
pub fn build_with_activity(
    states: &[ProviderState],
    activity: &std::collections::HashMap<aum_core::ProviderId, crate::activity::ActivityView>,
    now: DateTime<Utc>,
    poll_interval: Duration,
) -> TrayModel {
    use crate::activity::Level as Act;
    let mut top_activity = Act::None;
    let mut sections = Vec::new();
    let mut tooltip_parts = Vec::new();
    let mut worst: Option<u8> = None;
    let mut session: Option<u8> = None;
    let mut any_stale = false;
    let mut any_problem = false;

    for st in states {
        let stale = st.is_stale(now, poll_interval);
        let ok = st.status == ConnectionStatus::Connected;
        any_stale |= stale;
        let checking = st.status == ConnectionStatus::Unknown;
        any_problem |= !ok && !checking;

        let mut rows = Vec::new();
        let mut short = Vec::new();
        if let Some(snap) = &st.snapshot {
            for w in &snap.windows {
                rows.push(window_row(w, now));
                if !w.is_expired(now) {
                    worst = Some(worst.map_or(w.used_percent, |x| x.max(w.used_percent)));
                    if w.kind == aum_core::WindowKind::Session {
                        // The tray icon follows the busiest provider.
                        session = Some(session.map_or(w.used_percent, |s| s.max(w.used_percent)));
                    }
                    short.push(format!("{} {}%", w.kind.label(), w.used_percent));
                }
            }
            for line in &snap.unrecognized {
                // A format change must be visible, never silently dropped.
                rows.push(format!(
                    "⚠ unrecognised output: {}",
                    line.chars().take(40).collect::<String>()
                ));
            }
        }
        if rows.is_empty() {
            rows.push("No usage data available".into());
        }

        let updated = match &st.snapshot {
            None => "Never updated".to_owned(),
            Some(s) => {
                let age = (now - s.fetched_at).to_std().unwrap_or_default();
                if stale {
                    format!("Stale: updated {}", ago(age))
                } else {
                    format!("Updated {}", ago(age))
                }
            }
        };

        let name = st.provider.display_name();
        let mut line = if short.is_empty() {
            format!("{name}: no data")
        } else {
            format!("{name}: {}", short.join(", "))
        };
        if checking {
            line.push_str(" (checking…)");
        } else if !ok {
            line.push_str(" (!)");
        } else if stale {
            line.push_str(" (stale)");
        }
        let act = activity.get(&st.provider);
        if let Some(v) = act {
            top_activity = top_activity.max(v.level);
            if v.level != Act::None {
                if let Some(t) = v.text() {
                    line.push_str(&format!(" · {t}"));
                }
            }
        }
        tooltip_parts.push(line);

        sections.push(Section {
            title: name.to_owned(),
            status: status_text(&st.status),
            rows,
            updated,
            activity: act.map(|v| v.text().unwrap_or_else(|| "none running".to_owned())),
        });
    }

    let mut tooltip = if tooltip_parts.is_empty() {
        "No providers enabled".to_owned()
    } else {
        tooltip_parts.join("\n")
    };
    if tooltip.chars().count() > TOOLTIP_MAX {
        tooltip = tooltip.chars().take(TOOLTIP_MAX - 1).collect::<String>() + "…";
    }

    let icon_percent = session.or(worst);
    TrayModel {
        tooltip,
        title: icon_percent.map_or_else(|| "–".to_owned(), |p| format!("{p}%")),
        icon_percent,
        icon_level: icon_percent.map_or(Level::Unknown, level_for),
        icon_stale: any_stale || any_problem,
        badge: match top_activity {
            Act::NeedsInput => crate::icon::Badge::NeedsInput,
            Act::Working => crate::icon::Badge::Working,
            _ => crate::icon::Badge::None,
        },
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aum_core::{Provenance, ProviderId, ProviderSnapshot, WindowKind};
    use chrono::{Duration as CD, TimeZone};

    const IV: Duration = Duration::from_secs(300);

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 17, 0, 0).unwrap()
    }

    fn state(
        windows: Vec<UsageWindow>,
        fetched: DateTime<Utc>,
        status: ConnectionStatus,
    ) -> ProviderState {
        ProviderState {
            provider: ProviderId::Claude,
            status,
            snapshot: Some(ProviderSnapshot {
                provider: ProviderId::Claude,
                windows,
                plan: None,
                fetched_at: fetched,
                unrecognized: vec![],
            }),
            last_attempt_at: Some(fetched),
            consecutive_failures: 0,
        }
    }

    fn w(kind: WindowKind, pct: u32, resets_in_min: i64) -> UsageWindow {
        UsageWindow::new(
            kind,
            pct,
            now() + CD::minutes(resets_in_min),
            Provenance::Official,
        )
    }

    #[test]
    fn healthy_state() {
        let st = state(
            vec![
                w(WindowKind::Session, 35, 244),
                w(WindowKind::Weekly, 34, 6000),
            ],
            now(),
            ConnectionStatus::Connected,
        );
        let m = build(&[st], now(), IV);
        assert_eq!(m.icon_percent, Some(35));
        assert_eq!(m.icon_level, Level::Ok);
        assert!(!m.icon_stale);
        assert!(m.tooltip.contains("Session 35%") && m.tooltip.contains("Weekly 34%"));
        assert_eq!(
            m.sections[0].rows[0],
            "Session           35%   resets in 4h 04m"
        );
        assert_eq!(m.sections[0].updated, "Updated just now");
    }

    #[test]
    fn levels() {
        assert_eq!(level_for(74), Level::Ok);
        assert_eq!(level_for(75), Level::Warn);
        assert_eq!(level_for(90), Level::Critical);
        assert_eq!(level_for(100), Level::Critical);
    }

    #[test]
    fn expired_window_is_not_shown_as_current() {
        let st = state(
            vec![w(WindowKind::Session, 98, -5)],
            now() - CD::minutes(1),
            ConnectionStatus::Connected,
        );
        let m = build(&[st], now(), IV);
        assert!(m.sections[0].rows[0].contains("reset passed"));
        assert_eq!(
            m.icon_percent, None,
            "a rolled-over window must not drive the icon"
        );
        assert_eq!(m.icon_level, Level::Unknown);
    }

    #[test]
    fn stale_data_is_flagged() {
        let st = state(
            vec![w(WindowKind::Session, 50, 100)],
            now() - CD::minutes(30),
            ConnectionStatus::Offline,
        );
        let m = build(&[st], now(), IV);
        assert!(m.icon_stale);
        assert!(
            m.sections[0]
                .updated
                .starts_with("Stale: updated 30 min ago")
        );
        assert!(m.tooltip.contains("(!)"));
        assert_eq!(m.sections[0].status, "Offline");
    }

    #[test]
    fn no_data_is_unavailable_not_zero() {
        let st = ProviderState::new(ProviderId::Claude);
        let m = build(&[st], now(), IV);
        assert_eq!(m.icon_percent, None);
        assert_eq!(m.sections[0].rows, vec!["No usage data available"]);
        assert_eq!(m.sections[0].updated, "Never updated");
    }

    #[test]
    fn auth_problems_have_actionable_text() {
        assert!(status_text(&ConnectionStatus::NotLoggedIn).contains("claude"));
        assert!(status_text(&ConnectionStatus::AuthExpired).contains("sign in again"));
    }

    #[test]
    fn tooltip_respects_windows_limit() {
        let many: Vec<_> = (0..10)
            .map(|i| {
                w(
                    WindowKind::WeeklyModel(format!("VeryLongModelName{i}")),
                    10,
                    100,
                )
            })
            .collect();
        let m = build(
            &[state(many, now(), ConnectionStatus::Connected)],
            now(),
            IV,
        );
        assert!(m.tooltip.chars().count() <= 127);
    }

    #[test]
    fn unrecognised_lines_surface_as_warnings() {
        let mut st = state(
            vec![w(WindowKind::Session, 10, 100)],
            now(),
            ConnectionStatus::Connected,
        );
        st.snapshot.as_mut().unwrap().unrecognized = vec!["Current week (x): weird".into()];
        let m = build(&[st], now(), IV);
        assert!(m.sections[0].rows.iter().any(|r| r.starts_with('⚠')));
    }

    #[test]
    fn startup_state_says_checking_not_error() {
        let m = build(&[ProviderState::new(ProviderId::Claude)], now(), IV);
        assert!(m.tooltip.contains("checking"));
        assert!(!m.tooltip.contains("(!)"));
    }

    #[test]
    fn no_providers() {
        assert_eq!(build(&[], now(), IV).tooltip, "No providers enabled");
    }

    #[test]
    fn menu_bar_title_follows_the_busiest_provider() {
        let claude = state(
            vec![w(WindowKind::Session, 35, 244)],
            now(),
            ConnectionStatus::Connected,
        );
        let mut codex = state(
            vec![w(WindowKind::Session, 72, 100)],
            now(),
            ConnectionStatus::Connected,
        );
        codex.provider = ProviderId::Codex;
        let m = build(&[claude, codex], now(), IV);
        assert_eq!(m.title, "72%");
        assert_eq!(m.icon_percent, Some(72));
        assert!(m.tooltip.contains("Claude Code") && m.tooltip.contains("Codex CLI"));
    }

    #[test]
    fn menu_bar_title_is_a_dash_without_a_current_value() {
        let st = state(
            vec![w(WindowKind::Session, 98, -5)],
            now() - CD::minutes(1),
            ConnectionStatus::Connected,
        );
        assert_eq!(build(&[st], now(), IV).title, "–");
        assert_eq!(
            build(&[ProviderState::new(ProviderId::Claude)], now(), IV).title,
            "–"
        );
    }

    // ---- session activity ---------------------------------------------------------------------

    use crate::activity::{ActivityView, Chip, Level as Act};
    use std::collections::HashMap;

    fn act(level: Act, texts: &[&str]) -> ActivityView {
        ActivityView {
            chips: texts
                .iter()
                .map(|t| Chip {
                    level,
                    text: (*t).to_owned(),
                })
                .collect(),
            level,
            busy: texts.len(),
            problems: vec![],
        }
    }

    fn healthy() -> ProviderState {
        state(
            vec![w(WindowKind::Session, 35, 244)],
            now(),
            ConnectionStatus::Connected,
        )
    }

    #[test]
    fn activity_appears_in_the_menu_row_tooltip_and_badge() {
        let views = HashMap::from([(ProviderId::Claude, act(Act::Working, &["2 working"]))]);
        let m = build_with_activity(&[healthy()], &views, now(), IV);
        assert_eq!(m.sections[0].activity.as_deref(), Some("2 working"));
        assert!(
            m.tooltip.contains("Session 35% · 2 working"),
            "{}",
            m.tooltip
        );
        assert_eq!(m.badge, crate::icon::Badge::Working);
    }

    #[test]
    fn needs_input_wins_the_badge() {
        let views = HashMap::from([(
            ProviderId::Claude,
            act(Act::NeedsInput, &["needs input: permission prompt"]),
        )]);
        assert_eq!(
            build_with_activity(&[healthy()], &views, now(), IV).badge,
            crate::icon::Badge::NeedsInput
        );
    }

    #[test]
    fn switched_off_means_no_row_and_enabled_but_empty_says_none_running() {
        let off = build_with_activity(&[healthy()], &HashMap::new(), now(), IV);
        assert_eq!(off.sections[0].activity, None);
        assert_eq!(off.badge, crate::icon::Badge::None);
        let quiet = HashMap::from([(ProviderId::Claude, ActivityView::empty())]);
        let m = build_with_activity(&[healthy()], &quiet, now(), IV);
        assert_eq!(m.sections[0].activity.as_deref(), Some("none running"));
        assert!(
            !m.tooltip.contains("none running"),
            "the tooltip stays short when nothing is happening"
        );
    }

    #[test]
    fn idle_and_unavailable_do_not_light_the_badge() {
        let views = HashMap::from([(ProviderId::Claude, act(Act::Idle, &["2 idle"]))]);
        assert_eq!(
            build_with_activity(&[healthy()], &views, now(), IV).badge,
            crate::icon::Badge::None
        );
        let down = HashMap::from([(
            ProviderId::Claude,
            act(Act::None, &["activity unavailable"]),
        )]);
        let m = build_with_activity(&[healthy()], &down, now(), IV);
        assert_eq!(m.badge, crate::icon::Badge::None);
        assert_eq!(
            m.sections[0].activity.as_deref(),
            Some("activity unavailable")
        );
    }
}
