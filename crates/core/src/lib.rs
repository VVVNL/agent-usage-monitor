//! Domain models shared by every crate. No I/O lives here.
//!
//! Design rule: a value is either present with a [`Provenance`], or it is
//! absent. Nothing is ever invented to fill a gap.

pub mod forecast;

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a metric came from. Estimated or measured values must never be
/// presented as an official subscription quota percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provenance {
    /// Reported by the provider itself (e.g. the CLI's own `/usage`).
    Official,
    /// Counted locally from data on this machine.
    Measured,
    /// Derived or extrapolated.
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ProviderId {
    Claude,
    Codex,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude Code",
            ProviderId::Codex => "Codex CLI",
        }
    }
}

/// Which limit a window describes. Providers may expose any combination.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WindowKind {
    /// Short rolling window (Claude: the 5-hour "session").
    Session,
    /// Weekly limit across all models.
    Weekly,
    /// Weekly limit for one named model family.
    WeeklyModel(String),
    /// A limit window with its own label, for windows that are neither the short
    /// rolling window nor the weekly one (for example an unusual duration, or a
    /// separate metered bucket).
    Named(String),
}

impl WindowKind {
    pub fn label(&self) -> String {
        match self {
            WindowKind::Session => "Session".to_owned(),
            WindowKind::Weekly => "Weekly".to_owned(),
            WindowKind::WeeklyModel(m) => format!("Weekly ({m})"),
            WindowKind::Named(n) => n.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub kind: WindowKind,
    /// 0..=100. Values above 100 from a provider are clamped.
    pub used_percent: u8,
    pub resets_at: DateTime<Utc>,
    pub provenance: Provenance,
}

impl UsageWindow {
    pub fn new(
        kind: WindowKind,
        used_percent: u32,
        resets_at: DateTime<Utc>,
        provenance: Provenance,
    ) -> Self {
        Self {
            kind,
            used_percent: used_percent.min(100) as u8,
            resets_at,
            provenance,
        }
    }

    pub fn remaining_percent(&self) -> u8 {
        100 - self.used_percent
    }

    pub fn is_exhausted(&self) -> bool {
        self.used_percent >= 100
    }

    /// Time until reset, or `None` if the reset time has already passed
    /// (the value is then out of date and must not be shown as current).
    pub fn time_to_reset(&self, now: DateTime<Utc>) -> Option<Duration> {
        (self.resets_at - now)
            .to_std()
            .ok()
            .filter(|d| !d.is_zero())
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.resets_at <= now
    }
}

/// What a provider can report. The UI shows "unavailable" for anything a
/// provider does not declare, instead of a made-up value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub session_window: bool,
    pub weekly_window: bool,
    pub model_weekly_windows: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub provider: ProviderId,
    pub windows: Vec<UsageWindow>,
    /// Plan name if the provider reports one (e.g. "team").
    pub plan: Option<String>,
    pub fetched_at: DateTime<Utc>,
    /// Lines the parser did not recognise. Non-empty means the provider's
    /// output format may have changed.
    pub unrecognized: Vec<String>,
}

impl ProviderSnapshot {
    pub fn window(&self, kind: &WindowKind) -> Option<&UsageWindow> {
        self.windows.iter().find(|w| &w.kind == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    /// No fetch attempted yet.
    Unknown,
    Connected,
    NotLoggedIn,
    AuthExpired,
    RateLimited,
    Offline,
    /// The account or plan does not expose this data.
    Unsupported(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("not logged in")]
    NotLoggedIn,
    #[error("login expired")]
    AuthExpired,
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("offline")]
    Offline,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("could not parse provider output: {0}")]
    Parse(String),
    #[error("CLI error: {0}")]
    Cli(String),
    #[error("timed out")]
    Timeout,
}

impl ProviderError {
    pub fn status(&self) -> ConnectionStatus {
        match self {
            ProviderError::NotLoggedIn => ConnectionStatus::NotLoggedIn,
            ProviderError::AuthExpired => ConnectionStatus::AuthExpired,
            ProviderError::RateLimited { .. } => ConnectionStatus::RateLimited,
            ProviderError::Offline => ConnectionStatus::Offline,
            ProviderError::Unsupported(m) => ConnectionStatus::Unsupported(m.clone()),
            other => ConnectionStatus::Error(other.to_string()),
        }
    }
}

/// A source of usage data. Each adapter owns retrieval, parsing and
/// provider-specific error handling.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> Capabilities;
    async fn fetch(&self) -> Result<ProviderSnapshot, ProviderError>;
}

