//! Python file and package metadata detection.
//!
//! Handles detection of:
//! - Test files (by path and content patterns)
//! - Typed packages (py.typed marker)
//! - Namespace packages (PEP 420)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;

use super::super::resolvers::has_py_typed_marker;

/// Detect if a Python file is a test file based on path and content patterns.
pub(super) fn is_python_test_file(path: &Path, content: &str) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();

    // Path-based detection
    if path_str.contains("/tests/")
        || path_str.contains("/test/")
        || path_str.contains("/__tests__/")
        || path_str.ends_with("_test.py")
        || path_str.ends_with("_tests.py")
        || path_str.ends_with("test_.py")
        || path_str.contains("/test_")
        || path_str.contains("conftest.py")
        || path_str.contains("pytest_")
    {
        return true;
    }

    // Content-based detection: pytest imports or unittest usage
    if content.contains("import pytest")
        || content.contains("from pytest")
        || content.contains("import unittest")
        || content.contains("from unittest")
        || content.contains("@pytest.fixture")
        || content.contains("@pytest.mark")
        || content.contains("class Test")
        || content.contains("def test_")
    {
        return true;
    }

    false
}

/// Check if the file is part of a typed package (has py.typed marker upstream).
pub(super) fn check_typed_package(path: &Path, root: &Path) -> bool {
    let mut current = path.parent();
    while let Some(dir) = current {
        if has_py_typed_marker(dir) {
            return true;
        }
        // Stop at root or if we've gone above root
        if dir == root || !dir.starts_with(root) {
            break;
        }
        current = dir.parent();
    }
    false
}

/// Check if the file is part of a namespace package (no __init__.py upstream before root).
pub(super) fn check_namespace_package(path: &Path, root: &Path) -> bool {
    let mut current = path.parent();
    while let Some(dir) = current {
        // If there's an __init__.py, it's a traditional package
        if dir.join("__init__.py").exists() || dir.join("__init__.pyi").exists() {
            return false;
        }
        // If we reach root without finding __init__.py, check if it's a valid namespace
        if dir == root {
            break;
        }
        current = dir.parent();
    }
    // True if we have .py files but no __init__.py found in hierarchy
    path.parent().is_some_and(|p| {
        p.read_dir().ok().is_some_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "py" || ext == "pyi")
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_test_file_by_path() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("tests")).expect("mkdir");
        let test_path = root.join("tests/test_utils.py");
        std::fs::write(&test_path, "def test_foo(): pass").expect("write");

        assert!(is_python_test_file(&test_path, "def test_foo(): pass"));
    }

    #[test]
    fn detects_test_file_by_content() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("my_module.py");

        let content = r#"
import pytest

@pytest.fixture
def sample_fixture():
    return 42

def test_something(sample_fixture):
    assert sample_fixture == 42
"#;

        assert!(is_python_test_file(&path, content));
    }

    #[test]
    fn non_test_file() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("utils.py");

        let content = "def helper(): return 42";

        assert!(!is_python_test_file(&path, content));
    }

    #[test]
    fn detects_typed_package() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("mypackage")).expect("mkdir");
        std::fs::write(root.join("mypackage/__init__.py"), "").expect("write __init__");
        std::fs::write(root.join("mypackage/py.typed"), "").expect("write py.typed");

        let module_path = root.join("mypackage/utils.py");
        assert!(check_typed_package(&module_path, root));
    }

    #[test]
    fn detects_non_typed_package() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("mypackage")).expect("mkdir");
        std::fs::write(root.join("mypackage/__init__.py"), "").expect("write __init__");
        // No py.typed marker

        let module_path = root.join("mypackage/utils.py");
        assert!(!check_typed_package(&module_path, root));
    }

    #[test]
    fn detects_namespace_package() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        // Create namespace package (no __init__.py)
        std::fs::create_dir_all(root.join("namespace_pkg")).expect("mkdir");
        std::fs::write(root.join("namespace_pkg/module.py"), "VALUE = 1").expect("write module");

        let module_path = root.join("namespace_pkg/module.py");
        assert!(check_namespace_package(&module_path, root));
    }

    #[test]
    fn traditional_package_not_namespace() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir");
        std::fs::write(root.join("pkg/__init__.py"), "").expect("write __init__");
        std::fs::write(root.join("pkg/module.py"), "VALUE = 1").expect("write module");

        let module_path = root.join("pkg/module.py");
        assert!(!check_namespace_package(&module_path, root));
    }
}
