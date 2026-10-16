use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

const HOOK: &str = "from hatchling.builders.hooks.plugin.interface import BuildHookInterface\n\nclass CommitProvenanceHook(BuildHookInterface):\n    def initialize(self, version, build_data):\n        raise RuntimeError('static analysis must not execute this hook')\n\nclass UnrelatedUtility:\n    pass\n";

fn fixture(manifest: &str, config: &str, path: &str, source: &str) -> (TempDir, TempDir) {
    let root = TempDir::new().expect("fixture root");
    let cache = TempDir::new().expect("fixture cache");
    std::fs::create_dir_all(
        root.path()
            .join(manifest)
            .parent()
            .expect("manifest parent"),
    )
    .expect("manifest directory");
    std::fs::write(root.path().join(manifest), config).expect("manifest");
    let hook = root.path().join(path);
    std::fs::create_dir_all(hook.parent().expect("hook parent")).expect("hook directory");
    std::fs::write(hook, source).expect("hook source");
    (root, cache)
}

fn query(root: &Path, cache: &Path, args: &[&str]) -> Value {
    let output = cargo_bin_cmd!("loct")
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .env("LOCT_NO_GITIGNORE", "1")
        .env("LOCT_OPEN_BROWSER", "0")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    serde_json::from_slice(&output.stdout).expect("CLI JSON")
}

fn assert_registered(manifest: &str, config: &str, path: &str, source: &str, symbols: &[&str]) {
    let (root, cache) = fixture(manifest, config, path, source);
    let dead = query(root.path(), cache.path(), &["dead", "--full", "--json"]);
    for symbol in symbols {
        let candidate = dead
            .as_array()
            .expect("dead candidates")
            .iter()
            .find(|d| d["symbol"] == *symbol)
            .unwrap_or_else(|| panic!("missing raw candidate {symbol}: {dead}"));
        assert_eq!(candidate["entrypoint"], true, "{candidate}");
        assert_eq!(candidate["confidence"], "low", "{candidate}");
        assert!(
            candidate["reason"]
                .as_str()
                .expect("candidate reason")
                .contains("Hatch custom build hook"),
            "{candidate}"
        );
    }
    let unrelated = dead
        .as_array()
        .expect("dead candidates")
        .iter()
        .find(|d| d["symbol"] == "UnrelatedUtility")
        .expect("unrelated class remains a candidate");
    assert_eq!(
        unrelated["confidence"], "high",
        "module-wide immunity: {unrelated}"
    );
    assert_eq!(unrelated["entrypoint"], false);
    let findings = query(root.path(), cache.path(), &["findings"]);
    let wins = findings["quick_wins"].as_array().expect("quick wins");
    for symbol in symbols {
        assert!(!wins.iter().any(|w| w["symbol"] == *symbol), "{wins:?}");
    }
    let follow = query(
        root.path(),
        cache.path(),
        &["follow", "all", "--limit", "6", "--json"],
    );
    fn check(value: &Value, symbols: &[&str]) {
        match value {
            Value::Object(object) => {
                if symbols
                    .iter()
                    .any(|symbol| object.get("symbol") == Some(&Value::from(*symbol)))
                {
                    assert_ne!(
                        object.get("confidence"),
                        Some(&Value::from("high")),
                        "{value}"
                    );
                }
                for value in object.values() {
                    check(value, symbols);
                }
            }
            Value::Array(values) => {
                for value in values {
                    check(value, symbols);
                }
            }
            _ => {}
        }
    }
    check(&follow, symbols);
}

#[test]
fn hatch_default_path_is_not_a_high_confidence_delete_candidate() {
    assert_registered(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\n",
        "hatch_build.py",
        HOOK,
        &["CommitProvenanceHook"],
    );
}

#[test]
fn hatch_explicit_relative_path_and_conditional_hook() {
    assert_registered(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\npath = 'tooling/provenance.py'\nenable-by-default = false\n",
        "tooling/provenance.py",
        HOOK,
        &["CommitProvenanceHook"],
    );
}

#[test]
fn hatch_target_specific_configuration() {
    assert_registered(
        "pyproject.toml",
        "[tool.hatch.build.targets.wheel.hooks.custom]\n",
        "hatch_build.py",
        HOOK,
        &["CommitProvenanceHook"],
    );
}

#[test]
fn hatch_toml_global_and_target_configuration() {
    for section in ["build.hooks.custom", "build.targets.sdist.hooks.custom"] {
        assert_registered(
            "hatch.toml",
            &format!("[{section}]\npath = 'hooks.py'\n"),
            "hooks.py",
            HOOK,
            &["CommitProvenanceHook"],
        );
    }
}