/// Last known state of one provider, as consumed by the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderState {
    pub provider: ProviderId,
    pub status: ConnectionStatus,
    /// Last *successful* snapshot, kept while later fetches fail.
    pub snapshot: Option<ProviderSnapshot>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
}

impl ProviderState {
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            status: ConnectionStatus::Unknown,
            snapshot: None,
            last_attempt_at: None,
            consecutive_failures: 0,
        }
    }

    /// Data is stale when the last good snapshot is older than
    /// `2 * poll_interval`, or there is none.
    pub fn is_stale(&self, now: DateTime<Utc>, poll_interval: Duration) -> bool {
        match &self.snapshot {
            None => true,
            Some(s) => (now - s.fetched_at)
                .to_std()
                .map(|age| age > poll_interval * 2)
                .unwrap_or(false),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Session activity: which sessions are running or waiting right now. A separate concept from
// quota, with its own polling schedule and its own failure state.
// ---------------------------------------------------------------------------------------------

/// What one session is doing. `Unknown` is for statuses we do not recognise: it must never be
/// treated as idle (the provider's interface is a preview and may add values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Working,
    /// Blocked on the user. The text says why ("permission prompt", "input needed", ...).
    NeedsInput(String),
    Idle,
    Unknown,
}

/// One live session. Deliberately carries no project path, title or conversation text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionActivity {
    /// Stable for the life of the session and unique across environments: `source:pid:started`.
    pub key: String,
    pub state: SessionState,
    pub started_at: Option<DateTime<Utc>>,
    /// Where it runs, e.g. "windows" or "WSL Ubuntu". Shown only in breakdowns.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    pub provider: ProviderId,
    pub sessions: Vec<SessionActivity>,
    pub observed_at: DateTime<Utc>,
    /// Environments that could not be read this time (e.g. "WSL Ubuntu: timed out"). Sessions from
    /// the others are still shown, so a partial result is never mistaken for a complete one.
    pub problems: Vec<String>,
}

impl ActivitySnapshot {
    pub fn count(&self, f: impl Fn(&SessionState) -> bool) -> usize {
        self.sessions.iter().filter(|s| f(&s.state)).count()
    }
    pub fn working(&self) -> usize {
        self.count(|s| matches!(s, SessionState::Working))
    }
    pub fn needs_input(&self) -> usize {
        self.count(|s| matches!(s, SessionState::NeedsInput(_)))
    }
    pub fn idle(&self) -> usize {
        self.count(|s| matches!(s, SessionState::Idle))
    }
    /// Something is running or waiting: poll faster.
    pub fn is_busy(&self) -> bool {
        self.sessions
            .iter()
            .any(|s| matches!(s.state, SessionState::Working | SessionState::NeedsInput(_)))
    }
}

/// Source of session activity (parallel to [`Provider`], which reports quota).
#[async_trait::async_trait]
pub trait ActivitySource: Send + Sync {
    fn id(&self) -> ProviderId;
    async fn observe(&self) -> Result<ActivitySnapshot, ProviderError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityHealth {
    Unknown,
    Ok,
    /// Activity cannot be read (CLI missing, preview interface changed, ...). The last snapshot is
    /// dropped rather than shown stale: an out-of-date "2 working" is worse than nothing.
    Unavailable(String),
}

/// Last known activity of one provider, as consumed by the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityState {
    pub provider: ProviderId,
    pub health: ActivityHealth,
    pub snapshot: Option<ActivitySnapshot>,
    pub consecutive_failures: u32,
}

