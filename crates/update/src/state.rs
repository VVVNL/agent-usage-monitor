//! What the update check remembers between runs (`update.json`): when it last asked, the ETag that
//! lets the next question be answered with "unchanged", and the releases seen. Keeping the list
//! means "an update is available" survives a restart without any network use.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use aum_store::{LoadOutcome, Paths};

use crate::release::{Release, SeenRelease, newest};
use crate::version::Version;

/// The automatic check is due this often.
pub const CHECK_INTERVAL: Duration = Duration::hours(24);
/// A manual "Check now" is answered from memory if the last real request was this recent.
pub const MANUAL_MIN_GAP: Duration = Duration::seconds(60);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    pub last_check: Option<DateTime<Utc>>,
    pub etag: Option<String>,
    pub releases: Vec<SeenRelease>,
    /// Why the last request failed, if it did (plain text for the menu and the log).
    pub last_error: Option<String>,
    /// The tag we already told the user about, so an automatic check announces a version only once.
    pub announced: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub tag: String,
    pub version: Version,
    pub prerelease: bool,
}

impl UpdateState {
    pub fn load(paths: &Paths) -> Self {
        let (state, _outcome): (Self, LoadOutcome) =
            aum_store::read_json_or_default(&paths.dir.join("update.json"));
        state
    }

    pub fn save(&self, paths: &Paths) {
        // Best effort: forgetting this only costs one extra request tomorrow.
        let _ = aum_store::write_json_atomic(&paths.dir.join("update.json"), self);
    }

    /// The newest version above `current`, from what was seen.
    pub fn available(&self, current: &Version, include_beta: bool) -> Option<Available> {
        newest(&self.releases, current, include_beta).map(|r: Release| Available {
            tag: r.tag,
            prerelease: r.prerelease || r.version.is_prerelease(),
            version: r.version,
        })
    }

    /// Is the automatic once-a-day check due? A last-check time in the future (the clock was moved
    /// back) counts as due, so a wrong clock cannot silence the check forever.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        match self.last_check {
            None => true,
            Some(last) => last > now || now - last >= CHECK_INTERVAL,
        }
    }

    /// May a manual check send a request, or should it be answered from memory?
    pub fn manual_check_allowed(&self, now: DateTime<Utc>) -> bool {
        match self.last_check {
            None => true,
            Some(last) => last > now || now - last >= MANUAL_MIN_GAP,
        }
    }
}

/// The release page for a tag, built from the repository constant and a tag that already passed
/// `Version::parse`, never taken from the server's `html_url`.
pub fn release_page(tag: &str) -> Option<String> {
    Version::parse(tag).ok()?;
    Some(format!(
        "https://github.com/{}/releases/tag/{tag}",
        crate::fetch::REPO
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 25, h, 0, 0).unwrap()
    }

    #[test]
    fn due_when_never_checked_or_a_day_has_passed() {
        let mut s = UpdateState::default();
        assert!(s.is_due(t(10)));
        s.last_check = Some(t(10));
        assert!(!s.is_due(t(11)));
        assert!(s.is_due(t(10) + CHECK_INTERVAL));
    }

    #[test]
    fn a_clock_set_back_does_not_silence_the_check() {
        let s = UpdateState {
            last_check: Some(t(23)),
            ..Default::default()
        };
        assert!(s.is_due(t(1)));
    }

    #[test]
    fn manual_checks_are_rate_limited_to_one_a_minute() {
        let s = UpdateState {
            last_check: Some(t(10)),
            ..Default::default()
        };
        assert!(!s.manual_check_allowed(t(10) + Duration::seconds(30)));
        assert!(s.manual_check_allowed(t(10) + Duration::seconds(61)));
    }

    #[test]
    fn available_comes_from_remembered_releases() {
        let s = UpdateState {
            releases: vec![SeenRelease {
                tag: "v0.12.0".into(),
                prerelease: true,
            }],
            ..Default::default()
        };
        let cur = Version::parse("0.11.0").unwrap();
        let a = s.available(&cur, true).unwrap();
        assert_eq!((a.tag.as_str(), a.prerelease), ("v0.12.0", true));
        assert!(s.available(&cur, false).is_none());
        assert!(
            s.available(&Version::parse("0.12.0").unwrap(), true)
                .is_none()
        );
    }

    #[test]
    fn state_round_trips_and_survives_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        assert_eq!(UpdateState::load(&paths), UpdateState::default());
        let s = UpdateState {
            last_check: Some(t(9)),
            etag: Some("\"abc\"".into()),
            releases: vec![SeenRelease {
                tag: "v0.12.0".into(),
                prerelease: true,
            }],
            last_error: None,
            announced: Some("v0.12.0".into()),
        };
        s.save(&paths);
        assert_eq!(UpdateState::load(&paths), s);
        std::fs::write(dir.path().join("update.json"), "{nope").unwrap();
        assert_eq!(UpdateState::load(&paths), UpdateState::default());
    }

    #[test]
    fn release_page_is_built_from_the_tag_only() {
        assert_eq!(
            release_page("v0.12.0").unwrap(),
            "https://github.com/VVVNL/agent-usage-monitor/releases/tag/v0.12.0"
        );
        assert_eq!(release_page("v1.0.0/../../evil"), None);
        assert_eq!(release_page("https://evil.example/x"), None);
    }
}
