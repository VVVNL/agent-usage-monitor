//! Claude Code session activity, from `claude agents --json`.
//!
//! The official interface: the Agent view documentation calls it "the supported way to read session
//! state from outside Claude Code". It is a research preview with no stability promise, so the parser
//! fails closed: an unrecognised status becomes [`SessionState::Unknown`], never `Idle`, and a
//! response that is not the expected array is an error.
//!
//! Privacy: each entry also carries the project folder (`cwd`), the conversation title (`name`) and
//! the full session id. None of them is declared in [`Entry`], so serde drops them while parsing and
//! they are never stored, logged or shown.
//!
//! WSL: Windows and WSL keep separate session stores, and sessions started by the Claude desktop app
//! often live inside WSL. The bridge runs `claude agents --json` inside each *configured, running*
//! distro. A stopped distro cannot have running sessions, so it is skipped and never started.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aum_core::{
    ActivitySnapshot, ActivitySource, ProviderError, ProviderId, SessionActivity, SessionState,
    sanitize_label,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::runner::{CommandRunner, RunError, SystemRunner, resolve_cli};

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// More than this many configured distros is almost certainly a mistake and would slow every poll.
pub const MAX_WSL_DISTROS: usize = 4;

/// Constant script run inside WSL (nothing user-supplied is ever interpolated into it). `$1` is an
/// optional explicit path to `claude`; otherwise the usual install locations and `PATH` are tried.
const WSL_SCRIPT: &str = "p=$1; if [ -n \"$p\" ]; then exec \"$p\" agents --json; fi; \
for c in \"$HOME/.local/bin/claude\" \"$HOME/.claude/local/claude\" /usr/local/bin/claude /usr/bin/claude; \
do if [ -x \"$c\" ]; then exec \"$c\" agents --json; fi; done; \
if command -v claude >/dev/null 2>&1; then exec claude agents --json; fi; \
echo 'claude not found in this distribution' >&2; exit 127";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    pid: Option<i64>,
    started_at: Option<i64>,
    /// Process-level status, present while the process is alive: busy, waiting, idle.
    status: Option<String>,
    /// Background-session state: working, blocked, done, failed, stopped.
    state: Option<String>,
    waiting_for: Option<String>,
}

fn waiting_label(w: Option<&str>) -> String {
    let t = w.map(|s| sanitize_label(s, 40)).unwrap_or_default();
    if t.is_empty() {
        "input needed".to_owned()
    } else {
        t
    }
}

/// `None` means the entry is not a live session (finished, failed or stopped background job).
fn map_state(e: &Entry) -> Option<SessionState> {
    match e.status.as_deref() {
        Some("busy") => Some(SessionState::Working),
        Some("waiting") => Some(SessionState::NeedsInput(waiting_label(
            e.waiting_for.as_deref(),
        ))),
        Some("idle") => Some(SessionState::Idle),
        Some(_) => Some(SessionState::Unknown),
        None => match e.state.as_deref() {
            Some("working") => Some(SessionState::Working),
            Some("blocked") => Some(SessionState::NeedsInput(waiting_label(
                e.waiting_for.as_deref(),
            ))),
            // No process any more: finished, failed or stopped.
            Some("done" | "failed" | "stopped") => None,
            Some(_) | None => Some(SessionState::Unknown),
        },
    }
}

/// Parse the output of `claude agents --json`. `source` ("windows", "WSL Ubuntu") keeps session keys
/// unique across environments, where process ids can collide.
pub fn parse_agents(text: &str, source: &str) -> Result<Vec<SessionActivity>, ProviderError> {
    let trimmed = text.trim();
    let entries: Vec<Entry> = serde_json::from_str(trimmed)
        .or_else(|first| {
            // Tolerate a warning line before or after the array (some launchers print one).
            match (trimmed.find('['), trimmed.rfind(']')) {
                (Some(a), Some(b)) if a < b => serde_json::from_str(&trimmed[a..=b]),
                _ => Err(first),
            }
        })
        .map_err(|_| {
            ProviderError::Parse("unrecognised `claude agents --json` output".to_owned())
        })?;

    Ok(entries
        .iter()
        .filter_map(|e| {
            let state = map_state(e)?;
            let started = e
                .started_at
                .and_then(DateTime::<Utc>::from_timestamp_millis);
            let key = format!(
                "{source}:{}:{}",
                e.pid.map_or("-".to_owned(), |p| p.to_string()),
                e.started_at.unwrap_or(0)
            );
            Some(SessionActivity {
                key,
                state,
                started_at: started,
                source: source.to_owned(),
            })
        })
        .collect())
}