impl ActivityState {
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            health: ActivityHealth::Unknown,
            snapshot: None,
            consecutive_failures: 0,
        }
    }
}

/// Keep provider-supplied short text safe to show: no control characters, bounded length.
pub fn sanitize_label(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Human countdown: "2h 14m", "3d 4h", "<1m".
pub fn format_countdown(d: Duration) -> String {
    let secs = d.as_secs();
    let (days, hours, mins) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        "<1m".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, h, m, 0).unwrap()
    }

    #[test]
    fn named_window_uses_its_own_label() {
        assert_eq!(
            WindowKind::Named("Spark 5-hour".into()).label(),
            "Spark 5-hour"
        );
        assert_eq!(ProviderId::Codex.display_name(), "Codex CLI");
        assert_eq!(ProviderId::Codex.as_str(), "codex");
    }

    #[test]
    fn percent_is_clamped() {
        let w = UsageWindow::new(WindowKind::Session, 250, t(12, 0), Provenance::Official);
        assert_eq!(w.used_percent, 100);
        assert!(w.is_exhausted());
        assert_eq!(w.remaining_percent(), 0);
    }

    #[test]
    fn countdown_and_expiry() {
        let w = UsageWindow::new(WindowKind::Session, 10, t(16, 14), Provenance::Official);
        let d = w.time_to_reset(t(14, 0)).unwrap();
        assert_eq!(format_countdown(d), "2h 14m");
        assert!(w.time_to_reset(t(17, 0)).is_none());
        assert!(w.is_expired(t(16, 14)));
    }

    #[test]
    fn countdown_formats() {
        assert_eq!(format_countdown(Duration::from_secs(30)), "<1m");
        assert_eq!(format_countdown(Duration::from_secs(5 * 60)), "5m");
        assert_eq!(
            format_countdown(Duration::from_secs(3 * 86_400 + 4 * 3_600)),
            "3d 4h"
        );
    }

    #[test]
    fn stale_detection() {
        let mut st = ProviderState::new(ProviderId::Claude);
        let iv = Duration::from_secs(300);
        assert!(st.is_stale(t(12, 0), iv));
        st.snapshot = Some(ProviderSnapshot {
            provider: ProviderId::Claude,
            windows: vec![],
            plan: None,
            fetched_at: t(12, 0),
            unrecognized: vec![],
        });
        assert!(!st.is_stale(t(12, 9), iv));
        assert!(st.is_stale(t(12, 11), iv));
    }

    #[test]
    fn error_maps_to_status() {
        assert_eq!(
            ProviderError::NotLoggedIn.status(),
            ConnectionStatus::NotLoggedIn
        );
        assert!(matches!(
            ProviderError::Timeout.status(),
            ConnectionStatus::Error(_)
        ));
    }

    fn sess(state: SessionState) -> SessionActivity {
        SessionActivity {
            key: "k".into(),
            state,
            started_at: None,
            source: "windows".into(),
        }
    }

    #[test]
    fn activity_counts_and_busy() {
        let snap = ActivitySnapshot {
            provider: ProviderId::Claude,
            sessions: vec![
                sess(SessionState::Working),
                sess(SessionState::Working),
                sess(SessionState::NeedsInput("permission prompt".into())),
                sess(SessionState::Idle),
                sess(SessionState::Unknown),
            ],
            observed_at: t(12, 0),
            problems: vec![],
        };
        assert_eq!((snap.working(), snap.needs_input(), snap.idle()), (2, 1, 1));
        assert!(snap.is_busy());
        let quiet = ActivitySnapshot {
            sessions: vec![sess(SessionState::Idle), sess(SessionState::Unknown)],
            ..snap
        };
        assert!(
            !quiet.is_busy(),
            "unknown must not count as busy, nor as idle"
        );
        assert_eq!(quiet.idle(), 1);
    }

    #[test]
    fn labels_are_sanitised() {
        assert_eq!(
            sanitize_label("permission\u{7} prompt\n", 40),
            "permission prompt"
        );
        assert_eq!(sanitize_label(&"x".repeat(100), 10).len(), 10);
    }
}
