//! Filesystem-input validation for Layer 3 semantic analyzers and the idiom
//! override loader.
//!
//! Every path that crosses into `std::fs::read_to_string` / `std::fs::read_dir`
//! from a `FileAnalysis::path` field, an `IdiomRegistry` override directory,
//! or any other not-locally-constructed source must go through one of the
//! helpers here. They enforce three rules:
//!
//! 1. Empty paths are rejected at the boundary.
//! 2. Components containing `..` (parent-dir traversal) are rejected before
//!    the OS resolves them.
//! 3. The path is canonicalized; the canonical form is what we read.
//!
//! Override-directory iteration adds a fourth rule: every entry's canonical
//! path must remain a descendant of the canonicalized override root, so a
//! symlink pointing outside the workspace cannot smuggle non-`.toml` data
//! into the registry.
//!
//! These helpers exist so `cargo clippy` plus `semgrep` see structural
//! validation at the call site instead of trusting an upstream sensor
//! invariant. A `nosemgrep` annotation would be the wrong fix; the right
//! fix is making the validation a real precondition the compiler can see.

use std::path::{Component, Path, PathBuf};

use anyhow::Context;

/// Validate a path string and canonicalize it.
///
/// Returns the canonical absolute path on success. Errors when the path is
/// empty, contains a parent-dir traversal, or fails to canonicalize. The
/// canonicalize step also resolves symlinks, so callers downstream operate
/// on the underlying real file.
fn validate_and_canonicalize(path: &str) -> anyhow::Result<PathBuf> {
    if path.is_empty() {
        anyhow::bail!("empty path supplied to semantic input validator");
    }
    let raw = PathBuf::from(path);
    if raw
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("parent-dir traversal in semantic input path: {path}");
    }
    raw.canonicalize()
        .with_context(|| format!("canonicalize semantic input path {path}"))
}

/// Read the contents of a Layer 1 sensor input file after validation.
///
/// Strict variant: any I/O or validation failure is fatal. Use this when
/// the analyzer treats unreadable inputs as a bug (`MakeSemantics` does so
/// by design — see commit `e7a7579`).
pub(crate) fn read_validated_semantic_input(path: &str) -> anyhow::Result<String> {
    let canonical = validate_and_canonicalize(path)?;
    std::fs::read_to_string(&canonical)
        .with_context(|| format!("read semantic input {}", canonical.display()))
}

/// Read the contents of a Layer 1 sensor input file after validation,
/// returning `Ok(None)` if the file vanished between scan and analysis.
///
/// Soft variant: a missing file is a Living Tree race, not a bug. Use this
/// when the analyzer can skip the file silently (`ShellSemantics` does so
/// for dispatch / source / env analysis when the underlying file is gone).
/// Validation failures other than NotFound still propagate as errors.
pub(crate) fn try_read_validated_semantic_input(path: &str) -> anyhow::Result<Option<String>> {
    if path.is_empty() {
        anyhow::bail!("empty path supplied to semantic input validator");
    }
    let raw = PathBuf::from(path);
    if raw
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("parent-dir traversal in semantic input path: {path}");
    }
    let canonical = match raw.canonicalize() {
        Ok(p) => p,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(anyhow::Error::from(err)
                .context(format!("canonicalize semantic input path {path}")));
        }
    };
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("read semantic input {}", canonical.display()))?;
    Ok(Some(content))
}

/// Enumerate `*.toml` override files inside an idiom override directory.
///
/// Every returned path is canonical and verified to be a strict descendant
/// of the canonicalized `override_dir`. Symlinks pointing outside the
/// directory are rejected explicitly. The result is sorted for
/// deterministic merge ordering across operating systems.
pub(crate) fn list_idiom_override_files(override_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let canonical_root = override_dir
        .canonicalize()
        .with_context(|| format!("canonicalize idiom override dir {}", override_dir.display()))?;

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&canonical_root)
        .with_context(|| format!("read_dir {}", canonical_root.display()))?
    {
        let entry = entry?;
        let raw = entry.path();
        let canonical = raw
            .canonicalize()
            .with_context(|| format!("canonicalize idiom override entry {}", raw.display()))?;
        if !canonical.starts_with(&canonical_root) {
            anyhow::bail!(
                "idiom override entry escapes override dir: {} -> {}",
                raw.display(),
                canonical.display()
            );
        }
        if canonical.extension().and_then(|s| s.to_str()) == Some("toml") {
            paths.push(canonical);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_path() {
        let err = read_validated_semantic_input("").unwrap_err();
        assert!(err.to_string().contains("empty path"));
        let err = try_read_validated_semantic_input("").unwrap_err();
        assert!(err.to_string().contains("empty path"));
    }

    #[test]
    fn rejects_parent_traversal() {
        let err = read_validated_semantic_input("../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("parent-dir traversal"));
        let err = try_read_validated_semantic_input("foo/../bar").unwrap_err();
        assert!(err.to_string().contains("parent-dir traversal"));
    }

    #[test]
    fn try_read_returns_none_for_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist.sh");
        let result = try_read_validated_semantic_input(&missing.to_string_lossy()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_returns_content_for_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hello.sh");
        std::fs::write(&path, "echo hi\n").unwrap();
        let content = read_validated_semantic_input(&path.to_string_lossy()).unwrap();
        assert_eq!(content, "echo hi\n");
    }

    #[test]
    fn list_idiom_overrides_returns_only_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.toml"), "").unwrap();
        std::fs::write(tmp.path().join("b.toml"), "").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "").unwrap();
        let result = list_idiom_override_files(tmp.path()).unwrap();
        assert_eq!(result.len(), 2);
        assert!(
            result
                .iter()
                .all(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        );
    }

    #[test]
    fn list_idiom_overrides_rejects_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("evil.toml"), "").unwrap();
        let link = tmp.path().join("link.toml");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path().join("evil.toml"), &link).unwrap();
            let err = list_idiom_override_files(tmp.path()).unwrap_err();
            assert!(
                err.to_string().contains("escapes override dir"),
                "expected escape error, got: {err}"
            );
        }
        #[cfg(not(unix))]
        {
            // Windows symlinks need privilege; skip the negative test there.
            let _ = link;
        }
    }
}
