//! Custom LSP request: `loctree/slice`
//!
//! Returns a holographic slice (core + deps + consumers) for a target file
//! using `loctree::slicer::HolographicSlice`. Paths-only — no inline
//! content. Plan 05 of the LSP roadmap.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;

use loctree::slicer::{HolographicSlice, SliceConfig, SliceFile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::{CursorError, CursorState};
use crate::protocol::{DEFAULT_CHUNK_SIZE, Paginated, ResponseIdentity, paginate};

const SNAPSHOT_ID_FALLBACK: &str = "snapshot:unknown";
const DEPS_CURSOR_KIND: &str = "loctree/slice.deps";
const CONSUMERS_CURSOR_KIND: &str = "loctree/slice.consumers";

/// Parameters for `loctree/slice`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SliceParams {
    /// Target file. Repo-relative or absolute — normalized by the analyzer.
    pub target: PathBuf,
    /// When true, include the consumer layer (files that import target).
    #[serde(default)]
    pub consumers: bool,
    /// Maximum depth for dependency traversal. Defaults to slicer's default
    /// (currently 2) when omitted.
    #[serde(default)]
    pub depth: Option<usize>,
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context); ignored in single-workspace mode.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// Opaque Plan 12 cursor returned by `deps.next_cursor` or
    /// `consumers.next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Requested page size for the paginated deps/consumers layers.
    #[serde(default)]
    pub chunk_size: Option<usize>,
}

/// One layer entry — paths-only by contract (no inline content).
#[derive(Debug, Clone, Serialize)]
pub struct SliceFileEntry {
    /// Repo-relative path string.
    pub path: String,
    /// Depth from target (0 = core, 1 = direct dep/consumer, 2+ = transitive).
    pub depth: usize,
    /// Language tag (`rust`, `typescript`, etc.).
    pub lang: String,
    /// Lines of code in the file.
    pub loc: usize,
}

/// `loctree/slice` response payload.
#[derive(Debug, Clone, Serialize)]
pub struct SliceResponse {
    /// Snapshot/project authority for the routed response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<ResponseIdentity>,
    /// The target file itself, as a single-element layer.
    pub core: Vec<SliceFileEntry>,
    /// Files the target depends on (transitively up to `depth`).
    pub deps: Paginated<Vec<SliceFileEntry>>,
    /// Files that import the target (when `consumers: true`).
    pub consumers: Paginated<Vec<SliceFileEntry>>,
    /// Total file count across all returned layers.
    pub total_files: usize,
    /// Total LOC across all returned layers.
    pub total_loc: usize,
}

impl SliceResponse {
    /// Map a `HolographicSlice` from the analyzer into the LSP response shape.
    pub fn from_holographic(slice: &HolographicSlice) -> Self {
        Self::from_holographic_paginated(slice, None, DEFAULT_CHUNK_SIZE, SNAPSHOT_ID_FALLBACK)
            .expect("default slice pagination should not fail")
    }

    /// Map a `HolographicSlice` into the cursor-aware response shape.
    pub fn from_holographic_paginated(
        slice: &HolographicSlice,
        cursor: Option<&str>,
        chunk_size: usize,
        snapshot_id: &str,
    ) -> Result<Self, CursorError> {
        let cursor_state = match cursor {
            Some(token) => Some(CursorState::decode_raw(token)?),
            None => None,
        };
        let (deps_offset, consumers_offset) = layer_offsets(cursor_state.as_ref(), snapshot_id)?;
        let deps: Vec<SliceFileEntry> = slice.deps.iter().map(map_entry).collect();
        let consumers: Vec<SliceFileEntry> = slice.consumers.iter().map(map_entry).collect();

        Ok(SliceResponse {
            identity: None,
            core: slice.core.iter().map(map_entry).collect(),
            deps: paginate(
                &deps,
                deps_offset,
                chunk_size,
                snapshot_id,
                DEPS_CURSOR_KIND,
            )?,
            consumers: paginate(
                &consumers,
                consumers_offset,
                chunk_size,
                snapshot_id,
                CONSUMERS_CURSOR_KIND,
            )?,
            total_files: slice.stats.total_files,
            total_loc: slice.stats.total_loc,
        })
    }

    /// Attach response identity after the backend has resolved the workspace.
    pub fn with_identity(mut self, identity: ResponseIdentity) -> Self {
        self.identity = Some(identity);
        self
    }
}

fn layer_offsets(
    cursor: Option<&CursorState>,
    snapshot_id: &str,
) -> Result<(usize, usize), CursorError> {
    let Some(cursor) = cursor else {
        return Ok((0, 0));
    };
    if cursor.snapshot_id != snapshot_id {
        return Err(CursorError::SnapshotDrifted {
            expected: snapshot_id.into(),
            got: cursor.snapshot_id.clone(),
        });
    }
    match cursor.kind.as_str() {
        DEPS_CURSOR_KIND => Ok((cursor.offset, 0)),
        CONSUMERS_CURSOR_KIND => Ok((0, cursor.offset)),
        other => Err(CursorError::KindMismatch {
            expected: format!("{DEPS_CURSOR_KIND}|{CONSUMERS_CURSOR_KIND}"),
            got: other.into(),
        }),
    }
}

fn map_entry(file: &SliceFile) -> SliceFileEntry {
    SliceFileEntry {
        path: file.path.clone(),
        depth: file.depth,
        lang: file.language.clone(),
        loc: file.loc,
    }
}

/// Resolve a target into a string the slicer can consume.
///
/// Accepts both absolute and `file://` URI-style paths; the slicer's own
/// `Snapshot::normalize_path` does final cleanup (extension matching, etc.).
pub fn target_string(params: &SliceParams) -> String {
    params.target.to_string_lossy().into_owned()
}

/// Build a `SliceConfig` from params, applying the slicer's defaults for
/// missing fields.
pub fn config_from_params(params: &SliceParams) -> SliceConfig {
    let defaults = SliceConfig::default();
    SliceConfig {
        include_consumers: params.consumers,
        max_depth: params.depth.unwrap_or(defaults.max_depth),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_string_preserves_relative_path() {
        let params = SliceParams {
            target: PathBuf::from("src/lib.rs"),
            consumers: false,
            depth: None,
            project: None,
            cursor: None,
            chunk_size: None,
        };
        assert_eq!(target_string(&params), "src/lib.rs");
    }

    #[test]
    fn config_from_params_picks_default_depth_when_unset() {
        let params = SliceParams {
            target: PathBuf::from("x"),
            consumers: true,
            depth: None,
            project: None,
            cursor: None,
            chunk_size: None,
        };
        let cfg = config_from_params(&params);
        assert!(cfg.include_consumers);
        assert_eq!(cfg.max_depth, SliceConfig::default().max_depth);
    }

    #[test]
    fn config_from_params_respects_explicit_depth() {
        let params = SliceParams {
            target: PathBuf::from("x"),
            consumers: false,
            depth: Some(5),
            project: None,
            cursor: None,
            chunk_size: None,
        };
        let cfg = config_from_params(&params);
        assert_eq!(cfg.max_depth, 5);
        assert!(!cfg.include_consumers);
    }
}