/// Decode text printed by `wsl.exe` management commands: UTF-16LE (with or without a byte-order
/// mark), or plain UTF-8 when run as a pass-through.
pub fn decode_wsl_text(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE])
        || bytes.iter().skip(1).step_by(2).take(8).all(|b| *b == 0) && bytes.len() >= 2;
    if utf16 && bytes.len() >= 2 {
        let body = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Names from `wsl.exe -l --running -q`, one per line.
pub fn parse_running_distros(bytes: &[u8]) -> Vec<String> {
    decode_wsl_text(bytes)
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty() && !l.chars().any(char::is_control))
        .map(str::to_owned)
        .collect()
}

/// Which environments inside WSL to look at.
#[derive(Debug, Clone, Default)]
pub struct WslConfig {
    pub distros: Vec<String>,
    /// Explicit path to `claude` inside the distro; empty means automatic.
    pub claude_path: Option<String>,
    /// Off unless on Windows and at least one distro is configured.
    pub active: bool,
    pub wsl_exe: PathBuf,
}

impl WslConfig {
    pub fn new(distros: Vec<String>, claude_path: Option<String>) -> Self {
        let mut distros: Vec<String> = distros
            .into_iter()
            .map(|d| sanitize_label(&d, 64))
            .filter(|d| !d.is_empty())
            .collect();
        distros.dedup();
        distros.truncate(MAX_WSL_DISTROS);
        Self {
            active: cfg!(windows) && !distros.is_empty(),
            distros,
            claude_path: claude_path.filter(|p| !p.trim().is_empty()),
            wsl_exe: PathBuf::from("wsl.exe"),
        }
    }
}

pub struct ClaudeActivity {
    cli: PathBuf,
    runner: Arc<dyn CommandRunner>,
    wsl: WslConfig,
    now: fn() -> DateTime<Utc>,
}

fn native_source() -> String {
    match std::env::consts::OS {
        "windows" => "windows".to_owned(),
        "macos" => "macOS".to_owned(),
        other => other.to_owned(),
    }
}

impl ClaudeActivity {
    pub fn new(custom_cli: Option<PathBuf>, wsl: WslConfig) -> Self {
        Self::with_runner(
            resolve_cli("claude", custom_cli.as_deref()),
            wsl,
            Arc::new(SystemRunner),
        )
    }

