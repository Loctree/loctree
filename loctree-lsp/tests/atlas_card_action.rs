//! Integration test for the "Open Context Atlas card" code action (Plan 04).
//!
//! Covers the public surface used by clients: command name constant,
//! diagnostic-to-card mapping, atlas-missing zero-state, and arg
//! validation for the `loctree.openAtlasCard` executeCommand handler.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::{Path, PathBuf};

use loctree_lsp::actions::{
    OPEN_ATLAS_CARD_COMMAND, atlas_card_action, validate_open_atlas_card_args,
};
use tower_lsp::lsp_types::{Diagnostic, NumberOrString, Position, Range};

fn diag_with_code(code: &str) -> Diagnostic {
    Diagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 1)),
        severity: None,
        code: Some(NumberOrString::String(code.into())),
        code_description: None,
        source: Some("loctree".into()),
        message: format!("test diagnostic for {code}"),
        related_information: None,
        tags: None,
        data: None,
    }
}

fn write_card(workspace: &Path, filename: &str) {
    let dir = workspace.join(".loctree").join("context-atlas");
    std::fs::create_dir_all(&dir).expect("create context atlas dir");
    std::fs::write(dir.join(filename), format!("# {filename}\n"))
        .expect("write context atlas card");
}

#[test]
fn command_name_matches_protocol_contract() {
    assert_eq!(OPEN_ATLAS_CARD_COMMAND, "loctree.openAtlasCard");
}

#[test]
fn dead_export_diagnostic_routes_to_runtime_map() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "02-runtime-map.md");
    let action =
        atlas_card_action(&diag_with_code("dead-export"), temp.path()).expect("action emitted");
    assert!(action.title.contains("02-runtime-map.md"));
    assert!(action.diagnostics.is_some());
    assert_eq!(action.diagnostics.expect("diagnostic echo").len(), 1);
}

#[test]
fn cycle_diagnostic_routes_to_structural_map() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "01-structural-map.md");
    let action =
        atlas_card_action(&diag_with_code("circular-import"), temp.path()).expect("action emitted");
    assert!(action.title.contains("01-structural-map.md"));
}

#[test]
fn twin_diagnostic_routes_to_structural_map() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "01-structural-map.md");
    let action =
        atlas_card_action(&diag_with_code("exact-twin"), temp.path()).expect("action emitted");
    assert!(action.title.contains("01-structural-map.md"));
}

#[test]
fn production_twin_export_diagnostic_routes_to_open_atlas_card_command() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "01-structural-map.md");

    let action =
        atlas_card_action(&diag_with_code("twin-export"), temp.path()).expect("action emitted");
    let command = action.command.expect("command form");

    assert_eq!(command.command, OPEN_ATLAS_CARD_COMMAND);
    assert_eq!(
        command
            .arguments
            .as_ref()
            .and_then(|args| args.first())
            .and_then(|payload| payload.get("diagnostic_code")),
        Some(&serde_json::json!("twin-export"))
    );
}

#[test]
fn unknown_diagnostic_emits_no_action() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "02-runtime-map.md");
    write_card(temp.path(), "01-structural-map.md");
    assert!(
        atlas_card_action(&diag_with_code("untagged"), temp.path()).is_none(),
        "diagnostics outside the known families must not trigger an atlas action"
    );
}

#[test]
fn atlas_missing_emits_no_action() {
    // No `.loctree/context-atlas/` exists in this tempdir — no action.
    let temp = tempfile::tempdir().expect("create temp workspace");
    assert!(
        atlas_card_action(&diag_with_code("dead-export"), temp.path()).is_none(),
        "must not offer a broken link when atlas isn't materialized"
    );
}

#[test]
fn action_command_arguments_carry_card_path_and_uri() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "02-runtime-map.md");
    let action =
        atlas_card_action(&diag_with_code("dead-export"), temp.path()).expect("action emitted");
    let cmd = action.command.expect("command form");
    assert_eq!(cmd.command, OPEN_ATLAS_CARD_COMMAND);
    let args = cmd.arguments.expect("args present");
    let payload = args[0].as_object().expect("command payload is object");

    let card_path = payload["card_path"]
        .as_str()
        .expect("card_path is a string");
    assert!(card_path.ends_with("02-runtime-map.md"));
    let card_uri = payload["card_uri"].as_str().expect("card_uri is a string");
    assert!(card_uri.starts_with("file://"));
    assert_eq!(payload["diagnostic_code"], serde_json::json!("dead-export"));
}

#[test]
fn validate_args_accepts_well_formed_payload() {
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "02-runtime-map.md");
    let card_path = temp.path().join(".loctree/context-atlas/02-runtime-map.md");

    let resolved = validate_open_atlas_card_args(
        &[serde_json::json!({
            "card_path": card_path.display().to_string(),
        })],
        temp.path(),
    )
    .expect("valid args resolve");
    // Canonicalize for comparison: tempdirs can sit behind a symlink
    // (e.g. `/var` -> `/private/var` on macOS).
    assert_eq!(
        resolved,
        card_path.canonicalize().expect("canonicalize card")
    );
}

#[test]
fn validate_args_rejects_card_outside_repo() {
    // Pointing at a path that doesn't exist must fail validation.
    let temp = tempfile::tempdir().expect("create temp workspace");
    let bogus = PathBuf::from("/tmp/loctree-card-that-does-not-exist.md");
    let err = validate_open_atlas_card_args(
        &[serde_json::json!({
            "card_path": bogus.display().to_string(),
        })],
        temp.path(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("atlas card not on disk"));
}

#[test]
fn validate_args_rejects_existing_path_outside_atlas() {
    // A file that exists on disk but lives outside the workspace atlas
    // directory must be rejected (hardening #12 — `path.exists()` alone
    // is too loose).
    let temp = tempfile::tempdir().expect("create temp workspace");
    write_card(temp.path(), "02-runtime-map.md");

    let outside = tempfile::tempdir().expect("create outside dir");
    let stray = outside.path().join("stray.md");
    std::fs::write(&stray, "# not an atlas card\n").expect("write stray file");

    let err = validate_open_atlas_card_args(
        &[serde_json::json!({
            "card_path": stray.display().to_string(),
        })],
        temp.path(),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("refusing to open path outside the workspace atlas directory")
    );
}
