//! Code action: "Open Context Atlas card" for loctree diagnostics.
//!
//! Each loctree diagnostic (dead export, cycle, twin) maps to one of
//! the atlas cards materialized at `<repo>/.loctree/context-atlas/`
//! (the per-repo layout from Plan 01). We surface that mapping as a
//! `CodeAction` with command form. The server validates the card
//! exists and returns the path; the client (VS Code, Helix, etc.)
//! opens the file via its own `loctree.openAtlasCard` handler.
//!
//! Plan 04 of the LSP roadmap.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::{CodeAction, CodeActionKind, Command, Diagnostic, NumberOrString, Url};

use crate::diagnostic_codes::atlas_card_for_diagnostic_code;

/// JSON-RPC method name the server registers under
/// `executeCommandProvider.commands`. Clients invoke this command to
/// open the resolved card.
pub const OPEN_ATLAS_CARD_COMMAND: &str = "loctree.openAtlasCard";

/// Compute the on-disk atlas directory for a workspace root.
/// Matches Plan 01's per-repo layout.
pub fn atlas_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".loctree").join("context-atlas")
}

/// Map a diagnostic's `code` field to the atlas card filename that
/// best explains the failure mode. `None` for codes the atlas doesn't
/// cover today (rather than pointing at a wrong card).
pub fn card_for_diagnostic_code(code: &str) -> Option<&'static str> {
    atlas_card_for_diagnostic_code(code)
}