    pub fn with_runner(cli: PathBuf, wsl: WslConfig, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            cli,
            runner,
            wsl,
            now: Utc::now,
        }
    }

    async fn native(&self) -> Result<Vec<SessionActivity>, ProviderError> {
        let out = self
            .runner
            .run(&self.cli, &["agents", "--json"], CALL_TIMEOUT)
            .await
            .map_err(|e| match e {
                RunError::NotFound(_) => ProviderError::Cli("Claude Code CLI not found".to_owned()),
                RunError::Timeout => ProviderError::Timeout,
                RunError::Io(m) => ProviderError::Cli(m),
            })?;
        if !out.success {
            let text = format!("{}{}", out.stdout, out.stderr).to_lowercase();
            return Err(
                if text.contains("unknown command") || text.contains("unrecognized") {
                    ProviderError::Unsupported(
                        "this Claude Code version has no `claude agents` (update it)".to_owned(),
                    )
                } else {
                    ProviderError::Cli(format!(
                        "`claude agents --json` failed (exit {})",
                        out.code.map_or("?".to_owned(), |c| c.to_string())
                    ))
                },
            );
        }
        parse_agents(&out.stdout, &native_source())
    }

    /// Distros from the configuration that `wsl.exe` reports as running right now.
    async fn running_distros(&self) -> Result<Vec<String>, ProviderError> {
        let out = self
            .runner
            .run(&self.wsl.wsl_exe, &["-l", "--running", "-q"], CALL_TIMEOUT)
            .await
            .map_err(|e| match e {
                RunError::NotFound(_) => ProviderError::Cli("wsl.exe not found".to_owned()),
                RunError::Timeout => ProviderError::Timeout,
                RunError::Io(m) => ProviderError::Cli(m),
            })?;
        let running = parse_running_distros(&out.stdout_bytes);
        // WSL distro names are case-insensitive.
        Ok(self
            .wsl
            .distros
            .iter()
            .filter(|d| running.iter().any(|r| r.eq_ignore_ascii_case(d)))
            .cloned()
            .collect())
    }

    async fn in_distro(&self, distro: &str) -> Result<Vec<SessionActivity>, ProviderError> {
        let path = self.wsl.claude_path.as_deref().unwrap_or("");
        let out = self
            .runner
            .run(
                &self.wsl.wsl_exe,
                &["-d", distro, "--exec", "sh", "-c", WSL_SCRIPT, "sh", path],
                CALL_TIMEOUT,
            )
            .await
            .map_err(|e| match e {
                RunError::NotFound(_) => ProviderError::Cli("wsl.exe not found".to_owned()),
                RunError::Timeout => ProviderError::Timeout,
                RunError::Io(m) => ProviderError::Cli(m),
            })?;
        if !out.success {
            return Err(if out.code == Some(127) {
                ProviderError::Cli("claude not found inside the distribution".to_owned())
            } else {
                ProviderError::Cli(format!(
                    "exit {}",
                    out.code.map_or("?".to_owned(), |c| c.to_string())
                ))
            });
        }
        parse_agents(&out.stdout, &format!("WSL {distro}"))
    }
}

#[async_trait]
impl ActivitySource for ClaudeActivity {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    async fn observe(&self) -> Result<ActivitySnapshot, ProviderError> {
        let mut sessions = Vec::new();
        let mut problems = Vec::new();
        let mut any_source_read = false;
        let mut first_error: Option<ProviderError> = None;

        match self.native().await {
            Ok(s) => {
                any_source_read = true;
                sessions.extend(s);
            }
            // No native install is normal when Claude only lives in WSL.
            Err(ProviderError::Cli(m)) if self.wsl.active && m.contains("not found") => {}
            Err(e) => {
                problems.push(format!("{}: {e}", native_source()));
                first_error.get_or_insert(e);
            }
        }

        if self.wsl.active {
            match self.running_distros().await {
                Ok(running) => {
                    // Not running means no sessions, so reading "nothing" from WSL is a success.
                    any_source_read = true;
                    for distro in running {
                        match self.in_distro(&distro).await {
                            Ok(s) => sessions.extend(s),
                            Err(e) => problems.push(format!("WSL {distro}: {e}")),
                        }
                    }
                }
                Err(e) => {
                    problems.push(format!("WSL: {e}"));
                    first_error.get_or_insert(e);
                }
            }
        }

