//! Custom LSP request: `loctree/contextAtlas`
//!
//! Returns a typed pointer to the materialized Context Atlas at
//! `<workspace_root>/.loctree/context-atlas/manifest.json` (Plan 01).
//! Daemon-mode agents bootstrap from this in <1s — they get the card
//! manifest + reading order over JSON-RPC and open cards on disk
//! themselves. No 124 KB inline payload, no host-truncation risk.
//!
//! Plan 02 of the LSP roadmap.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Parameters for `loctree/contextAtlas`.
#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
pub struct ContextAtlasParams {
    /// Optional project root override. When `None`, the LSP backend
    /// substitutes its workspace root. Reserved for Plan 13
    /// (multi-workspace) — single-workspace clients can omit.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

/// One atlas card — paths-only by contract (no inline content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardPointer {
    pub id: String,
    pub title: String,
    pub path: String,
    pub lines: usize,
    pub why: String,
}

/// `loctree/contextAtlas` response payload.
#[derive(Debug, Clone, Serialize)]
pub struct ContextAtlasResponse {
    /// `"ready"` when the atlas is materialized; `"missing"` otherwise.
    pub status: String,
    /// Absolute path to the atlas directory (only populated when ready).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atlas_dir: Option<String>,
    /// Absolute path to the manifest markdown (the human-friendly entry).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Absolute path to the manifest JSON (machine-friendly entry).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_json: Option<String>,
    /// File the agent should read first (typically `00-core-map.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_start: Option<String>,
    /// Card pointers in canonical reading order. Empty when missing.
    pub cards: Vec<CardPointer>,
    /// Human-readable status message.
    pub message: String,
    /// Suggested next CLI action when the atlas isn't materialized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
}

impl ContextAtlasResponse {
    fn missing() -> Self {
        Self {
            status: "missing".into(),
            atlas_dir: None,
            manifest: None,
            manifest_json: None,
            recommended_start: None,
            cards: Vec::new(),
            message:
                "Atlas not materialized. Run `loct auto` (or any `loct context` command) to write \
                 cards under `<repo>/.loctree/context-atlas/`."
                    .into(),
            next_action: Some("loct auto".into()),
        }
    }

    fn parse_failure(reason: impl Into<String>) -> Self {
        Self {
            status: "missing".into(),
            atlas_dir: None,
            manifest: None,
            manifest_json: None,
            recommended_start: None,
            cards: Vec::new(),
            message: format!(
                "Atlas manifest exists but could not be parsed: {}. Re-run `loct auto` \
                 to refresh.",
                reason.into()
            ),
            next_action: Some("loct auto".into()),
        }
    }
}

/// Compute the manifest path for a workspace root.
pub fn manifest_path_for(root: &Path) -> PathBuf {
    root.join(".loctree")
        .join("context-atlas")
        .join("manifest.json")
}

/// Top-level entry: probe the atlas on disk and map into the LSP response.
pub fn compute(workspace_root: &Path, params: &ContextAtlasParams) -> ContextAtlasResponse {
    let project = params
        .project
        .clone()
        .unwrap_or_else(|| workspace_root.to_path_buf());
    let manifest_path = manifest_path_for(&project);

    if !manifest_path.exists() {
        return ContextAtlasResponse::missing();
    }

    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(content) => content,
        Err(err) => return ContextAtlasResponse::parse_failure(format!("read error: {err}")),
    };

    let value: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(err) => return ContextAtlasResponse::parse_failure(format!("json error: {err}")),
    };

    response_from_manifest(&value)
}

