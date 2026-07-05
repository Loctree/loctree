//! Shared file IO helpers for env-truth sensors.
//!
//! Consolidates value hashing, mtime extraction, and repo-relative path
//! normalization so individual sensors stay focused on their format.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// SHA-256 hash of a plain value, truncated to 12 hex chars.
///
/// Used for `ValuePresence::Plain { value_hash }` — never the literal value
/// reaches the report. Multi-source mismatch detection compares hashes.
pub fn hash_value(value: &str) -> String {
    let mut h = Sha256::new();
    h.update(value.as_bytes());
    let digest = h.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5]
    )
}

/// File mtime as RFC 3339 UTC string, plus age in days.
///
/// Returns `None` for the timestamp string when the system clock is before
/// `UNIX_EPOCH` (which would also break `SystemTime` math). Caller decides
/// whether to skip the source or store a placeholder.
pub fn mtime_info(path: &Path) -> (Option<String>, Option<u32>) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return (None, None),
    };
    let mtime = match meta.modified() {
        Ok(t) => t,
        Err(_) => return (None, None),
    };
    let stamp = system_time_to_rfc3339(mtime);
    let now = SystemTime::now();
    let age_days = now
        .duration_since(mtime)
        .map(|d| (d.as_secs() / 86_400) as u32)
        .ok();
    (stamp, age_days)
}

fn system_time_to_rfc3339(t: SystemTime) -> Option<String> {
    let dur = t.duration_since(UNIX_EPOCH).ok()?;
    let secs = i64::try_from(dur.as_secs()).ok()?;
    let dt = OffsetDateTime::from_unix_timestamp(secs).ok()?;
    dt.format(&Rfc3339).ok()
}

/// Convert an absolute path to a slash-separated repo-relative string.
///
/// Falls back to the absolute path's display form when stripping fails (we
/// never want to abort scan over a normalization error).
pub fn relativize(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_value_is_stable_and_short() {
        let a = hash_value("super-secret");
        let b = hash_value("super-secret");
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
        assert_ne!(hash_value("super-secret"), hash_value("super-secrep"));
    }

    #[test]
    fn relativize_strips_root() {
        let root = Path::new("/repo");
        let path = Path::new("/repo/k8s/deployment.yaml");
        assert_eq!(relativize(path, root), "k8s/deployment.yaml");
    }
}
