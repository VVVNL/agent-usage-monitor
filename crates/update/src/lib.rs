//! Update notifications, without an update server.
//!
//! The only thing this crate can do is find out whether GitHub lists a newer release of this
//! program than the one running, and remember the answer. It never downloads or runs anything.
//! Everything except the actual `curl` call is pure and unit-tested; see
//! docs/research/auto-update.md for the design and docs/PRIVACY.md for what leaves the computer.

pub mod fetch;
pub mod install;
pub mod release;
pub mod state;
pub mod sums;
pub mod verify;
pub mod version;

use std::path::Path;

use chrono::{DateTime, Utc};

use aum_providers::runner::CommandRunner;

pub use fetch::{FetchError, REPO};
pub use install::{Action, InstallError, InstallPlan, Staged, action, plan, same_dir, stage};
pub use release::SeenRelease;
pub use state::{Available, UpdateState, release_page};
pub use version::Version;

/// The project's update-signing public key: base64 of the 32 raw Ed25519 bytes (see
/// docs/RELEASE_CHECKLIST.md, "Update signing key"). **Empty until the key pair exists**: with no key
/// built in, updates are only announced, never installed.
pub const PUBLIC_KEY_B64: &str = "cqVsshgt0QLPtLyTmSASUm8LmPcmF4fEs1UC/qOQn+o=";

/// The compiled-in public key, if there is a usable one.
pub fn public_key() -> Option<[u8; 32]> {
    verify::parse_public_key(PUBLIC_KEY_B64)
}

/// Arguments the app gives the Inno Setup installer when it updates itself: show progress but ask
/// nothing, never reboot, close the running app, and (`/aumupdate=1`, read by the installer script)
/// start the new version afterwards.
pub const INSTALLER_ARGS: [&str; 4] = [
    "/SILENT",
    "/NORESTART",
    "/CLOSEAPPLICATIONS",
    "/aumupdate=1",
];

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub available: Option<Available>,
    /// Set when the request failed. `available` may still hold what an earlier check found.
    pub error: Option<String>,
}