/// Build a `ContextAtlasResponse` from a parsed manifest JSON value.
///
/// Public so integration tests can exercise the mapping with synthetic
/// fixtures without touching the filesystem.
pub fn response_from_manifest(value: &Value) -> ContextAtlasResponse {
    let cards = extract_cards(value);
    ContextAtlasResponse {
        status: "ready".into(),
        atlas_dir: string_field(value, "atlas_dir"),
        manifest: string_field(value, "manifest"),
        manifest_json: string_field(value, "manifest_json"),
        recommended_start: string_field(value, "recommended_start"),
        cards,
        message: string_field(value, "message").unwrap_or_else(|| {
            "Atlas ready — open the cards in the recommended reading order.".into()
        }),
        next_action: None,
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(|s| s.to_string())
}

fn extract_cards(value: &Value) -> Vec<CardPointer> {
    value
        .get("cards")
        .and_then(|c| c.as_array())
        .map(|cards| {
            cards
                .iter()
                .filter_map(|card| {
                    Some(CardPointer {
                        id: card.get("id")?.as_str()?.to_string(),
                        title: card.get("title")?.as_str()?.to_string(),
                        path: card.get("path")?.as_str()?.to_string(),
                        lines: card.get("lines").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                        why: card
                            .get("why")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_path_uses_per_repo_layout() {
        let root = Path::new("/tmp/repo");
        let path = manifest_path_for(root);
        assert_eq!(
            path,
            PathBuf::from("/tmp/repo/.loctree/context-atlas/manifest.json")
        );
    }

    #[test]
    fn compute_returns_missing_when_atlas_absent() {
        let temp = tempfile::tempdir().unwrap();
        let response = compute(temp.path(), &ContextAtlasParams::default());
        assert_eq!(response.status, "missing");
        assert_eq!(response.next_action.as_deref(), Some("loct auto"));
        assert!(response.cards.is_empty());
        assert!(response.atlas_dir.is_none());
    }

    #[test]
    fn compute_returns_parse_failure_on_bad_json() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_dir = temp.path().join(".loctree/context-atlas");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("manifest.json"), "{not json").unwrap();
        let response = compute(temp.path(), &ContextAtlasParams::default());
        assert_eq!(response.status, "missing");
        assert!(response.message.contains("could not be parsed"));
    }

    #[test]
    fn compute_returns_ready_with_cards_for_real_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_dir = temp.path().join(".loctree/context-atlas");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        let manifest = json!({
            "protocol": "loctree.context_atlas.v1",
            "status": "ready",
            "atlas_dir": format!("{}/.loctree/context-atlas", temp.path().display()),
            "manifest": format!("{}/.loctree/context-atlas/manifest.md", temp.path().display()),
            "manifest_json": format!("{}/.loctree/context-atlas/manifest.json", temp.path().display()),
            "recommended_start": format!("{}/.loctree/context-atlas/00-core-map.md", temp.path().display()),
            "cards": [
                {
                    "id": "core",
                    "title": "Core Map",
                    "path": "00-core-map.md",
                    "lines": 226,
                    "bytes": 9238,
                    "why": "Repo identity, current risk, authority labels, safe next commands.",
                    "saves_you_from": "wrong project state"
                },
                {
                    "id": "structural",
                    "title": "Structural Map",
                    "path": "01-structural-map.md",
                    "lines": 20,
                    "bytes": 503,
                    "why": "Files, symbols, imports, consumers, entrypoints.",
                    "saves_you_from": "missed consumers"
                }
            ],
            "message": "Atlas ready — read core, structural, runtime first."
        });
        std::fs::write(
            manifest_dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();

        let response = compute(temp.path(), &ContextAtlasParams::default());
        assert_eq!(response.status, "ready");
        assert!(response.next_action.is_none());
        assert_eq!(response.cards.len(), 2);
        assert_eq!(response.cards[0].id, "core");
        assert_eq!(response.cards[0].lines, 226);
        assert_eq!(response.cards[1].title, "Structural Map");
        assert!(
            response
                .recommended_start
                .unwrap()
                .ends_with("00-core-map.md")
        );
    }

    #[test]
    fn compute_honors_project_param_override() {
        let temp = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        // workspace_root has no atlas; override project points to other_dir
        // which also has no atlas — both should report missing.
        let response = compute(
            temp.path(),
            &ContextAtlasParams {
                project: Some(other_dir.path().to_path_buf()),
            },
        );
        assert_eq!(response.status, "missing");
    }

    #[test]
    fn response_serializes_with_optional_fields_omitted_when_missing() {
        let response = ContextAtlasResponse::missing();
        let json = serde_json::to_value(&response).unwrap();
        let obj = json.as_object().unwrap();
        // `atlas_dir`, `manifest`, etc. are None → must not appear.
        assert!(!obj.contains_key("atlas_dir"));
        assert!(!obj.contains_key("manifest"));
        assert!(!obj.contains_key("manifest_json"));
        assert!(!obj.contains_key("recommended_start"));
        // `next_action` IS populated for missing → must appear.
        assert_eq!(obj["next_action"], json!("loct auto"));
        assert_eq!(obj["status"], json!("missing"));
        assert!(obj["cards"].as_array().unwrap().is_empty());
    }
}
