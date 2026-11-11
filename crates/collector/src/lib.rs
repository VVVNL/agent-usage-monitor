//! Background polling service.
//!
//! One task per provider, so a slow, failing or panicking provider cannot
//! affect another. State is published through a `watch` channel; consumers
//! (tray, widget, notifier) wake only when something they display changed.

pub mod activity;

use std::sync::Arc;
use std::time::Duration;

use aum_core::{Provider, ProviderError, ProviderId, ProviderState};
use chrono::Utc;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct CollectorConfig {
    /// Normal poll interval. Conservative default: 5 minutes.
    pub interval: Duration,
    /// Ceiling for failure backoff.
    pub max_backoff: Duration,
    /// Hard limit for one fetch, regardless of what the adapter does.
    pub fetch_timeout: Duration,
    /// Minimum gap between fetches, including manual refreshes.
    pub min_gap: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(300),
            max_backoff: Duration::from_secs(1800),
            fetch_timeout: Duration::from_secs(45),
            min_gap: Duration::from_secs(5),
        }
    }
}

impl CollectorConfig {
    /// Shortest interval a user may configure.
    pub const MIN_INTERVAL: Duration = Duration::from_secs(60);

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval.max(Self::MIN_INTERVAL);
        self
    }
}

/// Delay before the next scheduled fetch: the normal interval after a
/// success, otherwise exponential backoff capped at `max_backoff`. A
/// provider-supplied `retry_after` is honoured if longer.
pub fn next_delay(cfg: &CollectorConfig, failures: u32, retry_after: Option<Duration>) -> Duration {
    let base = if failures == 0 {
        cfg.interval
    } else {
        let factor = 1u32 << failures.min(10);
        cfg.interval
            .saturating_mul(factor)
            .min(cfg.max_backoff.max(cfg.interval))
    };
    retry_after.map_or(base, |r| base.max(r))
}

pub type States = Vec<ProviderState>;

