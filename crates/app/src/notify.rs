//! Decides *when* to alert. Pure logic: no OS calls, fully tested.
//!
//! Rules
//! - Usage alerts fire once per limit window per level (warn, critical, exhausted).
//!   If usage jumps past several levels, only the highest is announced.
//! - Reset alerts fire when a window that had reached the warn level rolls over.
//! - Error alerts fire for sign-in problems and unreadable provider output, at most
//!   once per cooldown, and re-arm after the provider has been healthy again.
//! - Offline and rate-limit states never alert, and stale data never triggers alerts.
//! - Providers report reset times with about a minute of jitter (observed:
//!   9:10pm then 9:09pm), so two reset times within [`SAME_WINDOW`] are one window.

use std::collections::{HashMap, HashSet};

use aum_core::{ConnectionStatus, ProviderId, ProviderState, WindowKind, format_countdown};
use aum_store::{ActivitySettings, NotifyConfig};
use chrono::{DateTime, Duration, Utc};

use crate::activity::Event;

/// Reset times closer than this belong to the same limit window.
const SAME_WINDOW: Duration = Duration::minutes(15);
/// Minimum time between two alerts of the same error class.
const ERROR_COOLDOWN: Duration = Duration::hours(6);
/// Two session alerts of the same kind closer together than this are suppressed (flapping guard).
const ACTIVITY_COOLDOWN: Duration = Duration::seconds(20);
/// Remembered alerts older than this are pruned from the persisted set.
const KEEP_FIRED: Duration = Duration::days(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Warn,
    Critical,
    Exhausted,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Warn => "warn",
            Level::Critical => "crit",
            Level::Exhausted => "exh",
        }
    }
}

#[derive(Debug, Clone)]
struct Seen {
    anchor: DateTime<Utc>,
    percent: u8,
}