        if !any_source_read {
            return Err(first_error
                .unwrap_or_else(|| ProviderError::Cli("Claude Code CLI not found".to_owned())));
        }
        Ok(ActivitySnapshot {
            provider: ProviderId::Claude,
            sessions,
            observed_at: (self.now)(),
            problems,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CmdOutput;
    use std::path::Path;
    use std::sync::Mutex;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/claude/agents/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn utf16(s: &str) -> Vec<u8> {
        std::iter::once(0xFEFFu16)
            .chain(s.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    #[test]
    fn interactive_sessions() {
        let s = parse_agents(&fixture("agents_interactive.json"), "windows").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].state, SessionState::Working);
        assert_eq!(s[1].state, SessionState::Idle);
        assert_eq!(s[0].key, "windows:4101:1794300000000");
        assert_eq!(s[0].source, "windows");
        assert_eq!(
            s[0].started_at.unwrap().timestamp_millis(),
            1_794_300_000_000
        );
    }

    #[test]
    fn waiting_sessions_say_why() {
        let s = parse_agents(&fixture("agents_waiting.json"), "windows").unwrap();
        assert_eq!(
            s[0].state,
            SessionState::NeedsInput("permission prompt".into())
        );
        assert_eq!(s[1].state, SessionState::NeedsInput("dialog open".into()));
        assert_eq!(s[2].state, SessionState::Working);
        // `waiting` with no reason still says something useful.
        let v = r#"[{"pid":1,"startedAt":1,"status":"waiting"}]"#;
        assert_eq!(
            parse_agents(v, "x").unwrap()[0].state,
            SessionState::NeedsInput("input needed".into())
        );
    }

    #[test]
    fn background_sessions_and_dead_jobs() {
        let s = parse_agents(&fixture("agents_background.json"), "windows").unwrap();
        assert_eq!(
            s.len(),
            2,
            "done and stopped jobs have no live process and are not sessions"
        );
        assert_eq!(s[0].state, SessionState::Working);
        assert_eq!(s[1].state, SessionState::NeedsInput("input needed".into()));
    }

    #[test]
    fn an_unknown_status_is_never_idle() {
        let s = parse_agents(&fixture("agents_unknown_status.json"), "windows").unwrap();
        assert_eq!(s[0].state, SessionState::Unknown);
        assert_eq!(s[1].state, SessionState::Working);
        let none = r#"[{"pid":1,"startedAt":1}]"#;
        assert_eq!(
            parse_agents(none, "x").unwrap()[0].state,
            SessionState::Unknown
        );
    }

    #[test]
    fn empty_and_malformed_output() {
        assert!(
            parse_agents(&fixture("agents_empty.json"), "x")
                .unwrap()
                .is_empty()
        );
        for bad in ["", "not json", "{\"pid\":1}", "null", "[1,2,3]"] {
            assert!(
                parse_agents(bad, "x").is_err()
                    || parse_agents(bad, "x")
                        .unwrap()
                        .iter()
                        .all(|s| s.state == SessionState::Unknown),
                "{bad}"
            );
        }
        assert!(matches!(
            parse_agents("not json", "x"),
            Err(ProviderError::Parse(_))
        ));
        assert!(matches!(
            parse_agents("{\"pid\":1}", "x"),
            Err(ProviderError::Parse(_))
        ));
    }

    #[test]
    fn a_warning_line_around_the_array_is_tolerated() {
        let noisy = format!(
            "wsl: some warning\n{}\ntrailing note",
            fixture("agents_interactive.json")
        );
        assert_eq!(parse_agents(&noisy, "x").unwrap().len(), 2);
    }

    #[test]
    fn project_folder_and_title_are_never_kept() {
        let s = parse_agents(&fixture("agents_interactive.json"), "windows").unwrap();
        let dump = format!("{s:?}");
        assert!(
            !dump.contains("project-a")
                && !dump.contains("REDACTED")
                && !dump.contains("0000-0000"),
            "{dump}"
        );
    }

    #[test]
    fn keys_differ_across_environments_with_the_same_pid() {
        let v = r#"[{"pid":10,"startedAt":5,"status":"busy"}]"#;
        let a = &parse_agents(v, "windows").unwrap()[0].key;
        let b = &parse_agents(v, "WSL Ubuntu").unwrap()[0].key;
        assert_ne!(a, b);
    }

    #[test]
    fn wsl_text_decoding() {
        assert_eq!(
            parse_running_distros(&utf16("Ubuntu\r\ndocker-desktop\r\n")),
            vec!["Ubuntu", "docker-desktop"]
        );
        assert_eq!(
            parse_running_distros(b"Ubuntu\ndebian\n"),
            vec!["Ubuntu", "debian"]
        );
        let no_bom: Vec<u8> = "Ubuntu\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_running_distros(&no_bom), vec!["Ubuntu"]);
        assert!(parse_running_distros(b"").is_empty());
        assert_eq!(
            parse_running_distros(&utf16("My Distro\r\n")),
            vec!["My Distro"]
        );
    }

    #[test]
    fn wsl_config_is_sanitised_and_bounded() {
        let w = WslConfig::new(
            vec![
                "  Ubuntu ".into(),
                "".into(),
                "a\u{7}b".into(),
                "d3".into(),
                "d4".into(),
                "d5".into(),
                "d6".into(),
            ],
            Some("  ".into()),
        );
        assert_eq!(w.distros.len(), MAX_WSL_DISTROS);
        assert_eq!(w.distros[0], "Ubuntu");
        assert_eq!(w.distros[1], "ab");
        assert_eq!(w.claude_path, None);
        assert!(!WslConfig::new(vec![], None).active);
    }

    // ---- adapter, with a scripted runner ------------------------------------------------------

    type Reply = Result<CmdOutput, RunError>;

    struct Fake {
        calls: Mutex<Vec<Vec<String>>>,
        native: Reply,
        wsl_list: Reply,
        wsl_run: Box<dyn Fn(&str) -> Reply + Send + Sync>,
    }

    #[async_trait]
    impl CommandRunner for Fake {
        async fn run(&self, program: &Path, args: &[&str], _t: Duration) -> Reply {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| (*s).to_owned()).collect());
            let prog = program.to_string_lossy();
            if prog.contains("wsl") {
                if args.first() == Some(&"-l") {
                    return self.wsl_list.clone();
                }
                return (self.wsl_run)(args[1]);
            }
            self.native.clone()
        }
    }

