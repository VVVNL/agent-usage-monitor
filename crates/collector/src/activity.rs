//! Polling service for session activity. Separate from the quota collector: different cadence
//! (fast while a session is busy, slow when quiet), different failure semantics (a failed read drops
//! the snapshot instead of showing it stale), same isolation (one task per source, hard timeout,
//! panics contained).

use std::sync::Arc;
use std::time::Duration;

use aum_core::{ActivityHealth, ActivitySource, ActivityState, ProviderError};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct ActivityConfig {
    /// Poll interval while a session is working or waiting.
    pub busy_interval: Duration,
    /// Poll interval when nothing is happening.
    pub quiet_interval: Duration,
    pub max_backoff: Duration,
    pub timeout: Duration,
    /// Minimum gap between polls, including manual refreshes.
    pub min_gap: Duration,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            // Each poll starts the CLI (about 0.14 s of CPU), so these floors matter.
            busy_interval: Duration::from_secs(15),
            quiet_interval: Duration::from_secs(60),
            max_backoff: Duration::from_secs(600),
            timeout: Duration::from_secs(45),
            min_gap: Duration::from_secs(5),
        }
    }
}

/// Delay before the next poll.
pub fn next_delay(cfg: &ActivityConfig, busy: bool, failures: u32) -> Duration {
    if failures > 0 {
        let factor = 1u32 << failures.min(10);
        return cfg
            .quiet_interval
            .saturating_mul(factor)
            .min(cfg.max_backoff.max(cfg.quiet_interval));
    }
    if busy {
        cfg.busy_interval
    } else {
        cfg.quiet_interval
    }
}

pub type States = Vec<ActivityState>;

pub struct ActivityService {
    rx: watch::Receiver<States>,
    refresh: Vec<Arc<Notify>>,
    tasks: Vec<JoinHandle<()>>,
}

