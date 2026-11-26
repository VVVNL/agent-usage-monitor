//! Fetching and verifying a newer installer (Windows copies installed with the setup program).
//!
//! Nothing here runs anything: `stage` downloads the checksum list, its signature and the installer
//! into a private folder and only returns when the installer is proven to be the published one.
//! Running it is the app shell's job, after the user said yes. See docs/research/auto-update.md.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aum_providers::runner::{CommandRunner, RunError};

use crate::fetch::REPO;
use crate::state::Available;
use crate::sums::{Sums, SumsError};
use crate::verify::{self, VerifyError};
use crate::version::Version;

/// Size limits: the real installer is about 7 MB.
pub const MAX_SETUP_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_SUMS_BYTES: u64 = 64 * 1024;
pub const MAX_SIG_BYTES: u64 = 4 * 1024;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(330);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub tag: String,
    pub version: Version,
    pub setup_name: String,
    pub setup_url: String,
    pub sums_url: String,
    pub sig_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("not a release version")]
    BadVersion,
    #[error("download failed: {0}")]
    Download(String),
    #[error("{0}")]
    Verify(#[from] VerifyError),
    #[error("{0}")]
    Sums(#[from] SumsError),
    #[error("the signed checksum list has no entry for {0}")]
    NoEntry(String),
    #[error("a downloaded file is larger than allowed")]
    TooLarge,
    #[error("could not use the download folder: {0}")]
    Io(String),
}

/// Download links built from the repository constant and the version, never taken from the server.
pub fn plan(found: &Available) -> Result<InstallPlan, InstallError> {
    // `Available.version` came from `Version::parse(tag)`; parse again so a hand-built value cannot
    // smuggle odd characters into a URL.
    let version = Version::parse(&found.tag).map_err(|_| InstallError::BadVersion)?;
    let setup_name = format!("agent-usage-monitor-{version}-setup.exe");
    let base = format!("https://github.com/{REPO}/releases/download/{}", found.tag);
    Ok(InstallPlan {
        setup_url: format!("{base}/{setup_name}"),
        sums_url: format!("{base}/SHA256SUMS.txt"),
        sig_url: format!("{base}/SHA256SUMS.txt.sig"),
        tag: found.tag.clone(),
        version,
        setup_name,
    })
}

/// What choosing "update" does for this copy of the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Download, verify and run the installer (after a confirmation).
    Install,
    /// Open the release page; the user installs by hand.
    OpenPage,
}

pub fn action(windows: bool, installed_by_setup: bool, key_configured: bool) -> Action {
    if windows && installed_by_setup && key_configured {
        Action::Install
    } else {
        Action::OpenPage
    }
}

/// Is `exe_dir` the folder the setup program recorded as the install location (`InstallLocation`
/// in the uninstall key)? Case-insensitive, ignores a trailing slash.
pub fn same_dir(install_location: &str, exe_dir: &Path) -> bool {
    fn norm(s: &str) -> String {
        s.trim()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    }
    let loc = norm(install_location);
    !loc.is_empty() && loc == norm(&exe_dir.to_string_lossy())
}

/// Only https, only GitHub's own hosts (the release page and the CDN it redirects to).
pub fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') || authority.is_empty() {
        return false;
    }
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(h, p)| (h, Some(p)));
    if port.is_some_and(|p| p != "443") {
        return false;
    }
    let host = host.to_ascii_lowercase();
    host == "github.com" || host.ends_with(".githubusercontent.com")
}

/// A verified installer on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub setup_path: PathBuf,
    pub sha256: [u8; 32],
    pub size: u64,
}