    fn ok(s: &str) -> Reply {
        Ok(CmdOutput::from_text(true, Some(0), s, ""))
    }

    fn wsl_on(distros: &[&str]) -> WslConfig {
        WslConfig {
            active: true,
            ..WslConfig::new(distros.iter().map(|s| (*s).to_owned()).collect(), None)
        }
    }

    fn adapter(fake: Fake, wsl: WslConfig) -> (ClaudeActivity, Arc<Fake>) {
        let fake = Arc::new(fake);
        (
            ClaudeActivity::with_runner("claude".into(), wsl, fake.clone()),
            fake,
        )
    }

    fn base() -> Fake {
        Fake {
            calls: Mutex::default(),
            native: ok(&fixture("agents_interactive.json")),
            wsl_list: Ok(CmdOutput {
                stdout_bytes: utf16("Ubuntu\r\n"),
                ..CmdOutput::from_text(true, Some(0), "", "")
            }),
            wsl_run: Box::new(|_| ok(&fixture("agents_waiting.json"))),
        }
    }

    #[tokio::test]
    async fn native_only_never_touches_wsl() {
        let (a, f) = adapter(base(), WslConfig::default());
        let snap = a.observe().await.unwrap();
        assert_eq!(snap.sessions.len(), 2);
        assert!(snap.problems.is_empty());
        assert_eq!(
            *f.calls.lock().unwrap(),
            vec![vec!["agents".to_owned(), "--json".into()]]
        );
    }

    #[tokio::test]
    async fn bridge_merges_native_and_wsl_sessions() {
        let (a, f) = adapter(base(), wsl_on(&["Ubuntu"]));
        let snap = a.observe().await.unwrap();
        assert_eq!(snap.sessions.len(), 5);
        assert!(snap.sessions.iter().any(|s| s.source == "WSL Ubuntu"));
        assert_eq!((snap.working(), snap.needs_input(), snap.idle()), (2, 2, 1));
        let calls = f.calls.lock().unwrap();
        assert_eq!(
            calls[1],
            vec!["-l", "--running", "-q"],
            "running distros are checked first"
        );
        assert_eq!(&calls[2][..4], ["-d", "Ubuntu", "--exec", "sh"]);
        assert!(
            calls[2][5].contains("agents --json"),
            "the constant script runs `claude agents --json`"
        );
    }