impl ActivityService {
    pub fn start(sources: Vec<Arc<dyn ActivitySource>>, cfg: ActivityConfig) -> Self {
        let (tx, rx) = watch::channel(
            sources
                .iter()
                .map(|s| ActivityState::new(s.id()))
                .collect::<States>(),
        );
        let tx = Arc::new(tx);
        let mut refresh = Vec::new();
        let mut tasks = Vec::new();
        for (index, source) in sources.into_iter().enumerate() {
            let notify = Arc::new(Notify::new());
            refresh.push(notify.clone());
            tasks.push(tokio::spawn(poll_loop(
                index,
                source,
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

    /// Poll now (subject to `min_gap`). Never blocks.
    pub fn refresh_now(&self) {
        self.refresh.iter().for_each(|n| n.notify_one());
    }

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
    source: Arc<dyn ActivitySource>,
    cfg: ActivityConfig,
    tx: Arc<watch::Sender<States>>,
    refresh: Arc<Notify>,
) {
    let mut last: Option<Instant> = None;
    loop {
        if let Some(t) = last {
            tokio::time::sleep_until(t + cfg.min_gap).await;
        }
        last = Some(Instant::now());

        let s = source.clone();
        let result =
            match tokio::time::timeout(cfg.timeout, tokio::spawn(async move { s.observe().await }))
                .await
            {
                Err(_) => Err(ProviderError::Timeout),
                Ok(Err(join)) => Err(ProviderError::Cli(format!(
                    "activity task failed: {}",
                    if join.is_panic() {
                        "panic"
                    } else {
                        "cancelled"
                    }
                ))),
                Ok(Ok(r)) => r,
            };

        let (mut busy, mut failures) = (false, 0);
        tx.send_if_modified(|states| {
            let st = &mut states[index];
            let before = (
                st.health.clone(),
                st.snapshot
                    .as_ref()
                    .map(|s| (s.sessions.clone(), s.problems.clone())),
            );
            match result {
                Ok(snap) => {
                    busy = snap.is_busy();
                    st.health = ActivityHealth::Ok;
                    st.consecutive_failures = 0;
                    st.snapshot = Some(snap);
                }
                Err(e) => {
                    st.health = ActivityHealth::Unavailable(e.to_string());
                    st.consecutive_failures = st.consecutive_failures.saturating_add(1);
                    st.snapshot = None; // never show a stale "N working"
                }
            }
            failures = st.consecutive_failures;
            let after = (
                st.health.clone(),
                st.snapshot
                    .as_ref()
                    .map(|s| (s.sessions.clone(), s.problems.clone())),
            );
            before != after
        });

        let delay = next_delay(&cfg, busy, failures);
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
    use aum_core::{ActivitySnapshot, ProviderId, SessionActivity, SessionState};
    use chrono::Utc;

    fn snap(working: usize) -> ActivitySnapshot {
        ActivitySnapshot {
            provider: ProviderId::Claude,
            sessions: (0..working)
                .map(|i| SessionActivity {
                    key: format!("k{i}"),
                    state: SessionState::Working,
                    started_at: None,
                    source: "windows".into(),
                })
                .collect(),
            observed_at: Utc::now(),
            problems: vec![],
        }
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Quiet,
        Busy,
        Fail,
        Panic,
        Hang,
        FailThenBusy(u32),
    }

    struct Fake(Mode, Arc<AtomicU32>);

    #[async_trait]
    impl ActivitySource for Fake {
        fn id(&self) -> ProviderId {
            ProviderId::Claude
        }
        async fn observe(&self) -> Result<ActivitySnapshot, ProviderError> {
            let n = self.1.fetch_add(1, Ordering::SeqCst);
            match self.0 {
                Mode::Quiet => Ok(snap(0)),
                Mode::Busy => Ok(snap(2)),
                Mode::Fail => Err(ProviderError::Cli("boom".into())),
                Mode::Panic => panic!("source bug"),
                Mode::Hang => {
                    tokio::time::sleep(Duration::from_secs(100_000)).await;
                    Ok(snap(0))
                }
                Mode::FailThenBusy(k) => {
                    if n < k {
                        Err(ProviderError::Timeout)
                    } else {
                        Ok(snap(1))
                    }
                }
            }
        }
    }

    fn start(mode: Mode) -> (ActivityService, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let svc = ActivityService::start(
            vec![Arc::new(Fake(mode, calls.clone()))],
            ActivityConfig::default(),
        );
        (svc, calls)
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[test]
    fn schedule() {
        let c = ActivityConfig::default();
        assert_eq!(next_delay(&c, true, 0), Duration::from_secs(15));
        assert_eq!(next_delay(&c, false, 0), Duration::from_secs(60));
        assert_eq!(
            next_delay(&c, true, 1),
            Duration::from_secs(120),
            "errors back off even if it was busy"
        );
        assert_eq!(next_delay(&c, false, 3), Duration::from_secs(480));
        assert_eq!(next_delay(&c, false, 20), c.max_backoff);
    }

    #[tokio::test(start_paused = true)]
    async fn polls_fast_while_busy_and_slow_when_quiet() {
        let (busy, b) = start(Mode::Busy);
        let (quiet, q) = start(Mode::Quiet);
        settle().await;
        assert_eq!(busy.snapshot()[0].snapshot.as_ref().unwrap().working(), 2);
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(b.load(Ordering::SeqCst), 5, "initial + one every 15 s");
        assert_eq!(q.load(Ordering::SeqCst), 2, "initial + one after 60 s");
        busy.shutdown().await;
        quiet.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn failure_drops_the_snapshot_instead_of_showing_it_stale() {
        let (svc, _) = start(Mode::FailThenBusy(1));
        settle().await;
        let st = &svc.snapshot()[0];
        assert!(matches!(st.health, ActivityHealth::Unavailable(_)));
        assert!(st.snapshot.is_none());
        tokio::time::sleep(Duration::from_secs(125)).await; // first retry after the 120 s backoff
        let st = &svc.snapshot()[0];
        assert_eq!(st.health, ActivityHealth::Ok);
        assert_eq!(st.snapshot.as_ref().unwrap().working(), 1);
        assert_eq!(st.consecutive_failures, 0);
        svc.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_source_shows_no_numbers() {
        let (svc, _) = start(Mode::Fail);
        settle().await;
        assert!(svc.snapshot()[0].snapshot.is_none());
        svc.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn panics_and_hangs_are_contained() {
        let (p, _) = start(Mode::Panic);
        let (h, _) = start(Mode::Hang);
        settle().await;
        assert!(
            matches!(&p.snapshot()[0].health, ActivityHealth::Unavailable(m) if m.contains("panic"))
        );
        tokio::time::sleep(Duration::from_secs(46)).await;
        assert!(
            matches!(&h.snapshot()[0].health, ActivityHealth::Unavailable(m) if m.contains("timed out"))
        );
        p.shutdown().await;
        h.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn manual_refresh_is_rate_limited_and_coalesced() {
        let (svc, calls) = start(Mode::Quiet);
        settle().await;
        for _ in 0..40 {
            svc.refresh_now();
        }
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        svc.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn identical_polls_do_not_wake_subscribers() {
        let (svc, _) = start(Mode::Busy);
        let mut rx = svc.subscribe();
        settle().await;
        rx.borrow_and_update();
        tokio::time::sleep(Duration::from_secs(50)).await; // several identical polls
        assert!(
            !rx.has_changed().unwrap(),
            "unchanged activity must not repaint anything"
        );
        svc.shutdown().await;
    }
}