#[test]
fn hatch_factory_and_multiple_subclasses_are_not_proven_unused() {
    let multiple = format!("{HOOK}\nclass AlternativeHook(BuildHookInterface):\n    pass\n");
    assert_registered(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\n",
        "hatch_build.py",
        &multiple,
        &["CommitProvenanceHook", "AlternativeHook"],
    );
    let source = format!(
        "{HOOK}\nclass AlternativeHook(BuildHookInterface):\n    pass\n\ndef get_build_hook():\n    return AlternativeHook\n"
    );
    assert_registered(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\n",
        "hatch_build.py",
        &source,
        &["CommitProvenanceHook", "get_build_hook"],
    );
}

#[test]
fn factory_without_a_statically_resolvable_return_is_registered() {
    assert_registered(
        "hatch.toml",
        "[build.hooks.custom]\n",
        "hatch_build.py",
        "def get_build_hook():\n    return resolve_hook_dynamically()\n\nclass UnrelatedUtility:\n    pass\n",
        &["get_build_hook"],
    );
}

#[test]
fn unconfigured_hatch_filename_has_no_immunity() {
    let (root, cache) = fixture(
        "pyproject.toml",
        "[project]\nname = 'negative'\n",
        "hatch_build.py",
        HOOK,
    );
    let dead = query(root.path(), cache.path(), &["dead", "--full", "--json"]);
    let hook = dead
        .as_array()
        .expect("dead candidates")
        .iter()
        .find(|d| d["symbol"] == "CommitProvenanceHook")
        .expect("hook candidate");
    assert_eq!(hook["confidence"], "high");
    assert_eq!(hook["entrypoint"], false);
    assert_eq!(hook["action"], "delete_candidate");
}

fn assert_unregistered(root: &Path, cache: &Path) {
    let dead = query(root, cache, &["dead", "--full", "--json"]);
    let hook = dead
        .as_array()
        .expect("dead candidates")
        .iter()
        .find(|d| d["symbol"] == "CommitProvenanceHook")
        .expect("unregistered hook candidate");
    assert_eq!(hook["confidence"], "high", "{hook}");
    assert_eq!(hook["entrypoint"], false, "{hook}");
}

#[test]
fn hatch_toml_build_overrides_pyproject_without_recursive_merge() {
    let (root, cache) = fixture(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\n",
        "hatch_build.py",
        HOOK,
    );
    std::fs::write(
        root.path().join("hatch.toml"),
        "[build.targets.wheel]\npackages = ['example']\n",
    )
    .expect("Hatch build override");
    assert_unregistered(root.path(), cache.path());
}

#[test]
fn unrelated_hatch_toml_keys_preserve_pyproject_build() {
    let (root, cache) = fixture(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\n",
        "hatch_build.py",
        HOOK,
    );
    std::fs::write(
        root.path().join("hatch.toml"),
        "[envs.default]\ndependencies = []\n",
    )
    .expect("unrelated Hatch configuration");
    let dead = query(root.path(), cache.path(), &["dead", "--full", "--json"]);
    let hook = dead
        .as_array()
        .expect("dead candidates")
        .iter()
        .find(|d| d["symbol"] == "CommitProvenanceHook")
        .expect("hook candidate");
    assert_eq!(hook["entrypoint"], true, "{hook}");
}

#[test]
fn hook_paths_reject_absolute_parent_and_invalid_values() {
    for path in ["../hatch_build.py", "/hatch_build.py", "", "42"] {
        let value = if path == "42" {
            path.to_string()
        } else {
            format!("'{path}'")
        };
        let (root, cache) = fixture(
            "pyproject.toml",
            &format!("[tool.hatch.build.hooks.custom]\npath = {value}\n"),
            "hatch_build.py",
            HOOK,
        );
        assert_unregistered(root.path(), cache.path());
    }
}

#[test]
fn nested_project_manifest_keeps_symbol_scope() {
    assert_registered(
        "package/pyproject.toml",
        "[tool.hatch.build.hooks.custom]\n",
        "package/hatch_build.py",
        HOOK,
        &["CommitProvenanceHook"],
    );
}

#[cfg(unix)]
#[test]
fn symlinked_external_hook_is_not_a_local_registration() {
    let external = TempDir::new().expect("external fixture");
    std::fs::write(external.path().join("external.py"), HOOK).expect("external hook");
    let (root, cache) = fixture(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\npath = 'hook_link.py'\n",
        "hatch_build.py",
        HOOK,
    );
    std::os::unix::fs::symlink(
        external.path().join("external.py"),
        root.path().join("hook_link.py"),
    )
    .expect("external hook symlink");
    assert_unregistered(root.path(), cache.path());
}

#[test]
fn parent_manifest_cannot_register_another_repository_hook() {
    let (root, cache) = fixture(
        "pyproject.toml",
        "[tool.hatch.build.hooks.custom]\npath = 'nested/hatch_build.py'\n",
        "nested/hatch_build.py",
        HOOK,
    );
    std::fs::create_dir(root.path().join("nested/.git")).expect("nested Git boundary");
    std::fs::write(root.path().join("hatch_build.py"), HOOK).expect("parent hook");
    assert_unregistered(root.path(), cache.path());
}
