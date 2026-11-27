//! View-model for the floating widget. Pure data, no GUI types, fully tested.

use std::time::Duration;

use aum_core::forecast::{Forecast, forecast};
use aum_core::{ConnectionStatus, ProviderState, WindowKind, format_countdown};
use chrono::{DateTime, Utc};

use std::collections::HashMap;

use aum_core::ProviderId;

use crate::activity::{ActivityView, Chip, Level as ActivityLevel};
use crate::icon::Level;
use crate::view::{ago, level_for, status_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarRow {
    pub label: String,
    /// `None` when the value must not be shown as current (window rolled over).
    pub percent: Option<u8>,
    pub level: Level,
    /// "resets in 4h 04m" or "reset passed, refreshing".
    pub detail: String,
    /// The pace estimate ("Pace estimate: runs out in ~1h 44m"), or empty when none applies: too early
    /// in the window, the value is stale or in error, the limit is reached, or the window's length is
    /// unknown. See `aum_core::forecast`.
    pub forecast: String,
    pub forecast_level: Level,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetSection {
    pub title: String,
    /// Connection problem or empty when everything is fine.
    pub problem: String,
    pub bars: Vec<BarRow>,
    /// "Updated 3 min ago" or "Stale: updated 40 min ago".
    pub updated: String,
    pub stale: bool,
    /// One line for compact mode: "Claude Code  35% · 34%".
    pub compact: String,
    /// Session activity chips ("2 working", "finished 4 min ago"); empty when there is nothing to say.
    pub chips: Vec<Chip>,
    /// Highest activity level, for the compact-mode dot.
    pub activity_level: ActivityLevel,
    /// Sessions working or waiting, for the compact-mode count.
    pub activity_busy: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetModel {
    pub sections: Vec<WidgetSection>,
}

pub fn build(states: &[ProviderState], now: DateTime<Utc>, poll_interval: Duration) -> WidgetModel {
    build_with_activity(states, &HashMap::new(), now, poll_interval)
}

/// `activity` has an entry for every provider whose session activity is switched on.
pub fn build_with_activity(
    states: &[ProviderState],
    activity: &HashMap<ProviderId, ActivityView>,
    now: DateTime<Utc>,
    poll_interval: Duration,
) -> WidgetModel {
    WidgetModel {
        sections: states
            .iter()
            .map(|s| {
                let mut sec = section(s, now, poll_interval);
                if let Some(v) = activity.get(&s.provider) {
                    sec.chips = v.chips.clone();
                    sec.activity_level = v.level;
                    sec.activity_busy = v.busy;
                }
                sec
            })
            .collect(),
    }
}

/// Words and colour for a forecast. Red only when the limit would be reached before the reset; amber
/// when the projection ends close to the limit.
fn forecast_text(f: Option<Forecast>) -> (String, Level) {
    match f {
        Some(Forecast::RunsOut { in_ }) => (
            format!("Pace estimate: runs out in ~{}", format_countdown(in_)),
            Level::Critical,
        ),
        Some(Forecast::OnTrack { projected_percent }) => (
            format!("Pace estimate: ~{projected_percent}% by reset"),
            if projected_percent >= 85 {
                Level::Warn
            } else {
                Level::Ok
            },
        ),
        Some(Forecast::TooEarly) | None => (String::new(), Level::Unknown),
    }
}

fn section(st: &ProviderState, now: DateTime<Utc>, interval: Duration) -> WidgetSection {
    let stale = st.is_stale(now, interval);
    // A projection from old numbers would mislead (the percentage no longer matches the clock).
    let can_forecast = !stale && st.status == ConnectionStatus::Connected;
    let mut bars = Vec::new();
    let mut compact_values = Vec::new();

    if let Some(snap) = &st.snapshot {
        for w in &snap.windows {
            let expired = w.is_expired(now);
            let percent = (!expired).then_some(w.used_percent);
            let detail = match w.time_to_reset(now) {
                Some(d) => format!("resets in {}", format_countdown(d)),
                None => "reset passed, refreshing".to_owned(),
            };
            let (forecast, forecast_level) = if can_forecast {
                forecast_text(forecast(w, now))
            } else {
                (String::new(), Level::Unknown)
            };
            bars.push(BarRow {
                label: w.kind.label(),
                percent,
                level: percent.map_or(Level::Unknown, level_for),
                detail,
                forecast,
                forecast_level,
            });
            // Compact mode shows only the two headline limits.
            if matches!(w.kind, WindowKind::Session | WindowKind::Weekly) {
                compact_values.push(percent.map_or("—".to_owned(), |p| format!("{p}%")));
            }
        }
    }

    let problem = match &st.status {
        ConnectionStatus::Connected | ConnectionStatus::Unknown => String::new(),
        other => status_text(other),
    };
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
    let compact = if compact_values.is_empty() {
        format!("{name}  no data")
    } else {
        format!("{name}  {}", compact_values.join(" · "))
    };

    WidgetSection {
        title: name.to_owned(),
        problem,
        bars,
        updated,
        stale,
        compact,
        chips: Vec::new(),
        activity_level: ActivityLevel::None,
        activity_busy: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aum_core::{Provenance, ProviderId, ProviderSnapshot, UsageWindow};
    use chrono::{Duration as CD, TimeZone};

    const IV: Duration = Duration::from_secs(300);

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 17, 0, 0).unwrap()
    }

    fn w(kind: WindowKind, pct: u32, mins: i64) -> UsageWindow {
        UsageWindow::new(kind, pct, now() + CD::minutes(mins), Provenance::Official)
    }

    fn state(windows: Vec<UsageWindow>, status: ConnectionStatus, age_min: i64) -> ProviderState {
        ProviderState {
            provider: ProviderId::Claude,
            status,
            snapshot: Some(ProviderSnapshot {
                provider: ProviderId::Claude,
                windows,
                plan: None,
                fetched_at: now() - CD::minutes(age_min),
                unrecognized: vec![],
            }),
            last_attempt_at: None,
            consecutive_failures: 0,
        }
    }

    #[test]
    fn healthy_section_has_bars_and_compact_line() {
        let st = state(
            vec![
                w(WindowKind::Session, 35, 244),
                w(WindowKind::Weekly, 34, 6000),
                w(WindowKind::WeeklyModel("Fable".into()), 0, 6000),
            ],
            ConnectionStatus::Connected,
            1,
        );
        let m = build(&[st], now(), IV);
        let s = &m.sections[0];
        assert_eq!(s.bars.len(), 3);
        assert_eq!(s.bars[0].percent, Some(35));
        assert_eq!(s.bars[0].detail, "resets in 4h 04m");
        assert_eq!(
            s.compact, "Claude Code  35% · 34%",
            "model-weekly is left out of compact mode"
        );
        assert_eq!(s.problem, "");
        assert!(!s.stale);
        assert_eq!(s.updated, "Updated 1 min ago");
    }

    #[test]
    fn levels_follow_thresholds() {
        let st = state(
            vec![
                w(WindowKind::Session, 50, 60),
                w(WindowKind::Weekly, 80, 60),
                w(WindowKind::WeeklyModel("X".into()), 95, 60),
            ],
            ConnectionStatus::Connected,
            0,
        );
        let l: Vec<_> = build(&[st], now(), IV).sections[0]
            .bars
            .iter()
            .map(|b| b.level)
            .collect();
        assert_eq!(l, vec![Level::Ok, Level::Warn, Level::Critical]);
    }

    #[test]
    fn rolled_over_window_is_not_shown_as_current() {
        let st = state(
            vec![
                w(WindowKind::Session, 98, -3),
                w(WindowKind::Weekly, 40, 100),
            ],
            ConnectionStatus::Connected,
            1,
        );
        let s = &build(&[st], now(), IV).sections[0];
        assert_eq!(s.bars[0].percent, None);
        assert_eq!(s.bars[0].level, Level::Unknown);
        assert_eq!(s.bars[0].detail, "reset passed, refreshing");
        assert_eq!(s.compact, "Claude Code  — · 40%");
    }

    #[test]
    fn problems_and_stale_data_are_visible() {
        let st = state(
            vec![w(WindowKind::Session, 35, 60)],
            ConnectionStatus::Offline,
            30,
        );
        let s = &build(&[st], now(), IV).sections[0];
        assert_eq!(s.problem, "Offline");
        assert!(s.stale);
        assert_eq!(s.updated, "Stale: updated 30 min ago");
        assert_eq!(s.bars[0].percent, Some(35), "last good values stay visible");
    }

    #[test]
    fn auth_problem_text_is_actionable() {
        let mut st = ProviderState::new(ProviderId::Claude);
        st.status = ConnectionStatus::NotLoggedIn;
        let s = &build(&[st], now(), IV).sections[0];
        assert!(s.problem.contains("claude"));
        assert!(s.bars.is_empty());
        assert_eq!(s.compact, "Claude Code  no data");
        assert_eq!(s.updated, "Never updated");
    }

    #[test]
    fn startup_state_is_not_an_error() {
        let s = &build(&[ProviderState::new(ProviderId::Claude)], now(), IV).sections[0];
        assert_eq!(s.problem, "");
    }

    #[test]
    fn no_providers_gives_no_sections() {
        assert!(build(&[], now(), IV).sections.is_empty());
    }

    #[test]
    fn activity_chips_reach_the_widget_section_and_default_to_none() {
        let st = state(
            vec![w(WindowKind::Session, 35, 244)],
            ConnectionStatus::Connected,
            1,
        );
        let plain = build(std::slice::from_ref(&st), now(), IV);
        assert!(plain.sections[0].chips.is_empty());
        assert_eq!(
            (
                plain.sections[0].activity_level,
                plain.sections[0].activity_busy
            ),
            (ActivityLevel::None, 0)
        );

        let view = ActivityView {
            chips: vec![Chip {
                level: ActivityLevel::Working,
                text: "2 working".into(),
            }],
            level: ActivityLevel::Working,
            busy: 2,
            problems: vec![],
        };
        let m = build_with_activity(
            &[st],
            &HashMap::from([(ProviderId::Claude, view)]),
            now(),
            IV,
        );
        let s = &m.sections[0];
        assert_eq!(s.chips[0].text, "2 working");
        assert_eq!(
            (s.activity_level, s.activity_busy),
            (ActivityLevel::Working, 2)
        );
        assert_eq!(
            s.compact, "Claude Code  35%",
            "compact text itself is unchanged; the dot is drawn by the UI"
        );
    }

    // ----- pace forecast line under each bar -----

    #[test]
    fn heavy_early_use_shows_when_the_limit_would_run_out() {
        let st = state(
            vec![w(WindowKind::Session, 35, 244)],
            ConnectionStatus::Connected,
            0,
        );
        let b = &build(&[st], now(), IV).sections[0].bars[0];
        assert_eq!(b.forecast, "Pace estimate: runs out in ~1h 44m");
        assert_eq!(b.forecast_level, Level::Critical);
    }

    #[test]
    fn steady_use_shows_the_projected_percent_and_warns_when_it_is_close() {
        // Weekly 34% with 100 h left: 40.5% of the week elapsed, so about 84% by the reset.
        let st = state(
            vec![w(WindowKind::Weekly, 34, 6000)],
            ConnectionStatus::Connected,
            0,
        );
        let b = &build(&[st], now(), IV).sections[0].bars[0];
        assert_eq!(b.forecast, "Pace estimate: ~84% by reset");
        assert_eq!(b.forecast_level, Level::Ok);
        // 40% used, same time left: about 99%, which is close enough to warn.
        let st = state(
            vec![w(WindowKind::Weekly, 40, 6000)],
            ConnectionStatus::Connected,
            0,
        );
        let b = &build(&[st], now(), IV).sections[0].bars[0];
        assert_eq!(b.forecast, "Pace estimate: ~99% by reset");
        assert_eq!(b.forecast_level, Level::Warn);
    }

    #[test]
    fn no_forecast_line_when_there_is_nothing_sound_to_say() {
        let empty =
            |st: ProviderState| build(&[st], now(), IV).sections[0].bars[0].forecast.clone();
        // Just opened.
        let fresh = state(
            vec![w(WindowKind::Session, 3, 295)],
            ConnectionStatus::Connected,
            0,
        );
        assert_eq!(empty(fresh), "");
        // Window rolled over: the value is not current, so no projection from it.
        let rolled = state(
            vec![w(WindowKind::Session, 98, -3)],
            ConnectionStatus::Connected,
            0,
        );
        assert_eq!(empty(rolled), "");
        // Limit already reached.
        let full = state(
            vec![w(WindowKind::Session, 100, 60)],
            ConnectionStatus::Connected,
            0,
        );
        assert_eq!(empty(full), "");
        // A window with an unknown length.
        let odd = state(
            vec![w(WindowKind::Named("3-day".into()), 40, 600)],
            ConnectionStatus::Connected,
            0,
        );
        assert_eq!(empty(odd), "");
    }

    #[test]
    fn stale_or_failing_data_is_not_extrapolated() {
        // 30 minutes old: the percentage no longer matches the clock, so a projection would mislead.
        let stale = state(
            vec![w(WindowKind::Session, 35, 244)],
            ConnectionStatus::Connected,
            30,
        );
        assert_eq!(build(&[stale], now(), IV).sections[0].bars[0].forecast, "");
        let offline = state(
            vec![w(WindowKind::Session, 35, 244)],
            ConnectionStatus::Offline,
            0,
        );
        assert_eq!(
            build(&[offline], now(), IV).sections[0].bars[0].forecast,
            ""
        );
    }

    #[test]
    fn the_forecast_does_not_change_the_existing_detail_or_compact_text() {
        let st = state(
            vec![
                w(WindowKind::Session, 35, 244),
                w(WindowKind::Weekly, 34, 6000),
            ],
            ConnectionStatus::Connected,
            1,
        );
        let s = &build(&[st], now(), IV).sections[0];
        assert_eq!(s.bars[0].detail, "resets in 4h 04m");
        assert_eq!(s.compact, "Claude Code  35% · 34%");
    }
}
