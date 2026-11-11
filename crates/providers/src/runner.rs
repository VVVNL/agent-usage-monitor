//! Running external CLIs, behind a trait so adapters can be tested with
//! scripted output.
//!
//! Safety rules for the real runner:
//! - the program is spawned directly with an argument array, never through a
//!   shell (a shell can rewrite `/usage` into a path and send it to the model);
//! - stdin is closed, a hard timeout applies and the child is killed on drop;
//! - on Windows no console window is created.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub success: bool,
    pub code: Option<i32>,
    /// Standard output decoded as UTF-8 (lossily).
    pub stdout: String,
    /// The same output as raw bytes, for programs that do not print UTF-8 (`wsl.exe -l` prints UTF-16).
    pub stdout_bytes: Vec<u8>,
    pub stderr: String,
}

impl CmdOutput {
    /// Build from text (used by tests and fakes).
    pub fn from_text(success: bool, code: Option<i32>, stdout: &str, stderr: &str) -> Self {
        Self {
            success,
            code,
            stdout: stdout.to_owned(),
            stdout_bytes: stdout.as_bytes().to_vec(),
            stderr: stderr.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("program not found: {0}")]
    NotFound(String),
    #[error("timed out")]
    Timeout,
    #[error("io error: {0}")]
    Io(String),
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CmdOutput, RunError>;
}

/// Spawns real processes.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

/// Output beyond this is dropped; usage output is a few hundred bytes.
const MAX_OUTPUT_BYTES: u64 = 256 * 1024;

#[async_trait]
impl CommandRunner for SystemRunner {
    async fn run(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CmdOutput, RunError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound(program.display().to_string()),
            _ => RunError::Io(e.to_string()),
        })?;
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");

        let pid = child.id();
        let mut work = Box::pin(async {
            let (mut o, mut e) = (Vec::new(), Vec::new());
            let mut out_limited = (&mut out).take(MAX_OUTPUT_BYTES);
            let mut err_limited = (&mut err).take(MAX_OUTPUT_BYTES);
            let (ro, re) = tokio::join!(
                out_limited.read_to_end(&mut o),
                err_limited.read_to_end(&mut e),
            );
            ro.map_err(|e| RunError::Io(e.to_string()))?;
            re.map_err(|e| RunError::Io(e.to_string()))?;
            let status = child
                .wait()
                .await
                .map_err(|e| RunError::Io(e.to_string()))?;
            Ok(CmdOutput {
                success: status.success(),
                code: status.code(),
                stdout: String::from_utf8_lossy(&o).into_owned(),
                stdout_bytes: o,
                stderr: String::from_utf8_lossy(&e).into_owned(),
            })
        });

        // On timeout, kill the whole process tree *while the parent is still alive* (so its
        // children can still be found), then drop `work`, whose kill_on_drop ends the parent.
        match tokio::time::timeout(timeout, &mut work).await {
            Ok(result) => result,
            Err(_) => {
                if let Some(pid) = pid {
                    kill_tree(pid).await;
                }
                Err(RunError::Timeout)
            }
        }
    }
}

/// A line-oriented conversation with a child process (JSON-RPC over stdio).
#[async_trait]
pub trait RpcSession: Send {
    /// Write one line (a newline is appended).
    async fn send(&mut self, line: &str) -> Result<(), RunError>;
    /// Next stdout line. `Err(Timeout)` if none arrives in time, `Err(Io)` at end of output.
    async fn recv(&mut self, timeout: Duration) -> Result<String, RunError>;
    /// End the conversation and kill the process and everything it started.
    async fn close(&mut self);
}

#[async_trait]
pub trait RpcSpawner: Send + Sync {
    async fn spawn(&self, program: &Path, args: &[&str]) -> Result<Box<dyn RpcSession>, RunError>;
}

struct ChildSession {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    pid: Option<u32>,
}

#[async_trait]
impl RpcSession for ChildSession {
    async fn send(&mut self, line: &str) -> Result<(), RunError> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| RunError::Io("input closed".into()))?;
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|e| RunError::Io(e.to_string()))?;
        stdin.flush().await.map_err(|e| RunError::Io(e.to_string()))
    }

    async fn recv(&mut self, timeout: Duration) -> Result<String, RunError> {
        match tokio::time::timeout(timeout, self.lines.next_line()).await {
            Err(_) => Err(RunError::Timeout),
            Ok(Ok(Some(line))) => Ok(line),
            Ok(Ok(None)) => Err(RunError::Io("process closed its output".into())),
            Ok(Err(e)) => Err(RunError::Io(e.to_string())),
        }
    }

    async fn close(&mut self) {
        self.stdin = None; // closing stdin is the polite way to ask a server to stop
        if let Some(pid) = self.pid.take() {
            kill_tree(pid).await;
        }
        let _ = self.child.kill().await;
    }
}

#[async_trait]
impl RpcSpawner for SystemRunner {
    async fn spawn(&self, program: &Path, args: &[&str]) -> Result<Box<dyn RpcSession>, RunError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound(program.display().to_string()),
            _ => RunError::Io(e.to_string()),
        })?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RunError::Io("no stdout".into()))?;
        Ok(Box::new(ChildSession {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            pid,
        }))
    }
}

/// Terminate a process and everything it started. Best effort: failure is ignored
/// because the parent is killed regardless.
#[cfg(windows)]
async fn kill_tree(pid: u32) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let _ = cmd.status().await;
}

