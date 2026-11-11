//! Codex CLI adapter.
//!
//! Source: the CLI's own `codex app-server` (JSON-RPC over stdio, one JSON object per line), so we
//! never read or handle Codex credentials: Codex reads its own login.
//!
//! Conversation, per poll (the process is started and stopped each time):
//! 1. `initialize` (request) then `initialized` (notification), as the protocol requires
//! 2. `account/read`            -> signed out / API key / ChatGPT plan
//! 3. `account/rateLimits/read` -> the limit windows (see [`parse`])
//!
//! `app-server` is labelled experimental by its authors, and the documented caveat that asking for
//! limits immediately after the handshake can return no windows is handled with one delayed retry.
//!
//! **Verification status:** field names come from Codex's published JSON schema and the tests use
//! schema-derived fixtures. This adapter has not yet been run against a real Codex.

pub mod parse;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aum_core::{Capabilities, Provider, ProviderError, ProviderId, ProviderSnapshot};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::runner::{RpcSession, RpcSpawner, RunError, SystemRunner, resolve_cli};

/// Upper bound for the whole conversation, including the retry.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause before asking again when the first answer contained no windows.
const RETRY_DELAY: Duration = Duration::from_millis(1500);

pub struct CodexProvider {
    cli: PathBuf,
    spawner: Arc<dyn RpcSpawner>,
    now: fn() -> DateTime<Utc>,
    retry_delay: Duration,
}

fn map_run(e: RunError) -> ProviderError {
    match e {
        RunError::NotFound(_) => ProviderError::Cli(
            "Codex CLI not found (install it or set its path in settings)".to_owned(),
        ),
        RunError::Timeout => ProviderError::Timeout,
        RunError::Io(m) => ProviderError::Cli(m),
    }
}

/// Turn a JSON-RPC `error` object into the closest [`ProviderError`].
fn rpc_error(err: &Value) -> ProviderError {
    let msg = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    let lower = msg.to_lowercase();
    if lower.contains("expired") {
        ProviderError::AuthExpired
    } else if lower.contains("not logged in")
        || lower.contains("not signed in")
        || lower.contains("sign in")
        || lower.contains("login")
        || lower.contains("unauthorized")
        || lower.contains("authentication")
    {
        ProviderError::NotLoggedIn
    } else {
        ProviderError::Cli(msg.chars().take(120).collect())
    }
}

impl CodexProvider {
    /// `custom_cli` is the optional user-configured path to the CLI.
    pub fn new(custom_cli: Option<PathBuf>) -> Self {
        Self::with_spawner(
            resolve_cli("codex", custom_cli.as_deref()),
            Arc::new(SystemRunner),
        )
    }

    pub fn with_spawner(cli: PathBuf, spawner: Arc<dyn RpcSpawner>) -> Self {
        Self {
            cli,
            spawner,
            now: Utc::now,
            retry_delay: RETRY_DELAY,
        }
    }

    #[cfg(test)]
    fn for_test(mut self) -> Self {
        self.retry_delay = Duration::ZERO;
        self
    }

    /// Send a request and wait for the response with the same id. Notifications and any
    /// non-JSON output (log lines) in between are skipped.
    async fn request(
        session: &mut dyn RpcSession,
        id: i64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, ProviderError> {
        let mut msg = json!({ "method": method, "id": id });
        if let Some(p) = params {
            msg["params"] = p;
        }
        session.send(&msg.to_string()).await.map_err(map_run)?;
        loop {
            let line = session.recv(OVERALL_TIMEOUT).await.map_err(map_run)?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_i64) != Some(id) {
                continue; // a notification or someone else's response
            }
            if let Some(err) = v.get("error") {
                return Err(rpc_error(err));
            }
            return v
                .get("result")
                .cloned()
                .ok_or_else(|| ProviderError::Parse("response has no result".to_owned()));
        }
    }

