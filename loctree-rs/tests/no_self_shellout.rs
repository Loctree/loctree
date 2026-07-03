//! Architectural guard: the loctree library and its binaries must never shell
//! out to their own CLI (`loct` / `loctree`) as a substitute for using the
//! library APIs that already exist (`Snapshot::load`, `scan_results_from_snapshot`,
//! `loctree::analyzer::*`, etc.).
//!
//! The companion guard for the MCP surface lives in
//! `loctree-mcp/src/main.rs::tests::mcp_server_does_not_shell_out_to_loctree_cli`.
//! That test scans the MCP source for ANY subprocess bridge — appropriate for a
//! thin library wrapper. The test in this file is broader in coverage (every
//! `loctree-rs/src/**/*.rs` file) but narrower in pattern: it forbids only the
//! recursive self-call to the `loct` / `loctree` binaries. Other subprocesses
//! (`git`, `open`, `xdg-open`, `aicx` from the bounded `aicx/` module) are
//! intentional architectural choices and remain allowed.
//!
//! Forbidden patterns are constructed via `concat!` so the test file itself
//! does not match its own checks.

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn loctree_library_does_not_shell_out_to_its_own_cli() {
    let forbidden: &[&str] = &[
        concat!("Command::new(\"", "loct\")"),
        concat!("Command::new(\"", "loctree\")"),
        concat!("Command::new(\"", "loct.exe\")"),
        concat!("Command::new(\"", "loctree.exe\")"),
        concat!("Command::new(&\"", "loct\""),
        concat!("Command::new(&\"", "loctree\""),
    ];

    let src_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        src_root.is_dir(),
        "expected loctree-rs/src to exist at {}",
        src_root.display()
    );

    let mut violations: Vec<String> = Vec::new();
    visit_rust_files(&src_root, &mut |path, content| {
        for pattern in forbidden {
            if content.contains(pattern) {
                violations.push(format!("{}: contains `{}`", path.display(), pattern));
            }
        }
    });

    assert!(
        violations.is_empty(),
        "loctree must not shell out to its own CLI — use the library APIs \
         (`loctree::analyzer::*`, `Snapshot::load`, `scan_results_from_snapshot`, …).\n\
         If you genuinely need a new subprocess boundary (for an external tool, \
         not loct/loctree itself), add it to a bounded module and document why.\n\
         Violations:\n  {}",
        violations.join("\n  ")
    );
}

fn visit_rust_files(dir: &Path, sink: &mut dyn FnMut(&Path, &str)) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_rust_files(&path, sink);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(content) = fs::read_to_string(&path) {
                sink(&path, &content);
            }
        }
    }
}