fn download_args(url: &str, dest: &Path, max_bytes: u64) -> Vec<String> {
    [
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--max-redirs",
        "3",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--max-time",
        "300",
        "--max-filesize",
        &max_bytes.to_string(),
        "-H",
        "User-Agent: agent-usage-monitor",
        "-o",
        &dest.to_string_lossy(),
        "-w",
        "%{url_effective}",
        url,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

async fn download(
    runner: &dyn CommandRunner,
    curl: &Path,
    url: &str,
    dest: &Path,
    max_bytes: u64,
) -> Result<(), InstallError> {
    let args = download_args(url, dest, max_bytes);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = runner
        .run(curl, &refs, DOWNLOAD_TIMEOUT)
        .await
        .map_err(|e| match e {
            RunError::NotFound(_) => InstallError::Download("curl is not available".into()),
            RunError::Timeout => InstallError::Download("timed out".into()),
            RunError::Io(m) => InstallError::Download(m),
        })?;
    if !out.success {
        let why = match out.code {
            Some(63) => "file too large".to_owned(),
            Some(c) => format!("curl exit code {c}"),
            None => "curl was stopped".to_owned(),
        };
        return Err(InstallError::Download(why));
    }
    // Where the redirects ended up must still be GitHub.
    if !allowed_url(out.stdout.trim()) {
        return Err(InstallError::Download("redirected away from GitHub".into()));
    }
    let size = std::fs::metadata(dest)
        .map_err(|e| InstallError::Io(e.to_string()))?
        .len();
    if size > max_bytes {
        return Err(InstallError::TooLarge);
    }
    Ok(())
}

fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>, InstallError> {
    let data = std::fs::read(path).map_err(|e| InstallError::Io(e.to_string()))?;
    if data.len() as u64 > max {
        return Err(InstallError::TooLarge);
    }
    Ok(data)
}

/// Download and verify. On any failure nothing usable is left in `dir`.
///
/// Order matters: the signed checksum list is checked first, so an installer is only downloaded for
/// a release the project's key vouches for.
pub async fn stage(
    runner: &dyn CommandRunner,
    curl: &Path,
    plan: &InstallPlan,
    public_key: Option<[u8; 32]>,
    dir: &Path,
) -> Result<Staged, InstallError> {
    std::fs::create_dir_all(dir).map_err(|e| InstallError::Io(e.to_string()))?;
    let sums_part = dir.join("SHA256SUMS.txt.part");
    let sig_part = dir.join("SHA256SUMS.txt.sig.part");
    let setup_part = dir.join(format!("{}.part", plan.setup_name));
    let result = stage_inner(
        runner,
        curl,
        plan,
        public_key,
        dir,
        &sums_part,
        &sig_part,
        &setup_part,
    )
    .await;
    for p in [&sums_part, &sig_part, &setup_part] {
        let _ = std::fs::remove_file(p);
    }
    if result.is_err() {
        let _ = std::fs::remove_file(dir.join(&plan.setup_name));
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn stage_inner(
    runner: &dyn CommandRunner,
    curl: &Path,
    plan: &InstallPlan,
    public_key: Option<[u8; 32]>,
    dir: &Path,
    sums_part: &Path,
    sig_part: &Path,
    setup_part: &Path,
) -> Result<Staged, InstallError> {
    // Refuse early when there is no key: nothing is downloaded for an update that cannot be proven.
    if public_key.is_none() {
        return Err(VerifyError::NoKey.into());
    }

    download(runner, curl, &plan.sums_url, sums_part, MAX_SUMS_BYTES).await?;
    download(runner, curl, &plan.sig_url, sig_part, MAX_SIG_BYTES).await?;
    let sums_bytes = read_limited(sums_part, MAX_SUMS_BYTES)?;
    let sig_text = String::from_utf8(read_limited(sig_part, MAX_SIG_BYTES)?)
        .map_err(|_| VerifyError::BadSignatureFormat)?;
    verify::verify_signature(public_key, &sums_bytes, &sig_text)?;

    let sums = Sums::parse(&String::from_utf8_lossy(&sums_bytes))?;
    let expected = sums
        .hash_for(&plan.setup_name)
        .ok_or_else(|| InstallError::NoEntry(plan.setup_name.clone()))?;

    download(runner, curl, &plan.setup_url, setup_part, MAX_SETUP_BYTES).await?;
    let (actual, size) =
        verify::sha256_file(setup_part).map_err(|e| InstallError::Io(e.to_string()))?;
    if actual != expected {
        return Err(VerifyError::HashMismatch.into());
    }
    let setup_path = dir.join(&plan.setup_name);
    std::fs::rename(setup_part, &setup_path).map_err(|e| InstallError::Io(e.to_string()))?;
    Ok(Staged {
        setup_path,
        sha256: actual,
        size,
    })
}

/// Re-hash the installer immediately before it is run, so a file swapped after verification is
/// caught.
pub fn recheck(staged: &Staged) -> Result<(), InstallError> {
    let (hash, size) =
        verify::sha256_file(&staged.setup_path).map_err(|e| InstallError::Io(e.to_string()))?;
    if hash == staged.sha256 && size == staged.size {
        Ok(())
    } else {
        Err(VerifyError::HashMismatch.into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use aum_providers::runner::CmdOutput;

    use super::*;
    use crate::verify::testkeys::{public, sign_b64};

    fn found(tag: &str) -> Available {
        Available {
            tag: tag.to_owned(),
            version: Version::parse(tag).unwrap(),
            prerelease: true,
        }
    }

    // ----- pure helpers -----

    #[test]
    fn plan_builds_exact_urls_from_the_tag() {
        let p = plan(&found("v0.12.0")).unwrap();
        assert_eq!(p.setup_name, "agent-usage-monitor-0.12.0-setup.exe");
        assert_eq!(
            p.setup_url,
            "https://github.com/VVVNL/agent-usage-monitor/releases/download/v0.12.0/agent-usage-monitor-0.12.0-setup.exe"
        );
        assert!(p.sums_url.ends_with("/v0.12.0/SHA256SUMS.txt"));
        assert!(p.sig_url.ends_with("/v0.12.0/SHA256SUMS.txt.sig"));
        assert_eq!(
            plan(&found("v1.0.0-beta.2")).unwrap().setup_name,
            "agent-usage-monitor-1.0.0-beta.2-setup.exe"
        );
        let mut bad = found("v0.12.0");
        bad.tag = "v0.12.0/../../evil".into();
        assert_eq!(plan(&bad), Err(InstallError::BadVersion));
    }

    #[test]
    fn only_a_signed_installed_windows_copy_installs_itself() {
        assert_eq!(action(true, true, true), Action::Install);
        assert_eq!(
            action(true, true, false),
            Action::OpenPage,
            "no key built in: announce only"
        );
        assert_eq!(action(true, false, true), Action::OpenPage, "portable copy");
        assert_eq!(
            action(false, true, true),
            Action::OpenPage,
            "macOS and others"
        );
    }

    #[test]
    fn install_folder_comparison_ignores_case_and_trailing_slash() {
        let dir = Path::new(r"C:\Users\Me\AppData\Local\Programs\AgentUsageMonitor");
        assert!(same_dir(
            r"C:\Users\Me\AppData\Local\Programs\AgentUsageMonitor\",
            dir
        ));
        assert!(same_dir(
            r"c:\users\me\appdata\local\programs\agentusagemonitor",
            dir
        ));
        assert!(!same_dir(r"C:\Other", dir));
        assert!(!same_dir("", dir));
        assert!(!same_dir("   ", Path::new("")), "empty never matches");
    }

    #[test]
    fn only_https_github_hosts_are_allowed() {
        for ok in [
            "https://github.com/VVVNL/agent-usage-monitor/releases/download/v1/x.exe",
            "https://objects.githubusercontent.com/github-production-release-asset/abc?X=1",
            "https://release-assets.githubusercontent.com/a/b",
            "https://github.com:443/x",
        ] {
            assert!(allowed_url(ok), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "https://evil.example/github.com",
            "https://github.com.evil.example/x",
            "https://evilgithubusercontent.com/x",
            "https://user@github.com/x",
            "https://github.com@evil.example/x",
            "https://github.com:8443/x",
            "https://",
            "ftp://github.com/x",
            "",
        ] {
            assert!(!allowed_url(bad), "{bad}");
        }
    }

    // ----- staging against a scripted curl -----

    /// Pretends to be curl: writes the scripted body for a URL to the `-o` path and prints the
    /// effective URL, like `-w %{url_effective}`.
    struct FakeCurl {
        bodies: HashMap<String, Vec<u8>>,
        effective: Mutex<Option<String>>,
        fail_url: Option<String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeCurl {
        fn new(bodies: Vec<(&str, Vec<u8>)>) -> Self {
            Self {
                bodies: bodies.into_iter().map(|(u, b)| (u.to_owned(), b)).collect(),
                effective: Mutex::new(None),
                fail_url: None,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl CommandRunner for FakeCurl {
        async fn run(&self, _p: &Path, args: &[&str], _t: Duration) -> Result<CmdOutput, RunError> {
            let url = args.last().unwrap().to_string();
            self.calls.lock().unwrap().push(url.clone());
            if self.fail_url.as_deref() == Some(url.as_str()) {
                return Ok(CmdOutput::from_text(false, Some(22), "", "HTTP 404"));
            }
            let dest = args[args.iter().position(|a| *a == "-o").unwrap() + 1];
            match self.bodies.get(&url) {
                Some(body) => {
                    std::fs::write(dest, body).unwrap();
                    let eff = self.effective.lock().unwrap().clone().unwrap_or(url);
                    Ok(CmdOutput::from_text(true, Some(0), &eff, ""))
                }
                None => Ok(CmdOutput::from_text(false, Some(22), "", "HTTP 404")),
            }
        }
    }

    struct World {
        plan: InstallPlan,
        setup: Vec<u8>,
        sums: Vec<u8>,
        key_seed: u8,
    }

    fn world() -> World {
        let plan = plan(&found("v0.12.0")).unwrap();
        let setup = b"MZ pretend installer bytes".to_vec();
        let hash: String = verify::sha256_bytes(&setup)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let sums = format!(
            "{hash}  {}\r\n{}  agent-usage-monitor-0.12.0-windows-x64-portable.zip\r\n",
            plan.setup_name,
            "0".repeat(64)
        )
        .into_bytes();
        World {
            plan,
            setup,
            sums,
            key_seed: 1,
        }
    }

    impl World {
        fn curl(&self) -> FakeCurl {
            FakeCurl::new(vec![
                (&self.plan.setup_url, self.setup.clone()),
                (&self.plan.sums_url, self.sums.clone()),
                (
                    &self.plan.sig_url,
                    sign_b64(self.key_seed, &self.sums).into_bytes(),
                ),
            ])
        }
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn good_download_is_verified_and_staged() {
        let w = world();
        let dir = tempfile::tempdir().unwrap();
        let staged = stage(
            &w.curl(),
            Path::new("curl"),
            &w.plan,
            Some(public(1)),
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(staged.setup_path, dir.path().join(&w.plan.setup_name));
        assert_eq!(std::fs::read(&staged.setup_path).unwrap(), w.setup);
        assert_eq!(staged.size, w.setup.len() as u64);
        assert_eq!(staged.sha256, verify::sha256_bytes(&w.setup));
        assert_eq!(recheck(&staged), Ok(()));
        assert_eq!(
            leftovers(dir.path())
                .iter()
                .filter(|n| n.ends_with(".part"))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn a_swapped_installer_is_refused_and_removed() {
        let mut w = world();
        let curl = w.curl();
        w.setup = b"MZ something else".to_vec(); // signed list still names the original hash
        let curl = FakeCurl::new(vec![
            (&w.plan.setup_url, w.setup.clone()),
            (&w.plan.sums_url, curl.bodies[&w.plan.sums_url].clone()),
            (&w.plan.sig_url, curl.bodies[&w.plan.sig_url].clone()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let err = stage(
            &curl,
            Path::new("curl"),
            &w.plan,
            Some(public(1)),
            dir.path(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, InstallError::Verify(VerifyError::HashMismatch));
        assert!(!dir.path().join(&w.plan.setup_name).exists());
        assert!(
            leftovers(dir.path()).is_empty(),
            "{:?}",
            leftovers(dir.path())
        );
    }

    #[tokio::test]
    async fn a_checksum_list_signed_by_another_key_is_refused_before_the_installer_is_fetched() {
        let w = world();
        let dir = tempfile::tempdir().unwrap();
        let curl = w.curl();
        let err = stage(
            &curl,
            Path::new("curl"),
            &w.plan,
            Some(public(2)),
            dir.path(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, InstallError::Verify(VerifyError::BadSignature));
        assert!(
            !curl.calls.lock().unwrap().contains(&w.plan.setup_url),
            "the installer was never downloaded"
        );
        assert!(leftovers(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn no_key_built_in_means_nothing_is_installed() {
        let w = world();
        let dir = tempfile::tempdir().unwrap();
        let err = stage(&w.curl(), Path::new("curl"), &w.plan, None, dir.path())
            .await
            .unwrap_err();
        assert_eq!(err, InstallError::Verify(VerifyError::NoKey));
        assert!(leftovers(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn tampered_list_and_missing_entry_are_refused() {
        let mut w = world();
        let sig_for_original = sign_b64(1, &w.sums);
        // Attacker edits the list (new hash) but cannot re-sign it.
        let edited: Vec<u8> = String::from_utf8(w.sums.clone())
            .unwrap()
            .replacen('0', "1", 1)
            .into_bytes();
        let curl = FakeCurl::new(vec![
            (&w.plan.sums_url, edited),
            (&w.plan.sig_url, sig_for_original.into_bytes()),
            (&w.plan.setup_url, w.setup.clone()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            stage(
                &curl,
                Path::new("curl"),
                &w.plan,
                Some(public(1)),
                dir.path()
            )
            .await
            .unwrap_err(),
            InstallError::Verify(VerifyError::BadSignature)
        );
        // A correctly signed list that does not mention the installer.
        w.sums = format!("{}  other.exe\n", "a".repeat(64)).into_bytes();
        let err = stage(
            &w.curl(),
            Path::new("curl"),
            &w.plan,
            Some(public(1)),
            dir.path(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, InstallError::NoEntry(w.plan.setup_name.clone()));
        // A correctly signed but malformed list.
        w.sums = b"garbage\n".to_vec();
        assert!(matches!(
            stage(
                &w.curl(),
                Path::new("curl"),
                &w.plan,
                Some(public(1)),
                dir.path()
            )
            .await
            .unwrap_err(),
            InstallError::Sums(_)
        ));
    }

    #[tokio::test]
    async fn download_failures_and_redirects_away_are_errors() {
        let w = world();
        let dir = tempfile::tempdir().unwrap();
        let mut curl = w.curl();
        curl.fail_url = Some(w.plan.sig_url.clone());
        assert!(matches!(
            stage(
                &curl,
                Path::new("curl"),
                &w.plan,
                Some(public(1)),
                dir.path()
            )
            .await
            .unwrap_err(),
            InstallError::Download(_)
        ));
        let curl = w.curl();
        *curl.effective.lock().unwrap() = Some("https://evil.example/x".into());
        assert_eq!(
            stage(
                &curl,
                Path::new("curl"),
                &w.plan,
                Some(public(1)),
                dir.path()
            )
            .await
            .unwrap_err(),
            InstallError::Download("redirected away from GitHub".into())
        );
        assert!(leftovers(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn oversized_files_are_refused() {
        let mut w = world();
        w.sums = vec![b'x'; (MAX_SUMS_BYTES + 1) as usize];
        let dir = tempfile::tempdir().unwrap();
        let err = stage(
            &w.curl(),
            Path::new("curl"),
            &w.plan,
            Some(public(1)),
            dir.path(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, InstallError::TooLarge);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn recheck_catches_a_file_changed_after_verification() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("setup.exe");
        std::fs::write(&p, b"original").unwrap();
        let (hash, size) = verify::sha256_file(&p).unwrap();
        let staged = Staged {
            setup_path: p.clone(),
            sha256: hash,
            size,
        };
        assert_eq!(recheck(&staged), Ok(()));
        std::fs::write(&p, b"swapped!").unwrap();
        assert_eq!(
            recheck(&staged),
            Err(InstallError::Verify(VerifyError::HashMismatch))
        );
        std::fs::remove_file(&p).unwrap();
        assert!(matches!(recheck(&staged), Err(InstallError::Io(_))));
    }

    #[test]
    fn download_arguments_are_https_only_bounded_and_credential_free() {
        let a = download_args("https://github.com/x", Path::new("out.part"), 1234);
        assert!(a.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(a.windows(2).any(|w| w == ["--proto-redir", "=https"]));
        assert!(a.windows(2).any(|w| w == ["--max-filesize", "1234"]));
        assert!(a.windows(2).any(|w| w == ["--max-redirs", "3"]));
        assert!(a.contains(&"--fail".to_owned()));
        assert!(!a.iter().any(|x| x.to_lowercase().contains("authorization")));
        assert_eq!(a.last().unwrap(), "https://github.com/x");
    }
}
