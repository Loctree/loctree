//! Cursor-paginated Context Atlas endpoint for HTTP MCP clients.
//!
//! `/context_pack` serves the materialized Loctree Context Atlas one card at a
//! time. The CLI keeps its full, unchunked `loct context --full` behavior; this
//! endpoint is the bounded HTTP fetch surface for agents.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use loctree::atlas::{
    ContextAtlasManifest, ContextOptions, atlas_dir_for_project,
    compose_context_pack_from_snapshot, materialize_context_atlas,
};
use loctree::snapshot::Snapshot;

const CURSOR_VERSION: &str = "v1";
const CURSOR_TTL_SECS: i64 = 60 * 60;

static CURSOR_SECRET: LazyLock<[u8; 32]> = LazyLock::new(|| {
    let mut hasher = Sha256::new();
    hasher.update(Uuid::new_v4().as_bytes());
    hasher.update(std::process::id().to_le_bytes());
    if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
        hasher.update(duration.as_nanos().to_le_bytes());
    }
    hasher.finalize().into()
});

#[derive(Debug, Deserialize)]
pub(crate) struct ContextPackQuery {
    project: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    cards: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContextPackResponse {
    pub section: usize,
    pub card: String,
    pub title: String,
    pub content: String,
    pub total_sections: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorState {
    project: String,
    fingerprint: String,
    cards: Vec<String>,
    section: usize,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct SelectedCard {
    id: String,
    title: String,
    path: String,
}

#[derive(Debug)]
pub(crate) enum ContextPackError {
    BadRequest(String),
    NotFound(String),
    Gone(String),
    Internal(String),
}

impl IntoResponse for ContextPackError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Gone(message) => (StatusCode::GONE, message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, message).into_response()
    }
}

pub(crate) async fn context_pack_handler(
    Query(query): Query<ContextPackQuery>,
) -> Result<Json<ContextPackResponse>, ContextPackError> {
    let project = validate_project_path(&query.project)?;
    let manifest = load_or_materialize_manifest(&project)?;
    let fingerprint = atlas_fingerprint(&manifest)?;
    let selected_cards = if let Some(cursor) = query.cursor.as_deref() {
        let cursor = decode_cursor(cursor)?;
        if cursor.project != project.to_string_lossy() {
            return Err(ContextPackError::Gone(
                "cursor project no longer matches request project".to_string(),
            ));
        }
        if cursor.fingerprint != fingerprint {
            return Err(ContextPackError::Gone(
                "context atlas changed; restart pagination without a cursor".to_string(),
            ));
        }
        if cursor.expires_at < now_timestamp() {
            return Err(ContextPackError::Gone(
                "context pack cursor expired; restart pagination".to_string(),
            ));
        }
        let cards = select_cards(&manifest, Some(cursor.cards.join(",")))?;
        return render_section(&project, &fingerprint, cards, cursor.section);
    } else {
        select_cards(&manifest, query.cards.clone())?
    };

    render_section(&project, &fingerprint, selected_cards, 0)
}

fn render_section(
    project: &Path,
    fingerprint: &str,
    selected_cards: Vec<SelectedCard>,
    section: usize,
) -> Result<Json<ContextPackResponse>, ContextPackError> {
    if selected_cards.is_empty() {
        return Err(ContextPackError::NotFound(
            "context atlas has no matching cards".to_string(),
        ));
    }
    let Some(card) = selected_cards.get(section).cloned() else {
        return Err(ContextPackError::Gone(
            "cursor points past the available context atlas cards".to_string(),
        ));
    };

    let atlas_dir = atlas_card_root(project)?;
    let content = read_card(&atlas_dir, &card.path)?;

    let next_section = section + 1;
    let next_cursor = if next_section < selected_cards.len() {
        let state = CursorState {
            project: project.to_string_lossy().to_string(),
            fingerprint: fingerprint.to_string(),
            cards: selected_cards.iter().map(|card| card.id.clone()).collect(),
            section: next_section,
            expires_at: now_timestamp() + CURSOR_TTL_SECS,
        };
        Some(encode_cursor(&state)?)
    } else {
        None
    };

    Ok(Json(ContextPackResponse {
        section,
        card: card.id,
        title: card.title,
        content,
        total_sections: selected_cards.len(),
        next_cursor,
    }))
}

fn validate_project_path(input: &str) -> Result<PathBuf, ContextPackError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ContextPackError::BadRequest(
            "project path is required".to_string(),
        ));
    }
    if trimmed.contains('\0') {
        return Err(ContextPackError::BadRequest(
            "project path contains a NUL byte".to_string(),
        ));
    }

    let raw = Path::new(trimmed);
    if raw
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ContextPackError::BadRequest(
            "project path must not contain parent-dir components".to_string(),
        ));
    }

    let lexical = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| ContextPackError::Internal(format!("failed to read cwd: {err}")))?
            .join(raw)
    };

    let canonical = lexical.canonicalize().map_err(|err| {
        ContextPackError::BadRequest(format!("invalid project path {}: {err}", lexical.display()))
    })?;
    if !canonical.is_dir() {
        return Err(ContextPackError::BadRequest(format!(
            "project path is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn load_or_materialize_manifest(project: &Path) -> Result<ContextAtlasManifest, ContextPackError> {
    let atlas_dir = atlas_dir_for_project(project);
    let manifest_path = atlas_dir.join("manifest.json");
    if !manifest_path.exists() {
        materialize_atlas(project)?;
    }
    load_manifest(&manifest_path)
}

fn materialize_atlas(project: &Path) -> Result<(), ContextPackError> {
    let snapshot = load_or_scan_snapshot(project)?;
    let opts = ContextOptions {
        project: Some(project.to_path_buf()),
        with_aicx: true,
        no_aicx: false,
        ..ContextOptions::default()
    };
    let pack = compose_context_pack_from_snapshot(&opts, project, &snapshot).map_err(|err| {
        ContextPackError::Internal(format!("failed to compose context pack: {err}"))
    })?;
    materialize_context_atlas(&pack, project, None).map_err(|err| {
        ContextPackError::Internal(format!("failed to materialize context atlas: {err}"))
    })?;
    Ok(())
}

/// Thin call into the snapshot freshness authority in the loctree lib —
/// staleness decisions (DRIFT / fence / watch fast-path) and the unified
/// rescan file universe live in `loctree::snapshot::acquire_snapshot`.
fn load_or_scan_snapshot(project: &Path) -> Result<Snapshot, ContextPackError> {
    loctree::snapshot::acquire_snapshot(
        &[project.to_path_buf()],
        loctree::snapshot::SnapshotReusePolicy::Strict,
        &loctree::snapshot::AcquireOptions {
            quiet: true,
            ..Default::default()
        },
    )
    .map_err(|err| ContextPackError::Internal(format!("context atlas snapshot failed: {err}")))
}

fn load_manifest(path: &Path) -> Result<ContextAtlasManifest, ContextPackError> {
    let content = fs::read_to_string(path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => ContextPackError::NotFound(format!(
            "context atlas manifest missing: {}",
            path.display()
        )),
        _ => ContextPackError::Internal(format!(
            "failed to read context atlas manifest {}: {err}",
            path.display()
        )),
    })?;
    serde_json::from_str(&content).map_err(|err| {
        ContextPackError::Internal(format!(
            "failed to parse context atlas manifest {}: {err}",
            path.display()
        ))
    })
}

fn select_cards(
    manifest: &ContextAtlasManifest,
    cards: Option<String>,
) -> Result<Vec<SelectedCard>, ContextPackError> {
    let wanted = cards
        .as_deref()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|card| !card.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if wanted.is_empty() {
        return Ok(manifest
            .cards
            .iter()
            .map(|card| SelectedCard {
                id: card.id.clone(),
                title: card.title.clone(),
                path: card.path.clone(),
            })
            .collect());
    }

    let mut selected = Vec::with_capacity(wanted.len());
    for id in wanted {
        let Some(card) = manifest.cards.iter().find(|card| card.id == id) else {
            return Err(ContextPackError::BadRequest(format!(
                "unknown context atlas card: {id}"
            )));
        };
        selected.push(SelectedCard {
            id: card.id.clone(),
            title: card.title.clone(),
            path: card.path.clone(),
        });
    }
    Ok(selected)
}

/// Trusted atlas card root for an already-validated `project`.
///
/// Derived from the project path via [`atlas_dir_for_project`] — never from the
/// manifest's own `atlas_dir` string. A tampered or attacker-pointed manifest
/// therefore cannot redirect card reads outside the project's atlas directory.
fn atlas_card_root(project: &Path) -> Result<PathBuf, ContextPackError> {
    atlas_dir_for_project(project)
        .canonicalize()
        .map_err(|err| {
            ContextPackError::NotFound(format!("context atlas directory missing: {err}"))
        })
}

/// Read a single atlas card, confined to `atlas_dir`.
///
/// Atlas cards are flat files named by exactly one path component (e.g.
/// `00-core-map.md`). The name is rejected unless it is a single `Normal`
/// component — no separators, parent-dir escapes, absolute paths, or `.`/`..`
/// tricks. The joined path is canonicalized and re-checked for containment
/// before the read, so symlink games inside the atlas dir cannot escape either.
fn read_card(atlas_dir: &Path, card_name: &str) -> Result<String, ContextPackError> {
    let mut components = Path::new(card_name).components();
    let (Some(Component::Normal(name)), None) = (components.next(), components.next()) else {
        return Err(ContextPackError::NotFound(
            "context atlas card name must be a single path component".to_string(),
        ));
    };

    let canonical = atlas_dir
        .join(name)
        .canonicalize()
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => {
                ContextPackError::NotFound(format!("context atlas card missing: {card_name}"))
            }
            _ => ContextPackError::Internal(format!(
                "failed to resolve context atlas card {card_name}: {err}"
            )),
        })?;
    if !canonical.starts_with(atlas_dir) {
        return Err(ContextPackError::NotFound(
            "context atlas card path escapes atlas directory".to_string(),
        ));
    }

    fs::read_to_string(&canonical).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => {
            ContextPackError::NotFound(format!("context atlas card missing: {card_name}"))
        }
        _ => ContextPackError::Internal(format!(
            "failed to read context atlas card {card_name}: {err}"
        )),
    })
}