#[derive(Debug, Default)]
pub struct Engine {
    /// Keys: `provider|kind|tag|epoch`.
    fired: HashSet<String>,
    seen: HashMap<(ProviderId, WindowKind), Seen>,
    error_at: HashMap<(ProviderId, &'static str), DateTime<Utc>>,
    activity_at: HashMap<(ProviderId, &'static str), DateTime<Utc>>,
}

fn kind_key(k: &WindowKind) -> String {
    match k {
        WindowKind::Session => "session".to_owned(),
        WindowKind::Weekly => "weekly".to_owned(),
        WindowKind::WeeklyModel(m) => format!("weekly:{m}"),
        WindowKind::Named(n) => format!("named:{n}"),
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore the persisted set of already-shown alerts.
    pub fn import(&mut self, keys: impl IntoIterator<Item = String>) {
        self.fired.extend(keys);
    }

    /// The set to persist (old entries dropped).
    pub fn export(&self, now: DateTime<Utc>) -> Vec<String> {
        let mut keys: Vec<String> = self
            .fired
            .iter()
            .filter(|k| epoch_of(k).is_some_and(|e| e > now - KEEP_FIRED))
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    fn already(&self, provider: ProviderId, kind: &str, tag: &str, epoch: DateTime<Utc>) -> bool {
        let prefix = format!("{}|{kind}|{tag}|", provider.as_str());
        self.fired
            .iter()
            .filter(|k| k.starts_with(&prefix))
            .filter_map(|k| epoch_of(k))
            .any(|e| (e - epoch).abs() < SAME_WINDOW)
    }

    fn mark(&mut self, provider: ProviderId, kind: &str, tag: &str, epoch: DateTime<Utc>) {
        self.fired.insert(format!(
            "{}|{kind}|{tag}|{}",
            provider.as_str(),
            epoch.timestamp()
        ));
    }

    pub fn evaluate(
        &mut self,
        states: &[ProviderState],
        cfg_for: impl Fn(ProviderId) -> NotifyConfig,
        now: DateTime<Utc>,
    ) -> Vec<Notification> {
        let mut out = Vec::new();
        for st in states {
            let cfg = cfg_for(st.provider);
            if !cfg.enabled {
                continue;
            }
            self.usage(st, &cfg, now, &mut out);
            self.errors(st, &cfg, now, &mut out);
        }
        out
    }

    /// Alerts for session activity events. Several events of one kind in one call become a single
    /// alert, and a kind that just fired stays quiet for a short cooldown.
    pub fn activity(
        &mut self,
        provider: ProviderId,
        events: &[Event],
        cfg: &ActivitySettings,
        now: DateTime<Utc>,
    ) -> Vec<Notification> {
        let name = provider.display_name();
        let mut out = Vec::new();

        let waits: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::NeedsInput { reason } => Some(reason.as_str()),
                Event::Finished { .. } => None,
            })
            .collect();
        if cfg.alert_needs_input
            && !waits.is_empty()
            && self.activity_due(provider, "needs-input", now)
        {
            out.push(if waits.len() == 1 {
                Notification {
                    title: format!("{name} needs your input"),
                    body: waits[0].to_owned(),
                }
            } else {
                Notification {
                    title: format!("{} {name} sessions need your input", waits.len()),
                    body: String::new(),
                }
            });
        }

        let min_run = cfg.min_run();
        let ran: Vec<std::time::Duration> = events
            .iter()
            .filter_map(|e| match e {
                Event::Finished { ran } if *ran >= min_run => Some(*ran),
                _ => None,
            })
            .collect();
        if cfg.alert_finished && !ran.is_empty() && self.activity_due(provider, "finished", now) {
            out.push(if ran.len() == 1 {
                Notification {
                    title: format!("{name} finished"),
                    body: format!("Ran {}.", format_run(ran[0])),
                }
            } else {
                Notification {
                    title: format!("{} {name} sessions finished", ran.len()),
                    body: String::new(),
                }
            });
        }
        out
    }

    fn activity_due(
        &mut self,
        provider: ProviderId,
        kind: &'static str,
        now: DateTime<Utc>,
    ) -> bool {
        let due = self
            .activity_at
            .get(&(provider, kind))
            .is_none_or(|t| now - *t >= ACTIVITY_COOLDOWN);
        if due {
            self.activity_at.insert((provider, kind), now);
        }
        due
    }

    fn usage(
        &mut self,
        st: &ProviderState,
        cfg: &NotifyConfig,
        now: DateTime<Utc>,
        out: &mut Vec<Notification>,
    ) {
        // Only fresh, successful data may trigger alerts.
        let (ConnectionStatus::Connected, Some(snap)) = (&st.status, &st.snapshot) else {
            return;
        };
        let id = st.provider;
        let name = id.display_name();
        let (warn, crit) = cfg.levels();

        for w in snap.windows.iter().filter(|w| !w.is_expired(now)) {
            let kind = kind_key(&w.kind);
            let label = w.kind.label();
            let p = w.used_percent;

            // Which limit window is this? Keep the first-seen reset time as its anchor so
            // one-minute jitter does not look like a new window.
            let key = (id, w.kind.clone());
            let prev = self.seen.get(&key).cloned();
            let anchor = match &prev {
                Some(s) if (w.resets_at - s.anchor).abs() < SAME_WINDOW => s.anchor,
                _ => w.resets_at,
            };

            // Rolled over from a window that had reached the warn level?
            if let Some(s) = &prev {
                if anchor != s.anchor
                    && s.percent >= warn
                    && cfg.reset
                    && !self.already(id, &kind, "reset", anchor)
                {
                    self.mark(id, &kind, "reset", anchor);
                    out.push(Notification {
                        title: format!("{name}: {label} limit reset"),
                        body: format!("Usage is back to {p}%."),
                    });
                }
            }
            self.seen.insert(key, Seen { anchor, percent: p });

            let level = if p >= 100 && cfg.exhausted {
                Some(Level::Exhausted)
            } else if p >= crit {
                Some(Level::Critical)
            } else if p >= warn {
                Some(Level::Warn)
            } else {
                None
            };
            let Some(level) = level else { continue };

            // Mark every level up to the current one; announce only the highest new one.
            let mut announce = None;
            for l in [Level::Warn, Level::Critical, Level::Exhausted]
                .into_iter()
                .filter(|l| *l <= level)
            {
                if !self.already(id, &kind, l.tag(), anchor) {
                    self.mark(id, &kind, l.tag(), anchor);
                    announce = Some(l);
                }
            }
            if let Some(l) = announce {
                let resets = w.time_to_reset(now).map_or(String::new(), |d| {
                    format!("Resets in {}.", format_countdown(d))
                });
                out.push(match l {
                    Level::Exhausted => Notification {
                        title: format!("{name}: {label} limit reached"),
                        body: resets,
                    },
                    _ => Notification {
                        title: format!("{name}: {label} at {p}%"),
                        body: resets,
                    },
                });
            }
        }
    }

    fn errors(
        &mut self,
        st: &ProviderState,
        cfg: &NotifyConfig,
        now: DateTime<Utc>,
        out: &mut Vec<Notification>,
    ) {
        let id = st.provider;
        let name = id.display_name();
        if st.status == ConnectionStatus::Connected {
            // Healthy again: re-arm error alerts.
            self.error_at.retain(|(p, _), _| *p != id);
            return;
        }
        let (class, title, body): (&'static str, String, String) = match &st.status {
            ConnectionStatus::NotLoggedIn => (
                "auth",
                format!("{name}: sign-in needed"),
                "Run `claude` in a terminal and sign in. Usage can't be read until then."
                    .to_owned(),
            ),
            ConnectionStatus::AuthExpired => (
                "auth",
                format!("{name}: login expired"),
                "Run `claude` in a terminal to sign in again.".to_owned(),
            ),
            // One failed read can be a blip; two in a row is worth telling the user about.
            ConnectionStatus::Error(e) if st.consecutive_failures >= 2 => (
                "error",
                format!("{name}: can't read usage"),
                e.chars().take(120).collect(),
            ),
            ConnectionStatus::Unsupported(why) => (
                "unsupported",
                format!("{name}: usage not available"),
                why.clone(),
            ),
            // Unknown (starting up), Offline and RateLimited never alert.
            _ => return,
        };
        if !cfg.errors {
            return;
        }
        let due = self
            .error_at
            .get(&(id, class))
            .is_none_or(|t| now - *t >= ERROR_COOLDOWN);
        if due {
            self.error_at.insert((id, class), now);
            out.push(Notification { title, body });
        }
    }
}

/// "45s", "4m 12s", "1h 03m".
pub fn format_run(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    match (secs / 3600, (secs % 3600) / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m:02}m"),
    }
}

/// The epoch (last `|` segment) of a persisted key.
fn epoch_of(key: &str) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(key.rsplit('|').next()?.parse().ok()?, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aum_core::{Provenance, ProviderSnapshot, UsageWindow};
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 17, 0, 0).unwrap()
    }
    fn mins(m: i64) -> Duration {
        Duration::minutes(m)
    }

    fn win(kind: WindowKind, pct: u32, resets: DateTime<Utc>) -> UsageWindow {
        UsageWindow::new(kind, pct, resets, Provenance::Official)
    }

    fn state(windows: Vec<UsageWindow>) -> ProviderState {
        ProviderState {
            provider: ProviderId::Claude,
            status: ConnectionStatus::Connected,
            snapshot: Some(ProviderSnapshot {
                provider: ProviderId::Claude,
                windows,
                plan: None,
                fetched_at: t0(),
                unrecognized: vec![],
            }),
            last_attempt_at: None,
            consecutive_failures: 0,
        }
    }

    fn failing(status: ConnectionStatus, failures: u32) -> ProviderState {
        ProviderState {
            status,
            consecutive_failures: failures,
            ..ProviderState::new(ProviderId::Claude)
        }
    }

    fn eval(e: &mut Engine, s: &ProviderState, now: DateTime<Utc>) -> Vec<Notification> {
        e.evaluate(std::slice::from_ref(s), |_| NotifyConfig::default(), now)
    }

    fn session(pct: u32, resets: DateTime<Utc>) -> ProviderState {
        state(vec![win(WindowKind::Session, pct, resets)])
    }

    #[test]
    fn below_warn_is_silent() {
        let mut e = Engine::new();
        assert!(eval(&mut e, &session(74, t0() + mins(120)), t0()).is_empty());
    }

    #[test]
    fn warn_then_critical_then_exhausted_each_fire_once() {
        let mut e = Engine::new();
        let r = t0() + mins(120);
        let n = eval(&mut e, &session(75, r), t0());
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].title, "Claude Code: Session at 75%");
        assert_eq!(n[0].body, "Resets in 2h 00m.");
        // Polling again at the same level: nothing.
        assert!(eval(&mut e, &session(76, r), t0() + mins(5)).is_empty());
        assert!(eval(&mut e, &session(80, r), t0() + mins(10)).is_empty());
        let n = eval(&mut e, &session(90, r), t0() + mins(15));
        assert_eq!(n[0].title, "Claude Code: Session at 90%");
        assert!(eval(&mut e, &session(92, r), t0() + mins(20)).is_empty());
        let n = eval(&mut e, &session(100, r), t0() + mins(25));
        assert_eq!(n[0].title, "Claude Code: Session limit reached");
        assert!(eval(&mut e, &session(100, r), t0() + mins(30)).is_empty());
    }

    #[test]
    fn a_jump_across_levels_announces_only_the_highest() {
        let mut e = Engine::new();
        let n = eval(&mut e, &session(95, t0() + mins(60)), t0());
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("95%"));
        // The skipped warn level must not fire afterwards either.
        assert!(eval(&mut e, &session(96, t0() + mins(60)), t0() + mins(5)).is_empty());
    }

    #[test]
    fn reset_time_jitter_does_not_cause_repeat_alerts() {
        let mut e = Engine::new();
        let r = t0() + mins(120);
        assert_eq!(eval(&mut e, &session(80, r), t0()).len(), 1);
        // Same window reported one minute earlier/later (observed in real output).
        assert!(eval(&mut e, &session(80, r - mins(1)), t0() + mins(5)).is_empty());
        assert!(eval(&mut e, &session(81, r + mins(1)), t0() + mins(10)).is_empty());
    }

    #[test]
    fn a_new_window_rearms_alerts_and_announces_the_reset() {
        let mut e = Engine::new();
        let r1 = t0() + mins(60);
        eval(&mut e, &session(92, r1), t0());
        // Next window: new reset time 5 hours later, usage back to 3%.
        let later = r1 + mins(10);
        let n = eval(&mut e, &session(3, r1 + mins(300)), later);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].title, "Claude Code: Session limit reset");
        assert_eq!(n[0].body, "Usage is back to 3%.");
        // And the new window can alert again.
        let n = eval(&mut e, &session(76, r1 + mins(300)), later + mins(30));
        assert_eq!(n.len(), 1);
    }

    #[test]
    fn no_reset_alert_if_the_old_window_was_quiet() {
        let mut e = Engine::new();
        let r1 = t0() + mins(60);
        eval(&mut e, &session(20, r1), t0());
        assert!(eval(&mut e, &session(2, r1 + mins(300)), r1 + mins(10)).is_empty());
    }

    #[test]
    fn restart_does_not_repeat_alerts() {
        let r = t0() + mins(120);
        let mut first = Engine::new();
        assert_eq!(eval(&mut first, &session(91, r), t0()).len(), 1);
        let saved = first.export(t0());
        assert!(!saved.is_empty());

        let mut second = Engine::new();
        second.import(saved);
        // Even with the reset time jittered by a minute after the restart.
        assert!(eval(&mut second, &session(91, r - mins(1)), t0() + mins(2)).is_empty());
    }

    #[test]
    fn export_prunes_old_entries() {
        let mut e = Engine::new();
        e.import(["claude|session|warn|1000".to_owned()]);
        assert!(e.export(t0()).is_empty());
    }

    #[test]
    fn model_weekly_windows_alert_independently() {
        let mut e = Engine::new();
        let r = t0() + mins(6000);
        let s = state(vec![
            win(WindowKind::Weekly, 40, r),
            win(WindowKind::WeeklyModel("Fable".into()), 91, r),
        ]);
        let n = eval(&mut e, &s, t0());
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("Weekly (Fable)"));
    }

    #[test]
    fn disabled_config_and_flags_are_respected() {
        let r = t0() + mins(60);
        let mut e = Engine::new();
        let off = NotifyConfig {
            enabled: false,
            ..NotifyConfig::default()
        };
        assert!(
            e.evaluate(&[session(99, r)], |_| off.clone(), t0())
                .is_empty()
        );

        // Exhausted disabled: 100% is announced as a critical alert instead.
        let mut e = Engine::new();
        let cfg = NotifyConfig {
            exhausted: false,
            ..NotifyConfig::default()
        };
        let n = e.evaluate(&[session(100, r)], |_| cfg.clone(), t0());
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("100%"));

        // Reset alerts disabled.
        let mut e = Engine::new();
        let cfg = NotifyConfig {
            reset: false,
            ..NotifyConfig::default()
        };
        e.evaluate(&[session(95, r)], |_| cfg.clone(), t0());
        assert!(
            e.evaluate(&[session(1, r + mins(300))], |_| cfg.clone(), r + mins(5))
                .is_empty()
        );
    }

    #[test]
    fn custom_thresholds_apply() {
        let mut e = Engine::new();
        let cfg = NotifyConfig {
            warn_percent: 50,
            critical_percent: 60,
            ..NotifyConfig::default()
        };
        let n = e.evaluate(&[session(55, t0() + mins(60))], |_| cfg.clone(), t0());
        assert_eq!(n.len(), 1);
    }

    #[test]
    fn stale_or_failing_data_never_triggers_usage_alerts() {
        let mut e = Engine::new();
        let mut s = session(99, t0() + mins(60));
        s.status = ConnectionStatus::Offline;
        assert!(eval(&mut e, &s, t0()).is_empty());
    }

    #[test]
    fn expired_windows_are_ignored() {
        let mut e = Engine::new();
        assert!(eval(&mut e, &session(99, t0() - mins(5)), t0()).is_empty());
    }

    #[test]
    fn auth_error_alerts_once_then_respects_cooldown_and_rearms() {
        let mut e = Engine::new();
        let bad = failing(ConnectionStatus::NotLoggedIn, 1);
        let n = eval(&mut e, &bad, t0());
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("sign-in needed"));
        assert!(n[0].body.contains("claude"));
        // Every poll afterwards while still logged out: silent.
        assert!(eval(&mut e, &bad, t0() + mins(5)).is_empty());
        assert!(eval(&mut e, &bad, t0() + mins(300)).is_empty());
        // After the cooldown it reminds again.
        assert_eq!(eval(&mut e, &bad, t0() + mins(361)).len(), 1);
        // Recovery re-arms immediately.
        eval(&mut e, &session(10, t0() + mins(600)), t0() + mins(400));
        assert_eq!(eval(&mut e, &bad, t0() + mins(405)).len(), 1);
    }

    #[test]
    fn expired_login_has_its_own_wording() {
        let mut e = Engine::new();
        let n = eval(&mut e, &failing(ConnectionStatus::AuthExpired, 1), t0());
        assert!(n[0].title.contains("login expired"));
    }

    #[test]
    fn transient_problems_do_not_alert() {
        let mut e = Engine::new();
        for st in [
            ConnectionStatus::Offline,
            ConnectionStatus::RateLimited,
            ConnectionStatus::Unknown,
        ] {
            assert!(eval(&mut e, &failing(st, 5), t0()).is_empty());
        }
        // A single generic failure is a blip; two in a row alert.
        let one = failing(ConnectionStatus::Error("boom".into()), 1);
        assert!(eval(&mut e, &one, t0()).is_empty());
        let two = failing(ConnectionStatus::Error("boom".into()), 2);
        let n = eval(&mut e, &two, t0());
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("can't read usage"));
    }

    #[test]
    fn error_alerts_can_be_switched_off() {
        let mut e = Engine::new();
        let cfg = NotifyConfig {
            errors: false,
            ..NotifyConfig::default()
        };
        let bad = failing(ConnectionStatus::NotLoggedIn, 3);
        assert!(e.evaluate(&[bad], |_| cfg.clone(), t0()).is_empty());
    }

    // ---- session activity alerts -------------------------------------------------------------

    fn act_cfg(needs: bool, finished: bool) -> ActivitySettings {
        ActivitySettings {
            alert_needs_input: needs,
            alert_finished: finished,
            alert_min_secs: 60,
            ..ActivitySettings::default()
        }
    }
    fn waits(r: &str) -> Event {
        Event::NeedsInput { reason: r.into() }
    }
    fn done(secs: u64) -> Event {
        Event::Finished {
            ran: std::time::Duration::from_secs(secs),
        }
    }
    fn alerts(
        e: &mut Engine,
        ev: &[Event],
        cfg: &ActivitySettings,
        now: DateTime<Utc>,
    ) -> Vec<Notification> {
        e.activity(ProviderId::Claude, ev, cfg, now)
    }

    #[test]
    fn activity_alerts_are_off_by_default() {
        let mut e = Engine::new();
        assert!(
            alerts(
                &mut e,
                &[waits("permission prompt"), done(300)],
                &ActivitySettings::default(),
                t0()
            )
            .is_empty()
        );
    }

    #[test]
    fn needs_input_alert_says_why() {
        let mut e = Engine::new();
        let n = alerts(
            &mut e,
            &[waits("permission prompt")],
            &act_cfg(true, false),
            t0(),
        );
        assert_eq!(
            n,
            vec![Notification {
                title: "Claude Code needs your input".into(),
                body: "permission prompt".into()
            }]
        );
    }

    #[test]
    fn finished_alert_respects_the_minimum_run_time() {
        let mut e = Engine::new();
        let cfg = act_cfg(false, true);
        assert!(
            alerts(&mut e, &[done(45)], &cfg, t0()).is_empty(),
            "a 45 s turn is below the 60 s minimum"
        );
        let n = alerts(&mut e, &[done(252)], &cfg, t0() + mins(1));
        assert_eq!(
            n,
            vec![Notification {
                title: "Claude Code finished".into(),
                body: "Ran 4m 12s.".into()
            }]
        );
    }

    #[test]
    fn several_events_in_one_poll_become_one_alert() {
        let mut e = Engine::new();
        let n = alerts(
            &mut e,
            &[waits("a"), waits("b"), waits("c")],
            &act_cfg(true, true),
            t0(),
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].title, "3 Claude Code sessions need your input");
        let n = alerts(
            &mut e,
            &[done(120), done(300)],
            &act_cfg(true, true),
            t0() + mins(1),
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].title, "2 Claude Code sessions finished");
    }

    #[test]
    fn needs_input_and_finished_are_independent_kinds() {
        let mut e = Engine::new();
        let n = alerts(&mut e, &[waits("x"), done(300)], &act_cfg(true, true), t0());
        assert_eq!(n.len(), 2);
    }

    #[test]
    fn a_flapping_session_cannot_spam() {
        let mut e = Engine::new();
        let cfg = act_cfg(true, false);
        assert_eq!(alerts(&mut e, &[waits("x")], &cfg, t0()).len(), 1);
        assert!(alerts(&mut e, &[waits("x")], &cfg, t0() + Duration::seconds(10)).is_empty());
        assert_eq!(
            alerts(&mut e, &[waits("x")], &cfg, t0() + Duration::seconds(25)).len(),
            1
        );
    }

    #[test]
    fn run_time_formatting() {
        let d = std::time::Duration::from_secs;
        assert_eq!(format_run(d(5)), "5s");
        assert_eq!(format_run(d(65)), "1m 05s");
        assert_eq!(format_run(d(3_780)), "1h 03m");
    }
}
