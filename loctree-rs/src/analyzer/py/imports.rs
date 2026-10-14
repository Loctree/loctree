//! Python import resolution.
//!
//! Handles resolution of Python imports (absolute and relative) to their
//! source files, with stdlib detection.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::resolvers::{resolve_python_absolute, resolve_python_relative};
use crate::types::ImportResolutionKind;

/// Resolve a Python import to its source file and determine its resolution kind.
///
/// Returns a tuple of (resolved_path, resolution_kind):
/// - resolved_path: The absolute path to the resolved module, or None if not found
/// - resolution_kind: Whether this is a Local, Stdlib, or Unknown import
pub(super) fn resolve_python_import(
    module: &str,
    file_path: &Path,
    root: &Path,
    py_roots: &[PathBuf],
    extensions: Option<&HashSet<String>>,
    stdlib: &HashSet<String>,
) -> (Option<String>, ImportResolutionKind) {
    if module.starts_with('.') {
        let resolved = resolve_python_relative(module, file_path, root, extensions);
        let kind = if resolved.is_some() {
            ImportResolutionKind::Local
        } else {
            ImportResolutionKind::Unknown
        };
        return (resolved, kind);
    }

    // G10.2: check stdlib BEFORE local filesystem resolution. `import os` in
    // a repo that also happens to have an unrelated `src/os/` directory used
    // to resolve locally first (resolve_python_absolute ran before this
    // check), silently misclassifying a stdlib import as a local one just
    // because a same-named directory existed somewhere under a py_root.
    // Stdlib top-level names win unconditionally for absolute imports; a
    // project that genuinely wants to shadow a stdlib name locally still
    // reaches it via an explicit relative import (`.os`), handled above.
    let head = module.split('.').next().unwrap_or(module).to_lowercase();
    if stdlib.contains(&head) {
        return (None, ImportResolutionKind::Stdlib);
    }

    if let Some(resolved) = resolve_python_absolute(module, py_roots, root, extensions) {
        return (Some(resolved), ImportResolutionKind::Local);
    }

    (None, ImportResolutionKind::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn py_exts() -> HashSet<String> {
        ["py"].iter().map(|s| s.to_string()).collect()
    }

    fn stdlib() -> HashSet<String> {
        ["sys", "os", "json", "typing"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn resolves_stdlib_module() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("main.py");

        let (resolved, kind) = resolve_python_import(
            "sys",
            &path,
            root,
            &[root.to_path_buf()],
            Some(&py_exts()),
            &stdlib(),
        );

        assert!(resolved.is_none());
        assert_eq!(kind, ImportResolutionKind::Stdlib);
    }

    #[test]
    fn g10_2_resolves_stdlib_over_colliding_local_dir() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        // An unrelated local package that happens to shadow the stdlib
        // module name — this must NOT win over the stdlib classification.
        std::fs::create_dir_all(root.join("os")).expect("mkdir");
        std::fs::write(root.join("os/__init__.py"), "").expect("write __init__");
        let path = root.join("main.py");

        let (resolved, kind) = resolve_python_import(
            "os",
            &path,
            root,
            &[root.to_path_buf()],
            Some(&py_exts()),
            &stdlib(),
        );

        assert!(
            resolved.is_none(),
            "expected `import os` to resolve to stdlib, not the colliding local os/ dir, got: {resolved:?}"
        );
        assert_eq!(kind, ImportResolutionKind::Stdlib);
    }

    #[test]
    fn resolves_local_module() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("foo.py"), "VALUE = 1").expect("write foo.py");
        let path = root.join("main.py");

        let (resolved, kind) = resolve_python_import(
            "foo",
            &path,
            root,
            &[root.to_path_buf()],
            Some(&py_exts()),
            &stdlib(),
        );

        assert!(resolved.is_some());
        assert!(resolved.unwrap().ends_with("foo.py"));
        assert_eq!(kind, ImportResolutionKind::Local);
    }

    #[test]
    fn resolves_relative_import() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir");
        std::fs::write(root.join("pkg/__init__.py"), "").expect("write __init__");
        std::fs::write(root.join("pkg/helper.py"), "def help(): pass").expect("write helper");
        let path = root.join("pkg/main.py");

        let (resolved, kind) = resolve_python_import(
            ".helper",
            &path,
            root,
            &[root.to_path_buf()],
            Some(&py_exts()),
            &stdlib(),
        );

        assert!(resolved.is_some());
        assert!(resolved.unwrap().contains("helper.py"));
        assert_eq!(kind, ImportResolutionKind::Local);
    }

    #[test]
    fn unknown_for_unresolved_module() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("main.py");

        let (resolved, kind) = resolve_python_import(
            "nonexistent_package",
            &path,
            root,
            &[root.to_path_buf()],
            Some(&py_exts()),
            &stdlib(),
        );

        assert!(resolved.is_none());
        assert_eq!(kind, ImportResolutionKind::Unknown);
    }
}
