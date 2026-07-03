//! SOPS-encrypted file marker sensor — presence + age only.
//!
//! Reads only the file head (16 KiB) to detect SOPS markers and surface a
//! single `EncryptedDecodeBlocked` declaration per file. We deliberately do
//! NOT decode anything, even when keys are locally available: env-truth is
//! a read-only audit, and decoded values would leak through any output
//! channel.
//!
//! Detection markers:
//! - YAML / JSON: a top-level `sops:` block (encrypted file metadata).
//! - In-band ENC markers: `ENC[AES256_GCM,...]` substrings (used for
//!   in-place encryption of individual fields).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::io::Read;
use std::path::Path;

use super::io_helpers::{mtime_info, relativize};
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

/// Inspect a file for SOPS markers. Returns a single declaration with a
/// synthetic `__sops__` name pointing at the file (the real key names live
/// inside the encrypted payload — by design we cannot enumerate them).
pub fn parse_sops_file(path: &Path, root: &Path, base_rank: u8) -> Option<(String, EnvSource)> {
    let mut head = String::new();
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 16 * 1024];
    let n = file.read(&mut buf).ok()?;
    head.push_str(&String::from_utf8_lossy(&buf[..n]));
    if !is_sops_encrypted(&head) {
        return None;
    }
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    Some((
        "__sops__".to_string(),
        EnvSource {
            kind: EnvSourceKind::SopsFile,
            path: rel,
            line: None,
            mtime: mtime.unwrap_or_default(),
            mtime_age_days: age,
            git_age_days: None,
            value_present: ValuePresence::Encrypted {
                marker: "SOPS".into(),
            },
            precedence_rank: base_rank,
        },
    ))
}

/// Detect SOPS encryption markers in a head sample.
pub fn is_sops_encrypted(head: &str) -> bool {
    head.contains("ENC[AES256_GCM")
        || head.contains("\nsops:\n")
        || head.contains("\nsops:")
        || head.starts_with("sops:")
        || head.contains("\"sops\":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn detects_sops_yaml_block() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("secrets.sops.yaml");
        fs::write(
            &path,
            "DATABASE_URL: ENC[AES256_GCM,data:abcd,iv:efgh,tag:zzz,type:str]
sops:
    kms: []
    age:
        - recipient: age1xxx
",
        )
        .unwrap();
        let result = parse_sops_file(&path, tmp.path(), 78).unwrap();
        assert!(matches!(
            result.1.value_present,
            ValuePresence::Encrypted { .. }
        ));
        assert_eq!(result.1.kind, EnvSourceKind::SopsFile);
    }

    #[test]
    fn ignores_plain_yaml() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("plain.yaml");
        fs::write(&path, "DATABASE_URL: postgres://localhost/x\n").unwrap();
        assert!(parse_sops_file(&path, tmp.path(), 78).is_none());
    }
}
