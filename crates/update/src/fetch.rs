//! Asking GitHub for the list of releases, through the operating system's `curl`.
//!
//! Why `curl` and not a linked HTTP library: the program contains no HTTP client (see
//! docs/PRIVACY.md), TLS and the certificate store belong to the OS, and the process runner already
//! gives timeouts and cleanup. Every argument is fixed here; nothing from a response is ever put
//! on a command line except an ETag we validated.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aum_providers::runner::{CommandRunner, RunError};

/// The repository releases are read from. A fork changes this one line.
pub const REPO: &str = "VVVNL/agent-usage-monitor";

/// A handful is enough, and the runner keeps only the first 256 KB of output, so a long listing
/// would be cut off and fail to parse.
const LIST_URL_TAIL: &str = "releases?per_page=5";
const TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The ETag we sent still matches: nothing changed since the last answer.
    NotModified,
    Body {
        json: String,
        etag: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("curl is not available on this computer")]
    CurlMissing,
    #[error("GitHub did not answer in time")]
    Timeout,
    #[error("GitHub is limiting requests from this network; try again later")]
    RateLimited,
    #[error("the repository's releases are not publicly visible (HTTP 404)")]
    NotFound,
    #[error("GitHub answered HTTP {0}")]
    Http(u16),
    #[error("could not reach GitHub ({0})")]
    Transport(String),
    #[error("unreadable answer from GitHub")]
    Garbled,
}

pub fn list_url() -> String {
    format!("https://api.github.com/repos/{REPO}/{LIST_URL_TAIL}")
}

/// The system's own `curl`, by absolute path where we know it (so a look-alike earlier on `PATH`
/// is not picked up), otherwise by name.
pub fn curl_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let p = Path::new(&root).join("System32").join("curl.exe");
            if p.is_file() {
                return p;
            }
        }
        PathBuf::from("curl.exe")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/usr/bin/curl")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let p = Path::new("/usr/bin/curl");
        if p.is_file() {
            p.to_path_buf()
        } else {
            PathBuf::from("curl")
        }
    }
}

/// Only a plain quoted entity tag is ever sent back, so an answer cannot smuggle anything else
/// into the request.
pub fn valid_etag(etag: &str) -> bool {
    let t = etag.strip_prefix("W/").unwrap_or(etag);
    t.len() >= 3
        && t.len() <= 100
        && t.starts_with('"')
        && t.ends_with('"')
        && t[1..t.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn curl_args(etag: Option<&str>) -> Vec<String> {
    curl_args_for(&list_url(), etag)
}

fn curl_args_for(url: &str, etag: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = [
        "--silent",
        "--show-error",
        "--include",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--max-time",
        "20",
        "--max-filesize",
        "600000",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
        // Generic on purpose: no version, no machine details.
        "-H",
        "User-Agent: agent-usage-monitor",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if let Some(e) = etag.filter(|e| valid_etag(e)) {
        a.push("-H".into());
        a.push(format!("If-None-Match: {e}"));
    }
    a.push(url.to_owned());
    a
}

/// Split curl's `--include` output into status, headers and body, skipping any `1xx` interim block.
pub fn parse_response(raw: &str) -> Result<Fetched, FetchError> {
    let mut rest = raw;
    loop {
        if !rest.starts_with("HTTP/") {
            return Err(FetchError::Garbled);
        }
        let (head, body) = rest
            .split_once("\r\n\r\n")
            .or_else(|| rest.split_once("\n\n"))
            .ok_or(FetchError::Garbled)?;
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse().ok())
            .ok_or(FetchError::Garbled)?;
        if (100..200).contains(&status) {
            rest = body;
            continue;
        }
        let etag = lines
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("etag"))
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| valid_etag(v));
        return match status {
            200 => Ok(Fetched::Body {
                json: body.to_owned(),
                etag,
            }),
            304 => Ok(Fetched::NotModified),
            403 | 429 => Err(FetchError::RateLimited),
            404 => Err(FetchError::NotFound),
            other => Err(FetchError::Http(other)),
        };
    }
}

pub async fn fetch_list(
    runner: &dyn CommandRunner,
    curl: &Path,
    etag: Option<&str>,
) -> Result<Fetched, FetchError> {
    fetch_list_at(runner, curl, &list_url(), etag).await
}

