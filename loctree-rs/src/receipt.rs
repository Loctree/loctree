//! Query receipt identity — binds every query answer to the exact repo,
//! snapshot, and binary state that produced it (Trust Repair W1-A).
//!
//! # Schema — `loctree.receipt.v1`
//!
//! Every [`QueryReceipt`] binds five identities. A receipt that cannot prove
//! one of them says so explicitly (`None` / [`ReceiptAuthority::Unknown`])
//! instead of polishing the gap into a green zero:
//!
//! | Field | Meaning | Source |
//! |---|---|---|
//! | `root` | Canonical project root the query ran against | filesystem canonicalize |
//! | `head_full` | Full 40-hex live git HEAD at receipt time | `GitRepo::head_commit` |
//! | `dirty_fingerprint` | `clean` or `dirty:<n>:sha256:<hex16>` over `git status --porcelain` | live git |
//! | `snapshot_fingerprint` | Structural fingerprint of the snapshot backing the answer (`loctree-snapshot-authority-v1`) | [`Snapshot::fingerprint_report`] |
//! | `binary_id` | Checkout-identity stamp of the binary that answered (`<semver>+g<sha>[.dirty]`) | [`crate::BUILD_VERSION`] |
//!
//! `authority` is the fail-closed verdict: `fresh` is only granted when the
//! live HEAD and the snapshot's recorded commit are identity-compatible.
//! Mismatch → `stale`. No snapshot → `refused`. Missing git identity →
//! `unknown`. Wrong-commit receipts can never claim `fresh` (audit class
//! LCT-D / LCT-E / LCT-L).

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::snapshot::Snapshot;

/// Wire-schema identifier for the receipt payload.
pub const RECEIPT_SCHEMA_VERSION: &str = "loctree.receipt.v1";

/// Fail-closed authority verdict carried by every receipt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptAuthority {
    /// Live HEAD and snapshot commit are identity-compatible.
    Fresh,
    /// Snapshot commit differs from live HEAD — answers describe another tree.
    Stale,
    /// No snapshot backs this answer; structural authority is refused.
    Refused,
    /// Identity could not be established (no git, no commit recorded).
    #[default]
    Unknown,
}

/// Identity receipt bound to a single query answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryReceipt {
    pub schema_version: String,
    pub root: Option<String>,
    pub head_full: Option<String>,
    pub dirty_fingerprint: Option<String>,
    pub snapshot_fingerprint: Option<String>,
    /// Git commit recorded in the snapshot at scan time (short form).
    pub snapshot_commit: Option<String>,
    pub binary_id: String,
    pub authority: ReceiptAuthority,
    /// Human-readable reasons whenever authority is not `fresh`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    /// How many secret/token/IP/home-path replacements the AICX overlay
    /// scrub applied before this pack was marked commitable. Zero is omitted
    /// from the wire so legacy receipts stay byte-compatible.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub redactions: u32,
}

impl QueryReceipt {
    /// Receipt skeleton with binary identity only — used before any repo
    /// probing has happened. Authority stays `Unknown`.
    pub fn unbound() -> Self {
        Self {
            schema_version: RECEIPT_SCHEMA_VERSION.to_string(),
            binary_id: crate::BUILD_VERSION.to_string(),
            ..Self::default()
        }
    }

    /// Bind a receipt to live git state and (optionally) the snapshot that
    /// backs the answer. `snapshot: None` means no structural authority
    /// exists at all — the receipt is `Refused`, never silently empty.
    pub fn bind(root: &Path, snapshot: Option<&Snapshot>) -> Self {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let head_full = live_head_full(&canonical_root);
        let dirty_fingerprint = dirty_fingerprint(&canonical_root);

        let mut receipt = Self::unbound();
        receipt.root = Some(canonical_root.display().to_string());
        receipt.head_full = head_full.clone();
        receipt.dirty_fingerprint = dirty_fingerprint;

        match snapshot {
            None => {
                receipt.authority = ReceiptAuthority::Refused;
                receipt
                    .diagnostics
                    .push("no snapshot loaded; structural authority refused".to_string());
            }
            Some(snapshot) => {
                let fingerprint = snapshot.fingerprint_report();
                receipt.snapshot_fingerprint =
                    Some(format!("{}:{}", fingerprint.algorithm, fingerprint.value));
                receipt.snapshot_commit = snapshot.metadata.git_commit.clone();

                let (authority, diagnostic) = identity_verdict(
                    head_full.as_deref(),
                    snapshot.metadata.git_commit.as_deref(),
                );
                receipt.authority = authority;
                if let Some(diagnostic) = diagnostic {
                    receipt.diagnostics.push(diagnostic);
                }
            }
        }

        if receipt
            .dirty_fingerprint
            .as_deref()
            .is_some_and(|fp| fp != "clean")
        {
            receipt
                .diagnostics
                .push("worktree is dirty; snapshot may lag uncommitted edits".to_string());
        }

        receipt
    }
}