    #[tokio::test]
    async fn a_stopped_distro_is_never_started() {
        let mut f = base();
        f.wsl_list = Ok(CmdOutput {
            stdout_bytes: utf16("docker-desktop\r\n"),
            ..CmdOutput::from_text(true, Some(0), "", "")
        });
        let (a, f) = adapter(f, wsl_on(&["Ubuntu"]));
        let snap = a.observe().await.unwrap();
        assert_eq!(snap.sessions.len(), 2, "only the native sessions");
        let calls = f.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .all(|c| c.first().map(String::as_str) != Some("-d")),
            "no per-distro call was made: {calls:?}"
        );
    }

    #[tokio::test]
    async fn distro_names_match_case_insensitively() {
        let (a, _) = adapter(base(), wsl_on(&["ubuntu"]));
        assert_eq!(a.observe().await.unwrap().sessions.len(), 5);
    }

    #[tokio::test]
    async fn claude_only_in_wsl_is_fine_even_without_a_native_install() {
        let mut f = base();
        f.native = Err(RunError::NotFound("claude".into()));
        let (a, _) = adapter(f, wsl_on(&["Ubuntu"]));
        let snap = a.observe().await.unwrap();
        assert_eq!(snap.sessions.len(), 3);
        assert!(snap.problems.is_empty());
    }

    #[tokio::test]
    async fn no_native_install_and_wsl_not_running_means_no_sessions_not_an_error() {
        let mut f = base();
        f.native = Err(RunError::NotFound("claude".into()));
        f.wsl_list = Ok(CmdOutput::from_text(true, Some(0), "", ""));
        let (a, _) = adapter(f, wsl_on(&["Ubuntu"]));
        assert!(a.observe().await.unwrap().sessions.is_empty());
    }

    #[tokio::test]
    async fn a_failing_distro_is_reported_but_the_rest_survives() {
        let mut f = base();
        f.wsl_run = Box::new(|_| {
            Ok(CmdOutput::from_text(
                false,
                Some(127),
                "",
                "claude not found",
            ))
        });
        let (a, _) = adapter(f, wsl_on(&["Ubuntu"]));
        let snap = a.observe().await.unwrap();
        assert_eq!(snap.sessions.len(), 2);
        assert_eq!(
            snap.problems,
            vec!["WSL Ubuntu: CLI error: claude not found inside the distribution"]
        );
    }

    #[tokio::test]
    async fn nothing_readable_is_an_error() {
        let mut f = base();
        f.native = Err(RunError::NotFound("claude".into()));
        let (a, _) = adapter(f, WslConfig::default());
        assert!(
            matches!(a.observe().await.unwrap_err(), ProviderError::Cli(m) if m.contains("not found"))
        );

        let mut f = base();
        f.native = Ok(CmdOutput::from_text(
            false,
            Some(1),
            "",
            "error: unknown command 'agents'",
        ));
        let (a, _) = adapter(f, WslConfig::default());
        assert!(matches!(
            a.observe().await.unwrap_err(),
            ProviderError::Unsupported(_)
        ));

        let mut f = base();
        f.native = ok("<html>");
        let (a, _) = adapter(f, WslConfig::default());
        assert!(matches!(
            a.observe().await.unwrap_err(),
            ProviderError::Parse(_)
        ));
    }

    #[tokio::test]
    async fn timeouts_are_reported() {
        let mut f = base();
        f.native = Err(RunError::Timeout);
        let (a, _) = adapter(f, WslConfig::default());
        assert_eq!(a.observe().await.unwrap_err(), ProviderError::Timeout);
    }

    #[tokio::test]
    async fn wsl_unavailable_keeps_native_sessions_and_says_so() {
        let mut f = base();
        f.wsl_list = Err(RunError::NotFound("wsl.exe".into()));
        let (a, _) = adapter(f, wsl_on(&["Ubuntu"]));
        let snap = a.observe().await.unwrap();
        assert_eq!(snap.sessions.len(), 2);
        assert_eq!(snap.problems, vec!["WSL: CLI error: wsl.exe not found"]);
    }
}
