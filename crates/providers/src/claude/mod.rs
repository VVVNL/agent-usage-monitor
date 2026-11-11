//! Claude Code adapter.
//!
//! Source: the CLI's own commands, so we never read or handle credentials.
//! 1. `claude auth status`      -> login state and plan (structured JSON)
//! 2. `claude -p "/usage"`      -> official limit lines (text, see [`parse`])
//!
//! `/usage` makes no model call (verified: zero tokens, zero cost), so
//! polling it does not consume quota. `--no-session-persistence` is passed
//! because without it every poll writes a transcript file into
//! `~/.claude/projects/` (verified: ~4 KB per call).

pub mod activity;
pub mod parse;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aum_core::{Capabilities, Provider, ProviderError, ProviderId, ProviderSnapshot};
use chrono::Utc;

use crate::runner::{CmdOutput, CommandRunner, RunError, SystemRunner, resolve_cli};

const CLI_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ClaudeProvider {
    cli: PathBuf,
    runner: Arc<dyn CommandRunner>,
    now: fn() -> chrono::DateTime<Utc>,
}

impl ClaudeProvider {
    /// `custom_cli` is the optional user-configured path to the CLI.
    pub fn new(custom_cli: Option<PathBuf>) -> Self {
        Self::with_runner(
            resolve_cli("claude", custom_cli.as_deref()),
            Arc::new(SystemRunner),
        )
    }

