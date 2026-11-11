//! Turns session snapshots into what the user sees and hears. Pure logic, fully tested.
//!
//! The provider's interface says *what each session is doing now*, not *when it changed*. So
//! "finished 4 min ago" is inferred by watching a session go from working to idle between two polls;
//! a turn shorter than the poll interval can be missed entirely, and sessions that were already idle
//! when the app started are shown as idle, never as "finished".

use std::collections::HashMap;
use std::time::Duration;

use aum_core::{ActivityHealth, ActivitySnapshot, ActivityState, SessionState};
use chrono::{DateTime, Duration as CDuration, Utc};

use crate::view::ago;

/// A session first seen this soon after it started is "new": if its first sighting is already a
/// wait, that is announced (a fresh session can ask for permission before the next poll).
const NEW_SESSION: CDuration = CDuration::minutes(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A session went from working (or waiting) to idle. `ran` is how long it had been busy.
    Finished { ran: Duration },
    /// A session is now blocked on the user.
    NeedsInput { reason: String },
}

#[derive(Debug, Clone)]
struct Tracked {
    state: SessionState,
    /// When the current stretch of work started (as far as we know).
    worked_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    prev: HashMap<String, Tracked>,
    /// Moments at which we saw a session finish, newest last.
    finished: Vec<DateTime<Utc>>,
}

