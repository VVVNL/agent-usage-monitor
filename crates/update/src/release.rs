//! The GitHub "list releases" answer, reduced to what the check needs. Fail closed: a document
//! that is not what we expect is an error, never "no update" and never a guess.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::version::Version;

/// More than this many entries are ignored (we only ask for a handful).
const MAX_RELEASES: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenRelease {
    pub tag: String,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub prerelease: bool,
}

impl Release {
    pub fn from_seen(seen: &SeenRelease) -> Option<Self> {
        Some(Self {
            tag: seen.tag.clone(),
            version: Version::parse(&seen.tag).ok()?,
            prerelease: seen.prerelease,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("the answer is not JSON")]
    NotJson,
    #[error("expected a list of releases")]
    NotAList,
    #[error("a release entry is not what was expected ({0})")]
    BadEntry(&'static str),
}

/// Published (non-draft) releases whose tag is a version. Tags that are not versions ("nightly") are
/// skipped; an entry with the wrong shape is an error.
pub fn parse_releases(json: &str) -> Result<Vec<SeenRelease>, ParseError> {
    let value: Value = serde_json::from_str(json).map_err(|_| ParseError::NotJson)?;
    let list = value.as_array().ok_or(ParseError::NotAList)?;
    let mut out = Vec::new();
    for entry in list.iter().take(MAX_RELEASES) {
        let obj = entry
            .as_object()
            .ok_or(ParseError::BadEntry("not an object"))?;
        let tag = obj
            .get("tag_name")
            .and_then(Value::as_str)
            .ok_or(ParseError::BadEntry("tag_name"))?;
        let draft = obj
            .get("draft")
            .and_then(Value::as_bool)
            .ok_or(ParseError::BadEntry("draft"))?;
        let prerelease = obj
            .get("prerelease")
            .and_then(Value::as_bool)
            .ok_or(ParseError::BadEntry("prerelease"))?;
        if draft || Version::parse(tag).is_err() {
            continue;
        }
        out.push(SeenRelease {
            tag: tag.to_owned(),
            prerelease,
        });
    }
    Ok(out)
}

/// The highest version that is newer than `current`. Pre-releases count only with `include_beta`.
/// The highest *version* wins, not the most recently published entry, so a hotfix tagged on an old
/// line never looks newer than the current line.
pub fn newest(seen: &[SeenRelease], current: &Version, include_beta: bool) -> Option<Release> {
    seen.iter()
        .filter_map(Release::from_seen)
        .filter(|r| include_beta || !(r.prerelease || r.version.is_prerelease()))
        .filter(|r| r.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("../../../tests/fixtures/github/releases_list.json");

    fn cur(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_the_list_and_drops_drafts_and_non_versions() {
        let seen = parse_releases(LIST).unwrap();
        let tags: Vec<_> = seen.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(
            tags,
            ["v0.12.0", "v0.11.0", "v0.10.9"],
            "no draft v0.13.0, no 'nightly'"
        );
        assert!(seen.iter().all(|r| r.prerelease));
    }

    #[test]
    fn finds_the_newest_version_above_the_current_one() {
        let seen = parse_releases(LIST).unwrap();
        assert_eq!(newest(&seen, &cur("0.11.0"), true).unwrap().tag, "v0.12.0");
        assert!(
            newest(&seen, &cur("0.12.0"), true).is_none(),
            "already up to date"
        );
        assert!(
            newest(&seen, &cur("0.13.0"), true).is_none(),
            "ahead of every release"
        );
    }

    #[test]
    fn stable_channel_ignores_prereleases() {
        let seen = parse_releases(LIST).unwrap();
        assert!(newest(&seen, &cur("0.11.0"), false).is_none());
    }

    #[test]
    fn highest_version_wins_over_publication_order() {
        let json = r#"[
          {"tag_name":"v0.9.5","draft":false,"prerelease":false},
          {"tag_name":"v0.12.0","draft":false,"prerelease":false},
          {"tag_name":"v0.10.0","draft":false,"prerelease":false}]"#;
        let seen = parse_releases(json).unwrap();
        assert_eq!(newest(&seen, &cur("0.9.0"), false).unwrap().tag, "v0.12.0");
        assert_eq!(newest(&seen, &cur("0.12.0"), false), None);
    }

    #[test]
    fn a_stable_release_beats_its_own_release_candidate() {
        let json = r#"[
          {"tag_name":"v1.0.0-rc.1","draft":false,"prerelease":true},
          {"tag_name":"v1.0.0","draft":false,"prerelease":false}]"#;
        let seen = parse_releases(json).unwrap();
        assert_eq!(newest(&seen, &cur("0.11.0"), true).unwrap().tag, "v1.0.0");
    }

    #[test]
    fn a_version_tag_marked_prerelease_by_semver_alone_is_also_a_beta() {
        let json = r#"[{"tag_name":"v1.0.0-rc.1","draft":false,"prerelease":false}]"#;
        let seen = parse_releases(json).unwrap();
        assert!(newest(&seen, &cur("0.11.0"), false).is_none());
        assert!(newest(&seen, &cur("0.11.0"), true).is_some());
    }

    #[test]
    fn unexpected_documents_are_errors_not_silence() {
        assert_eq!(parse_releases("<html>"), Err(ParseError::NotJson));
        assert_eq!(
            parse_releases(r#"{"message":"Not Found","status":"404"}"#),
            Err(ParseError::NotAList)
        );
        assert!(matches!(
            parse_releases(r#"[1]"#),
            Err(ParseError::BadEntry(_))
        ));
        assert!(matches!(
            parse_releases(r#"[{"tag_name":"v1.0.0","draft":false}]"#),
            Err(ParseError::BadEntry("prerelease"))
        ));
        assert!(matches!(
            parse_releases(r#"[{"tag_name":7,"draft":false,"prerelease":false}]"#),
            Err(ParseError::BadEntry("tag_name"))
        ));
        assert_eq!(parse_releases("[]"), Ok(vec![]));
    }
}
