//! A stateless pace estimate for a usage window, from a single reading.
//!
//! The windows have known lengths (the Claude and Codex session is 5 hours, the weekly limit 7 days),
//! so "35% used, resets in 4h 04m" says how much of the window has gone. Assuming the average rate
//! since the window opened continues, that gives the usage expected at the reset, or the moment the
//! limit would be reached. It needs no history, so it reads the same on every computer for the same
//! account. It is an **estimate**: real usage comes in bursts, percentages are whole numbers, and a
//! window can be quiet for hours and then busy.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::{UsageWindow, WindowKind};

/// Below this fraction of the window elapsed there is too little to extrapolate from.
pub const MIN_ELAPSED_FRACTION: f64 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forecast {
    /// The window has only just opened (or the reading is inconsistent with its length).
    TooEarly,
    /// At the average pace so far the usage at the reset will be this many percent (at most 100).
    OnTrack { projected_percent: u8 },
    /// At the average pace so far the limit is reached in this long, before the window resets.
    RunsOut { in_: Duration },
}

impl WindowKind {
    /// The length of the window, where it is known. Other windows have no forecast.
    pub fn length(&self) -> Option<Duration> {
        match self {
            WindowKind::Session => Some(Duration::from_secs(5 * 3600)),
            WindowKind::Weekly | WindowKind::WeeklyModel(_) => {
                Some(Duration::from_secs(7 * 24 * 3600))
            }
            WindowKind::Named(_) => None,
        }
    }
}

/// `None` when no forecast applies: the window's length is unknown, its reset has passed (the value
/// is out of date), or the limit is already reached.
pub fn forecast(window: &UsageWindow, now: DateTime<Utc>) -> Option<Forecast> {
    let length = window.kind.length()?.as_secs_f64();
    let remaining = window.time_to_reset(now)?.as_secs_f64();
    if window.is_exhausted() {
        return None;
    }
    // A reset further away than the window is long (jitter) leaves a negative elapsed time.
    let elapsed = length - remaining;
    if elapsed / length < MIN_ELAPSED_FRACTION {
        return Some(Forecast::TooEarly);
    }
    let used = f64::from(window.used_percent);
    // The usage expected at the reset is used * length / elapsed; it exceeds 100 exactly when
    // used * length > 100 * elapsed (compared this way so the boundary is exact).
    if used * length > 100.0 * elapsed {
        let secs = elapsed * (100.0 - used) / used;
        return Some(Forecast::RunsOut {
            in_: Duration::from_secs(secs.round() as u64),
        });
    }
    let projected = (used * length / elapsed).round().min(100.0) as u8;
    Some(Forecast::OnTrack {
        projected_percent: projected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Provenance;
    use chrono::{Duration as CD, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 27, 12, 0, 0).unwrap()
    }

    fn win(kind: WindowKind, used: u32, resets_in: CD) -> UsageWindow {
        UsageWindow::new(kind, used, now() + resets_in, Provenance::Official)
    }

    #[test]
    fn window_lengths() {
        assert_eq!(
            WindowKind::Session.length(),
            Some(Duration::from_secs(18_000))
        );
        assert_eq!(
            WindowKind::Weekly.length(),
            Some(Duration::from_secs(604_800))
        );
        assert_eq!(
            WindowKind::WeeklyModel("Fable".into()).length(),
            WindowKind::Weekly.length()
        );
        assert_eq!(WindowKind::Named("3-day".into()).length(), None);
    }

    #[test]
    fn heavy_early_use_runs_out_before_the_reset() {
        // 35% used with 4h04m left: 56 min elapsed (18.7%), so the average pace reaches 100% after
        // 56 min * 65 / 35 = 104 min more, well before the reset.
        let w = win(WindowKind::Session, 35, CD::minutes(244));
        assert_eq!(
            forecast(&w, now()),
            Some(Forecast::RunsOut {
                in_: Duration::from_secs(104 * 60)
            })
        );
    }

    #[test]
    fn steady_weekly_use_is_on_track() {
        // 33% with 2d22h left: 4d2h of 7d elapsed (58.3%), so about 57% by the reset.
        let w = win(WindowKind::Weekly, 33, CD::hours(70));
        assert_eq!(
            forecast(&w, now()),
            Some(Forecast::OnTrack {
                projected_percent: 57
            })
        );
    }

    #[test]
    fn model_weekly_windows_use_the_weekly_length() {
        let a = win(WindowKind::Weekly, 20, CD::hours(100));
        let b = win(WindowKind::WeeklyModel("Fable".into()), 20, CD::hours(100));
        assert_eq!(forecast(&a, now()), forecast(&b, now()));
    }

    #[test]
    fn too_early_in_the_window_says_nothing() {
        // 10 minutes into a 5 h session: 3.3% elapsed.
        assert_eq!(
            forecast(&win(WindowKind::Session, 5, CD::minutes(290)), now()),
            Some(Forecast::TooEarly)
        );
        // 30 minutes is exactly 10%: enough.
        assert_ne!(
            forecast(&win(WindowKind::Session, 5, CD::minutes(270)), now()),
            Some(Forecast::TooEarly)
        );
        // A reset time further away than the window is long (clock or wording jitter): too early,
        // never a negative elapsed time.
        assert_eq!(
            forecast(&win(WindowKind::Session, 5, CD::minutes(302)), now()),
            Some(Forecast::TooEarly)
        );
    }

    #[test]
    fn no_usage_after_enough_time_is_on_track_at_zero() {
        let w = win(WindowKind::Session, 0, CD::minutes(100));
        assert_eq!(
            forecast(&w, now()),
            Some(Forecast::OnTrack {
                projected_percent: 0
            })
        );
    }

    #[test]
    fn exactly_on_the_line_is_on_track_at_100() {
        // 50% used with half the session gone (2h30m left).
        let w = win(WindowKind::Session, 50, CD::minutes(150));
        assert_eq!(
            forecast(&w, now()),
            Some(Forecast::OnTrack {
                projected_percent: 100
            })
        );
    }

    #[test]
    fn a_hair_over_the_line_runs_out() {
        let w = win(WindowKind::Session, 51, CD::minutes(150));
        assert!(matches!(
            forecast(&w, now()),
            Some(Forecast::RunsOut { .. })
        ));
    }

    #[test]
    fn more_usage_runs_out_sooner() {
        let at = |used| match forecast(&win(WindowKind::Session, used, CD::minutes(200)), now()) {
            Some(Forecast::RunsOut { in_ }) => in_,
            other => panic!("{other:?}"),
        };
        assert!(at(60) < at(50));
        assert!(at(90) < at(60));
    }

    #[test]
    fn no_forecast_when_it_does_not_apply() {
        // Limit reached, reset passed, unknown window length.
        assert_eq!(
            forecast(&win(WindowKind::Session, 100, CD::minutes(60)), now()),
            None
        );
        assert_eq!(
            forecast(&win(WindowKind::Session, 40, CD::minutes(-5)), now()),
            None
        );
        assert_eq!(
            forecast(
                &win(WindowKind::Named("3-day".into()), 40, CD::hours(10)),
                now()
            ),
            None
        );
    }

    #[test]
    fn projection_is_never_shown_above_100_percent() {
        // Anything over the limit is RunsOut, never OnTrack with a number above 100.
        for used in 1..100u32 {
            for left in [60, 120, 200, 280] {
                if let Some(Forecast::OnTrack { projected_percent }) =
                    forecast(&win(WindowKind::Session, used, CD::minutes(left)), now())
                {
                    assert!(projected_percent <= 100, "{used}% with {left} min left");
                }
            }
        }
    }
}