    pub fn with_runner(cli: PathBuf, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            cli,
            runner,
            now: Utc::now,
        }
    }

    #[cfg(test)]
    fn with_clock(mut self, now: fn() -> chrono::DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    async fn run(&self, args: &[&str]) -> Result<CmdOutput, ProviderError> {
        self.runner
            .run(&self.cli, args, CLI_TIMEOUT)
            .await
            .map_err(|e| match e {
                RunError::NotFound(_) => ProviderError::Cli(
                    "Claude Code CLI not found (install it or set its path in settings)".to_owned(),
                ),
                RunError::Timeout => ProviderError::Timeout,
                RunError::Io(m) => ProviderError::Cli(m),
            })
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
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

        // 1. Auth. Stdout is parsed even on a non-zero exit, since a logged-out
        //    CLI may report `loggedIn: false` that way.
        let auth_out = self.run(&["auth", "status"]).await?;
        let auth = parse::parse_auth_status(&auth_out.stdout).map_err(|_| {
            ProviderError::Cli(format!(
                "`claude auth status` failed (exit {})",
                auth_out.code.map_or("?".to_owned(), |c| c.to_string())
            ))
        })?;
        if !auth.logged_in {
            return Err(ProviderError::NotLoggedIn);
        }
        if auth.login_expires_at.is_some_and(|t| t <= now) {
            return Err(ProviderError::AuthExpired);
        }
        if auth
            .auth_method
            .as_deref()
            .is_some_and(|m| m != "claude.ai")
        {
            return Err(ProviderError::Unsupported(
                "subscription limits are only available when signed in with a claude.ai account"
                    .to_owned(),
            ));
        }

        // 2. Limits.
        let out = self
            .run(&[
                "-p",
                "/usage",
                "--no-session-persistence",
                "--output-format",
                "text",
            ])
            .await?;
        if !out.success {
            let text = format!("{}{}", out.stdout, out.stderr).to_lowercase();
            return Err(if text.contains("enotfound") || text.contains("network") {
                ProviderError::Offline
            } else if text.contains("rate limit") || text.contains("429") {
                ProviderError::RateLimited { retry_after: None }
            } else {
                ProviderError::Cli(format!(
                    "`claude -p /usage` failed (exit {})",
                    out.code.map_or("?".to_owned(), |c| c.to_string())
                ))
            });
        }

        let parsed = parse::parse_usage(&out.stdout, now)?;
        Ok(ProviderSnapshot {
            provider: ProviderId::Claude,
            windows: parsed.windows,
            plan: auth.subscription_type,
            fetched_at: now,
            unrecognized: parsed.unrecognized,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    use aum_core::WindowKind;
    use chrono::TimeZone;

    const USAGE: &str =
        include_str!("../../../../tests/fixtures/claude/usage/text_normal_28pct.txt");
    const LOGGED_OUT: &str =
        include_str!("../../../../tests/fixtures/claude/usage/text_logged_out.txt");
    const AUTH: &str =
        include_str!("../../../../tests/fixtures/claude/usage/auth_status_team.json");

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 17, 0, 0).unwrap()
    }

    /// Replies are matched on the first argument ("auth" or "-p").
    struct Script {
        auth: Result<CmdOutput, RunError>,
        usage: Result<CmdOutput, RunError>,
        calls: Mutex<Vec<String>>,
    }

    fn ok(stdout: &str) -> Result<CmdOutput, RunError> {
        Ok(CmdOutput::from_text(true, Some(0), stdout, ""))
    }
    fn fail(code: i32, stderr: &str) -> Result<CmdOutput, RunError> {
        Ok(CmdOutput::from_text(false, Some(code), "", stderr))
    }

    #[async_trait]
    impl CommandRunner for Script {
        async fn run(&self, _p: &Path, args: &[&str], _t: Duration) -> Result<CmdOutput, RunError> {
            self.calls.lock().unwrap().push(args.join(" "));
            if args.first() == Some(&"auth") {
                self.auth.clone()
            } else {
                self.usage.clone()
            }
        }
    }

    fn provider(
        auth: Result<CmdOutput, RunError>,
        usage: Result<CmdOutput, RunError>,
    ) -> (ClaudeProvider, Arc<Script>) {
        let s = Arc::new(Script {
            auth,
            usage,
            calls: Mutex::new(vec![]),
        });
        (
            ClaudeProvider::with_runner("claude".into(), s.clone()).with_clock(now),
            s,
        )
    }

    #[tokio::test]
    async fn happy_path_reports_all_windows() {
        let auth = AUTH.replace("2026-12-09", "2027-01-01");
        let (p, s) = provider(ok(&auth), ok(USAGE));
        let snap = p.fetch().await.unwrap();
        assert_eq!(snap.plan.as_deref(), Some("team"));
        assert_eq!(snap.windows.len(), 3);
        assert!(snap.window(&WindowKind::Session).is_some());
        assert_eq!(snap.fetched_at, now());
        // Exactly the two intended commands, no shell, no extra flags.
        assert_eq!(
            *s.calls.lock().unwrap(),
            vec![
                "auth status",
                "-p /usage --no-session-persistence --output-format text"
            ]
        );
    }

    #[tokio::test]
    async fn not_logged_in() {
        let (p, s) = provider(ok(r#"{"loggedIn": false}"#), ok(LOGGED_OUT));
        assert_eq!(p.fetch().await.unwrap_err(), ProviderError::NotLoggedIn);
        assert_eq!(
            s.calls.lock().unwrap().len(),
            1,
            "must not run /usage when logged out"
        );
    }

    #[tokio::test]
    async fn expired_login() {
        let auth = AUTH.replace("2026-12-09", "2026-11-01");
        let (p, _) = provider(ok(&auth), ok(USAGE));
        assert_eq!(p.fetch().await.unwrap_err(), ProviderError::AuthExpired);
    }

    #[tokio::test]
    async fn api_key_account_is_unsupported() {
        let (p, _) = provider(
            ok(r#"{"loggedIn": true, "authMethod": "api_key"}"#),
            ok(USAGE),
        );
        assert!(matches!(
            p.fetch().await.unwrap_err(),
            ProviderError::Unsupported(_)
        ));
    }

    #[tokio::test]
    async fn logged_in_but_no_limit_lines_is_a_parse_error() {
        let (p, _) = provider(
            ok(r#"{"loggedIn": true, "authMethod": "claude.ai"}"#),
            ok(LOGGED_OUT),
        );
        assert!(matches!(
            p.fetch().await.unwrap_err(),
            ProviderError::Parse(_)
        ));
    }

    #[tokio::test]
    async fn cli_missing_timeout_and_garbage_auth() {
        let (p, _) = provider(Err(RunError::NotFound("claude".into())), ok(USAGE));
        assert!(
            matches!(p.fetch().await.unwrap_err(), ProviderError::Cli(m) if m.contains("not found"))
        );
        let (p, _) = provider(Err(RunError::Timeout), ok(USAGE));
        assert_eq!(p.fetch().await.unwrap_err(), ProviderError::Timeout);
        let (p, _) = provider(ok("<html>oops</html>"), ok(USAGE));
        assert!(matches!(
            p.fetch().await.unwrap_err(),
            ProviderError::Cli(_)
        ));
    }

    #[tokio::test]
    async fn offline_and_rate_limit_are_classified() {
        let good_auth = r#"{"loggedIn": true, "authMethod": "claude.ai"}"#;
        let (p, _) = provider(
            ok(good_auth),
            fail(1, "getaddrinfo ENOTFOUND api.anthropic.com"),
        );
        assert_eq!(p.fetch().await.unwrap_err(), ProviderError::Offline);
        let (p, _) = provider(ok(good_auth), fail(1, "429 rate limit exceeded"));
        assert!(matches!(
            p.fetch().await.unwrap_err(),
            ProviderError::RateLimited { .. }
        ));
        let (p, _) = provider(ok(good_auth), fail(2, "boom"));
        assert!(matches!(
            p.fetch().await.unwrap_err(),
            ProviderError::Cli(_)
        ));
    }
}