pub struct Collector {
    rx: watch::Receiver<States>,
    refresh: Vec<Arc<Notify>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Collector {
    /// Start polling. `initial` seeds state from the on-disk cache so a
    /// restart shows last-known (stale-flagged) values immediately.
    pub fn start(
        providers: Vec<Arc<dyn Provider>>,
        cfg: CollectorConfig,
        initial: Vec<ProviderState>,
    ) -> Self {
        let states: States = providers
            .iter()
            .map(|p| {
                initial
                    .iter()
                    .find(|s| s.provider == p.id())
                    .cloned()
                    .unwrap_or_else(|| ProviderState::new(p.id()))
            })
            .collect();
        let (tx, rx) = watch::channel(states);
        let tx = Arc::new(tx);

        let mut refresh = Vec::new();
        let mut tasks = Vec::new();
        for (index, provider) in providers.into_iter().enumerate() {
            let notify = Arc::new(Notify::new());
            refresh.push(notify.clone());
            tasks.push(tokio::spawn(poll_loop(
                index,
                provider,
                cfg,
                tx.clone(),
                notify,
            )));
        }
        Self { rx, refresh, tasks }
    }

    pub fn subscribe(&self) -> watch::Receiver<States> {
        self.rx.clone()
    }

    pub fn snapshot(&self) -> States {
        self.rx.borrow().clone()
    }

    /// Ask every provider to refresh. Never blocks; requests made during a
    /// fetch coalesce into one follow-up fetch, spaced by `min_gap`.
    pub fn refresh_now(&self) {
        self.refresh.iter().for_each(|n| n.notify_one());
    }

    pub fn refresh_provider(&self, id: ProviderId) {
        if let Some(i) = self.rx.borrow().iter().position(|s| s.provider == id) {
            self.refresh[i].notify_one();
        }
    }

    /// Stop polling. In-flight CLI processes are killed (kill_on_drop).
    pub async fn shutdown(self) {
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

async fn poll_loop(
    index: usize,
    provider: Arc<dyn Provider>,
    cfg: CollectorConfig,
    tx: Arc<watch::Sender<States>>,
    refresh: Arc<Notify>,
) {
    let mut last_attempt: Option<Instant> = None;
    loop {
        // Space fetches by min_gap (guards against refresh spam).
        if let Some(t) = last_attempt {
            tokio::time::sleep_until(t + cfg.min_gap).await;
        }
        last_attempt = Some(Instant::now());

        // Fetch in its own task so a panic is contained, and under a hard timeout.
        let p = provider.clone();
        let joined = tokio::time::timeout(
            cfg.fetch_timeout,
            tokio::spawn(async move { p.fetch().await }),
        )
        .await;
        let result = match joined {
            Err(_) => Err(ProviderError::Timeout),
            Ok(Err(join_err)) => Err(ProviderError::Cli(format!(
                "provider task failed: {}",
                if join_err.is_panic() {
                    "panic"
                } else {
                    "cancelled"
                }
            ))),
            Ok(Ok(r)) => r,
        };

        let mut retry_after = None;
        let failures = {
            let mut failures = 0;
            tx.send_if_modified(|states| {
                let st = &mut states[index];
                let before = (
                    st.status.clone(),
                    st.snapshot.as_ref().map(|s| s.windows.clone()),
                    st.consecutive_failures,
                );
                st.last_attempt_at = Some(Utc::now());
                match result {
                    Ok(snap) => {
                        st.status = aum_core::ConnectionStatus::Connected;
                        st.consecutive_failures = 0;
                        st.snapshot = Some(snap);
                    }
                    Err(e) => {
                        // Keep the last good snapshot; it will be flagged stale.
                        if let ProviderError::RateLimited { retry_after: r } = &e {
                            retry_after = *r;
                        }
                        st.status = e.status();
                        st.consecutive_failures = st.consecutive_failures.saturating_add(1);
                    }
                }
                failures = st.consecutive_failures;
                let after = (
                    st.status.clone(),
                    st.snapshot.as_ref().map(|s| s.windows.clone()),
                    st.consecutive_failures,
                );
                // Always notify on a fresh success so "last updated" can move;
                // otherwise only when displayed data changed.
                before != after || failures == 0
            });
            failures
        };

        let delay = next_delay(&cfg, failures, retry_after);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = refresh.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use aum_core::{Capabilities, ConnectionStatus, ProviderSnapshot};

    fn snap(provider: ProviderId) -> ProviderSnapshot {
        ProviderSnapshot {
            provider,
            windows: vec![],
            plan: None,
            fetched_at: Utc::now(),
            unrecognized: vec![],
        }
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Ok,
        Fail,
        Panic,
        Hang,
        FailThenOk(u32),
    }

    struct Fake {
        id: ProviderId,
        mode: Mode,
        calls: Arc<AtomicU32>,
    }

    impl Fake {
        fn make(mode: Mode) -> (Arc<dyn Provider>, Arc<AtomicU32>) {
            let calls = Arc::new(AtomicU32::new(0));
            (
                Arc::new(Fake {
                    id: ProviderId::Claude,
                    mode,
                    calls: calls.clone(),
                }),
                calls,
            )
        }
    }

    #[async_trait]
    impl Provider for Fake {
        fn id(&self) -> ProviderId {
            self.id
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                session_window: true,
                weekly_window: true,
                model_weekly_windows: false,
            }
        }
        async fn fetch(&self) -> Result<ProviderSnapshot, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            match self.mode {
                Mode::Ok => Ok(snap(self.id)),
                Mode::Fail => Err(ProviderError::Offline),
                Mode::Panic => panic!("adapter bug"),
                Mode::Hang => {
                    tokio::time::sleep(Duration::from_secs(100_000)).await;
                    Ok(snap(self.id))
                }
                Mode::FailThenOk(k) => {
                    if n < k {
                        Err(ProviderError::AuthExpired)
                    } else {
                        Ok(snap(self.id))
                    }
                }
            }
        }
    }

    const IV: Duration = Duration::from_secs(300);

    #[test]
    fn backoff_schedule() {
        let c = CollectorConfig::default();
        assert_eq!(next_delay(&c, 0, None), IV);
        assert_eq!(next_delay(&c, 1, None), Duration::from_secs(600));
        assert_eq!(next_delay(&c, 2, None), Duration::from_secs(1200));
        assert_eq!(next_delay(&c, 3, None), c.max_backoff);
        assert_eq!(next_delay(&c, 50, None), c.max_backoff);
        // retry_after wins when longer, never shortens.
        assert_eq!(
            next_delay(&c, 0, Some(Duration::from_secs(900))),
            Duration::from_secs(900)
        );
        assert_eq!(
            next_delay(&c, 1, Some(Duration::from_secs(10))),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn interval_has_a_floor() {
        let c = CollectorConfig::default().with_interval(Duration::from_secs(1));
        assert_eq!(c.interval, CollectorConfig::MIN_INTERVAL);
    }

    async fn settle() {
        // Let spawned tasks run under paused time.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn polls_on_interval_and_publishes_state() {
        let (p, calls) = Fake::make(Mode::Ok);
        let c = Collector::start(vec![p], CollectorConfig::default(), vec![]);
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(c.snapshot()[0].status, ConnectionStatus::Connected);
        tokio::time::sleep(IV + Duration::from_secs(1)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        c.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn failures_back_off_and_recover() {
        let (p, calls) = Fake::make(Mode::FailThenOk(2));
        let c = Collector::start(vec![p], CollectorConfig::default(), vec![]);
        settle().await;
        assert_eq!(c.snapshot()[0].status, ConnectionStatus::AuthExpired);
        assert_eq!(c.snapshot()[0].consecutive_failures, 1);
        tokio::time::sleep(Duration::from_secs(301)).await; // < 600s backoff
        assert_eq!(calls.load(Ordering::SeqCst), 1, "must still be backing off");
        tokio::time::sleep(Duration::from_secs(300)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tokio::time::sleep(Duration::from_secs(1300)).await;
        assert_eq!(c.snapshot()[0].status, ConnectionStatus::Connected);
        assert_eq!(c.snapshot()[0].consecutive_failures, 0);
        c.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn failure_keeps_last_good_snapshot() {
        let (p, _) = Fake::make(Mode::Fail);
        let seeded = ProviderState {
            snapshot: Some(snap(ProviderId::Claude)),
            ..ProviderState::new(ProviderId::Claude)
        };
        let c = Collector::start(vec![p], CollectorConfig::default(), vec![seeded]);
        settle().await;
        let st = &c.snapshot()[0];
        assert_eq!(st.status, ConnectionStatus::Offline);
        assert!(
            st.snapshot.is_some(),
            "cached data survives an offline fetch"
        );
        c.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn panic_and_hang_are_contained() {
        let (bad, _) = Fake::make(Mode::Panic);
        let (hung, _) = Fake::make(Mode::Hang);
        let c = Collector::start(vec![bad, hung], CollectorConfig::default(), vec![]);
        settle().await;
        assert!(matches!(c.snapshot()[0].status, ConnectionStatus::Error(_)));
        tokio::time::sleep(Duration::from_secs(46)).await;
        assert!(
            matches!(c.snapshot()[1].status, ConnectionStatus::Error(ref m) if m.contains("timed out"))
        );
        c.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn one_provider_failing_does_not_affect_the_other() {
        struct Codex(Arc<AtomicU32>);
        #[async_trait]
        impl Provider for Codex {
            fn id(&self) -> ProviderId {
                ProviderId::Claude
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    session_window: true,
                    weekly_window: true,
                    model_weekly_windows: false,
                }
            }
            async fn fetch(&self) -> Result<ProviderSnapshot, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(snap(ProviderId::Claude))
            }
        }
        let (bad, _) = Fake::make(Mode::Panic);
        let n = Arc::new(AtomicU32::new(0));
        let good: Arc<dyn Provider> = Arc::new(Codex(n.clone()));
        let c = Collector::start(vec![bad, good], CollectorConfig::default(), vec![]);
        settle().await;
        assert!(matches!(c.snapshot()[0].status, ConnectionStatus::Error(_)));
        assert_eq!(c.snapshot()[1].status, ConnectionStatus::Connected);
        c.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn manual_refresh_works_but_is_rate_limited() {
        let (p, calls) = Fake::make(Mode::Ok);
        let c = Collector::start(vec![p], CollectorConfig::default(), vec![]);
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for _ in 0..50 {
            c.refresh_now(); // spam
        }
        settle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "min_gap holds back an immediate refetch"
        );
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "spam coalesces into one fetch"
        );
        c.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn changed_failure_counter_wakes_subscribers() {
        let (p, _) = Fake::make(Mode::Fail);
        let c = Collector::start(vec![p], CollectorConfig::default(), vec![]);
        let mut rx = c.subscribe();
        settle().await;
        rx.borrow_and_update();
        // Backoff makes the next fetch far away; once it fails identically
        // except for the failure counter, a notification is expected only
        // because the counter changed (a displayed value).
        tokio::time::sleep(Duration::from_secs(601)).await;
        assert!(rx.has_changed().unwrap());
        c.shutdown().await;
    }
}
