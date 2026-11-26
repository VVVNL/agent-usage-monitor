//! `SHA256SUMS.txt` as published with each release: one `<64 hex digits>  <file name>` line per file
//! (the format `sha256sum` writes). Strict: any line that is not exactly that is an error, so a
//! damaged or tampered list never half-works.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SumsError {
    #[error("line {0} of the checksum list is malformed")]
    Malformed(usize),
    #[error("the checksum list names {0} twice")]
    Duplicate(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sums {
    entries: Vec<(String, [u8; 32])>,
}

fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in text.as_bytes().chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = u8::try_from(hi * 16 + lo).ok()?;
    }
    Some(out)
}

impl Sums {
    pub fn parse(text: &str) -> Result<Self, SumsError> {
        let mut entries: Vec<(String, [u8; 32])> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let (hash, name) = line.split_once("  ").ok_or(SumsError::Malformed(i + 1))?;
            let hash = hex32(hash).ok_or(SumsError::Malformed(i + 1))?;
            if !name_ok(name) {
                return Err(SumsError::Malformed(i + 1));
            }
            if entries.iter().any(|(n, _)| n == name) {
                return Err(SumsError::Duplicate(name.to_owned()));
            }
            entries.push((name.to_owned(), hash));
        }
        Ok(Self { entries })
    }

    pub fn hash_for(&self, name: &str) -> Option<[u8; 32]> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| *h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const H2: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    #[test]
    fn parses_the_real_release_format_with_either_line_ending() {
        let text = format!(
            "{H1}  agent-usage-monitor-0.11.2-setup.exe\r\n{H2}  agent-usage-monitor-0.11.2-windows-x64-portable.zip\r\n"
        );
        let s = Sums::parse(&text).unwrap();
        let h = s.hash_for("agent-usage-monitor-0.11.2-setup.exe").unwrap();
        assert_eq!(h[0], 0x01);
        assert_eq!(h[31], 0xef);
        assert_eq!(
            s.hash_for("agent-usage-monitor-0.11.2-windows-x64-portable.zip")
                .unwrap(),
            [0xff; 32]
        );
        assert_eq!(s.hash_for("missing.exe"), None);
        assert!(Sums::parse(&text.replace("\r\n", "\n")).is_ok());
        assert!(Sums::parse("").unwrap().hash_for("x").is_none());
    }

    #[test]
    fn real_published_file_parses() {
        // SHA256SUMS.txt of release v0.11.2 (three Windows files, written by the release workflow).
        let text = include_str!("../../../tests/fixtures/github/SHA256SUMS_real.txt");
        let s = Sums::parse(text).unwrap();
        assert!(s.hash_for("agent-usage-monitor-0.11.2-setup.exe").is_some());
    }

    #[test]
    fn anything_unexpected_is_an_error() {
        for bad in [
            format!("{H1} name.exe"),            // one space
            format!("{H1}  *name.exe"),          // binary marker
            format!("{H1}  dir/name.exe"),       // path
            format!("{H1}  ..\\name.exe"),       // traversal
            format!("{H1}  .hidden"),            // leading dot
            format!("{}  name.exe", &H1[..63]),  // short hash
            format!("{}g  name.exe", &H1[..63]), // not hex
            format!("{H1}  "),                   // no name
            "garbage".to_owned(),
        ] {
            assert!(
                matches!(Sums::parse(&bad), Err(SumsError::Malformed(1))),
                "{bad:?}"
            );
        }
        let dup = format!("{H1}  a.exe\n{H2}  a.exe\n");
        assert_eq!(Sums::parse(&dup), Err(SumsError::Duplicate("a.exe".into())));
    }

    #[test]
    fn uppercase_hex_is_accepted_and_equal() {
        let s = Sums::parse(&format!("{}  a.exe", H1.to_uppercase())).unwrap();
        assert_eq!(
            s.hash_for("a.exe"),
            Sums::parse(&format!("{H1}  a.exe"))
                .unwrap()
                .hash_for("a.exe")
        );
    }
}