    async fn converse(
        &self,
        session: &mut dyn RpcSession,
        now: DateTime<Utc>,
    ) -> Result<ProviderSnapshot, ProviderError> {
        Self::request(
            session,
            0,
            "initialize",
            Some(json!({ "clientInfo": {
                "name": "agent_usage_monitor",
                "title": "Agent Usage Monitor",
                "version": env!("CARGO_PKG_VERSION"),
            }})),
        )
        .await?;
        session
            .send(&json!({ "method": "initialized", "params": {} }).to_string())
            .await
            .map_err(map_run)?;

        let account = Self::request(
            session,
            1,
            "account/read",
            Some(json!({ "refreshToken": false })),
        )
        .await?;
        let account_plan = match parse::parse_account(&account)? {
            parse::Account::SignedOut => return Err(ProviderError::NotLoggedIn),
            parse::Account::ApiKey => {
                return Err(ProviderError::Unsupported(
                    "usage limits are only available when signed in with a ChatGPT account"
                        .to_owned(),
                ));
            }
            parse::Account::Other(kind) => {
                return Err(ProviderError::Unsupported(format!(
                    "signed in with '{kind}', which has no ChatGPT usage limits"
                )));
            }
            parse::Account::ChatGpt { plan } => plan,
        };

        let mut id = 2;
        let parsed = loop {
            let result = Self::request(session, id, "account/rateLimits/read", None).await?;
            match parse::parse_rate_limits(&result) {
                Ok(p) => break p,
                // Asked too early the server may answer with no windows: wait and ask once more.
                Err(ProviderError::Parse(m)) if id == 2 && m.contains("no rate-limit windows") => {
                    tokio::time::sleep(self.retry_delay).await;
                    id = 3;
                }
                Err(e) => return Err(e),
            }
        };

        Ok(ProviderSnapshot {
            provider: ProviderId::Codex,
            windows: parsed.windows,
            plan: parsed.plan.or(account_plan),
            fetched_at: now,
            unrecognized: parsed.unrecognized,
        })
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_window: true,
            weekly_window: true,
            model_weekly_windows: true,
        }
    }

    async fn fetch(&self) -> Result<ProviderSnapshot, ProviderError> {
        let now = (self.now)();
        let mut session = self
            .spawner
            .spawn(&self.cli, &["app-server"])
            .await
            .map_err(map_run)?;
        let result =
            match tokio::time::timeout(OVERALL_TIMEOUT, self.converse(&mut *session, now)).await {
                Ok(r) => r,
                Err(_) => Err(ProviderError::Timeout),
            };
        session.close().await; // always: ends the process and anything it started
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use aum_core::WindowKind;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/codex/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .trim()
        .to_owned()
    }

    type Handler = dyn Fn(&Value) -> Vec<String> + Send + Sync;

    /// A fake `codex app-server`: `handler` returns the lines it prints in reply to each message.
    struct FakeServer {
        handler: Arc<Handler>,
        sent: Arc<Mutex<Vec<String>>>,
        closed: Arc<AtomicBool>,
        spawn_error: Option<RunError>,
    }

    struct FakeSession {
        handler: Arc<Handler>,
        queue: VecDeque<String>,
        sent: Arc<Mutex<Vec<String>>>,
        closed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl RpcSession for FakeSession {
        async fn send(&mut self, line: &str) -> Result<(), RunError> {
            let v: Value = serde_json::from_str(line).expect("we only send valid JSON");
            self.sent
                .lock()
                .unwrap()
                .push(v["method"].as_str().unwrap_or("?").to_owned());
            self.queue.extend((self.handler)(&v));
            Ok(())
        }
        async fn recv(&mut self, _t: Duration) -> Result<String, RunError> {
            self.queue.pop_front().ok_or(RunError::Timeout) // a silent server
        }
        async fn close(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl RpcSpawner for FakeServer {
        async fn spawn(&self, _p: &Path, args: &[&str]) -> Result<Box<dyn RpcSession>, RunError> {
            assert_eq!(
                args,
                ["app-server"],
                "must start exactly `codex app-server`"
            );
            if let Some(e) = &self.spawn_error {
                return Err(e.clone());
            }
            Ok(Box::new(FakeSession {
                handler: self.handler.clone(),
                queue: VecDeque::new(),
                sent: self.sent.clone(),
                closed: self.closed.clone(),
            }))
        }
    }

    fn reply(id: &Value, result_json: &str) -> String {
        format!(r#"{{"id":{id},"result":{result_json}}}"#)
    }

    /// Well-behaved server: answers each request with the given account and rate-limit JSON.
    fn normal(account: String, limits: Vec<String>) -> Arc<Handler> {
        let calls = Mutex::new(0usize);
        Arc::new(move |v: &Value| match v["method"].as_str() {
            Some("initialize") => vec![
                r#"{"method":"some/notification","params":{}}"#.to_owned(), // noise before the response
                "WARN codex_core: not json at all".to_owned(),
                reply(
                    &v["id"],
                    r#"{"userAgent":"codex/0.166.0","platformFamily":"windows","platformOs":"windows"}"#,
                ),
            ],
            Some("account/read") => vec![reply(&v["id"], &account)],
            Some("account/rateLimits/read") => {
                let mut n = calls.lock().unwrap();
                let body = limits[(*n).min(limits.len() - 1)].clone();
                *n += 1;
                vec![reply(&v["id"], &body)]
            }
            _ => vec![],
        })
    }

    fn provider(
        handler: Arc<Handler>,
    ) -> (CodexProvider, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>) {
        let sent = Arc::new(Mutex::new(vec![]));
        let closed = Arc::new(AtomicBool::new(false));
        let server = FakeServer {
            handler,
            sent: sent.clone(),
            closed: closed.clone(),
            spawn_error: None,
        };
        (
            CodexProvider::with_spawner("codex".into(), Arc::new(server)).for_test(),
            sent,
            closed,
        )
    }

    #[tokio::test]
    async fn happy_path() {
        let (p, sent, closed) = provider(normal(
            fixture("account_chatgpt.json"),
            vec![fixture("rate_limits_normal.json")],
        ));
        let snap = p.fetch().await.unwrap();
        assert_eq!(snap.provider, ProviderId::Codex);
        assert_eq!(snap.windows.len(), 2);
        assert!(snap.window(&WindowKind::Session).is_some());
        assert_eq!(snap.plan.as_deref(), Some("plus"));
        // Exact conversation, in order: handshake, account, limits. Nothing else is ever sent.
        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                "initialize",
                "initialized",
                "account/read",
                "account/rateLimits/read"
            ]
        );
        assert!(
            closed.load(Ordering::SeqCst),
            "the process must always be closed"
        );
    }

    #[tokio::test]
    async fn signed_out_stops_before_asking_for_limits() {
        let (p, sent, closed) = provider(normal(
            fixture("account_signed_out.json"),
            vec![fixture("rate_limits_normal.json")],
        ));
        assert_eq!(p.fetch().await.unwrap_err(), ProviderError::NotLoggedIn);
        assert!(
            !sent
                .lock()
                .unwrap()
                .contains(&"account/rateLimits/read".to_owned())
        );
        assert!(closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn api_key_and_other_accounts_are_unsupported() {
        let (p, _, _) = provider(normal(
            fixture("account_apikey.json"),
            vec![fixture("rate_limits_normal.json")],
        ));
        assert!(
            matches!(p.fetch().await.unwrap_err(), ProviderError::Unsupported(m) if m.contains("ChatGPT"))
        );
        let (p, _, _) = provider(normal(
            r#"{"account":{"type":"gcp"},"requiresOpenaiAuth":false}"#.into(),
            vec![fixture("rate_limits_normal.json")],
        ));
        assert!(
            matches!(p.fetch().await.unwrap_err(), ProviderError::Unsupported(m) if m.contains("gcp"))
        );
    }

    #[tokio::test]
    async fn empty_first_answer_is_retried_once() {
        let (p, sent, _) = provider(normal(
            fixture("account_chatgpt.json"),
            vec![
                fixture("rate_limits_empty.json"),
                fixture("rate_limits_normal.json"),
            ],
        ));
        assert_eq!(p.fetch().await.unwrap().windows.len(), 2);
        let limit_calls = sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == "account/rateLimits/read")
            .count();
        assert_eq!(limit_calls, 2);
    }

    #[tokio::test]
    async fn persistently_empty_is_an_error_not_zero_percent() {
        let (p, sent, _) = provider(normal(
            fixture("account_chatgpt.json"),
            vec![fixture("rate_limits_empty.json")],
        ));
        assert!(matches!(
            p.fetch().await.unwrap_err(),
            ProviderError::Parse(_)
        ));
        let limit_calls = sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == "account/rateLimits/read")
            .count();
        assert_eq!(limit_calls, 2, "exactly one retry, never a loop");
    }

    #[tokio::test]
    async fn rpc_errors_map_to_useful_states() {
        for (msg, want) in [
            ("Not logged in", ProviderError::NotLoggedIn),
            (
                "token expired, please log in again",
                ProviderError::AuthExpired,
            ),
            (
                "backend exploded",
                ProviderError::Cli("backend exploded".into()),
            ),
        ] {
            let m = msg.to_owned();
            let handler: Arc<Handler> = Arc::new(move |v: &Value| {
                vec![format!(
                    r#"{{"id":{},"error":{{"code":-32000,"message":"{m}"}}}}"#,
                    v["id"]
                )]
            });
            let (p, _, closed) = provider(handler);
            assert_eq!(p.fetch().await.unwrap_err(), want, "{msg}");
            assert!(closed.load(Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn silent_server_times_out_and_is_closed() {
        let (p, _, closed) = provider(Arc::new(|_: &Value| vec![]));
        assert_eq!(p.fetch().await.unwrap_err(), ProviderError::Timeout);
        assert!(closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn missing_cli_and_spawn_failures() {
        let server = FakeServer {
            handler: Arc::new(|_: &Value| vec![]),
            sent: Arc::default(),
            closed: Arc::default(),
            spawn_error: Some(RunError::NotFound("codex".into())),
        };
        let p = CodexProvider::with_spawner("codex".into(), Arc::new(server));
        assert!(
            matches!(p.fetch().await.unwrap_err(), ProviderError::Cli(m) if m.contains("not found"))
        );
    }

    #[tokio::test]
    async fn the_account_email_never_reaches_the_snapshot() {
        let (p, _, _) = provider(normal(
            fixture("account_chatgpt.json"),
            vec![fixture("rate_limits_normal.json")],
        ));
        let snap = p.fetch().await.unwrap();
        assert!(!format!("{snap:?}").contains("example.com"));
    }

    #[tokio::test]
    async fn plan_falls_back_to_the_account_when_limits_omit_it() {
        let limits = r#"{"rateLimits":{"primary":{"usedPercent":1,"windowDurationMins":300,"resetsAt":1779459394}}}"#.to_owned();
        let (p, _, _) = provider(normal(fixture("account_chatgpt.json"), vec![limits]));
        assert_eq!(p.fetch().await.unwrap().plan.as_deref(), Some("plus"));
    }
}