/// Same request against another `https://` URL; used by the live test against a public repository.
pub async fn fetch_list_at(
    runner: &dyn CommandRunner,
    curl: &Path,
    url: &str,
    etag: Option<&str>,
) -> Result<Fetched, FetchError> {
    let args = curl_args_for(url, etag);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = runner
        .run(curl, &refs, TIMEOUT)
        .await
        .map_err(|e| match e {
            RunError::NotFound(_) => FetchError::CurlMissing,
            RunError::Timeout => FetchError::Timeout,
            RunError::Io(m) => FetchError::Transport(m),
        })?;
    if !out.success {
        return Err(match out.code {
            Some(28) => FetchError::Timeout,
            Some(c) => FetchError::Transport(format!("curl exit code {c}")),
            None => FetchError::Transport("curl was stopped".to_owned()),
        });
    }
    parse_response(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_404: &str =
        include_str!("../../../tests/fixtures/github/response_404_private_repo.txt");

    #[test]
    fn a_private_repository_reads_as_not_found() {
        assert_eq!(parse_response(REAL_404), Err(FetchError::NotFound));
    }

    #[test]
    fn ok_answer_gives_body_and_etag() {
        let raw = "HTTP/2 200 \r\ncontent-type: application/json\r\nETag: W/\"abc123\"\r\n\r\n[]";
        assert_eq!(
            parse_response(raw),
            Ok(Fetched::Body {
                json: "[]".into(),
                etag: Some("W/\"abc123\"".into())
            })
        );
    }

    #[test]
    fn not_modified_rate_limit_and_other_statuses() {
        assert_eq!(
            parse_response("HTTP/2 304 \r\netag: \"a\"\r\n\r\n"),
            Ok(Fetched::NotModified)
        );
        assert_eq!(
            parse_response("HTTP/2 403 \r\n\r\n{}"),
            Err(FetchError::RateLimited)
        );
        assert_eq!(
            parse_response("HTTP/2 429 \r\n\r\n"),
            Err(FetchError::RateLimited)
        );
        assert_eq!(
            parse_response("HTTP/1.1 500 x\r\n\r\n"),
            Err(FetchError::Http(500))
        );
    }

    #[test]
    fn interim_blocks_are_skipped() {
        let raw = "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\netag: \"x\"\r\n\r\n[]";
        assert!(matches!(parse_response(raw), Ok(Fetched::Body { .. })));
    }

    #[test]
    fn garbage_is_garbled() {
        for raw in ["", "<html>", "HTTP/2", "HTTP/2 abc \r\n\r\n", "HTTP/2 200"] {
            assert_eq!(parse_response(raw), Err(FetchError::Garbled), "{raw:?}");
        }
    }

    #[test]
    fn a_hostile_etag_is_not_kept_or_sent() {
        let raw = "HTTP/2 200 \r\netag: \"a\"\r\n-H evil\r\n\r\n[]";
        let Ok(Fetched::Body { etag, .. }) = parse_response(raw) else {
            panic!()
        };
        assert_eq!(etag.as_deref(), Some("\"a\""));
        let raw = "HTTP/2 200 \r\netag: \"a\" -o /x\r\n\r\n[]";
        let Ok(Fetched::Body { etag, .. }) = parse_response(raw) else {
            panic!()
        };
        assert_eq!(etag, None);
        assert!(
            !curl_args(Some("\"a\" -o /x"))
                .iter()
                .any(|a| a.contains("If-None-Match"))
        );
    }

    #[test]
    fn arguments_are_https_only_and_bounded() {
        let a = curl_args(Some("\"abc\""));
        assert!(a.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(a.windows(2).any(|w| w[0] == "--max-time"));
        assert!(a.windows(2).any(|w| w[0] == "--max-filesize"));
        assert!(
            !a.iter().any(|x| x == "-L" || x == "--location"),
            "no redirects are followed"
        );
        assert!(a.contains(&"If-None-Match: \"abc\"".to_owned()));
        assert_eq!(a.last().unwrap(), &list_url());
        assert!(list_url().starts_with("https://api.github.com/repos/"));
        assert!(
            !a.iter().any(|x| x.contains("Authorization")),
            "no credentials, ever"
        );
    }
}
