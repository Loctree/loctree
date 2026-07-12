//! Code-lens provider: per-export importer counts (Plan 03).
//!
//! Passive structural annotation for IDE clients. Every top-level
//! export gets a CodeLens at its `line`:
//!
//!   - `"unused (0 importers)"` when zero importers,
//!   - `"1 importer"` when one,
//!   - `"N importers"` when more.
//!
//! ## v1 contract decision (closure note)
//!
//! The original plan called for `command: None` (purely informational).
//! In practice, mainstream LSP clients (VS Code, Helix, Zed, neovim's
//! `vim.lsp`) only render a CodeLens title when `command` is present —
//! a `None` command paired with `resolve_provider: Some(false)` produces
//! invisible lenses because the client has nowhere to fetch a title from.
//!
//! Closure: keep `command: Some(Command { title, command: "" })`. The
//! empty `command.command` string is a deliberate no-op — clicks do
//! nothing, but the title renders. This is the documented v1 contract.
//! Future iteration (tracked as a follow-up) can register
//! `loctree.showImporters` and wire click-through behaviour, mirroring
//! how Plan 04 registered `loctree.openAtlasCard`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use tower_lsp::lsp_types::{CodeLens, Position, Range};

use crate::snapshot::SnapshotState;

/// Build the code-lens annotations for a single file.
///
/// Returns an empty list when:
/// - no snapshot is loaded yet,
/// - the file is not in the snapshot,
/// - the file has no top-level exports.
pub async fn code_lens_for_file(state: &SnapshotState, file_path: &str) -> Vec<CodeLens> {
    let exports = match collect_exports(state, file_path).await {
        Some(e) => e,
        None => return Vec::new(),
    };

    let mut lenses = Vec::with_capacity(exports.len());
    for (name, line) in exports {
        let count = state.find_references(file_path, Some(&name)).await.len();
        let title = format_title(count);
        lenses.push(make_lens(line, &title));
    }
    lenses
}

/// Pull `(export_name, 1-based line)` pairs out of the snapshot.
/// Skips exports without a recorded `line` (we have nothing useful to
/// pin a CodeLens to).
async fn collect_exports(state: &SnapshotState, file_path: &str) -> Option<Vec<(String, usize)>> {
    let guard = state.get().await?;
    let loaded = guard.as_ref()?;
    let normalized = file_path.trim_start_matches('/');
    let analysis = loaded.snapshot.files.iter().find(|f| {
        f.path == file_path
            || f.path.trim_start_matches('/') == normalized
            || f.path.ends_with(file_path)
    })?;
    let exports: Vec<(String, usize)> = analysis
        .exports
        .iter()
        .filter_map(|exp| exp.line.map(|line| (exp.name.clone(), line)))
        .collect();
    Some(exports)
}

/// Format a count as the human-readable lens title.
pub fn format_title(count: usize) -> String {
    match count {
        0 => "unused (0 importers)".to_string(),
        1 => "1 importer".to_string(),
        n => format!("{n} importers"),
    }
}

/// Build a passive `CodeLens` at the export's 1-based line.
///
/// LSP positions are 0-based, so we subtract one (saturating to guard
/// against bogus 0 values from the analyzer).
///
/// ## Why `command: Some(Command { title, command: "" })` and not `None`
///
/// The original Plan 03 sketch asked for `command: None`. In practice,
/// mainstream LSP clients (VS Code, Helix, Zed, neovim) only render a
/// CodeLens title when the `command` field is populated. With
/// `resolve_provider: Some(false)` the client cannot ask the server to
/// fill in a missing command, so `None` produces invisible lenses.
///
/// We therefore emit a sentinel `Command` whose `command` string is
/// empty — clicks are no-ops, but the title renders. This is the
/// documented v1 contract; see this module's top-level comment for the
/// closure rationale.
pub(crate) fn make_lens(line_1_based: usize, title: &str) -> CodeLens {
    let line0 = line_1_based.saturating_sub(1) as u32;
    CodeLens {
        range: Range {
            start: Position::new(line0, 0),
            end: Position::new(line0, 0),
        },
        command: Some(tower_lsp::lsp_types::Command {
            title: title.to_string(),
            command: String::new(),
            arguments: None,
        }),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_title_zero_says_unused() {
        assert_eq!(format_title(0), "unused (0 importers)");
    }

    #[test]
    fn format_title_one_uses_singular() {
        assert_eq!(format_title(1), "1 importer");
    }

    #[test]
    fn format_title_many_uses_plural() {
        assert_eq!(format_title(2), "2 importers");
        assert_eq!(format_title(57), "57 importers");
    }

    #[test]
    fn lens_pins_to_zero_based_line() {
        let lens = make_lens(42, "57 importers");
        assert_eq!(lens.range.start.line, 41);
        assert_eq!(lens.range.end.line, 41);
        let cmd = lens.command.unwrap();
        assert_eq!(cmd.title, "57 importers");
        assert_eq!(cmd.command, "");
    }

    #[test]
    fn lens_handles_zero_line_gracefully() {
        // Some analyzer rows can report `line = 0` (e.g. computed-export
        // entries that don't survive line tracking). Saturate rather
        // than overflow.
        let lens = make_lens(0, "unused (0 importers)");
        assert_eq!(lens.range.start.line, 0);
    }
}
