//! Version numbers as used in release tags ("v0.12.0", "1.0.0-beta.2"): semantic versioning with
//! the precedence rules for pre-releases. Only what the update check needs.

use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Text(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            (Ident::Num(_), Ident::Text(_)) => Ordering::Less, // numbers rank below text
            (Ident::Text(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Text(a), Ident::Text(b)) => a.cmp(b),
        }
    }
}
impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a version: {0:?}")]
pub struct VersionError(pub String);

fn number(part: &str) -> Option<u64> {
    // Digits only, no sign, no absurd length.
    (!part.is_empty() && part.len() <= 15 && part.bytes().all(|b| b.is_ascii_digit()))
        .then(|| part.parse().ok())
        .flatten()
}

impl Version {
    /// Accepts an optional leading `v`, `MAJOR.MINOR.PATCH`, an optional `-pre.release` part and an
    /// ignored `+build` part. Anything else is an error, including every character that could not
    /// appear in a tag we publish.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let err = || VersionError(text.chars().take(40).collect());
        let t = text.strip_prefix('v').unwrap_or(text);
        let t = t.split_once('+').map_or(t, |(core, _build)| core);
        let (core, pre) = match t.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (t, None),
        };
        let mut nums = core.split('.');
        let (Some(a), Some(b), Some(c), None) =
            (nums.next(), nums.next(), nums.next(), nums.next())
        else {
            return Err(err());
        };
        let (major, minor, patch) = (
            number(a).ok_or_else(err)?,
            number(b).ok_or_else(err)?,
            number(c).ok_or_else(err)?,
        );
        let pre = match pre {
            None => Vec::new(),
            Some(p) => {
                let mut out = Vec::new();
                for id in p.split('.') {
                    if id.is_empty()
                        || id.len() > 32
                        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    {
                        return Err(err());
                    }
                    out.push(match number(id) {
                        Some(n) => Ident::Num(n),
                        None => Ident::Text(id.to_owned()),
                    });
                }
                out
            }
        };
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater, // 1.0.0 is newer than 1.0.0-rc.1
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            let ids: Vec<String> = self
                .pre
                .iter()
                .map(|i| match i {
                    Ident::Num(n) => n.to_string(),
                    Ident::Text(t) => t.clone(),
                })
                .collect();
            write!(f, "-{}", ids.join("."))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_tags_and_plain_versions() {
        assert_eq!(v("v0.11.0"), v("0.11.0"));
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("v1.0.0-beta.2+build.5").to_string(), "1.0.0-beta.2");
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "v",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "1.2.-3",
            "1.2.3-",
            "1.2.3-a..b",
            "nightly",
            "1.2.3 ",
            "1.2.3;rm",
            "v1.2.3/../x",
            "1.2.99999999999999999999",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn numbers_compare_as_numbers_not_text() {
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.11.1") > v("0.11.0"));
    }

    #[test]
    fn prereleases_rank_below_their_release() {
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.1") > v("1.0.0-beta.9"));
        assert!(v("1.0.0-beta.10") > v("1.0.0-beta.2"));
        assert!(v("1.0.0-beta") < v("1.0.0-beta.1"));
        assert!(
            v("1.0.0-1") < v("1.0.0-alpha"),
            "numeric identifiers rank below text"
        );
        assert!(v("0.12.0-rc.1") > v("0.11.0"));
    }

    #[test]
    fn build_metadata_is_ignored() {
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
    }
}