/// Pure verdict: can this (live HEAD, snapshot commit) pair claim `fresh`?
///
/// Both identities must be present and identity-compatible (mutual prefix,
/// same rule as the staleness gates elsewhere). Anything else degrades
/// honestly instead of defaulting to green.
pub fn identity_verdict(
    live_head: Option<&str>,
    snapshot_commit: Option<&str>,
) -> (ReceiptAuthority, Option<String>) {
    match (live_head, snapshot_commit) {
        (Some(head), Some(commit)) if !head.is_empty() && !commit.is_empty() => {
            if commits_identity_compatible(head, commit) {
                (ReceiptAuthority::Fresh, None)
            } else {
                (
                    ReceiptAuthority::Stale,
                    Some(format!(
                        "snapshot commit {} does not match live HEAD {}",
                        short(commit),
                        short(head)
                    )),
                )
            }
        }
        _ => (
            ReceiptAuthority::Unknown,
            Some("git identity unavailable; cannot verify snapshot freshness".to_string()),
        ),
    }
}

/// Mutual-prefix commit comparison (short vs full SHA compatibility).
pub fn commits_identity_compatible(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

fn short(sha: &str) -> &str {
    if sha.len() > 8 { &sha[..8] } else { sha }
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn live_head_full(root: &Path) -> Option<String> {
    crate::git::GitRepo::discover(root)
        .ok()
        .and_then(|repo| repo.head_commit().ok())
        .filter(|head| !head.is_empty())
}

/// Deterministic fingerprint of the dirty-worktree state.
///
/// - `Some("clean")` — porcelain status is empty.
/// - `Some("dirty:<n>:sha256:<hex16>")` — n dirty entries, hash over the
///   sorted porcelain lines.
/// - `None` — git status unavailable; explicitly unknown, never "clean".
fn dirty_fingerprint(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let mut lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return Some("clean".to_string());
    }
    lines.sort_unstable();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("dirty:{}:sha256:{}", lines.len(), &hex[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_receipt_verdict_fresh_on_mutual_prefix() {
        let (authority, diag) = identity_verdict(
            Some("aeada272868478e7e0a1c61090326694b46ed85d"),
            Some("aeada272"),
        );
        assert_eq!(authority, ReceiptAuthority::Fresh);
        assert!(diag.is_none());
    }

    #[test]
    fn identity_receipt_verdict_stale_on_wrong_commit() {
        // Audit LCT-D class: fresh receipt named develop@1cb669d4 while live
        // HEAD was 5132faae — that pair must never grade `fresh`.
        let (authority, diag) = identity_verdict(Some("5132faae"), Some("1cb669d4"));
        assert_eq!(authority, ReceiptAuthority::Stale);
        assert!(diag.unwrap().contains("does not match live HEAD"));
    }

    #[test]
    fn identity_receipt_verdict_unknown_without_git_identity() {
        for (head, commit) in [
            (None, Some("abc123")),
            (Some("abc123"), None),
            (None, None),
            (Some(""), Some("abc123")),
        ] {
            let (authority, diag) = identity_verdict(head, commit);
            assert_eq!(authority, ReceiptAuthority::Unknown, "{head:?}/{commit:?}");
            assert!(diag.is_some());
        }
    }

    #[test]
    fn identity_receipt_refused_without_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let receipt = QueryReceipt::bind(tmp.path(), None);
        assert_eq!(receipt.authority, ReceiptAuthority::Refused);
        assert_eq!(receipt.schema_version, RECEIPT_SCHEMA_VERSION);
        assert_eq!(receipt.binary_id, crate::BUILD_VERSION);
        assert!(
            receipt
                .diagnostics
                .iter()
                .any(|d| d.contains("authority refused"))
        );
    }

    #[test]
    fn identity_receipt_dirty_fingerprint_is_explicit_and_deterministic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init"]);
        assert_eq!(dirty_fingerprint(root).as_deref(), Some("clean"));

        std::fs::write(root.join("dirty.rs"), "fn main() {}\n").expect("write");
        let first = dirty_fingerprint(root).expect("fingerprint");
        assert!(first.starts_with("dirty:1:sha256:"), "{first}");
        assert_eq!(dirty_fingerprint(root).as_deref(), Some(first.as_str()));
    }

    #[test]
    fn identity_receipt_serde_roundtrip() {
        let mut receipt = QueryReceipt::unbound();
        receipt.root = Some("/tmp/repo".to_string());
        receipt.head_full = Some("aeada272868478e7e0a1c61090326694b46ed85d".to_string());
        receipt.dirty_fingerprint = Some("clean".to_string());
        receipt.snapshot_fingerprint =
            Some("sha256:loctree-snapshot-authority-v1:deadbeef".to_string());
        receipt.snapshot_commit = Some("aeada272".to_string());
        receipt.authority = ReceiptAuthority::Fresh;

        let json = serde_json::to_string(&receipt).expect("serialize");
        assert!(json.contains("\"authority\":\"fresh\""));
        let roundtrip: QueryReceipt = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(roundtrip, receipt);
    }
}
