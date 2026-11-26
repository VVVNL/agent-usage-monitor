//! Proving a downloaded file is the one the project published.
//!
//! The release workflow signs `SHA256SUMS.txt` with an Ed25519 key kept as a CI secret and publishes
//! the signature (`SHA256SUMS.txt.sig`, base64 of the 64 raw bytes). The matching public key is
//! compiled into the program. A file is accepted only if the signature over the checksum list is valid
//! AND the file's own SHA-256 equals the list's entry for that file name. Authenticode (Windows code
//! signing) is not used because the builds are not code-signed yet.

use std::io::Read;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("no update signing key is built into this version")]
    NoKey,
    #[error("the signature file is not valid")]
    BadSignatureFormat,
    #[error("the checksum list is not signed by the project's key")]
    BadSignature,
    #[error("the downloaded installer does not match its published checksum")]
    HashMismatch,
}

/// The compiled-in public key: base64 of the 32 raw bytes, or empty when none has been set. An empty
/// or invalid key means updates can never be installed (fail closed), only announced.
pub fn parse_public_key(b64: &str) -> Option<[u8; 32]> {
    let bytes = STANDARD.decode(b64.trim()).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// Check `signature_b64` over `message` (the exact bytes of `SHA256SUMS.txt`).
pub fn verify_signature(
    public_key: Option<[u8; 32]>,
    message: &[u8],
    signature_b64: &str,
) -> Result<(), VerifyError> {
    let key = public_key.ok_or(VerifyError::NoKey)?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| VerifyError::NoKey)?;
    let raw = STANDARD
        .decode(signature_b64.trim())
        .map_err(|_| VerifyError::BadSignatureFormat)?;
    let raw = <[u8; 64]>::try_from(raw.as_slice()).map_err(|_| VerifyError::BadSignatureFormat)?;
    // verify_strict also rejects malleable and small-order edge cases.
    key.verify_strict(message, &Signature::from_bytes(&raw))
        .map_err(|_| VerifyError::BadSignature)
}

/// SHA-256 and size of a file, read in chunks.
pub fn sha256_file(path: &Path) -> std::io::Result<([u8; 32], u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hasher.finalize().into(), total))
}

pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

#[cfg(test)]
pub(crate) mod testkeys {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    /// A throwaway key for tests only (never used for releases).
    pub fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
    pub fn public(seed: u8) -> [u8; 32] {
        signing_key(seed).verifying_key().to_bytes()
    }
    pub fn sign_b64(seed: u8, message: &[u8]) -> String {
        STANDARD.encode(signing_key(seed).sign(message).to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::testkeys::*;
    use super::*;

    const MSG: &[u8] = b"hash  agent-usage-monitor-0.12.0-setup.exe\r\n";

    #[test]
    fn a_valid_signature_is_accepted() {
        assert_eq!(
            verify_signature(Some(public(1)), MSG, &sign_b64(1, MSG)),
            Ok(())
        );
        // surrounding whitespace in the .sig file (trailing newline) is fine
        assert_eq!(
            verify_signature(Some(public(1)), MSG, &format!("{}\n", sign_b64(1, MSG))),
            Ok(())
        );
    }

    #[test]
    fn every_way_of_being_wrong_is_rejected() {
        let good = sign_b64(1, MSG);
        assert_eq!(
            verify_signature(Some(public(2)), MSG, &good),
            Err(VerifyError::BadSignature),
            "wrong key"
        );
        assert_eq!(
            verify_signature(Some(public(1)), b"changed", &good),
            Err(VerifyError::BadSignature),
            "tampered list"
        );
        assert_eq!(
            verify_signature(None, MSG, &good),
            Err(VerifyError::NoKey),
            "no key configured"
        );
        for bad in [
            "",
            "not base64!!",
            "AAAA",
            &STANDARD.encode([0u8; 63]),
            &STANDARD.encode([0u8; 65]),
        ] {
            assert_eq!(
                verify_signature(Some(public(1)), MSG, bad),
                Err(VerifyError::BadSignatureFormat),
                "{bad:?}"
            );
        }
        assert_eq!(
            verify_signature(Some(public(1)), MSG, &STANDARD.encode([0u8; 64])),
            Err(VerifyError::BadSignature),
            "all-zero signature"
        );
    }

    #[test]
    fn public_key_text_must_be_exactly_32_bytes() {
        assert_eq!(parse_public_key(""), None);
        assert_eq!(parse_public_key("   "), None);
        assert_eq!(parse_public_key(&STANDARD.encode([1u8; 31])), None);
        assert_eq!(parse_public_key(&STANDARD.encode([1u8; 33])), None);
        assert_eq!(
            parse_public_key(&format!("{}\n", STANDARD.encode([9u8; 32]))),
            Some([9u8; 32])
        );
    }

    #[test]
    fn file_hash_matches_a_known_digest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        let (h, n) = sha256_file(&p).unwrap();
        assert_eq!(n, 3);
        // SHA-256("abc")
        assert_eq!(h[..4], [0xba, 0x78, 0x16, 0xbf]);
        assert_eq!(sha256_bytes(b"abc"), h);
        assert!(sha256_file(&dir.path().join("missing")).is_err());
    }

    /// The release workflow signs with `openssl pkeyutl -sign -rawin`; this proves the program accepts
    /// that output and the documented public-key extraction (fixtures made with a discarded key).
    #[test]
    fn openssl_signature_of_the_real_checksum_list_verifies() {
        const SUMS: &[u8] = include_bytes!("../../../tests/fixtures/update/SHA256SUMS.txt");
        const SIG: &str = include_str!("../../../tests/fixtures/update/SHA256SUMS.txt.sig");
        const PUB: &str = include_str!("../../../tests/fixtures/update/test_public_key.b64");
        let key = parse_public_key(PUB);
        assert!(
            key.is_some(),
            "the documented extraction gives a 32-byte key"
        );
        assert_eq!(verify_signature(key, SUMS, SIG), Ok(()));
        // One changed byte (CRLF to LF) and the signature no longer holds.
        let lf: Vec<u8> = SUMS.iter().copied().filter(|b| *b != b'\r').collect();
        assert_eq!(
            verify_signature(key, &lf, SIG),
            Err(VerifyError::BadSignature)
        );
        // The project key (unset or different) does not accept it.
        assert_eq!(
            verify_signature(Some(public(1)), SUMS, SIG),
            Err(VerifyError::BadSignature)
        );
    }
}