fn is_new(started: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    started.is_some_and(|t| now - t < NEW_SESSION && now >= t)
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the latest snapshot (or `None` when activity could not be read) and get the events it
    /// caused. After an outage everything is forgotten, so recovering never invents transitions.
    pub fn observe(&mut self, snap: Option<&ActivitySnapshot>, now: DateTime<Utc>) -> Vec<Event> {
        let Some(snap) = snap else {
            self.prev.clear();
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut next = HashMap::new();

        for s in &snap.sessions {
            let old = self.prev.remove(&s.key);
            let mut worked_since = old.as_ref().and_then(|o| o.worked_since);
            match (old.as_ref().map(|o| &o.state), &s.state) {
                // First sighting.
                (None, SessionState::NeedsInput(r)) => {
                    if is_new(s.started_at, now) {
                        events.push(Event::NeedsInput { reason: r.clone() });
                    }
                }
                (None, SessionState::Working) => {
                    worked_since = Some(if is_new(s.started_at, now) {
                        s.started_at.unwrap_or(now)
                    } else {
                        now
                    });
                }
                // Busy -> idle: a turn finished.
                (Some(SessionState::Working | SessionState::NeedsInput(_)), SessionState::Idle) => {
                    if let Some(w) = worked_since {
                        let ran = (now - w).to_std().unwrap_or_default();
                        events.push(Event::Finished { ran });
                        self.finished.push(now);
                    }
                    worked_since = None;
                }
                (Some(SessionState::Idle), SessionState::Working) => worked_since = Some(now),
                (Some(SessionState::Working | SessionState::Idle), SessionState::NeedsInput(r)) => {
                    events.push(Event::NeedsInput { reason: r.clone() });
                }
                (Some(SessionState::NeedsInput(a)), SessionState::NeedsInput(b)) if a != b => {
                    events.push(Event::NeedsInput { reason: b.clone() });
                }
                // Anything involving an unknown status, or no change: nothing to report.
                _ => {}
            }
            if matches!(s.state, SessionState::Working | SessionState::NeedsInput(_))
                && worked_since.is_none()
            {
                worked_since = Some(now);
            }
            next.insert(
                s.key.clone(),
                Tracked {
                    state: s.state.clone(),
                    worked_since,
                },
            );
        }
        // Sessions that disappeared are not reported: a closed terminal cannot be told apart from
        // a session that ended normally.
        self.prev = next;
        events
    }

    /// The most recent finish still inside the visibility window.
    pub fn last_finished(&self, now: DateTime<Utc>, ttl: Duration) -> Option<DateTime<Utc>> {
        let ttl = CDuration::from_std(ttl).unwrap_or(CDuration::minutes(10));
        self.finished
            .iter()
            .rev()
            .find(|t| now - **t <= ttl && now >= **t)
            .copied()
    }

    /// Forget finishes older than `ttl`.
    pub fn prune(&mut self, now: DateTime<Utc>, ttl: Duration) {
        let ttl = CDuration::from_std(ttl).unwrap_or(CDuration::minutes(10));
        self.finished.retain(|t| now - *t <= ttl);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    None,
    Idle,
    Working,
    Finished,
    NeedsInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityView {
    pub chips: Vec<Chip>,
    /// The most important level among the chips (drives the dot colour and the tray badge).
    pub level: Level,
    /// Sessions that are working or waiting.
    pub busy: usize,
    /// Environments that could not be read, e.g. "WSL Ubuntu: timed out".
    pub problems: Vec<String>,
}

impl ActivityView {
    pub fn empty() -> Self {
        Self {
            chips: Vec::new(),
            level: Level::None,
            busy: 0,
            problems: Vec::new(),
        }
    }

    /// One line for the tray menu and tooltip, or `None` if there is nothing to say.
    pub fn text(&self) -> Option<String> {
        (!self.chips.is_empty()).then(|| {
            self.chips
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        })
    }
}

/// What to show for one provider. `state` is `None` when activity is switched off.
pub fn view(
    state: Option<&ActivityState>,
    tracker: &Tracker,
    now: DateTime<Utc>,
    ttl: Duration,
) -> ActivityView {
    let Some(state) = state else {
        return ActivityView::empty();
    };
    match &state.health {
        ActivityHealth::Unknown => return ActivityView::empty(),
        ActivityHealth::Unavailable(_) => {
            return ActivityView {
                chips: vec![Chip {
                    level: Level::None,
                    text: "activity unavailable".to_owned(),
                }],
                level: Level::None,
                busy: 0,
                problems: Vec::new(),
            };
        }
        ActivityHealth::Ok => {}
    }
    let Some(snap) = &state.snapshot else {
        return ActivityView::empty();
    };
    let (working, waiting, idle) = (snap.working(), snap.needs_input(), snap.idle());
    let unknown = snap.sessions.len() - working - waiting - idle;

    let mut chips = Vec::new();
    if waiting > 0 {
        let reasons: Vec<&str> = snap
            .sessions
            .iter()
            .filter_map(|s| match &s.state {
                SessionState::NeedsInput(r) => Some(r.as_str()),
                _ => None,
            })
            .collect();
        let text = match (waiting, reasons.first()) {
            (1, Some(r)) => format!("needs input: {r}"),
            (n, _) => format!("{n} need input"),
        };
        chips.push(Chip {
            level: Level::NeedsInput,
            text,
        });
    }
    if working > 0 {
        chips.push(Chip {
            level: Level::Working,
            text: format!("{working} working"),
        });
    }
    if let Some(at) = tracker.last_finished(now, ttl) {
        let age = (now - at).to_std().unwrap_or_default();
        chips.push(Chip {
            level: Level::Finished,
            text: format!("finished {}", ago(age)),
        });
    }
    if chips.is_empty() {
        // Idle sessions only matter when nothing else is going on; otherwise old idle sessions
        // would clutter every line forever.
        if idle > 0 {
            chips.push(Chip {
                level: Level::Idle,
                text: format!("{idle} idle"),
            });
        } else if unknown > 0 {
            chips.push(Chip {
                level: Level::Idle,
                text: "status unknown".to_owned(),
            });
        }
    }
    let level = chips.iter().map(|c| c.level).max().unwrap_or(Level::None);
    ActivityView {
        chips,
        level,
        busy: working + waiting,
        problems: snap.problems.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aum_core::{ProviderId, SessionActivity};
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap()
    }
    fn at(secs: i64) -> DateTime<Utc> {
        t0() + CDuration::seconds(secs)
    }

    fn s(key: &str, state: SessionState, started_secs_ago: i64) -> SessionActivity {
        SessionActivity {
            key: key.to_owned(),
            state,
            started_at: Some(t0() - CDuration::seconds(started_secs_ago)),
            source: "windows".into(),
        }
    }
    const OLD: i64 = 86_400; // started a day ago: not a "new" session

    fn snap(sessions: Vec<SessionActivity>) -> ActivitySnapshot {
        ActivitySnapshot {
            provider: ProviderId::Claude,
            sessions,
            observed_at: t0(),
            problems: vec![],
        }
    }

    fn working(k: &str) -> SessionActivity {
        s(k, SessionState::Working, OLD)
    }
    fn idle(k: &str) -> SessionActivity {
        s(k, SessionState::Idle, OLD)
    }
    fn waiting(k: &str, why: &str) -> SessionActivity {
        s(k, SessionState::NeedsInput(why.into()), OLD)
    }

    #[test]
    fn working_to_idle_is_a_finish_with_the_run_time() {
        let mut t = Tracker::new();
        assert!(t.observe(Some(&snap(vec![working("a")])), at(0)).is_empty());
        assert!(
            t.observe(Some(&snap(vec![working("a")])), at(15))
                .is_empty()
        );
        let ev = t.observe(Some(&snap(vec![idle("a")])), at(75));
        assert_eq!(
            ev,
            vec![Event::Finished {
                ran: Duration::from_secs(75)
            }]
        );
        assert_eq!(
            t.last_finished(at(80), Duration::from_secs(600)),
            Some(at(75))
        );
        // Staying idle repeats nothing.
        assert!(t.observe(Some(&snap(vec![idle("a")])), at(90)).is_empty());
    }

    #[test]
    fn already_idle_at_startup_is_not_a_finish() {
        let mut t = Tracker::new();
        assert!(
            t.observe(Some(&snap(vec![idle("a"), idle("b")])), at(0))
                .is_empty()
        );
        assert_eq!(t.last_finished(at(1), Duration::from_secs(600)), None);
    }

    #[test]
    fn waiting_is_announced_once_per_wait_and_a_reason_change_counts() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a")])), at(0));
        let ev = t.observe(Some(&snap(vec![waiting("a", "permission prompt")])), at(15));
        assert_eq!(
            ev,
            vec![Event::NeedsInput {
                reason: "permission prompt".into()
            }]
        );
        assert!(
            t.observe(Some(&snap(vec![waiting("a", "permission prompt")])), at(30))
                .is_empty()
        );
        let ev = t.observe(Some(&snap(vec![waiting("a", "dialog open")])), at(45));
        assert_eq!(
            ev,
            vec![Event::NeedsInput {
                reason: "dialog open".into()
            }]
        );
    }

    #[test]
    fn answering_a_prompt_and_finishing_counts_the_whole_run() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a")])), at(0));
        t.observe(Some(&snap(vec![waiting("a", "input needed")])), at(30));
        t.observe(Some(&snap(vec![working("a")])), at(60));
        let ev = t.observe(Some(&snap(vec![idle("a")])), at(120));
        assert_eq!(
            ev,
            vec![Event::Finished {
                ran: Duration::from_secs(120)
            }]
        );
        // Waiting straight to idle between two polls is also a finish.
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("b")])), at(0));
        t.observe(Some(&snap(vec![waiting("b", "x")])), at(15));
        assert!(matches!(
            t.observe(Some(&snap(vec![idle("b")])), at(99))[0],
            Event::Finished { .. }
        ));
    }

    #[test]
    fn a_brand_new_session_that_immediately_waits_is_announced_but_an_old_one_is_not() {
        let mut t = Tracker::new();
        let new = s(
            "n",
            SessionState::NeedsInput("permission prompt".into()),
            20,
        );
        assert_eq!(t.observe(Some(&snap(vec![new])), at(0)).len(), 1);
        let mut t = Tracker::new();
        assert!(
            t.observe(Some(&snap(vec![waiting("o", "permission prompt")])), at(0))
                .is_empty(),
            "already waiting when we started looking"
        );
    }

    #[test]
    fn unknown_status_never_creates_events() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a")])), at(0));
        assert!(
            t.observe(
                Some(&snap(vec![s("a", SessionState::Unknown, OLD)])),
                at(15)
            )
            .is_empty()
        );
        // Back to idle from unknown: no finish was seen, so none is invented.
        assert!(t.observe(Some(&snap(vec![idle("a")])), at(30)).is_empty());
        assert_eq!(t.last_finished(at(31), Duration::from_secs(600)), None);
    }

    #[test]
    fn a_vanished_session_is_not_reported_as_finished() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a"), working("b")])), at(0));
        assert!(
            t.observe(Some(&snap(vec![working("b")])), at(15))
                .is_empty()
        );
        assert_eq!(t.last_finished(at(16), Duration::from_secs(600)), None);
    }

    #[test]
    fn an_outage_forgets_state_so_recovery_invents_nothing() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a")])), at(0));
        assert!(t.observe(None, at(15)).is_empty());
        assert!(
            t.observe(Some(&snap(vec![idle("a")])), at(300)).is_empty(),
            "idle after an outage is not a finish"
        );
    }

    #[test]
    fn several_sessions_finishing_together_give_several_events() {
        let mut t = Tracker::new();
        t.observe(
            Some(&snap(vec![working("a"), working("b"), working("c")])),
            at(0),
        );
        let ev = t.observe(
            Some(&snap(vec![idle("a"), idle("b"), working("c")])),
            at(60),
        );
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn finished_memory_expires() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a")])), at(0));
        t.observe(Some(&snap(vec![idle("a")])), at(30));
        let ttl = Duration::from_secs(600);
        assert!(t.last_finished(at(30 + 599), ttl).is_some());
        assert!(t.last_finished(at(30 + 601), ttl).is_none());
        t.prune(at(30 + 601), ttl);
        assert!(t.finished.is_empty());
    }

    // ---- what the user sees ---------------------------------------------------------------------

    fn st(sessions: Vec<SessionActivity>) -> ActivityState {
        ActivityState {
            provider: ProviderId::Claude,
            health: ActivityHealth::Ok,
            snapshot: Some(snap(sessions)),
            consecutive_failures: 0,
        }
    }

    const TTL: Duration = Duration::from_secs(600);

    #[test]
    fn chips_for_each_situation() {
        let t = Tracker::new();
        let v = view(
            Some(&st(vec![working("a"), working("b"), idle("c")])),
            &t,
            at(0),
            TTL,
        );
        assert_eq!(
            v.text().as_deref(),
            Some("2 working"),
            "idle sessions are hidden while something is working"
        );
        assert_eq!((v.level, v.busy), (Level::Working, 2));

        let v = view(
            Some(&st(vec![waiting("a", "permission prompt"), working("b")])),
            &t,
            at(0),
            TTL,
        );
        assert_eq!(
            v.text().as_deref(),
            Some("needs input: permission prompt · 1 working")
        );
        assert_eq!(v.level, Level::NeedsInput);

        let v = view(
            Some(&st(vec![waiting("a", "x"), waiting("b", "y")])),
            &t,
            at(0),
            TTL,
        );
        assert_eq!(v.text().as_deref(), Some("2 need input"));

        let v = view(Some(&st(vec![idle("a"), idle("b")])), &t, at(0), TTL);
        assert_eq!(v.text().as_deref(), Some("2 idle"));
        assert_eq!(v.level, Level::Idle);

        let v = view(Some(&st(vec![])), &t, at(0), TTL);
        assert_eq!(v.text(), None, "no live session: nothing shown");
        assert_eq!(v.level, Level::None);

        let v = view(
            Some(&st(vec![s("u", SessionState::Unknown, OLD)])),
            &t,
            at(0),
            TTL,
        );
        assert_eq!(v.text().as_deref(), Some("status unknown"));
    }

    #[test]
    fn recently_finished_shows_alongside_the_rest_and_then_fades() {
        let mut t = Tracker::new();
        t.observe(Some(&snap(vec![working("a"), working("b")])), at(0));
        t.observe(Some(&snap(vec![idle("a"), working("b")])), at(60));
        let state = st(vec![idle("a"), working("b")]);
        let v = view(Some(&state), &t, at(60 + 240), TTL);
        assert_eq!(v.text().as_deref(), Some("1 working · finished 4 min ago"));
        let v = view(Some(&state), &t, at(60 + 700), TTL);
        assert_eq!(
            v.text().as_deref(),
            Some("1 working"),
            "the finished chip fades after the window"
        );
    }

    #[test]
    fn unavailable_and_off_states() {
        let t = Tracker::new();
        assert_eq!(view(None, &t, at(0), TTL), ActivityView::empty());
        let down = ActivityState {
            health: ActivityHealth::Unavailable("timed out".into()),
            snapshot: None,
            ..st(vec![])
        };
        assert_eq!(
            view(Some(&down), &t, at(0), TTL).text().as_deref(),
            Some("activity unavailable")
        );
        let starting = ActivityState {
            health: ActivityHealth::Unknown,
            snapshot: None,
            ..st(vec![])
        };
        assert_eq!(
            view(Some(&starting), &t, at(0), TTL),
            ActivityView::empty(),
            "startup is not an error"
        );
    }

    #[test]
    fn partial_results_carry_their_problems() {
        let mut state = st(vec![working("a")]);
        state.snapshot.as_mut().unwrap().problems = vec!["WSL Ubuntu: timed out".into()];
        let v = view(Some(&state), &Tracker::new(), at(0), TTL);
        assert_eq!(v.problems, vec!["WSL Ubuntu: timed out"]);
    }
}