#[cfg(not(windows))]
async fn kill_tree(_pid: u32) {}

/// Where to look for a CLI besides the process `PATH`. Apps started from Finder, the Start menu or
/// at login do not inherit the terminal's `PATH`, so `claude` and `codex` are often not found
/// without this.
#[derive(Debug, Default, Clone)]
pub struct SearchEnv {
    pub path: Option<std::ffi::OsString>,
    pub home: Option<PathBuf>,
    /// Windows `%APPDATA%` (where npm puts global shims).
    pub appdata: Option<PathBuf>,
}

impl SearchEnv {
    pub fn from_process() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
        }
    }

    /// Directories to search, in priority order: `PATH` first, then the usual install places.
    pub fn dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.path.iter().flat_map(std::env::split_paths).collect();
        if let Some(home) = &self.home {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".claude/local"));
            dirs.push(home.join(".npm-global/bin"));
            dirs.push(home.join(".bun/bin"));
            dirs.extend(nvm_bins(home));
        }
        if let Some(appdata) = &self.appdata {
            dirs.push(appdata.join("npm"));
        }
        for fixed in ["/opt/homebrew/bin", "/usr/local/bin"] {
            dirs.push(PathBuf::from(fixed));
        }
        dirs
    }
}

/// `~/.nvm/versions/node/*/bin`, newest version first.
fn nvm_bins(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let version = |name: &str| -> Vec<u64> {
        name.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let mut found: Vec<(Vec<u64>, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let bin = e.path().join("bin");
            bin.is_dir().then(|| (version(&name), bin))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Locate the CLI: an explicit path wins, otherwise search PATH and the usual install locations.
/// On Windows `.exe` and `.cmd` shims are considered. Falls back to the bare name so the OS
/// reports a clear "not found".
pub fn resolve_cli(name: &str, custom: Option<&Path>) -> PathBuf {
    resolve_cli_in(name, custom, &SearchEnv::from_process())
}

pub fn resolve_cli_in(name: &str, custom: Option<&Path>, env: &SearchEnv) -> PathBuf {
    if let Some(p) = custom {
        return p.to_path_buf();
    }
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd"]
    } else {
        &[""]
    };
    for dir in env.dirs() {
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_path_wins() {
        let p = resolve_cli("claude", Some(Path::new("/opt/x/claude")));
        assert_eq!(p, PathBuf::from("/opt/x/claude"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_and_captures() {
        let out = SystemRunner
            .run(
                Path::new("/bin/sh"),
                &["-c", "echo hi; echo err >&2"],
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.stdout.trim(), "hi");
        assert_eq!(out.stderr.trim(), "err");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn times_out_hung_process() {
        let r = SystemRunner
            .run(
                Path::new("/bin/sh"),
                &["-c", "sleep 30"],
                Duration::from_millis(200),
            )
            .await;
        assert_eq!(r.unwrap_err(), RunError::Timeout);
    }

    #[tokio::test]
    async fn missing_program_is_not_found() {
        let r = SystemRunner
            .run(
                Path::new("definitely-not-a-real-cli-xyz"),
                &[],
                Duration::from_secs(2),
            )
            .await;
        assert!(matches!(r.unwrap_err(), RunError::NotFound(_)));
    }

    #[cfg(unix)]
    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn finds_clis_in_the_usual_places_when_path_is_empty() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let env = SearchEnv {
            path: None,
            home: Some(h.to_path_buf()),
            appdata: None,
        };
        assert_eq!(
            resolve_cli_in("claude", None, &env),
            PathBuf::from("claude"),
            "nothing installed"
        );

        touch(&h.join(".local/bin/claude"));
        assert_eq!(
            resolve_cli_in("claude", None, &env),
            h.join(".local/bin/claude")
        );

        touch(&h.join(".npm-global/bin/codex"));
        assert_eq!(
            resolve_cli_in("codex", None, &env),
            h.join(".npm-global/bin/codex")
        );
    }

    #[cfg(unix)]
    #[test]
    fn nvm_uses_the_newest_node_version() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        for v in ["v18.20.4", "v22.1.0", "v22.12.0", "v9.0.0"] {
            touch(&h.join(format!(".nvm/versions/node/{v}/bin/codex")));
        }
        let env = SearchEnv {
            path: None,
            home: Some(h.to_path_buf()),
            appdata: None,
        };
        assert_eq!(
            resolve_cli_in("codex", None, &env),
            h.join(".nvm/versions/node/v22.12.0/bin/codex")
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_beats_fallback_locations_and_custom_beats_everything() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        touch(&a.path().join("claude"));
        touch(&b.path().join(".local/bin/claude"));
        let env = SearchEnv {
            path: Some(a.path().as_os_str().to_owned()),
            home: Some(b.path().to_path_buf()),
            appdata: None,
        };
        assert_eq!(
            resolve_cli_in("claude", None, &env),
            a.path().join("claude")
        );
        assert_eq!(
            resolve_cli_in("claude", Some(Path::new("/x/claude")), &env),
            PathBuf::from("/x/claude")
        );
    }

    #[test]
    fn windows_npm_directory_is_searched() {
        let env = SearchEnv {
            path: None,
            home: None,
            appdata: Some(PathBuf::from("C:/Users/u/AppData/Roaming")),
        };
        assert!(
            env.dirs()
                .contains(&PathBuf::from("C:/Users/u/AppData/Roaming/npm"))
        );
    }
}