/// Build the "Open Context Atlas card" code action for a single
/// diagnostic. Returns `None` when:
/// - the diagnostic has no `code`,
/// - the code doesn't map to any card,
/// - the card file isn't materialized on disk yet (atlas missing).
pub fn atlas_card_action(diag: &Diagnostic, workspace_root: &Path) -> Option<CodeAction> {
    let code_str = match diag.code.as_ref()? {
        NumberOrString::String(s) => s.as_str(),
        NumberOrString::Number(_) => return None,
    };
    let card_filename = card_for_diagnostic_code(code_str)?;
    let card_path = atlas_dir(workspace_root).join(card_filename);
    if !card_path.exists() {
        // Silent rather than offering a broken link — Plan 02's
        // `status: "missing"` already tells callers the atlas isn't ready.
        return None;
    }

    let card_uri = Url::from_file_path(&card_path).ok()?;
    let title = format!("Open Context Atlas card: {card_filename}");

    Some(CodeAction {
        title: title.clone(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        command: Some(Command {
            title,
            command: OPEN_ATLAS_CARD_COMMAND.to_string(),
            arguments: Some(vec![serde_json::json!({
                "card_path": card_path.display().to_string(),
                "card_uri": card_uri.as_str(),
                "diagnostic_code": code_str,
            })]),
        }),
        ..Default::default()
    })
}

/// Validate an `executeCommand` invocation: arguments parse, card path
/// exists, and the resolved path lives under the workspace atlas
/// directory (`<workspace_root>/.loctree/context-atlas/`). The server is
/// intentionally a no-op for opening — the client opens the file — but it
/// must not hand back an arbitrary on-disk path: any existing file would
/// otherwise pass `path.exists()` and the client would open whatever the
/// LSP returned. Returns the resolved (canonicalized) card path on
/// success so clients can pick it up from the response or their own
/// logging.
pub fn validate_open_atlas_card_args(
    args: &[serde_json::Value],
    workspace_root: &Path,
) -> Result<PathBuf, OpenAtlasCardError> {
    let first = args.first().ok_or(OpenAtlasCardError::MissingArguments)?;
    let card_path = first
        .get("card_path")
        .and_then(|v| v.as_str())
        .ok_or(OpenAtlasCardError::MissingCardPath)?;
    let path = PathBuf::from(card_path);
    if !path.exists() {
        return Err(OpenAtlasCardError::CardMissing(path));
    }

    // Path-boundary check (hardening #12): `path.exists()` alone lets any
    // existing file on disk through. Canonicalize both the requested card
    // and the atlas directory, then require the card to live under the
    // atlas boundary. Reject otherwise even if the file exists.
    let atlas_root = atlas_dir(workspace_root);
    let canonical_atlas = atlas_root
        .canonicalize()
        .map_err(|_| OpenAtlasCardError::OutsideAtlas(path.clone()))?;
    let canonical_card = path
        .canonicalize()
        .map_err(|_| OpenAtlasCardError::OutsideAtlas(path.clone()))?;
    if !canonical_card.starts_with(&canonical_atlas) {
        return Err(OpenAtlasCardError::OutsideAtlas(canonical_card));
    }

    Ok(canonical_card)
}

/// Errors surfaced by `validate_open_atlas_card_args`.
#[derive(Debug)]
pub enum OpenAtlasCardError {
    MissingArguments,
    MissingCardPath,
    CardMissing(PathBuf),
    OutsideAtlas(PathBuf),
}

impl std::fmt::Display for OpenAtlasCardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingArguments => write!(
                f,
                "loctree.openAtlasCard requires one argument object with `card_path`"
            ),
            Self::MissingCardPath => write!(
                f,
                "loctree.openAtlasCard arguments missing `card_path` string"
            ),
            Self::CardMissing(path) => write!(
                f,
                "atlas card not on disk: {} — re-run `loct auto`",
                path.display()
            ),
            Self::OutsideAtlas(path) => write!(
                f,
                "refusing to open path outside the workspace atlas directory: {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for OpenAtlasCardError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position, Range};

    fn diag_with_code(code: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            severity: None,
            code: Some(NumberOrString::String(code.into())),
            code_description: None,
            source: Some("loctree".into()),
            message: format!("test {code}"),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    #[test]
    fn card_mapping_covers_dead_family() {
        assert_eq!(
            card_for_diagnostic_code("dead-export"),
            Some("02-runtime-map.md")
        );
        assert_eq!(
            card_for_diagnostic_code("dead_export"),
            Some("02-runtime-map.md")
        );
        assert_eq!(
            card_for_diagnostic_code("dead_parrot"),
            Some("02-runtime-map.md")
        );
    }

    #[test]
    fn card_mapping_covers_cycle_family() {
        assert_eq!(
            card_for_diagnostic_code("cycle"),
            Some("01-structural-map.md")
        );
        assert_eq!(
            card_for_diagnostic_code("circular-import"),
            Some("01-structural-map.md")
        );
        assert_eq!(
            card_for_diagnostic_code("lazy_circular_import"),
            Some("01-structural-map.md")
        );
    }

    #[test]
    fn card_mapping_covers_twin_family() {
        assert_eq!(
            card_for_diagnostic_code("exact-twin"),
            Some("01-structural-map.md")
        );
        assert_eq!(
            card_for_diagnostic_code("exact_twin"),
            Some("01-structural-map.md")
        );
        assert_eq!(
            card_for_diagnostic_code("twin"),
            Some("01-structural-map.md")
        );
    }

    #[test]
    fn card_mapping_returns_none_for_unknown_code() {
        assert_eq!(card_for_diagnostic_code("untagged-warning"), None);
        assert_eq!(card_for_diagnostic_code(""), None);
    }

    #[test]
    fn action_silent_when_atlas_dir_missing() {
        // tempdir has no `.loctree/context-atlas/` — should not fabricate a link.
        let temp = tempfile::tempdir().expect("create temp workspace");
        let action = atlas_card_action(&diag_with_code("dead-export"), temp.path());
        assert!(action.is_none(), "must not offer a broken atlas link");
    }

    #[test]
    fn action_silent_when_diagnostic_has_no_code() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let mut diag = diag_with_code("dead-export");
        diag.code = None;
        assert!(atlas_card_action(&diag, temp.path()).is_none());
    }

    #[test]
    fn action_silent_when_code_is_numeric() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let mut diag = diag_with_code("dead-export");
        diag.code = Some(NumberOrString::Number(42));
        assert!(atlas_card_action(&diag, temp.path()).is_none());
    }

    #[test]
    fn action_emitted_when_atlas_card_exists() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let dir = atlas_dir(temp.path());
        std::fs::create_dir_all(&dir).expect("create atlas dir");
        std::fs::write(dir.join("02-runtime-map.md"), "# Runtime Map\n")
            .expect("write runtime atlas card");

        let action = atlas_card_action(&diag_with_code("dead-export"), temp.path())
            .expect("action should be emitted");
        assert!(
            action.title.contains("02-runtime-map.md"),
            "title carries the card filename: {}",
            action.title
        );
        let cmd = action.command.expect("command form");
        assert_eq!(cmd.command, OPEN_ATLAS_CARD_COMMAND);
        let args = cmd.arguments.expect("args present");
        assert_eq!(args.len(), 1);
        let payload = args[0].as_object().expect("arg is object");
        assert!(
            payload["card_path"]
                .as_str()
                .expect("card_path is a string")
                .ends_with("02-runtime-map.md")
        );
        assert_eq!(payload["diagnostic_code"], serde_json::json!("dead-export"));
    }

    #[test]
    fn validate_args_happy_path() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let dir = atlas_dir(temp.path());
        std::fs::create_dir_all(&dir).expect("create atlas dir");
        let card = dir.join("01-structural-map.md");
        std::fs::write(&card, "# Structural Map\n").expect("write structural atlas card");

        let resolved = validate_open_atlas_card_args(
            &[serde_json::json!({
                "card_path": card.display().to_string(),
            })],
            temp.path(),
        )
        .expect("should validate");
        // Compare canonically: tempdirs may sit behind a symlink (e.g.
        // `/var` -> `/private/var` on macOS).
        assert_eq!(resolved, card.canonicalize().expect("canonicalize card"));
    }

    #[test]
    fn validate_args_rejects_missing_arguments() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let err = validate_open_atlas_card_args(&[], temp.path()).unwrap_err();
        assert!(matches!(err, OpenAtlasCardError::MissingArguments));
    }

    #[test]
    fn validate_args_rejects_missing_card_path() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let err =
            validate_open_atlas_card_args(&[serde_json::json!({ "wrong": "shape" })], temp.path())
                .unwrap_err();
        assert!(matches!(err, OpenAtlasCardError::MissingCardPath));
    }

    #[test]
    fn validate_args_rejects_nonexistent_card() {
        let temp = tempfile::tempdir().expect("create temp workspace");
        let err = validate_open_atlas_card_args(
            &[serde_json::json!({
                "card_path": "/definitely/does/not/exist/atlas-card.md"
            })],
            temp.path(),
        )
        .unwrap_err();
        assert!(matches!(err, OpenAtlasCardError::CardMissing(_)));
    }

    #[test]
    fn validate_args_rejects_existing_path_outside_atlas() {
        // A real file on disk that exists but lives OUTSIDE the workspace
        // atlas directory must be rejected — `path.exists()` alone is too
        // loose (hardening #12).
        let temp = tempfile::tempdir().expect("create temp workspace");
        std::fs::create_dir_all(atlas_dir(temp.path())).expect("create atlas dir");

        let outside = tempfile::tempdir().expect("create outside dir");
        let stray = outside.path().join("not-a-card.md");
        std::fs::write(&stray, "# definitely not an atlas card\n").expect("write stray file");

        let err = validate_open_atlas_card_args(
            &[serde_json::json!({
                "card_path": stray.display().to_string(),
            })],
            temp.path(),
        )
        .unwrap_err();
        assert!(matches!(err, OpenAtlasCardError::OutsideAtlas(_)));
    }
}