fn atlas_fingerprint(manifest: &ContextAtlasManifest) -> Result<String, ContextPackError> {
    let mut hasher = Sha256::new();
    hasher.update(manifest.protocol.as_bytes());
    hasher.update(b"\0");
    hasher.update(manifest.project.as_bytes());
    hasher.update(b"\0");
    hasher.update(manifest.snapshot.as_bytes());
    hasher.update(b"\0");
    hasher.update(manifest.generated_at.as_bytes());
    for card in &manifest.cards {
        hasher.update(b"\0");
        hasher.update(card.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(card.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(card.lines.to_le_bytes());
        hasher.update(card.bytes.to_le_bytes());
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn encode_cursor(state: &CursorState) -> Result<String, ContextPackError> {
    let payload = serde_json::to_vec(state)
        .map_err(|err| ContextPackError::Internal(format!("failed to encode cursor: {err}")))?;
    let payload_hex = hex_encode(&payload);
    let signature = hmac_sha256_hex(&*CURSOR_SECRET, payload_hex.as_bytes());
    Ok(format!("{CURSOR_VERSION}.{payload_hex}.{signature}"))
}

fn decode_cursor(cursor: &str) -> Result<CursorState, ContextPackError> {
    let mut parts = cursor.split('.');
    let version = parts.next();
    let payload_hex = parts.next();
    let signature = parts.next();
    if version != Some(CURSOR_VERSION) || parts.next().is_some() {
        return Err(ContextPackError::BadRequest(
            "invalid cursor format".to_string(),
        ));
    }
    let payload_hex = payload_hex
        .ok_or_else(|| ContextPackError::BadRequest("invalid cursor format".to_string()))?;
    let signature = signature
        .ok_or_else(|| ContextPackError::BadRequest("invalid cursor format".to_string()))?;
    let expected = hmac_sha256_hex(&*CURSOR_SECRET, payload_hex.as_bytes());
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err(ContextPackError::BadRequest(
            "cursor signature is invalid".to_string(),
        ));
    }
    let payload = hex_decode(payload_hex)?;
    serde_json::from_slice(&payload)
        .map_err(|err| ContextPackError::BadRequest(format!("invalid cursor payload: {err}")))
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        normalized[..digest.len()].copy_from_slice(&digest);
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36_u8; BLOCK];
    let mut opad = [0x5c_u8; BLOCK];
    for idx in 0..BLOCK {
        ipad[idx] ^= normalized[idx];
        opad[idx] ^= normalized[idx];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    hex_encode(&outer.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(input: &str) -> Result<Vec<u8>, ContextPackError> {
    if !input.len().is_multiple_of(2) {
        return Err(ContextPackError::BadRequest(
            "cursor payload is not valid hex".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    for chunk in input.as_bytes().chunks_exact(2) {
        let text = std::str::from_utf8(chunk).map_err(|_| {
            ContextPackError::BadRequest("cursor payload is not valid UTF-8".to_string())
        })?;
        let byte = u8::from_str_radix(text, 16).map_err(|_| {
            ContextPackError::BadRequest("cursor payload is not valid hex".to_string())
        })?;
        out.push(byte);
    }
    Ok(out)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (l, r)| acc | (l ^ r))
        == 0
}

fn now_timestamp() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_detects_tampering() {
        let state = CursorState {
            project: "/tmp/project".to_string(),
            fingerprint: "abc".to_string(),
            cards: vec!["core".to_string(), "risk".to_string()],
            section: 1,
            expires_at: now_timestamp() + 60,
        };

        let cursor = encode_cursor(&state).expect("cursor");
        let decoded = decode_cursor(&cursor).expect("decode");
        assert_eq!(decoded.project, state.project);
        assert_eq!(decoded.cards, state.cards);
        assert_eq!(decoded.section, 1);

        let tampered = cursor.replacen("v1.", "v1.ff", 1);
        assert!(matches!(
            decode_cursor(&tampered),
            Err(ContextPackError::BadRequest(_))
        ));
    }
}