/// Ask GitHub (unless the ETag says nothing changed), update `state`, and report. Never panics and
/// never returns an error: a failed check is a `Report` with `error` set.
pub async fn check(
    runner: &dyn CommandRunner,
    curl: &Path,
    state: &mut UpdateState,
    current: &Version,
    include_beta: bool,
    now: DateTime<Utc>,
) -> Report {
    let outcome = fetch::fetch_list(runner, curl, state.etag.as_deref()).await;
    // Even a failed attempt counts, so the daily check is not retried every hour.
    state.last_check = Some(now);
    let error = match outcome {
        Ok(fetch::Fetched::NotModified) => {
            state.last_error = None;
            None
        }
        Ok(fetch::Fetched::Body { json, etag }) => match release::parse_releases(&json) {
            Ok(seen) => {
                state.releases = seen;
                state.etag = etag;
                state.last_error = None;
                None
            }
            Err(e) => Some(e.to_string()),
        },
        Err(e) => Some(e.to_string()),
    };
    state.last_error.clone_from(&error);
    Report {
        available: state.available(current, include_beta),
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use aum_providers::runner::{CmdOutput, RunError};
    use chrono::TimeZone;

    const LIST: &str = include_str!("../../../tests/fixtures/github/releases_list.json");

    struct Fake {
        answers: Mutex<Vec<Result<CmdOutput, RunError>>>,
        seen_args: Mutex<Vec<Vec<String>>>,
    }

    impl Fake {
        fn new(answers: Vec<Result<CmdOutput, RunError>>) -> Self {
            Self {
                answers: Mutex::new(answers),
                seen_args: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl CommandRunner for Fake {
        async fn run(
            &self,
            _program: &Path,
            args: &[&str],
            _timeout: Duration,
        ) -> Result<CmdOutput, RunError> {
            self.seen_args
                .lock()
                .unwrap()
                .push(args.iter().map(|s| (*s).to_owned()).collect());
            self.answers.lock().unwrap().remove(0)
        }
    }

    fn ok(stdout: &str) -> Result<CmdOutput, RunError> {
        Ok(CmdOutput::from_text(true, Some(0), stdout, ""))
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap()
    }

    fn cur() -> Version {
        Version::parse("0.11.0").unwrap()
    }

    fn body(etag: &str) -> String {
        format!("HTTP/2 200 \r\netag: {etag}\r\n\r\n{LIST}")
    }

    #[tokio::test]
    async fn finds_an_update_remembers_it_and_then_uses_the_etag() {
        let runner = Fake::new(vec![ok(&body("\"v1\"")), ok("HTTP/2 304 \r\n\r\n")]);
        let mut state = UpdateState::default();

        let r = check(&runner, Path::new("curl"), &mut state, &cur(), true, now()).await;
        assert_eq!(r.error, None);
        assert_eq!(r.available.unwrap().tag, "v0.12.0");
        assert_eq!(state.etag.as_deref(), Some("\"v1\""));
        assert_eq!(state.last_check, Some(now()));

        let r = check(&runner, Path::new("curl"), &mut state, &cur(), true, now()).await;
        assert_eq!(r.error, None);
        assert_eq!(
            r.available.unwrap().tag,
            "v0.12.0",
            "kept from memory on 304"
        );
        let args = runner.seen_args.lock().unwrap();
        assert!(!args[0].iter().any(|a| a.contains("If-None-Match")));
        assert!(args[1].contains(&"If-None-Match: \"v1\"".to_owned()));
    }

    #[tokio::test]
    async fn up_to_date_reports_nothing_available() {
        let runner = Fake::new(vec![ok(&body("\"v1\""))]);
        let mut state = UpdateState::default();
        let current = Version::parse("0.12.0").unwrap();
        let r = check(
            &runner,
            Path::new("curl"),
            &mut state,
            &current,
            true,
            now(),
        )
        .await;
        assert_eq!(
            r,
            Report {
                available: None,
                error: None
            }
        );
    }

    #[tokio::test]
    async fn failures_are_reported_and_keep_what_was_known() {
        let mut state = UpdateState {
            releases: vec![SeenRelease {
                tag: "v0.12.0".into(),
                prerelease: true,
            }],
            ..Default::default()
        };
        for (answer, text) in [
            (
                Err(RunError::NotFound("curl".into())),
                "curl is not available",
            ),
            (Err(RunError::Timeout), "did not answer in time"),
            (ok("HTTP/2 403 \r\n\r\n{}"), "limiting requests"),
            (ok("HTTP/2 404 \r\n\r\n{}"), "not publicly visible"),
            (ok("HTTP/2 200 \r\n\r\n<html>"), "not JSON"),
            (
                Ok(CmdOutput::from_text(false, Some(6), "", "x")),
                "curl exit code 6",
            ),
            (Ok(CmdOutput::from_text(false, Some(28), "", "")), "in time"),
        ] {
            let runner = Fake::new(vec![answer]);
            let r = check(&runner, Path::new("curl"), &mut state, &cur(), true, now()).await;
            assert!(r.error.as_deref().unwrap().contains(text), "{r:?} / {text}");
            assert_eq!(
                r.available.unwrap().tag,
                "v0.12.0",
                "an earlier finding survives"
            );
            assert!(state.last_error.is_some());
            assert_eq!(
                state.last_check,
                Some(now()),
                "a failed attempt still counts"
            );
        }
    }

    #[tokio::test]
    async fn a_malformed_list_is_not_stored() {
        let runner = Fake::new(vec![ok(
            "HTTP/2 200 \r\netag: \"z\"\r\n\r\n{\"message\":\"x\"}",
        )]);
        let mut state = UpdateState::default();
        let r = check(&runner, Path::new("curl"), &mut state, &cur(), true, now()).await;
        assert!(r.error.is_some());
        assert!(state.releases.is_empty());
        assert_eq!(
            state.etag, None,
            "the ETag of an answer we could not use is not kept"
        );
    }

    /// Needs the network: `cargo test -p aum-update -- --ignored --nocapture`. Runs the real `curl`
    /// request against a large public repository to prove the request, the answer parser and the
    /// release parser work on a real GitHub response, and whether an ETag gives "not modified".
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_public_repository() {
        use aum_providers::runner::SystemRunner;
        let url = "https://api.github.com/repos/cli/cli/releases?per_page=5";
        let curl = fetch::curl_path();
        let first = fetch::fetch_list_at(&SystemRunner, &curl, url, None).await;
        let Ok(fetch::Fetched::Body { json, etag }) = first else {
            panic!("first request: {first:?}");
        };
        let seen = release::parse_releases(&json).expect("real releases parse");
        println!(
            "{} releases, newest {:?}, etag {:?}",
            seen.len(),
            seen.first(),
            etag
        );
        assert!(!seen.is_empty());
        let second = fetch::fetch_list_at(&SystemRunner, &curl, url, etag.as_deref()).await;
        println!("second request with the ETag: {second:?}");
        assert_eq!(second, Ok(fetch::Fetched::NotModified));
    }

    /// Needs the network: the same check the app performs, against this project's real releases.
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_project_repository() {
        use aum_providers::runner::SystemRunner;
        let curl = fetch::curl_path();
        let mut state = UpdateState::default();
        let older = Version::parse("0.10.0").unwrap();
        let r = check(&SystemRunner, &curl, &mut state, &older, true, now()).await;
        println!("running 0.10.0: {r:?}");
        assert_eq!(r.error, None);
        assert_eq!(r.available.expect("an update from 0.10.0").tag, "v0.11.0");
        // Second request with the stored ETag, as the next day's check would do.
        let r = check(&SystemRunner, &curl, &mut state, &cur(), true, now()).await;
        println!("running 0.11.0 (ETag kept): {r:?}");
        assert_eq!(
            r,
            Report {
                available: None,
                error: None
            }
        );
    }

    #[test]
    fn the_built_in_key_is_either_unset_or_a_valid_32_byte_key() {
        // Empty (updates announced only) or exactly a 32-byte key; never a half-valid value that
        // would silently disable verification differently from "no key".
        assert!(
            PUBLIC_KEY_B64.is_empty() || public_key().is_some(),
            "PUBLIC_KEY_B64 is set but not a 32-byte key"
        );
        assert_eq!(public_key().is_some(), !PUBLIC_KEY_B64.is_empty());
    }

    #[test]
    fn installer_arguments_are_silent_never_reboot_and_flag_an_update() {
        assert!(INSTALLER_ARGS.contains(&"/NORESTART"));
        assert!(
            INSTALLER_ARGS.contains(&"/SILENT"),
            "progress stays visible"
        );
        assert!(!INSTALLER_ARGS.contains(&"/VERYSILENT"));
        assert!(INSTALLER_ARGS.contains(&"/aumupdate=1"));
    }
}
