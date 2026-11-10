//! Requested-root source evidence, exercised through real parser + CLI subprocesses.
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write fixture");
}

fn run(root: &Path, cwd: &Path, cache: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_loct"))
        .current_dir(cwd)
        .env("LOCT_CACHE_DIR", cache)
        .env("LOCT_NO_GITIGNORE", "1")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(["--quiet"])
        .args(args)
        .arg(root)
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON")
}

fn group<'a>(value: &'a Value, name: &str) -> &'a Value {
    value["exact_twins"]
        .as_array()
        .expect("twins")
        .iter()
        .find(|g| g["name"] == name)
        .unwrap_or_else(|| panic!("missing {name}: {value}"))
}

#[test]
fn randomized_registries_in_separate_cli_processes_emit_bit_exact_similarity() {
    let project = tempfile::tempdir().expect("project");
    let empty = tempfile::tempdir().expect("empty cwd");
    let cache = tempfile::tempdir().expect("cache");
    for (i, (params, result)) in [
        ("a: Alpha, b: Beta, c: Gamma", "Delta"),
        ("a: Alpha", "Beta"),
        ("a: Gamma", "Delta"),
        ("a: Alpha, b: Beta, c: Gamma", "Delta"),
        ("a: Zeta", "Theta"),
        ("a: Alpha", "Delta"),
        ("a: Beta", "Gamma"),
        ("a: Alpha, b: Delta", "Theta"),
    ]
    .iter()
    .enumerate()
    {
        write(
            project.path(),
            &format!("src/shape_{i}.rs"),
            &format!("pub fn compareEvidence({params}) -> {result} {{ todo!() }}\n"),
        );
    }
    let expected = run(
        project.path(),
        project.path(),
        cache.path(),
        &["twins", "--json"],
    );
    let similarity = group(&expected, "compareEvidence")["signature_similarity"]
        .as_f64()
        .expect("nontrivial mean");
    assert!(similarity > 0.0 && similarity < 1.0);
    for i in 0..24 {
        let cwd = if i % 2 == 0 {
            project.path()
        } else {
            empty.path()
        };
        let actual = run(
            project.path(),
            cwd,
            cache.path(),
            &["--no-scan", "twins", "--json"],
        );
        assert_eq!(
            expected, actual,
            "complete payload from independent process {i}"
        );
    }
}

#[test]
fn rust_parser_twins_ignore_foreign_comments_and_keep_attribute_declarations() {
    let project = tempfile::tempdir().expect("project");
    let foreign = tempfile::tempdir().expect("foreign");
    let empty = tempfile::tempdir().expect("empty");
    let cache = tempfile::tempdir().expect("cache");
    for file in ["src/a.rs", "src/b.rs"] {
        write(
            project.path(),
            file,
            "#[derive(Debug)] pub struct Attributed;\npub fn useful(input: SharedInput) -> SharedOutput { todo!() }\n",
        );
        write(
            foreign.path(),
            file,
            "// foreign misleading line\n// foreign misleading line\n",
        );
    }
    let expected = run(
        project.path(),
        project.path(),
        cache.path(),
        &["twins", "--json"],
    );
    assert_eq!(group(&expected, "useful")["class"], "EXACT");
    assert_eq!(group(&expected, "useful")["exclude_from_score"], false);
    assert_eq!(group(&expected, "Attributed")["locations"][0]["line"], 1);
    for cwd in [empty.path(), foreign.path()] {
        let actual = run(
            project.path(),
            cwd,
            cache.path(),
            &["--no-scan", "twins", "--json"],
        );
        assert_eq!(
            expected, actual,
            "all findings, classes, positions and counts must match"
        );
    }
    // Actual owned comments still fence stale snapshot declarations.
    write(
        project.path(),
        "src/a.rs",
        "// owned comment\n// owned comment\n",
    );
    let commented = run(
        project.path(),
        foreign.path(),
        cache.path(),
        &["--no-scan", "twins", "--json"],
    );
    assert!(
        commented["exact_twins"]
            .as_array()
            .expect("twins")
            .iter()
            .all(|g| g["name"] != "useful" && g["name"] != "Attributed")
    );
    // Missing and escaping owned paths cannot borrow the foreign comments.
    fs::remove_file(project.path().join("src/a.rs")).expect("remove owned source");
    let missing = run(
        project.path(),
        foreign.path(),
        cache.path(),
        &["--no-scan", "twins", "--json"],
    );
    assert_eq!(group(&missing, "useful")["exclude_from_score"], true);
    assert!(
        group(&missing, "useful")["action"]
            .as_str()
            .expect("action")
            .contains("verify owned source")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            foreign.path().join("src/a.rs"),
            project.path().join("src/a.rs"),
        )
        .expect("escaping symlink");
        let escaped = run(
            project.path(),
            foreign.path(),
            cache.path(),
            &["--no-scan", "twins", "--json"],
        );
        assert_eq!(group(&escaped, "useful")["exclude_from_score"], true);
        assert!(
            group(&escaped, "useful")["action"]
                .as_str()
                .expect("action")
                .contains("verify owned source")
        );
    }
}

#[test]
fn swiftpm_one_and_two_target_fences_use_owned_manifest_from_every_cwd() {
    for target_count in [1, 2] {
        let project = tempfile::tempdir().expect("project");
        let foreign = tempfile::tempdir().expect("foreign");
        let empty = tempfile::tempdir().expect("empty");
        let cache = tempfile::tempdir().expect("cache");
        let targets = if target_count == 1 {
            ".target(name: \"App\")"
        } else {
            ".target(name: \"App\"), .target(name: \"Kit\")"
        };
        write(
            project.path(),
            "Package.swift",
            &format!(
                "import PackageDescription\nlet package = Package(name: \"Fixture\", targets: [{targets}])\n"
            ),
        );
        write(
            foreign.path(),
            "Package.swift",
            "let package = Package(targets: [.target(name: \"Wrong\"), .target(name: \"Other\")])\n",
        );
        if target_count == 2 {
            write(
                foreign.path(),
                "Package.swift",
                "let package = Package(targets: [.target(name: \"Wrong\")])\n",
            );
        }
        for file in ["Sources/App/A.swift", "Sources/App/B.swift"] {
            write(
                project.path(),
                file,
                "public struct ViewModel {\n public var text: String\n}\n",
            );
            write(foreign.path(), file, "// wrong\n// wrong\n// wrong\n");
        }
        let expected = run(
            project.path(),
            project.path(),
            cache.path(),
            &["twins", "--json"],
        );
        let text = group(&expected, "text");
        assert_eq!(text["single_module_target"], target_count == 1);
        assert_eq!(text["exclude_from_score"], target_count == 1);
        for cwd in [empty.path(), foreign.path()] {
            assert_eq!(
                expected,
                run(
                    project.path(),
                    cwd,
                    cache.path(),
                    &["--no-scan", "twins", "--json"]
                )
            );
        }
    }
}
