use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::fs_utils::SanitizedPath;

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::aicx::overlay::{
    LocalTruth, OverlayAuthority, OverlayDoc, OverlayEntry, OverlayLifecycle, OverlayTarget,
    OverlayVerification, load_cached_overlay, overlay_cache_path, refresh_command, short_revision,
    staleness_reason,
};
use crate::context_render::current_iso_timestamp;
use crate::pack::{
    ActionSlice, AuthorityLabel, AuthoritySlice, ContextPack, HighFanInFile, HotspotFile,
    RiskCacheScope, RuntimeDispatchEdge, RuntimeFrameworkHint, RuntimeSlice,
};

pub const CONTEXT_ATLAS_PROTOCOL: &str = "loctree.context_atlas.v1";
pub const CONTEXT_ATLAS_DIR: &str = "context-atlas";
pub const CONTEXT_ATLAS_RUNS_DIR: &str = "runs";
/// Human-facing identity table and MCP pointer payload cap (G-CONTEXT-CAP).
pub const ATLAS_IDENTITY_LIST_LIMIT: usize = 20;
const ATLAS_IDENTITIES_FULL_JSON: &str = "manifest.full.json";
const ATLAS_STATUS_READY: &str = "atlas_ready";
const ATLAS_STATUS_HOLLOW: &str = "hollow";
const ATLAS_STATUS_STALE: &str = "stale";
const CARD_STATUS_UNREAD_UNUSABLE: &str = "unread/unusable";

/// Stable identity for one persisted atlas scope. The project-wide atlas keeps
/// the human-readable `project` id; every narrowed scope/task is keyed by a
/// digest of the selectors, scope fingerprint, and task contract.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAtlasIdentity {
    pub atlas_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selectors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// Address of one retained scope-keyed atlas. The root manifest is the
/// catalog; flat cards at the root are only a compatibility view of the most
/// recently materialized identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAtlasReference {
    pub identity: ContextAtlasIdentity,
    pub snapshot: String,
    pub generated_at: String,
    pub atlas_dir: String,
    pub manifest: String,
    pub manifest_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAtlasManifest {
    pub protocol: String,
    pub status: String,
    pub project: String,
    pub snapshot: String,
    pub generated_at: String,
    pub atlas_dir: String,
    pub manifest: String,
    pub manifest_json: String,
    pub recommended_start: String,
    /// Identity of the cards described by this manifest.
    #[serde(default)]
    pub identity: ContextAtlasIdentity,
    /// All scope-keyed atlases retained under this repository atlas root.
    #[serde(default)]
    pub atlases: Vec<ContextAtlasReference>,
    /// Domain-ownership map (kanon v4, L1-02): every cross-card data domain
    /// has exactly ONE owner card. This map — not the section headers — is the
    /// source of truth; headers are its projection. Values are card stems
    /// (`01-structural-map`) so verifiers can match them against receipt paths.
    #[serde(default)]
    pub domain_owners: BTreeMap<String, String>,
    pub cards: Vec<ContextAtlasCard>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAtlasCard {
    pub id: String,
    pub title: String,
    pub path: String,
    pub lines: usize,
    pub bytes: usize,
    pub why: String,
    pub saves_you_from: String,
    /// True when the on-card JSON fence was capped to the per-card line
    /// budget. The manifest and on-card markers surface this so an agent never
    /// reads a clipped card as canonical truth.
    #[serde(default)]
    pub truncated: bool,
    /// Lines dropped from the on-card JSON fence when capped (0 when whole).
    #[serde(default)]
    pub dropped_lines: usize,
    /// Relative filename of the canonical-payload sibling artifact (e.g.
    /// `01-structural-map.full.json`). Present for every card with a non-empty
    /// canonical payload — regardless of fence truncation.
    #[serde(default)]
    pub full_path: Option<String>,
    /// Line count of the canonical sibling JSON payload. Present whenever the
    /// sibling artifact was written; for truncated cards `lines` is the
    /// materialized `.md` card length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_payload_lines: Option<usize>,
    /// SHA-256 (hex) of the canonical payload serialization (sorted object
    /// keys, newline-terminated) — byte-identical to the sibling `.full.json`
    /// and stable across regenerations on the same snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_hash: Option<String>,
    /// Number of base facts in the card's coverage receipt (FactSet size).
    #[serde(default)]
    pub fact_count: usize,
    /// Hollow-atlas cards are unread/unusable until `loct scan` produces a
    /// snapshot. Absent when the card is a readable product of a real snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl ContextAtlasManifest {
    pub fn pointer_payload(&self) -> serde_json::Value {
        let (shown, omitted) = bound_atlas_identities(&self.atlases);
        json!({
            "protocol": self.protocol,
            "status": self.status,
            "project": self.project,
            "snapshot": self.snapshot,
            "atlas_dir": self.atlas_dir,
            "manifest": self.manifest,
            "manifest_json": self.manifest_json,
            "recommended_start": self.recommended_start,
            "identity": self.identity,
            "atlases": shown,
            "atlases_omitted": omitted,
            "atlases_full_path": ATLAS_IDENTITIES_FULL_JSON,
            "domain_owners": self.domain_owners,
            "cards": self.cards,
            "message": self.message,
        })
    }

    pub fn render_cli_summary(&self) -> String {
        let total_lines: usize = self.cards.iter().map(|card| card.lines).sum();
        let mut out = String::new();
        out.push_str("╭─ Loctree Context Atlas ─────────────────────────────────────────────╮\n");
        out.push_str("│ Repo understanding materialized as small, named cards.              │\n");
        out.push_str("╰─────────────────────────────────────────────────────────────────────╯\n\n");
        out.push_str(&format!("Status: {}\n", self.status));
        if self.status == ATLAS_STATUS_HOLLOW {
            out.push_str("Run `loct scan` before reading cards — they are unread/unusable.\n");
        }
        out.push_str(&format!("Project: {}\n", self.project));
        out.push_str(&format!("Snapshot: {}\n", self.snapshot));
        out.push_str(&format!("Atlas identity: {}\n", self.identity.atlas_id));
        out.push_str(&format!("Atlas dir: {}\n", self.atlas_dir));
        out.push_str(&format!("Start here: {}\n", self.manifest));
        out.push_str(&format!(
            "Cards: {} cards, {} readable lines\n\n",
            self.cards.len(),
            total_lines
        ));
        out.push_str("Recommended reading path:\n");
        for (idx, card) in self.cards.iter().enumerate() {
            out.push_str(&format!(
                "  {}. {}  ({})\n     Why: {}\n     Saves you from: {}\n",
                idx,
                card.path,
                card_line_label(card),
                card.why,
                card.saves_you_from
            ));
            if card.truncated {
                out.push_str(&format!(
                    "     ⚠ Partial: {} payload line(s) capped — read complete payload at {}\n",
                    card.dropped_lines,
                    card.full_path
                        .as_deref()
                        .unwrap_or("the sibling .full.json")
                ));
            }
        }
        out.push('\n');
        out.push_str("Completeness cue:\n");
        out.push_str(&self.message);
        out.push('\n');
        out.push_str("Tip: use `loct context --full --json` for the full machine-readable ContextPack, or `loct context --full --markdown` for the full human-readable pack.\n");
        out
    }
}

pub const ATLAS_REPO_DIR: &str = ".loctree";

/// Kanon v4 (L1-02): one owner card per cross-card data domain. The manifest
/// map is the source of truth for ownership — section headers are only its
/// projection, so a duplicate smuggled without a header still counts as a
/// violation. `intent → 03-intent-map` since M1-01: the intent-card upgrade
/// took over the domain, the owner stays card 03.
fn atlas_domain_owners() -> BTreeMap<String, String> {
    [
        ("identity", "00-core-map"),
        ("safe_next_commands", "00-core-map"),
        ("hubs", "01-structural-map"),
        ("edges", "01-structural-map"),
        ("import_graph", "01-structural-map"),
        ("hotspots", "01-structural-map"),
        ("authority", "01-structural-map"),
        ("reachability", "01-structural-map"),
        ("entrypoints", "02-runtime-map"),
        ("env_contracts", "02-runtime-map"),
        ("framework_hints", "02-runtime-map"),
        ("dispatch", "02-runtime-map"),
        ("intent", "03-intent-map"),
        ("gates", "04-verification-gates"),
        ("likely_tests", "04-verification-gates"),
        ("freshness", "05-risk-register"),
        ("stale_assumptions", "05-risk-register"),
        ("actions", "05-risk-register"),
    ]
    .into_iter()
    .map(|(domain, owner)| (domain.to_string(), owner.to_string()))
    .collect()
}

pub fn atlas_dir_for_project(project_root: &Path) -> PathBuf {
    project_root.join(ATLAS_REPO_DIR).join(CONTEXT_ATLAS_DIR)
}

pub fn materialize_context_atlas(
    pack: &ContextPack,
    project_root: &Path,
    atlas_dir: Option<&Path>,
) -> io::Result<ContextAtlasManifest> {
    let identity = context_atlas_identity(pack);
    if let Some(atlas_dir) = atlas_dir {
        return materialize_context_atlas_at(
            pack,
            project_root,
            atlas_dir.to_path_buf(),
            identity,
            Vec::new(),
        );
    }

    let atlas_root = atlas_dir_for_project(project_root);
    fs::create_dir_all(atlas_root.join(CONTEXT_ATLAS_RUNS_DIR))?;
    // Exclusive lock covers catalog reload + run-dir write + flat-root
    // mirror so concurrent scoped contexts cannot tear shared cards or
    // drop catalog identities (G-ATLAS-SHARED-MUTABLE).
    let _lock = acquire_atlas_mirror_lock(&atlas_root)?;
    let known_atlases = load_retained_atlases(&atlas_root)?;
    let run_dir = atlas_root
        .join(CONTEXT_ATLAS_RUNS_DIR)
        .join(&identity.atlas_id);
    let run_manifest =
        materialize_context_atlas_at(pack, project_root, run_dir, identity, known_atlases)?;

    mirror_current_atlas(&run_manifest, &atlas_root)
}

fn materialize_context_atlas_at(
    pack: &ContextPack,
    project_root: &Path,
    atlas_dir: PathBuf,
    identity: ContextAtlasIdentity,
    mut known_atlases: Vec<ContextAtlasReference>,
) -> io::Result<ContextAtlasManifest> {
    fs::create_dir_all(&atlas_dir)?;

    let project = pack
        .project
        .canonical_root
        .clone()
        .unwrap_or_else(|| project_root.display().to_string());
    let snapshot = snapshot_label(pack);
    let generated_at = current_iso_timestamp();
    let status = atlas_status(pack);
    let card_status =
        (status == ATLAS_STATUS_HOLLOW).then(|| CARD_STATUS_UNREAD_UNUSABLE.to_string());
    let message = atlas_status_message(status);

    // Cards 00 and 03 read the same intent layer — card 00 for the identity
    // revisions, card 03 for the theses — so it is resolved once here.
    let intent_source = resolve_intent_card_source(pack, project_root);

    let specs = vec![
        CardSpec {
            id: "core",
            title: "Core Map",
            filename: "00-core-map.md",
            why: "Repo identity, current risk, authority labels, safe next commands.",
            saves: "wrong project state, stale assumptions, unsafe first actions",
            body: render_core_card(pack, &intent_source),
        },
        CardSpec {
            id: "structural",
            title: "Structural Map",
            filename: "01-structural-map.md",
            why: "Files, symbols, imports, consumers, entrypoints; read before edits/refactors.",
            saves: "missed consumers, wrong impact, blind dependency edits",
            body: render_card_body(pack, "structural", "01-structural-map.md", "Structural Map"),
        },
        CardSpec {
            id: "runtime",
            title: "Runtime Map",
            filename: "02-runtime-map.md",
            why: "Runtime behavior, framework hints, env contracts, reachability.",
            saves: "wrong tests, hidden runtime coupling, config mistakes",
            body: render_card_body(pack, "runtime", "02-runtime-map.md", "Runtime Map"),
        },
        CardSpec {
            id: "intent",
            title: "Intent Map",
            filename: "03-intent-map.md",
            why: "Formative decisions, intents, anti-recommendations, and superseded history pinned to structure (aicx overlay).",
            saves: "repeated work, re-decided decisions, revived refuted approaches",
            body: render_intent_card(pack, &intent_source),
        },
        CardSpec {
            id: "verification",
            title: "Verification Gates",
            filename: "04-verification-gates.md",
            why: "Commands and likely tests most relevant to validate changes.",
            saves: "wrong validation path, skipped downstream checks, false confidence",
            body: render_card_body(
                pack,
                "verification",
                "04-verification-gates.md",
                "Verification Gates",
            ),
        },
        CardSpec {
            id: "risk",
            title: "Risk Register",
            filename: "05-risk-register.md",
            why: "Hotspots, cache/snapshot health, stale assumptions, next risk-reducing actions.",
            saves: "release blockers, high fan-in surprises, stale-cache decisions",
            body: render_card_body(pack, "risk", "05-risk-register.md", "Risk Register"),
        },
    ];

    // M1-01 upgrade hygiene: karta 03 was renamed memory-trail → intent-map.
    // A leftover pre-upgrade artifact in the atlas dir would masquerade as a
    // live card — remove it before writing the current set.
    for legacy in ["03-memory-trail.md", "03-memory-trail.full.json"] {
        let _ = fs::remove_file(atlas_dir.join(legacy));
    }

    let mut cards = Vec::new();
    let mut card_receipts: Vec<(String, Vec<FactId>)> = Vec::new();
    for spec in specs {
        let path = atlas_dir.join(spec.filename);
        write_atlas_file_atomic(&path, spec.body.markdown.as_bytes())?;
        // The canonical payload is the contract, not a truncation side-effect:
        // write the `.full.json` sibling for every card with a non-empty
        // payload, regardless of whether the on-card fence was capped.
        let (full_path, full_payload_lines, payload_hash) =
            if payload_is_empty(&spec.body.canonical_payload) {
                (None, None, None)
            } else {
                let full_filename = full_json_filename(spec.filename);
                let canonical = canonical_json_pretty(&spec.body.canonical_payload);
                write_atlas_file_atomic(atlas_dir.join(&full_filename), canonical.as_bytes())?;
                (
                    Some(full_filename),
                    Some(line_count(&canonical)),
                    Some(spec.body.payload_hash.clone()),
                )
            };
        let (truncated, dropped_lines) = match spec.body.fence_dropped_lines {
            Some(dropped) => (true, dropped),
            None => (false, 0),
        };
        cards.push(ContextAtlasCard {
            id: spec.id.to_string(),
            title: spec.title.to_string(),
            path: spec.filename.to_string(),
            lines: line_count(&spec.body.markdown),
            bytes: spec.body.markdown.len(),
            why: spec.why.to_string(),
            saves_you_from: spec.saves.to_string(),
            truncated,
            dropped_lines,
            full_path,
            full_payload_lines,
            payload_hash,
            fact_count: spec.body.coverage_receipt.len(),
            status: card_status.clone(),
        });
        card_receipts.push((spec.filename.to_string(), spec.body.coverage_receipt));
    }

    let manifest_path = atlas_dir.join("manifest.md");
    let manifest_json_path = atlas_dir.join("manifest.json");
    let receipt_path = atlas_dir.join("receipt.json");

    let current_reference = ContextAtlasReference {
        identity: identity.clone(),
        snapshot: snapshot.clone(),
        generated_at: generated_at.clone(),
        atlas_dir: atlas_dir.display().to_string(),
        manifest: manifest_path.display().to_string(),
        manifest_json: manifest_json_path.display().to_string(),
    };
    upsert_atlas_reference(&mut known_atlases, current_reference);

    let mut manifest = ContextAtlasManifest {
        protocol: CONTEXT_ATLAS_PROTOCOL.to_string(),
        status: status.to_string(),
        project,
        snapshot,
        generated_at,
        atlas_dir: atlas_dir.display().to_string(),
        manifest: manifest_path.display().to_string(),
        manifest_json: manifest_json_path.display().to_string(),
        recommended_start: atlas_dir.join("00-core-map.md").display().to_string(),
        identity,
        atlases: known_atlases,
        domain_owners: atlas_domain_owners(),
        cards,
        message,
    };

    write_manifest_files(&manifest)?;
    manifest.manifest = manifest_path.display().to_string();
    write_atlas_file_atomic(
        &receipt_path,
        serde_json::to_string_pretty(&json!({
            "protocol": CONTEXT_ATLAS_PROTOCOL,
            "generated_at": manifest.generated_at,
            "project": manifest.project,
            "snapshot": manifest.snapshot,
            "cards": manifest.cards.iter().map(|card| &card.path).collect::<Vec<_>>(),
            // Per-card FactSets: the base-fact ids each canonical payload
            // carries, so a downstream verifier can compare against the set it
            // reconstructs from the markdown card (set equality, not substring).
            "coverage_receipts": card_receipts.iter().map(|(path, receipt)| {
                json!({ "path": path, "fact_count": receipt.len(), "coverage_receipt": receipt })
            }).collect::<Vec<_>>()
        }))
        .map_err(io::Error::other)?,
    )?;

    Ok(manifest)
}

fn context_atlas_identity(pack: &ContextPack) -> ContextAtlasIdentity {
    let selectors = pack
        .scope
        .as_ref()
        .map(|scope| scope.selectors.clone())
        .unwrap_or_default();
    let scope_fingerprint = pack.scope.as_ref().map(|scope| scope.fingerprint.clone());
    let task = pack.task.as_ref().map(|task| task.text.clone());
    if scope_fingerprint.is_none() && selectors.is_empty() && task.is_none() {
        return ContextAtlasIdentity {
            atlas_id: "project".to_string(),
            kind: "project".to_string(),
            scope_fingerprint: None,
            selectors,
            task,
        };
    }

    let kind = if scope_fingerprint.is_some() || !selectors.is_empty() {
        "scope"
    } else {
        "task"
    };
    let payload = json!({
        "kind": kind,
        "scope_fingerprint": scope_fingerprint,
        "selectors": selectors,
        "task": task,
        "task_mode": pack.task.as_ref().map(|task| task.mode.as_str()),
    });
    let digest = payload_hash_hex(&canonical_json_pretty(&payload));
    ContextAtlasIdentity {
        atlas_id: format!("{kind}-{}", &digest[..16]),
        kind: kind.to_string(),
        scope_fingerprint,
        selectors,
        task,
    }
}

fn upsert_atlas_reference(
    atlases: &mut Vec<ContextAtlasReference>,
    current: ContextAtlasReference,
) {
    atlases.retain(|atlas| atlas.identity.atlas_id != current.identity.atlas_id);
    atlases.push(current);
    atlases.sort_by(|left, right| left.identity.atlas_id.cmp(&right.identity.atlas_id));
}

fn load_retained_atlases(atlas_root: &Path) -> io::Result<Vec<ContextAtlasReference>> {
    let manifest_path = atlas_root.join("manifest.json");
    if !manifest_path.exists() {
        return Ok(Vec::new());
    }
    let raw = crate::fs_utils::read_to_string_within(atlas_root, &manifest_path)?;
    let manifest: ContextAtlasManifest = serde_json::from_str(&raw).map_err(io::Error::other)?;
    if !manifest.atlases.is_empty() {
        return Ok(manifest.atlases);
    }
    if !manifest.identity.atlas_id.is_empty() {
        return Ok(vec![atlas_reference(&manifest)]);
    }

    // First run after the scope-keyed layout upgrade: preserve the existing
    // flat atlas before replacing the compatibility view. Its old manifest
    // has no scope contract, so label it honestly as legacy instead of
    // guessing that it was project-wide.
    let digest = payload_hash_hex(&raw);
    let identity = ContextAtlasIdentity {
        atlas_id: format!("legacy-{}", &digest[..16]),
        kind: "legacy_unknown_scope".to_string(),
        ..ContextAtlasIdentity::default()
    };
    let legacy_dir = atlas_root
        .join(CONTEXT_ATLAS_RUNS_DIR)
        .join(&identity.atlas_id);
    fs::create_dir_all(&legacy_dir)?;
    copy_atlas_payload(&manifest, atlas_root, &legacy_dir)?;

    let mut archived = manifest;
    archived.identity = identity;
    archived.atlas_dir = legacy_dir.display().to_string();
    archived.manifest = legacy_dir.join("manifest.md").display().to_string();
    archived.manifest_json = legacy_dir.join("manifest.json").display().to_string();
    archived.recommended_start = legacy_dir.join("00-core-map.md").display().to_string();
    let reference = atlas_reference(&archived);
    archived.atlases = vec![reference.clone()];
    write_manifest_files(&archived)?;
    Ok(vec![reference])
}

fn atlas_reference(manifest: &ContextAtlasManifest) -> ContextAtlasReference {
    ContextAtlasReference {
        identity: manifest.identity.clone(),
        snapshot: manifest.snapshot.clone(),
        generated_at: manifest.generated_at.clone(),
        atlas_dir: manifest.atlas_dir.clone(),
        manifest: manifest.manifest.clone(),
        manifest_json: manifest.manifest_json.clone(),
    }
}

/// Resolve a manifest-supplied atlas directory and prove it stays under
/// `atlas_root`.
///
/// `atlas_dir` is read back out of a manifest on disk, so it is data, not a
/// trusted path. A stale, hand-edited, or hostile manifest must not be able to
/// steer the mirror copy at a directory outside the atlas root. Routed through
/// the crate's validated-root gate: `..` refused, canonicalized, and required to
/// resolve underneath `atlas_root` (symlinks included).
fn contained_atlas_dir(raw: &str, atlas_root: &Path) -> io::Result<PathBuf> {
    // `join` keeps an absolute `raw` as-is and anchors a relative one at the
    // atlas root; `SanitizedPath::within` then rejects `..`, NUL bytes and —
    // after resolving symlinks — anything that does not live under the root.
    let candidate = atlas_root.join(raw);
    let sanitized = SanitizedPath::within(atlas_root, &candidate)
        .map_err(|err| io::Error::new(err.kind(), format!("atlas_dir rejected ({raw}): {err}")))?;
    Ok(sanitized.as_path().to_path_buf())
}

fn mirror_current_atlas(
    run_manifest: &ContextAtlasManifest,
    atlas_root: &Path,
) -> io::Result<ContextAtlasManifest> {
    let run_dir = contained_atlas_dir(&run_manifest.atlas_dir, atlas_root)?;
    copy_atlas_payload(run_manifest, &run_dir, atlas_root)?;

    let mut root_manifest = run_manifest.clone();
    root_manifest.atlas_dir = atlas_root.display().to_string();
    root_manifest.manifest = atlas_root.join("manifest.md").display().to_string();
    root_manifest.manifest_json = atlas_root.join("manifest.json").display().to_string();
    root_manifest.recommended_start = atlas_root.join("00-core-map.md").display().to_string();
    write_manifest_files(&root_manifest)?;
    Ok(root_manifest)
}

fn copy_atlas_payload(
    manifest: &ContextAtlasManifest,
    source_dir: &Path,
    destination_dir: &Path,
) -> io::Result<()> {
    fs::create_dir_all(destination_dir)?;
    for card in &manifest.cards {
        copy_flat_atlas_file(source_dir, destination_dir, &card.path)?;
        if let Some(full_path) = card.full_path.as_deref() {
            copy_flat_atlas_file(source_dir, destination_dir, full_path)?;
        }
    }
    if source_dir.join("receipt.json").is_file() {
        copy_flat_atlas_file(source_dir, destination_dir, "receipt.json")?;
    }
    for legacy in ["03-memory-trail.md", "03-memory-trail.full.json"] {
        let _ = fs::remove_file(destination_dir.join(legacy));
    }
    Ok(())
}

/// Copy one atlas card between directories.
///
/// This is the validated root object for manifest-supplied artifact names: the
/// name must be exactly one `Normal` path component, which rejects `..`, an
/// absolute root, and any nested path before it ever reaches the filesystem.
fn copy_flat_atlas_file(source_dir: &Path, destination_dir: &Path, name: &str) -> io::Result<()> {
    // Lexical gate on the raw string, before it ever becomes a path: exactly
    // one non-empty component, no separators, no `.`/`..`, no NUL.
    let flat = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
    if !flat {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("atlas artifact path must be one flat component: {name}"),
        ));
    }
    // Filesystem gate next to the sink: `SanitizedPath::within` requires the
    // source to canonicalize underneath `source_dir` (symlinks included)
    // before the bytes are atomically persisted at the destination.
    let sanitized = SanitizedPath::within(source_dir, &source_dir.join(name)).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("atlas artifact rejected ({name}): {err}"),
        )
    })?;
    let bytes = fs::read(sanitized.as_path())?;
    write_atlas_file_atomic(destination_dir.join(name), bytes)?;
    Ok(())
}

fn write_manifest_files(manifest: &ContextAtlasManifest) -> io::Result<()> {
    let manifest_path = Path::new(&manifest.manifest);
    let dir = manifest_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atlas manifest path has no parent",
        )
    })?;
    write_atlas_file_atomic(manifest_path, render_manifest(manifest).as_bytes())?;
    write_atlas_file_atomic(
        Path::new(&manifest.manifest_json),
        serde_json::to_string_pretty(manifest).map_err(io::Error::other)?,
    )?;
    write_atlas_file_atomic(
        dir.join(ATLAS_IDENTITIES_FULL_JSON),
        serde_json::to_string_pretty(&manifest.atlases).map_err(io::Error::other)?,
    )?;
    Ok(())
}

struct CardSpec {
    id: &'static str,
    title: &'static str,
    filename: &'static str,
    why: &'static str,
    saves: &'static str,
    body: CardBody,
}

/// Machine-reconstructable base-fact identifier (`kind:path[:detail]`).
/// The grammar per card domain is frozen in `docs/contracts/atlas-card-format.md`
/// (kanon v4, L1-02): `hub:` / `edge:` / `hotspots:` / `authority:` /
/// `reachability:` / `entry:` / `env:` / `dispatch:` / `gate:` / `test:` /
/// `thesis:`. The fact-id prefix names the data domain, so receipt-level
/// ownership checks against manifest `domain_owners` are non-vacuous.
type FactId = String;

/// A rendered atlas card: the markdown surface plus the canonical payload
/// contract. The payload is a first-class product of rendering — not a
/// truncation side-effect — so the materializer writes the `<stem>.full.json`
/// sibling for every card with a non-empty payload, capped fence or not.
struct CardBody {
    /// Card content written to `<filename>` (fence capped to the per-card budget).
    markdown: String,
    /// Canonical JSON payload backing this card.
    canonical_payload: serde_json::Value,
    /// SHA-256 (hex) of the canonical serialization (sorted object keys,
    /// newline-terminated) — byte-identical to the `.full.json` sibling.
    payload_hash: String,
    /// FactSet: `fact_id` of every base fact the payload carries. A downstream
    /// verifier compares this set (set equality, not substring) against the
    /// set it reconstructs from the markdown card.
    coverage_receipt: Vec<FactId>,
    /// Lines dropped from the on-card JSON fence when capped (`None` = whole).
    fence_dropped_lines: Option<usize>,
}

fn snapshot_label(pack: &ContextPack) -> String {
    if snapshot_missing(pack) {
        return "no snapshot".to_string();
    }
    let branch = pack.project.branch.as_deref().unwrap_or("unknown");
    let commit = pack.project.commit.as_deref().unwrap_or("unknown");
    format!("{}@{}", branch, commit)
}

fn atlas_freshness_line(pack: &ContextPack) -> String {
    if snapshot_missing(pack) {
        "NO SNAPSHOT - run loct scan before relying on this card.".to_string()
    } else if pack.risk.stale_snapshot {
        format!(
            "STALE - card snapshot {} lags live git state; refresh with `loct context --full` before relying on this card.",
            snapshot_label(pack)
        )
    } else if pack.risk.dirty_worktree {
        "DIRTY - card was generated from a dirty worktree; verify changed files before relying on this card."
            .to_string()
    } else {
        "fresh - card matches the loaded snapshot authority.".to_string()
    }
}

fn snapshot_missing(pack: &ContextPack) -> bool {
    matches!(pack.risk.cache_scope, RiskCacheScope::MissingSnapshot)
        || pack.risk.snapshot_health.as_deref() == Some("missing_snapshot")
}

fn atlas_corpus_size(pack: &ContextPack) -> usize {
    pack.structural.files.len() + pack.structural.symbols.len() + pack.structural.imports.len()
}

fn atlas_status(pack: &ContextPack) -> &'static str {
    if snapshot_missing(pack) {
        ATLAS_STATUS_HOLLOW
    } else if pack.risk.stale_snapshot {
        ATLAS_STATUS_STALE
    } else if atlas_corpus_size(pack) == 0 {
        ATLAS_STATUS_HOLLOW
    } else {
        ATLAS_STATUS_READY
    }
}

fn atlas_status_message(status: &str) -> String {
    match status {
        ATLAS_STATUS_HOLLOW => {
            "This atlas is hollow: no snapshot (or empty corpus). Run `loct scan` before treating cards as repository truth. Cards are unread/unusable until a snapshot exists."
                .to_string()
        }
        ATLAS_STATUS_STALE => {
            "This atlas is stale relative to live git state. Refresh with `loct scan` or `loct context --full` before relying on cards."
                .to_string()
        }
        _ => {
            "This atlas contains the repo understanding an agent would otherwise rediscover manually. Start with manifest.md, then read the recommended cards; broad repo-level answers are incomplete until core, structural, and runtime are read."
                .to_string()
        }
    }
}

fn bound_atlas_identities(atlases: &[ContextAtlasReference]) -> (&[ContextAtlasReference], usize) {
    if atlases.len() <= ATLAS_IDENTITY_LIST_LIMIT {
        (atlases, 0)
    } else {
        (
            &atlases[..ATLAS_IDENTITY_LIST_LIMIT],
            atlases.len() - ATLAS_IDENTITY_LIST_LIMIT,
        )
    }
}

/// Exclusive lock for the shared atlas root (catalog + flat-card mirror).
struct AtlasMirrorLock {
    file: File,
}

impl Drop for AtlasMirrorLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn acquire_atlas_mirror_lock(atlas_root: &Path) -> io::Result<AtlasMirrorLock> {
    fs::create_dir_all(atlas_root)?;
    let lock_path = atlas_root.join(".mirror.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    FileExt::lock_exclusive(&file)?;
    Ok(AtlasMirrorLock { file })
}

/// Same tmp+persist/rename shape as `snapshot::write_atomic`: readers never
/// observe a partially written card under the destination path.
fn write_atlas_file_atomic(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atlas write path has no parent",
        )
    })?;
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".loctree-atlas-")
        .tempfile_in(dir)?;
    tmp.write_all(contents.as_ref())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

fn render_manifest(manifest: &ContextAtlasManifest) -> String {
    let mut out = String::new();
    out.push_str("# Loctree Context Atlas\n\n");
    out.push_str(&format!("Project: `{}`\n", manifest.project));
    out.push_str(&format!("Snapshot: `{}`\n", manifest.snapshot));
    out.push_str(&format!("Generated: `{}`\n\n", manifest.generated_at));
    out.push_str(&format!(
        "Active identity: `{}` (`{}`)\n\n",
        manifest.identity.atlas_id, manifest.identity.kind
    ));
    out.push_str("This atlas is precomputed repository understanding. It contains the repo map an agent would otherwise have to rediscover manually through search/open cycles.\n\n");
    out.push_str("Tokens are cheaper than wrong assumptions.\n\n");
    out.push_str("## Persisted atlas identities\n\n");
    out.push_str("The flat cards in this directory are a compatibility view of the active identity. Scope-keyed atlases below remain independently addressable.\n\n");
    out.push_str("| Identity | Kind | Selectors | Task | Snapshot | Manifest |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    let (shown, omitted) = bound_atlas_identities(&manifest.atlases);
    for atlas in shown {
        let selectors = if atlas.identity.selectors.is_empty() {
            "—".to_string()
        } else {
            atlas.identity.selectors.join(", ")
        };
        let task = atlas.identity.task.as_deref().unwrap_or("—");
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` | `{}` | `{}` |\n",
            atlas.identity.atlas_id,
            atlas.identity.kind,
            selectors.replace('`', "'"),
            task.replace('`', "'"),
            atlas.snapshot,
            atlas.manifest
        ));
    }
    if omitted > 0 {
        out.push_str(&format!(
            "\n… and {omitted} more (full list: `{ATLAS_IDENTITIES_FULL_JSON}`)\n"
        ));
    }
    out.push('\n');
    out.push_str("## Recommended Reading Path\n\n");
    out.push_str("| Step | File | Lines | Why read it | Saves you from |\n");
    out.push_str("|---:|---|---:|---|---|\n");
    for (idx, card) in manifest.cards.iter().enumerate() {
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} |\n",
            idx,
            card.path,
            card_line_label(card),
            card.why,
            card.saves_you_from
        ));
    }
    out.push_str("\n## Domain owners\n\n");
    out.push_str("One domain = one owner (source of truth: `domain_owners` in manifest.json; section headers project that map). Other cards reference a domain in one line; they never duplicate it.\n\n");
    let mut owners_by_card: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (domain, owner) in &manifest.domain_owners {
        owners_by_card
            .entry(owner.as_str())
            .or_default()
            .push(domain.as_str());
    }
    for (owner, domains) in owners_by_card {
        out.push_str(&format!("{owner}: {}\n", domains.join(" · ")));
    }

    out.push_str("\n## Completeness\n\n");
    out.push_str(&format!("Atlas status: `{}`\n", manifest.status));
    if manifest.status == ATLAS_STATUS_HOLLOW {
        out.push_str(
            "Run `loct scan` — cards below are unread/unusable until a snapshot exists.\n",
        );
    }
    out.push_str("Current reading state: `0/");
    out.push_str(&manifest.cards.len().to_string());
    out.push_str("` context cards read.\n");
    out.push_str("A broad repo-level answer is incomplete until at least `00-core-map.md`, `01-structural-map.md`, and `02-runtime-map.md` have been read.\n");

    let partial: Vec<&ContextAtlasCard> = manifest.cards.iter().filter(|c| c.truncated).collect();
    if !partial.is_empty() {
        out.push_str("Partial-card completeness is stricter: the atlas is not fully read until each listed `.full.json` sibling has been opened too.\n");
        out.push_str("\n## Partial cards\n\n");
        out.push_str("These cards were capped to the per-card screen budget. Do not treat them as exhaustive — read the complete payload at the sibling artifact before relying on the card:\n\n");
        for card in partial {
            let full = card.full_path.as_deref().unwrap_or("(sibling .full.json)");
            out.push_str(&format!(
                "- `{}` — {}; {} payload line(s) dropped. Complete payload: `{}`\n",
                card.path,
                card_line_label(card),
                card.dropped_lines,
                full
            ));
        }
    }
    out
}

fn card_line_label(card: &ContextAtlasCard) -> String {
    let base = if !card.truncated {
        format!("{} lines", card.lines)
    } else {
        match card.full_payload_lines {
            Some(full_payload_lines) => format!(
                "{} materialized lines / {} full-payload lines ⚠ partial",
                card.lines, full_payload_lines
            ),
            None => format!("{} materialized lines ⚠ partial", card.lines),
        }
    };
    match card.status.as_deref() {
        Some(status) => format!("{base} · {status}"),
        None => base,
    }
}

/// Dispatch a card kind to its dense-markdown renderer. Unknown kinds fall
/// back to the generic JSON card so a future card never ships silently empty
/// (kanon v4: `render_json_card` stays as fallback, zero calls for 00-05).
/// Cards 00 (core) and 03 (intent) are rendered directly by
/// `materialize_context_atlas` — both read the I1-01 overlay layer, not the
/// pack alone.
fn render_card_body(pack: &ContextPack, kind: &str, filename: &str, title: &str) -> CardBody {
    match kind {
        "structural" => render_structural_card(pack),
        "runtime" => render_runtime_card(pack),
        "verification" => render_verification_card(pack),
        "risk" => render_risk_card(pack),
        _ => render_json_card(
            pack,
            filename,
            title,
            "Unknown card kind — generic JSON payload fallback.",
            "Unknown card kind: no domain coverage map exists for this card.",
            serde_json::Value::Null,
            Vec::new(),
        ),
    }
}

fn repo_short_name(pack: &ContextPack) -> String {
    pack.project
        .canonical_root
        .as_deref()
        .and_then(|root| Path::new(root).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Shared card frame per `docs/contracts/atlas-card-format.md` (shared
/// rules): title line, freshness, canonical-payload pointer, one-line lead.
fn card_frame_header(pack: &ContextPack, title: &str, filename: &str, lead: &str) -> String {
    format!(
        "# {} · {} @ {}\nFreshness: {}\nFull payload: {}\n\n{}\n",
        title,
        repo_short_name(pack),
        snapshot_label(pack),
        atlas_freshness_line(pack),
        full_json_filename(filename),
        lead
    )
}

fn card_frame_footer(missing: &str) -> String {
    format!("\n## What this card does not cover\n\n{missing}\n")
}

/// One explicit line for an empty section — a section never disappears
/// (contract: `no <what> — corpus: <N>`).
fn empty_section_line(what: &str, corpus: usize) -> String {
    format!("no {what} — corpus: {corpus}\n")
}

fn authority_name(label: AuthorityLabel) -> &'static str {
    match label {
        AuthorityLabel::RepoVerified => "RepoVerified",
        AuthorityLabel::LoctreeDerived => "LoctreeDerived",
        AuthorityLabel::AicxOperator => "AicxOperator",
        AuthorityLabel::AicxAgent => "AicxAgent",
        AuthorityLabel::AicxFailure => "AicxFailure",
        AuthorityLabel::SemanticGuess => "SemanticGuess",
        AuthorityLabel::StaleOrUnknown => "StaleOrUnknown",
    }
}

/// Pack-wide per-label claim counts in the fixed label order. Card 01 (the
/// authority-domain owner) renders this as ONE counter line whose grammar
/// yields fact ids `authority:<Label>` for every label with count > 0.
fn authority_counter_pairs(authority: &AuthoritySlice) -> [(&'static str, usize); 7] {
    [
        (
            authority_name(AuthorityLabel::RepoVerified),
            authority.repo_verified.len(),
        ),
        (
            authority_name(AuthorityLabel::LoctreeDerived),
            authority.loctree_derived.len(),
        ),
        (
            authority_name(AuthorityLabel::AicxOperator),
            authority.aicx_operator.len(),
        ),
        (
            authority_name(AuthorityLabel::AicxAgent),
            authority.aicx_agent.len(),
        ),
        (
            authority_name(AuthorityLabel::AicxFailure),
            authority.aicx_failure.len(),
        ),
        (
            authority_name(AuthorityLabel::SemanticGuess),
            authority.semantic_guess.len(),
        ),
        (
            authority_name(AuthorityLabel::StaleOrUnknown),
            authority.stale_or_unknown.len(),
        ),
    ]
}

/// Render any serializable enum/struct as a single-line value (quotes stripped
/// for plain string variants) so card lines never depend on `Debug` shape.
fn one_line_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "unknown".to_string())
        .trim_matches('"')
        .to_string()
}

/// Declarative one-line thesis distillate: flattened whitespace, ≤200 chars.
/// `←` is neutralized so free-form memory text can never fake an edge-fact
/// grammar line on a card whose receipt is empty.
fn one_line_thesis(text: &str) -> String {
    let flat = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('←', "<-");
    if flat.chars().count() <= 200 {
        flat
    } else {
        let mut out: String = flat.chars().take(199).collect();
        out.push('…');
        out
    }
}

/// Seal a dense card: dedup the receipt into a set, hash the canonical payload.
fn finish_card(
    markdown: String,
    canonical_payload: serde_json::Value,
    mut coverage_receipt: Vec<FactId>,
) -> CardBody {
    coverage_receipt.sort();
    coverage_receipt.dedup();
    let payload_hash = payload_hash_hex(&canonical_json_pretty(&canonical_payload));
    CardBody {
        markdown,
        canonical_payload,
        payload_hash,
        coverage_receipt,
        fence_dropped_lines: None,
    }
}

fn render_core_card(pack: &ContextPack, source: &IntentCardSource) -> CardBody {
    // Core is a projection card (identity + risk summary) — its facts are
    // owned by the structural/runtime/risk domains, so its receipt is empty.
    let mut md = card_frame_header(
        pack,
        "Core Map",
        "00-core-map.md",
        "This card tells you where you are, what is risky, and what actions are safe next.",
    );

    md.push_str("\n## Identity\n\n");
    md.push_str(&format!("repo_id: {}\n", repo_short_name(pack)));
    md.push_str(&format!(
        "branch: {}\n",
        pack.project
            .branch
            .as_deref()
            .unwrap_or("unavailable — snapshot does not carry a branch")
    ));
    md.push_str(&format!(
        "snapshot_commit: {}\n",
        pack.project
            .commit
            .as_deref()
            .unwrap_or("unavailable — snapshot does not carry a commit")
    ));
    md.push_str(&format!(
        "snapshot_id: {}\n",
        pack.project
            .snapshot_id
            .as_deref()
            .unwrap_or("unavailable — pack composed without a snapshot id")
    ));
    // Intent-layer revisions. The pack's own overlay state is the richest
    // source — it is judged against full local truth and carries the anchor
    // catalog revision too. Compositions that carry no overlay state (LSP,
    // --with-aicx) still have the cached producer document behind card 03,
    // which knows the store and overlay revisions. Only when both are absent
    // is the layer genuinely cold, and the card says so in those words.
    let state = pack
        .memory
        .overlay
        .as_ref()
        .filter(|state| !state.store_revision.is_empty());
    let cold = "unavailable — no overlay document (cold store); refresh with `aicx overlay`";
    md.push_str(&format!(
        "store_revision: {}\n",
        state
            .map(|state| short_revision(&state.store_revision))
            .or_else(|| source
                .doc
                .as_ref()
                .map(|doc| short_revision(&doc.store_revision)))
            .unwrap_or_else(|| cold.to_string())
    ));
    md.push_str(&format!(
        "overlay_revision: {}\n",
        state
            .map(|state| short_revision(&state.overlay_revision))
            .or_else(|| source
                .doc
                .as_ref()
                .map(|doc| short_revision(&doc.overlay_revision)))
            .unwrap_or_else(|| cold.to_string())
    ));
    md.push_str(&format!(
        "anchor_catalog_revision: {}\n",
        state
            .map(|state| short_revision(&state.anchor_catalog_revision))
            .or_else(|| source
                .doc
                .as_ref()
                .map(|doc| short_revision(&doc.anchor_catalog_revision)))
            .unwrap_or_else(|| cold.to_string())
    ));

    md.push_str("\n## Freshness\n\n");
    md.push_str(&format!("{}\n", atlas_freshness_line(pack)));
    md.push_str(
        "cache/snapshot details → 05-risk-register.md §Cache & Snapshot Health (freshness domain owner)\n",
    );

    md.push_str("\n## Risk Summary\n\n");
    // Projection of the hotspots domain (owner: karta 01) — top-5 pointer
    // lines only, never the full register.
    let mut ranked: Vec<&HighFanInFile> = pack.risk.high_fan_in.iter().collect();
    ranked.sort_by(|a, b| b.importers.cmp(&a.importers).then(a.file.cmp(&b.file)));
    if ranked.is_empty() {
        md.push_str(&format!(
            "no fan-in risks in pack — hotspots: {} → 01-structural-map.md\n",
            pack.risk.hotspots.len()
        ));
    } else {
        for hub in ranked.iter().take(5) {
            md.push_str(&format!(
                "- {} · fan-in {} → 01-structural-map.md\n",
                hub.file, hub.importers
            ));
        }
    }

    md.push_str(
        "\nauthority (per-label counters) → 01-structural-map.md §Authority (authority domain owner)\n",
    );

    md.push_str("\n## Safe Next Commands\n\n");
    // Karta 00 owns the safe-next-commands domain: baseline commands plus the
    // scope-aware power-path suggestions, deduped by command text. The LSP
    // contextPack "core" page serves this card — executable guidance must
    // survive the dense render.
    let mut listed_commands: BTreeSet<&str> = BTreeSet::new();
    for command in &pack.action.next_safe_commands {
        if !listed_commands.insert(command.as_str()) {
            continue;
        }
        match pack
            .action
            .power_path
            .iter()
            .find(|suggested| &suggested.command == command)
        {
            Some(suggested) => md.push_str(&format!("{} — {}\n", command, suggested.reason)),
            None => md.push_str(&format!("{command}\n")),
        }
    }
    for suggested in &pack.action.power_path {
        if listed_commands.insert(suggested.command.as_str()) {
            md.push_str(&format!("{} — {}\n", suggested.command, suggested.reason));
        }
    }
    if listed_commands.is_empty() {
        md.push_str(&empty_section_line("bezpiecznych komend w packu", 0));
    }

    md.push_str(&card_frame_footer(
        "This Core Map does not include dependency consumers, runtime entrypoints, or prior decisions. For code changes, read `01-structural-map.md` next.",
    ));
    finish_card(
        md,
        json!({
            "schema_version": pack.schema_version,
            "project": &pack.project,
            "risk": &pack.risk,
            "action": &pack.action,
            "authority": &pack.authority,
        }),
        Vec::new(),
    )
}

fn render_structural_card(pack: &ContextPack) -> CardBody {
    let structural = &pack.structural;
    let hubs = &pack.risk.high_fan_in;
    let mut md = card_frame_header(
        pack,
        "Structural Map",
        "01-structural-map.md",
        "This card contains dependency and symbol topology for the selected scope.",
    );

    let mut fan_out: BTreeMap<&str, usize> = BTreeMap::new();
    for import in &structural.imports {
        if import.resolved_path.is_some() {
            *fan_out.entry(import.file.as_str()).or_insert(0) += 1;
        }
    }
    let file_meta: BTreeMap<&str, (&str, usize)> = structural
        .files
        .iter()
        .map(|file| (file.path.as_str(), (file.language.as_str(), file.loc)))
        .collect();

    let threshold = hubs.first().map(|hub| hub.threshold).unwrap_or(10);
    md.push_str(&format!(
        "\n## Hubs (fan-in ≥ {threshold}) — domain owner: this card\n\n"
    ));
    if hubs.is_empty() {
        md.push_str(&empty_section_line(
            "hubs above fan-in threshold",
            structural.files.len(),
        ));
    } else {
        md.push_str("| # | File | Fan-in | Fan-out | Role |\n|--:|---|--:|--:|---|\n");
        let mut ranked: Vec<&HighFanInFile> = hubs.iter().collect();
        ranked.sort_by(|a, b| b.importers.cmp(&a.importers).then(a.file.cmp(&b.file)));
        for (idx, hub) in ranked.iter().enumerate() {
            let (lang, loc) = file_meta
                .get(hub.file.as_str())
                .copied()
                .unwrap_or(("?", 0));
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} · {} LOC |\n",
                idx + 1,
                hub.file,
                hub.importers,
                fan_out.get(hub.file.as_str()).copied().unwrap_or(0),
                lang,
                loc,
            ));
        }
    }

    // Hotspots share the Hubs section — karta 01 owns the hotspots domain
    // (manifest `domain_owners`); karta 05 keeps a one-line reference only.
    if pack.risk.hotspots.is_empty() {
        md.push_str(&empty_section_line("hotspots in pack", hubs.len()));
    } else {
        md.push('\n');
        let mut ranked_hotspots: Vec<&HotspotFile> = pack.risk.hotspots.iter().collect();
        ranked_hotspots.sort_by(|a, b| b.importers.cmp(&a.importers).then(a.file.cmp(&b.file)));
        let mut seen_hotspots: BTreeSet<&str> = BTreeSet::new();
        for hotspot in ranked_hotspots {
            if seen_hotspots.insert(hotspot.file.as_str()) {
                md.push_str(&format!(
                    "hotspots:{} · importers {} · mitigation: `loct slice` before edit, `loct impact` before delete\n",
                    hotspot.file, hotspot.importers,
                ));
            }
        }
    }

    md.push_str("\n## Consumers per hub (DECISION-COMPLETE: all edges inline, grouped)\n\n");
    // target -> importer-dir -> importer basenames. Grammar (kanon v4):
    // `hub ← dir/{a,b}` expands deterministically to `edge:<hub>:<dir>/<a>` …
    let mut edges: BTreeMap<&str, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let mut resolved_edges = 0usize;
    for import in &structural.imports {
        if let Some(target) = import.resolved_path.as_deref() {
            resolved_edges += 1;
            let importer = Path::new(&import.file);
            let dir = importer
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default();
            let base = importer
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| import.file.clone());
            edges
                .entry(target)
                .or_default()
                .entry(dir)
                .or_default()
                .insert(base);
        }
    }
    if edges.is_empty() {
        md.push_str(&empty_section_line(
            "resolved import edges",
            structural.imports.len(),
        ));
    } else {
        let mut ordered: Vec<(&str, usize)> = edges
            .iter()
            .map(|(target, dirs)| (*target, dirs.values().map(BTreeSet::len).sum()))
            .collect();
        ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (target, _) in ordered {
            let rendered: Vec<String> = edges[target]
                .iter()
                .map(|(dir, names)| {
                    let prefix = if dir.is_empty() {
                        String::new()
                    } else {
                        format!("{dir}/")
                    };
                    if names.len() == 1 {
                        format!("{prefix}{}", names.iter().next().expect("non-empty group"))
                    } else {
                        format!(
                            "{prefix}{{{}}}",
                            names.iter().cloned().collect::<Vec<_>>().join(",")
                        )
                    }
                })
                .collect();
            md.push_str(&format!("{target} ← {}\n", rendered.join(" · ")));
        }
    }

    md.push_str("\n## Import graph — shape\n\n");
    md.push_str(&format!(
        "{} edges resolved from {} imports · {} files · {} symbols · {} slice consumers\n",
        resolved_edges,
        structural.imports.len(),
        structural.files.len(),
        structural.symbols.len(),
        structural.consumers.len(),
    ));
    let mut clusters: BTreeMap<String, usize> = BTreeMap::new();
    for file in &structural.files {
        let top = file.path.split('/').next().unwrap_or("").to_string();
        *clusters.entry(top).or_insert(0) += 1;
    }
    let mut ranked_clusters: Vec<(String, usize)> = clusters.into_iter().collect();
    ranked_clusters.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let shown: Vec<String> = ranked_clusters
        .iter()
        .take(6)
        .map(|(dir, count)| format!("{dir} ({count})"))
        .collect();
    md.push_str(&format!(
        "File clusters: {}\n",
        if shown.is_empty() {
            "none".to_string()
        } else {
            shown.join(" · ")
        }
    ));
    if structural.entrypoints.is_empty() {
        md.push_str("structural entrypoints: none in slice\n");
    } else {
        let entries: Vec<String> = structural
            .entrypoints
            .iter()
            .map(|entry| format!("{} [{}]", entry.path, entry.kinds.join(",")))
            .collect();
        md.push_str(&format!(
            "structural entrypoints: {}\n",
            entries.join(" · ")
        ));
    }

    md.push_str("\n## Reachability — domain owner: this card\n\n");
    // Reachability = import graph (this card) composed with runtime
    // entrypoints; the domain lives here per manifest `domain_owners`, karta
    // 02 keeps a one-line reference.
    let reachability = &pack.runtime.reachability;
    if reachability.is_empty() {
        md.push_str("no reachability data — semantic pass found no entrypoints\n");
    } else {
        let reached = reachability.iter().filter(|claim| claim.reached).count();
        md.push_str(&format!(
            "reachable: {} of {} reachability claims\n",
            reached,
            reachability.len()
        ));
        let mut unreachable: BTreeMap<&str, &str> = BTreeMap::new();
        for claim in reachability {
            if !claim.reached
                && let Some((file, _)) = claim.symbol.split_once("::")
            {
                unreachable.entry(file).or_insert(claim.reason.as_str());
            }
        }
        if unreachable.is_empty() {
            md.push_str(&format!(
                "no unreachable surfaces — claims: {}\n",
                reachability.len()
            ));
        } else {
            for (file, reason) in &unreachable {
                md.push_str(&format!(
                    "reachability:{file} · unreachable · hypothesis: {reason}\n"
                ));
            }
        }
    }

    md.push_str("\n## Authority — domain owner: this card\n\n");
    md.push_str(&format!(
        "authority:{}\n",
        authority_counter_pairs(&pack.authority)
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(" · ")
    ));

    md.push_str(&card_frame_footer(
        "This Structural Map does not include runtime behavior, env contracts, or verification gates. Read `02-runtime-map.md` and `04-verification-gates.md` before changing behavior.",
    ));
    finish_card(
        md,
        json!({ "structural": structural, "high_fan_in": hubs }),
        structural_coverage_receipt(pack),
    )
}

fn render_runtime_card(pack: &ContextPack) -> CardBody {
    let runtime = &pack.runtime;
    let mut md = card_frame_header(
        pack,
        "Runtime Map",
        "02-runtime-map.md",
        "This card contains runtime signals derived from semantic facts and framework bridges.",
    );

    md.push_str("\n## Entrypoints\n\n");
    let mut entry_symbols: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for hint in &runtime.framework_hints {
        if is_runtime_owner_hint(&hint.kind) {
            entry_symbols
                .entry(hint.file.as_str())
                .or_default()
                .insert(format!("{}:{}", hint.kind, hint.symbol));
        }
    }
    if entry_symbols.is_empty() {
        md.push_str(&empty_section_line(
            "runtime entrypoints in pack",
            runtime.framework_hints.len(),
        ));
    } else {
        for (file, symbols) in &entry_symbols {
            let listed: Vec<&str> = symbols.iter().map(String::as_str).take(5).collect();
            let extra = symbols.len().saturating_sub(listed.len());
            let suffix = if extra > 0 {
                format!(" +{extra} more")
            } else {
                String::new()
            };
            md.push_str(&format!(
                "entry:{file} · symbols: {}{suffix}\n",
                listed.join(", ")
            ));
        }
    }

    md.push_str("\n## Env Contracts\n\n");
    let mut env_seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for contract in &runtime.env_contracts {
        for file in &contract.used_in_files {
            if !env_seen.insert((contract.name.as_str(), file.as_str())) {
                continue;
            }
            let requirement = if contract.required {
                "required"
            } else {
                "requiredness unproven"
            };
            md.push_str(&format!(
                "env:{} · {} · {}\n",
                contract.name, file, requirement
            ));
        }
    }
    if env_seen.is_empty() {
        if let Some(coverage) = &runtime.inventory_coverage {
            md.push_str(&format!(
                "no named env reads in inventoried classes after scanning {} files; omission outside those classes does not prove zero\n",
                coverage.source_files_scanned
            ));
        } else {
            md.push_str("no env contracts in scope; missing data does not prove zero\n");
        }
    }
    if let Some(coverage) = &runtime.inventory_coverage {
        md.push_str(&format!(
            "inventory env classes: {}\n",
            coverage.env_read_classes.join(", ")
        ));
        md.push_str(&format!(
            "inventory owner classes: {}\n",
            coverage.owner_classes.join(", ")
        ));
    }

    md.push_str("\n## Framework Hints\n\n");
    let mut hint_kinds: BTreeMap<&str, Vec<&RuntimeFrameworkHint>> = BTreeMap::new();
    for hint in &runtime.framework_hints {
        if !is_runtime_owner_hint(&hint.kind) {
            hint_kinds.entry(hint.kind.as_str()).or_default().push(hint);
        }
    }
    if hint_kinds.is_empty() {
        md.push_str(&empty_section_line(
            "framework hints in pack",
            runtime.framework_hints.len(),
        ));
    } else {
        for (kind, hints) in &hint_kinds {
            let files: BTreeSet<&str> = hints.iter().map(|hint| hint.file.as_str()).collect();
            let sample: Vec<&str> = files.iter().copied().take(3).collect();
            let extra = files.len().saturating_sub(sample.len());
            let suffix = if extra > 0 {
                format!(", +{extra} more files")
            } else {
                String::new()
            };
            md.push_str(&format!(
                "hint: {kind} · {} occurrences · {}{}\n",
                hints.len(),
                sample.join(", "),
                suffix
            ));
        }
    }
    md.push_str(&format!(
        "idiom-tags: {} · full list: 02-runtime-map.full.json#idiom_tags\n",
        runtime.idiom_tags.len()
    ));
    md.push_str(&format!(
        "tauri: commands {} · events {}\n",
        runtime.tauri_commands.len(),
        runtime.tauri_events.len()
    ));

    md.push_str("\n## Dispatch Edges\n\n");
    let mut dispatch: BTreeMap<(&str, &str), &RuntimeDispatchEdge> = BTreeMap::new();
    for edge in &runtime.dispatch_edges {
        let target = edge
            .handler_file
            .as_deref()
            .unwrap_or(edge.handler_symbol.as_str());
        dispatch
            .entry((edge.from_file.as_str(), target))
            .or_insert(edge);
    }
    if dispatch.is_empty() {
        md.push_str(&empty_section_line("dispatch edges in pack", 0));
    } else {
        for ((from, target), edge) in &dispatch {
            let mut detail = edge.dispatch_kind.clone();
            if let Some(framework) = &edge.framework {
                detail.push_str(&format!(" · {framework}"));
            }
            if let (Some(method), Some(path)) = (&edge.http_method, &edge.http_path) {
                detail.push_str(&format!(" · {method} {path}"));
            }
            md.push_str(&format!("dispatch:{from}→{target} · {detail}\n"));
        }
    }

    md.push_str(
        "\nreachability → 01-structural-map.md §Reachability (reachability domain owner)\n",
    );

    md.push_str(&card_frame_footer(
        "This Runtime Map does not include prior decisions or full risk history. Read `03-intent-map.md` when continuing work and `05-risk-register.md` before release decisions.",
    ));
    let mut runtime_payload = runtime.clone();
    for contract in &mut runtime_payload.env_contracts {
        for occurrence in &mut contract.occurrences {
            occurrence.default = None;
        }
    }
    finish_card(
        md,
        json!(runtime_payload),
        runtime_coverage_receipt(runtime),
    )
}

fn is_runtime_owner_hint(kind: &str) -> bool {
    matches!(
        kind,
        "entrypoint"
            | "executable_owner"
            | "cargo_default_run"
            | "cargo_bin"
            | "cargo_library_crate_type"
            | "make_target_owner"
            | "swiftui_app"
            | "swiftpm_executable_target"
    )
}

/// I1-01 seam resolution for the intent card: the typed overlay document read
/// from the cache (never a producer spawn — the detached refresh owned by the
/// CLI handler is the only road to the producer) plus an explicit staleness
/// marker when the layer is degraded.
#[derive(Default)]
struct IntentCardSource {
    doc: Option<OverlayDoc>,
    /// Raw producer emission as parsed JSON — carried into the canonical
    /// payload so `unresolved_attributions` stay payload-only (Doktryna 7)
    /// without this consumer deserializing them.
    raw: Option<serde_json::Value>,
    /// `intent layer stale (<reason>) — refresh: <command>` when degraded.
    stale: Option<String>,
}

fn resolve_intent_card_source(pack: &ContextPack, project_root: &Path) -> IntentCardSource {
    let refresh = refresh_command(project_root);
    let (doc, load_reason) = match load_cached_overlay(project_root) {
        Ok(Some(cached)) => (Some(cached.doc), None),
        Ok(None) => (None, Some("no overlay cache yet".to_string())),
        Err(err) => (None, Some(format!("cache rejected: {err}"))),
    };
    let raw = if doc.is_some() {
        fs::read_to_string(overlay_cache_path(project_root))
            .ok()
            .and_then(|body| serde_json::from_str(&body).ok())
    } else {
        None
    };

    let mut stale = load_reason.map(|reason| stale_intent_line(&reason, &refresh));
    if stale.is_none() {
        stale = match pack.memory.overlay.as_ref() {
            // The bare-path composer already judged freshness against full
            // local truth (snapshot commit + anchor catalog + repo identity).
            Some(state) => state.stale_marker(),
            // Non-bare compositions (e.g. --with-aicx, LSP): prove what the
            // pack alone can prove — the snapshot commit.
            None => doc.as_ref().and_then(|doc| {
                let local = LocalTruth {
                    repo_id: None,
                    snapshot_commit: pack
                        .project
                        .commit
                        .clone()
                        .filter(|commit| !commit.is_empty()),
                    anchor_catalog_revision: None,
                };
                staleness_reason(&doc.key(), &local)
                    .map(|reason| stale_intent_line(&reason, &refresh))
            }),
        };
    }
    if stale.is_none() && !crate::aicx::transport_reachable_for_render() {
        // Even a fresh cache cannot converge without the producer binary —
        // the operator must see that the layer is running on borrowed light.
        stale = Some(stale_intent_line(
            "aicx transport unreachable — serving last cached correct data",
            &refresh,
        ));
    }

    IntentCardSource { doc, raw, stale }
}

/// The explicit degradation marker in the resilience-gate grammar.
fn stale_intent_line(reason: &str, refresh: &str) -> String {
    format!("intent layer stale ({reason}) — refresh: `{refresh}`")
}

/// Producer authority tier as the snake-case name the golden spec uses on
/// thesis lines (`operator_confirmed` / `agent_derived` / `inferred`).
fn intent_authority_name(authority: OverlayAuthority) -> &'static str {
    match authority {
        OverlayAuthority::OperatorConfirmed => "operator_confirmed",
        OverlayAuthority::AgentDerived => "agent_derived",
        OverlayAuthority::Inferred => "inferred",
    }
}

/// Attribution path of an entry, when the producer pinned it to one.
fn intent_target_path(entry: &OverlayEntry) -> Option<&str> {
    match &entry.target {
        OverlayTarget::Path { path } => Some(path),
        OverlayTarget::Symbol {
            path: Some(path), ..
        } => Some(path),
        _ => None,
    }
}

/// Human-facing target for non-hub theses (`None` for repo-wide targets —
/// the section header already says repo-wide).
fn intent_target_display(entry: &OverlayEntry) -> Option<&str> {
    match &entry.target {
        OverlayTarget::Repo => None,
        OverlayTarget::Path { path } => Some(path),
        OverlayTarget::Symbol {
            qualified_symbol, ..
        } => Some(qualified_symbol),
    }
}

/// Opaque contract ref → sed-readable card ref: `session:<id>#<span>` becomes
/// `session <short-id> §<span>`; `chunk:<id>` and other opaque refs stay
/// verbatim. Never an absolute path (bucket-leak rule).
fn intent_ref(entry: &OverlayEntry) -> String {
    match entry.refs.first() {
        Some(reference) => {
            let raw = reference.store_ref.as_str();
            if let Some(rest) = raw.strip_prefix("session:")
                && let Some((id, span)) = rest.split_once('#')
            {
                let short: String = id.chars().take(8).collect();
                return format!("session {short} §{span}");
            }
            raw.to_string()
        }
        None => "no-ref".to_string(),
    }
}

/// One thesis = ONE line: `lifecycle[evidence] · date · authority · thesis ·
/// ref`. Lifecycle and evidence are separate axes; a contract-violating
/// `current + refuted` entry is demoted to ✗ so `✓[R]` can never render.
fn intent_thesis_line(entry: &OverlayEntry, hub_pinned: bool) -> String {
    let mark = match entry.status {
        OverlayLifecycle::Current
            if matches!(entry.verification_status, OverlayVerification::Refuted) =>
        {
            '✗'
        }
        OverlayLifecycle::Current => '✓',
        OverlayLifecycle::Superseded => '⊘',
        OverlayLifecycle::Disputed => '✗',
    };
    let evidence = match entry.verification_status {
        OverlayVerification::Verified => 'V',
        OverlayVerification::Unverified => 'U',
        OverlayVerification::Refuted => 'R',
    };
    let date = entry.valid_from.get(..10).unwrap_or(&entry.valid_from);
    let mut thesis = one_line_thesis(&entry.thesis);
    if !hub_pinned && let Some(target) = intent_target_display(entry) {
        thesis = format!("{thesis} → `{target}`");
    }
    format!(
        "  {mark}[{evidence}] {date} · {} · {thesis} · {}",
        intent_authority_name(entry.authority),
        intent_ref(entry),
    )
}

fn render_intent_card(pack: &ContextPack, source: &IntentCardSource) -> CardBody {
    let mut md = card_frame_header(
        pack,
        "Intent Map",
        "03-intent-map.md",
        "This card pins recorded decisions and intents (aicx overlay v1) to the structure they shaped.",
    );

    match &source.doc {
        Some(doc) => md.push_str(&format!(
            "Source: aicx overlay v1 · store_revision {} · overlay_revision {} · producer {}\n",
            short_revision(&doc.store_revision),
            short_revision(&doc.overlay_revision),
            doc.producer_version,
        )),
        None => md.push_str("Source: aicx overlay v1 · no overlay document (cold store)\n"),
    }
    md.push_str(
        "Evidence drill-down: `aicx read <ref>` · Markers: ✓ current · ⊘ superseded · ✗ anti-recommendation || evidence: [V] verified · [U] unverified · [R] refuted\n",
    );
    if let Some(stale) = &source.stale {
        md.push_str(&format!("\n{stale}\n"));
    }

    let entries: &[OverlayEntry] = source
        .doc
        .as_ref()
        .map(|doc| doc.entries.as_slice())
        .unwrap_or(&[]);
    let corpus = entries.len();

    let hub_files: BTreeSet<&str> = pack
        .risk
        .high_fan_in
        .iter()
        .map(|hub| hub.file.as_str())
        .collect();
    let threshold = pack
        .risk
        .high_fan_in
        .first()
        .map(|hub| hub.threshold)
        .unwrap_or(10);

    let mut per_hub: BTreeMap<&str, Vec<&OverlayEntry>> = BTreeMap::new();
    let mut repo_wide: Vec<&OverlayEntry> = Vec::new();
    let mut anti: Vec<&OverlayEntry> = Vec::new();
    let mut superseded: Vec<&OverlayEntry> = Vec::new();
    for entry in entries {
        match entry.status {
            OverlayLifecycle::Superseded => superseded.push(entry),
            OverlayLifecycle::Disputed => anti.push(entry),
            OverlayLifecycle::Current => {
                if matches!(entry.verification_status, OverlayVerification::Refuted) {
                    // refuted ⊥ current (contract) — demote, never ✓[R].
                    anti.push(entry);
                } else {
                    match intent_target_path(entry) {
                        Some(path) if hub_files.contains(path) => {
                            per_hub.entry(path).or_default().push(entry)
                        }
                        _ => repo_wide.push(entry),
                    }
                }
            }
        }
    }

    // Rendered theses in card order — the receipt and the payload's
    // `rendered_theses` derive from this list, so receipt ↔ payload ↔ card
    // stay one set by construction.
    let mut rendered: Vec<(&'static str, &OverlayEntry)> = Vec::new();

    md.push_str(&format!(
        "\n## Per-hub — formative decisions (fan-in ≥ {threshold})\n\n"
    ));
    if per_hub.is_empty() {
        md.push_str(&format!(
            "no registered per-hub decisions — corpus: {corpus} overlay entries\n"
        ));
    } else {
        for (hub, hub_entries) in &per_hub {
            md.push_str(&format!("{hub}\n"));
            for entry in hub_entries {
                md.push_str(&intent_thesis_line(entry, true));
                md.push('\n');
                rendered.push(("per_hub", entry));
            }
        }
    }

    md.push_str("\n## Repo-wide\n\n");
    if repo_wide.is_empty() {
        md.push_str(&format!("no repo-wide entries — corpus: {corpus}\n"));
    } else {
        for entry in &repo_wide {
            md.push_str(&intent_thesis_line(entry, false));
            md.push('\n');
            rendered.push(("repo_wide", entry));
        }
    }

    md.push_str("\n## Anti-recommendations (AicxFailure)\n\n");
    if anti.is_empty() {
        md.push_str(&format!("no anti-recommendations — corpus: {corpus}\n"));
    } else {
        for entry in &anti {
            md.push_str(&intent_thesis_line(entry, false));
            md.push('\n');
            rendered.push(("anti", entry));
        }
    }

    md.push_str("\n## Superseded (history — 1 line/entry)\n\n");
    if superseded.is_empty() {
        md.push_str(&format!("no superseded entries — corpus: {corpus}\n"));
    } else {
        for entry in &superseded {
            md.push_str(&intent_thesis_line(entry, false));
            md.push('\n');
            rendered.push(("superseded", entry));
        }
    }

    md.push_str(&card_frame_footer(
        "This Intent Map does not replace repo-verified facts and carries only above-threshold attributions (unresolved candidates live in the payload only). Re-check structural/runtime cards before editing.",
    ));

    let receipt: Vec<FactId> = rendered
        .iter()
        .map(|(_, entry)| format!("thesis:{}", entry.intent_id))
        .collect();
    let rendered_theses: Vec<serde_json::Value> = rendered
        .iter()
        .map(|(section, entry)| json!({ "intent_id": entry.intent_id, "section": section }))
        .collect();

    finish_card(
        md,
        json!({
            "overlay": source.raw,
            "stale": source.stale,
            "rendered_theses": rendered_theses,
            "memory_slice": &pack.memory,
        }),
        receipt,
    )
}

fn render_verification_card(pack: &ContextPack) -> CardBody {
    let action = &pack.action;
    let mut md = card_frame_header(
        pack,
        "Verification Gates",
        "04-verification-gates.md",
        "This card lists verification gates and likely tests derived from the current context.",
    );

    md.push_str("\n## Gates\n\n");
    if action.verification_gates.is_empty() {
        md.push_str(&empty_section_line("gates in pack", 0));
    } else {
        let gates: BTreeSet<&str> = action
            .verification_gates
            .iter()
            .map(String::as_str)
            .collect();
        for gate in gates {
            md.push_str(&format!("gate:{gate}\n"));
        }
    }

    md.push_str("\n## Likely Tests\n\n");
    if action.likely_tests.is_empty() {
        md.push_str(&empty_section_line("likely tests in pack", 0));
    } else {
        let tests: BTreeSet<&str> = action.likely_tests.iter().map(String::as_str).collect();
        for test in tests {
            md.push_str(&format!("test:{test}\n"));
        }
    }

    md.push_str("\n## Downstream Checks\n\n");
    md.push_str(&empty_section_line("downstream checks data in pack", 0));
    md.push_str(
        "snapshot risk (stale/dirty) before release → 05-risk-register.md (freshness domain owner)\n",
    );

    md.push_str(&card_frame_footer(
        "This Verification Gates card does not prove correctness by itself. Run the commands before release or submit.",
    ));
    finish_card(
        md,
        json!({
            "next_safe_commands": &action.next_safe_commands,
            "verification_gates": &action.verification_gates,
            "likely_tests": &action.likely_tests,
            "risk": &pack.risk,
        }),
        verification_coverage_receipt(action),
    )
}

fn render_risk_card(pack: &ContextPack) -> CardBody {
    let risk = &pack.risk;
    let mut md = card_frame_header(
        pack,
        "Risk Register",
        "05-risk-register.md",
        "This card collects risk signals and recommended actions for the current context scope.",
    );

    md.push_str(
        "\nhotspots + hubs (fan-in) → 01-structural-map.md §Hubs (hotspots and hubs domain owner)\nauthority (per-label counters) → 01-structural-map.md §Authority (authority domain owner)\n",
    );

    md.push_str("\n## Cache & Snapshot Health\n\n");
    md.push_str(&format!(
        "snapshot_health: {}\n",
        risk.snapshot_health.as_deref().unwrap_or("unknown")
    ));
    md.push_str(&format!(
        "cache_scope: {}\n",
        one_line_json(&risk.cache_scope)
    ));
    md.push_str(&format!(
        "stale_snapshot: {} · dirty_worktree: {}\n",
        risk.stale_snapshot, risk.dirty_worktree
    ));

    md.push_str("\n## Stale Assumptions\n\n");
    md.push_str(&empty_section_line(
        "registered stale assumptions in pack",
        0,
    ));

    md.push_str("\n## Actions\n\n");
    // Command guidance is owned by karta 00 (Safe Next Commands) — a copy of
    // power_path here would duplicate the domain; 05 keeps the pointer plus
    // an honest corpus count.
    md.push_str(&format!(
        "executable actions (safe next + power-path, {} items) → 00-core-map.md (safe next commands domain owner)\n",
        pack.action.next_safe_commands.len() + pack.action.power_path.len()
    ));

    md.push_str(&card_frame_footer(
        "This Risk Register does not include full source content. Use `loct slice`/`loct impact` for exact file-level surgery.",
    ));
    // Receipt is empty since L1-02: the hotspots domain (with its fact ids)
    // moved to karta 01 per manifest `domain_owners`. The machine payload
    // still carries the risk slice — cross-card payload duplication is legal;
    // markdown-surface and receipt duplication is not.
    finish_card(
        md,
        json!({ "risk": risk, "action": &pack.action, "authority": &pack.authority }),
        Vec::new(),
    )
}

/// Hard cap on lines for each Atlas card body (JSON payload only — frame
/// lines like the header / lead / footer are added on top). Mirrors the
/// 2026-05-21 operator decision that no single card should pass 1000 lines
/// and surface the truncation honestly when it does.
///
/// loctree-fail hak 2026-05-23 #3: the Memory Trail card was emitting
/// 740 lines / 35 KB out of a ~50 KB atlas (87 % of total bytes). Without
/// a per-card cap, one fat slice silently pushed the other five cards
/// off the operator's screen budget. The cap below truncates the JSON
/// payload to `ATLAS_CARD_JSON_LINE_CAP` lines and appends a clear
/// `// truncated: N more lines, run \`loct context --full --json\` for
/// raw data` marker so the operator never reads a clipped card as
/// canonical truth.
pub(crate) const ATLAS_CARD_JSON_LINE_CAP: usize = 1000;

fn render_json_card(
    pack: &ContextPack,
    filename: &str,
    title: &str,
    lead: &str,
    missing: &str,
    value: serde_json::Value,
    mut coverage_receipt: Vec<FactId>,
) -> CardBody {
    let fence_json = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
    let full_filename = full_json_filename(filename);
    let (json, dropped) = cap_json_payload(&fence_json, ATLAS_CARD_JSON_LINE_CAP, &full_filename);
    let markdown = format!(
        "# {}\n\nProject: `{}`\nSnapshot: `{}`\nFreshness: `{}`\n\n{}\n\n```json\n{}\n```\n\n## What this card does not cover\n\n{}\n",
        title,
        pack.project.canonical_root.as_deref().unwrap_or("unknown"),
        snapshot_label(pack),
        atlas_freshness_line(pack),
        lead,
        json,
        missing
    );
    // The receipt is a set: deterministic order, no duplicates.
    coverage_receipt.sort();
    coverage_receipt.dedup();
    let payload_hash = payload_hash_hex(&canonical_json_pretty(&value));
    CardBody {
        markdown,
        canonical_payload: value,
        payload_hash,
        coverage_receipt,
        fence_dropped_lines: dropped,
    }
}

/// Rebuild the value with object keys re-inserted in sorted order. Explicit
/// re-insertion keeps the serialization canonical under BOTH serde_json map
/// backends: the default BTreeMap sorts anyway, and `preserve_order`/IndexMap
/// replays insertion order — which is now sorted. Never hash a Display-form
/// `Value` whose key order is a feature-flag lottery.
fn canonicalize_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for key in keys {
                out.insert(key.clone(), canonicalize_value(&map[key]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_value).collect())
        }
        other => other.clone(),
    }
}

/// Canonical payload serialization: pretty JSON with sorted object keys,
/// newline-terminated. This exact byte sequence is what gets hashed and what
/// the `.full.json` sibling contains, so `sha256(<stem>.full.json)` equals the
/// manifest `payload_hash`.
fn canonical_json_pretty(value: &serde_json::Value) -> String {
    let mut out = serde_json::to_string_pretty(&canonicalize_value(value))
        .unwrap_or_else(|_| "{}".to_string());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn payload_hash_hex(canonical: &str) -> String {
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A payload with no content (null / empty object / empty array) earns no
/// `.full.json` sibling; every real card payload is a keyed object.
fn payload_is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Object(map) => map.is_empty(),
        serde_json::Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

/// FactSet of karta 01 — the owner of the hubs/edges + hotspots + authority +
/// reachability domains (manifest `domain_owners`, L1-02): one
/// `edge:<target>:<importer>` per resolved import edge, `hub:<file>` per
/// high-fan-in file, `hotspots:<file>` per hotspot, `authority:<Label>` per
/// label with count > 0, and `reachability:<file>` per unreached surface.
/// The machine payloads of cards 02/05 still carry the underlying slices —
/// payload duplication is legal, receipt/markdown duplication is not.
fn structural_coverage_receipt(pack: &ContextPack) -> Vec<FactId> {
    let mut facts: Vec<FactId> = pack
        .structural
        .imports
        .iter()
        .filter_map(|import| {
            import
                .resolved_path
                .as_deref()
                .map(|target| format!("edge:{}:{}", target, import.file))
        })
        .collect();
    for hub in &pack.risk.high_fan_in {
        facts.push(format!("hub:{}", hub.file));
    }
    for hotspot in &pack.risk.hotspots {
        facts.push(format!("hotspots:{}", hotspot.file));
    }
    for (name, count) in authority_counter_pairs(&pack.authority) {
        if count > 0 {
            facts.push(format!("authority:{name}"));
        }
    }
    for claim in &pack.runtime.reachability {
        if !claim.reached
            && let Some((file, _)) = claim.symbol.split_once("::")
        {
            facts.push(format!("reachability:{file}"));
        }
    }
    facts
}

/// FactSet of the runtime payload: dispatch edges, env contracts per consumer
/// file, and entrypoint hints (karta 02 grammar). Unreached surfaces moved to
/// karta 01 with the reachability domain (L1-02).
fn runtime_coverage_receipt(runtime: &RuntimeSlice) -> Vec<FactId> {
    let mut facts = Vec::new();
    for edge in &runtime.dispatch_edges {
        let target = edge
            .handler_file
            .as_deref()
            .unwrap_or(edge.handler_symbol.as_str());
        facts.push(format!("dispatch:{}:{}", edge.from_file, target));
    }
    for contract in &runtime.env_contracts {
        for file in &contract.used_in_files {
            facts.push(format!("env:{}:{}", contract.name, file));
        }
    }
    for hint in &runtime.framework_hints {
        if is_runtime_owner_hint(&hint.kind) {
            facts.push(format!("entry:{}", hint.file));
        }
    }
    facts
}

/// FactSet of the verification payload: `gate:<command>` per verification gate
/// and `test:<path>` per likely test (karta 04 grammar) — the payload now
/// carries `verification_gates`, so the `gate:` facts are in.
fn verification_coverage_receipt(action: &ActionSlice) -> Vec<FactId> {
    let mut facts: Vec<FactId> = action
        .verification_gates
        .iter()
        .map(|gate| format!("gate:{gate}"))
        .collect();
    facts.extend(
        action
            .likely_tests
            .iter()
            .map(|test| format!("test:{test}")),
    );
    facts
}

/// Truncate the JSON payload of an atlas card to `cap` lines, appending a
/// comment-marker tail that points at the concrete `full_filename` sibling
/// artifact when content was dropped. Returns the original payload unchanged
/// (and `None`) when it already fits; otherwise returns the capped payload and
/// `Some(dropped)` line count.
fn cap_json_payload(json: &str, cap: usize, full_filename: &str) -> (String, Option<usize>) {
    let line_total = json.lines().count();
    if line_total <= cap {
        return (json.to_string(), None);
    }
    let keep = cap.saturating_sub(1).max(1);
    let dropped = line_total - keep;
    let mut out = String::with_capacity(json.len());
    for (idx, line) in json.lines().enumerate() {
        if idx >= keep {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "// truncated: {dropped} more line(s); read the complete payload at `{full_filename}` (sibling of this card), or run `loct context --full --json` for the whole pack\n"
    ));
    (out, Some(dropped))
}

/// Map a card filename to its complete-payload sibling artifact:
/// `01-structural-map.md` -> `01-structural-map.full.json`.
fn full_json_filename(card_filename: &str) -> String {
    let stem = card_filename.strip_suffix(".md").unwrap_or(card_filename);
    format!("{stem}.full.json")
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_scope::{ScopeReport, TaskReport};
    use crate::pack::{
        AuthorityLabel, ProjectIdentity, RuntimeEnvContract, RuntimeEnvOccurrence,
        RuntimeInventoryCoverage, RuntimeReachability, StructuralFile, StructuralImport,
        StructuralRole,
    };
    use tempfile::TempDir;

    /// Test-side mirror of the verifier grammar (`tools/atlas_factset_check.py`):
    /// reconstruct base-fact ids from rendered card lines. Grammar frozen in
    /// `docs/contracts/atlas-card-format.md` (kanon v4).
    fn parse_card_facts(md: &str) -> BTreeSet<String> {
        let mut facts = BTreeSet::new();
        for raw in md.lines() {
            let line = raw.trim_end();
            if let Some(rest) = line.strip_prefix("entry:") {
                facts.insert(format!("entry:{}", fact_head(rest)));
            } else if let Some(rest) = line.strip_prefix("env:") {
                let parts: Vec<&str> = rest.split(" · ").collect();
                if parts.len() >= 2 {
                    facts.insert(format!("env:{}:{}", parts[0].trim(), parts[1].trim()));
                }
            } else if let Some(rest) = line.strip_prefix("dispatch:") {
                if let Some((from, target)) = fact_head(rest).split_once('→') {
                    facts.insert(format!("dispatch:{from}:{target}"));
                }
            } else if let Some(rest) = line.strip_prefix("reachability:") {
                facts.insert(format!("reachability:{}", fact_head(rest)));
            } else if let Some(rest) = line.strip_prefix("hotspots:") {
                facts.insert(format!("hotspots:{}", fact_head(rest)));
            } else if let Some(rest) = line.strip_prefix("authority:") {
                // One counter line carries one fact per label with count > 0
                // (`authority:RepoVerified 1 · LoctreeDerived 148 · ...`).
                for part in rest.split(" · ") {
                    let mut tokens = part.split_whitespace();
                    if let (Some(label), Some(count)) = (tokens.next(), tokens.next())
                        && count.parse::<usize>().is_ok_and(|n| n > 0)
                    {
                        facts.insert(format!("authority:{label}"));
                    }
                }
            } else if let Some(rest) = line.strip_prefix("gate:") {
                facts.insert(format!("gate:{}", fact_head(rest)));
            } else if let Some(rest) = line.strip_prefix("test:") {
                facts.insert(format!("test:{}", fact_head(rest)));
            } else if let Some((target, groups)) = line.split_once(" ← ") {
                for group in groups.split(" · ") {
                    if let Some((prefix, inner)) = group.split_once('{') {
                        let inner = inner.strip_suffix('}').unwrap_or(inner);
                        for entry in inner.split(',') {
                            facts.insert(format!("edge:{target}:{prefix}{entry}"));
                        }
                    } else {
                        facts.insert(format!("edge:{target}:{group}"));
                    }
                }
            } else if line.starts_with('|') {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                if cells.len() > 2 && cells[1].parse::<usize>().is_ok() {
                    facts.insert(format!("hub:{}", cells[2]));
                }
            }
        }
        facts
    }

    #[test]
    fn runtime_card_lists_inventory_classes_without_env_values() {
        let mut pack = ContextPack::empty(ProjectIdentity::default());
        pack.runtime.env_contracts.push(RuntimeEnvContract {
            name: "VISIBLE_NAME".to_string(),
            used_in_files: vec!["src/main.rs".to_string()],
            required_for: Vec::new(),
            occurrences: vec![RuntimeEnvOccurrence {
                file: "src/main.rs".to_string(),
                line: 7,
                access_kind: "std::env::var".to_string(),
                default: Some("must-not-render".to_string()),
                required: false,
            }],
            required: false,
            authority: AuthorityLabel::RepoVerified,
        });
        pack.runtime.inventory_coverage = Some(RuntimeInventoryCoverage {
            owner_classes: vec!["cargo_bin".to_string(), "swiftui_main".to_string()],
            env_read_classes: vec!["rust_std_env".to_string()],
            source_files_scanned: 1,
        });

        let card = render_runtime_card(&pack);
        assert!(
            card.markdown
                .contains("env:VISIBLE_NAME · src/main.rs · requiredness unproven")
        );
        assert!(
            card.markdown
                .contains("inventory env classes: rust_std_env")
        );
        assert!(
            card.markdown
                .contains("inventory owner classes: cargo_bin, swiftui_main")
        );
        assert!(!card.markdown.contains("must-not-render"));
        assert!(!card.markdown.contains("default="));
        assert!(
            !card
                .canonical_payload
                .to_string()
                .contains("must-not-render")
        );
    }

    fn fact_head(rest: &str) -> &str {
        rest.split(" · ").next().unwrap_or(rest).trim()
    }

    /// Per-card FactSets from the materialized `receipt.json`.
    fn receipt_fact_map(atlas_dir: &Path) -> BTreeMap<String, BTreeSet<String>> {
        let receipt: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(atlas_dir.join("receipt.json")).expect("receipt.json"),
        )
        .expect("receipt parses");
        receipt["coverage_receipts"]
            .as_array()
            .expect("coverage_receipts array")
            .iter()
            .map(|entry| {
                let path = entry["path"].as_str().expect("receipt path").to_string();
                let facts: BTreeSet<String> = entry["coverage_receipt"]
                    .as_array()
                    .expect("coverage_receipt array")
                    .iter()
                    .filter_map(|fact| fact.as_str().map(str::to_string))
                    .collect();
                (path, facts)
            })
            .collect()
    }

    fn fixture_import(
        file: &str,
        target: &str,
        symbols: Vec<String>,
        line: usize,
    ) -> StructuralImport {
        StructuralImport {
            file: file.to_string(),
            source: "crate::hub".to_string(),
            source_raw: "crate::hub".to_string(),
            kind: "static".to_string(),
            resolution: "local".to_string(),
            resolved_path: Some(target.to_string()),
            line: Some(line),
            symbols,
            is_bare: false,
            authority: AuthorityLabel::LoctreeDerived,
        }
    }

    /// Fixture pack that exercises every base-fact family the receipts carry:
    /// edges (incl. a grouped importer dir), hubs, dispatch, env, entrypoints,
    /// unreachable surfaces, hotspots, gates, and likely tests.
    fn rich_pack(root: &Path) -> ContextPack {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(root.display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: Some("scan-1".to_string()),
        });
        pack.structural.imports = vec![
            fixture_import("src/consumer.rs", "src/hub.rs", vec!["Hub".to_string()], 3),
            fixture_import("src/other/consumer_b.rs", "src/hub.rs", vec![], 4),
            fixture_import("src/other/consumer_c.rs", "src/hub.rs", vec![], 5),
        ];
        pack.runtime.dispatch_edges = vec![RuntimeDispatchEdge {
            from_file: "src/cli.rs".to_string(),
            from_line: 10,
            dispatch_kind: "case_statement".to_string(),
            handler_symbol: "handle_scan".to_string(),
            handler_file: Some("src/handlers.rs".to_string()),
            framework: None,
            http_method: None,
            http_path: None,
            authority: AuthorityLabel::LoctreeDerived,
        }];
        pack.runtime.env_contracts = vec![RuntimeEnvContract {
            name: "LOCT_HOME".to_string(),
            used_in_files: vec!["src/config.rs".to_string()],
            required_for: vec![],
            occurrences: vec![],
            required: false,
            authority: AuthorityLabel::LoctreeDerived,
        }];
        pack.runtime.framework_hints = vec![RuntimeFrameworkHint {
            kind: "entrypoint".to_string(),
            symbol: "main".to_string(),
            file: "src/main.rs".to_string(),
            line: Some(1),
            detail: None,
            authority: AuthorityLabel::SemanticGuess,
        }];
        pack.runtime.reachability = vec![
            RuntimeReachability {
                symbol: "src/main.rs::main".to_string(),
                reached: true,
                reason: "entrypoint".to_string(),
                authority: AuthorityLabel::SemanticGuess,
            },
            RuntimeReachability {
                symbol: "src/dead.rs::unused".to_string(),
                reached: false,
                reason: "no_path_from_entrypoint".to_string(),
                authority: AuthorityLabel::SemanticGuess,
            },
        ];
        pack.risk.hotspots = vec![HotspotFile {
            file: "src/snapshot.rs".to_string(),
            importers: 39,
            authority: AuthorityLabel::LoctreeDerived,
        }];
        pack.risk.high_fan_in = vec![HighFanInFile {
            file: "src/types.rs".to_string(),
            importers: 82,
            threshold: 10,
            authority: AuthorityLabel::LoctreeDerived,
        }];
        pack.action.verification_gates =
            vec!["cargo clippy --workspace --all-targets -- -D warnings".to_string()];
        pack.action.likely_tests = vec!["loctree-rs/tests/context_pack_contract.rs".to_string()];
        pack.authority.repo_verified = vec!["risk.cache_scope".to_string()];
        pack.authority.loctree_derived = vec![
            "structural.files".to_string(),
            "structural.imports".to_string(),
        ];
        pack
    }

    #[test]
    fn materializes_manifest_and_named_cards_with_line_counts() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(tmp.path().display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: Some("scan-1".to_string()),
        });
        pack.action.next_safe_commands = vec!["cargo test --workspace".to_string()];

        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");

        assert_eq!(manifest.protocol, CONTEXT_ATLAS_PROTOCOL);
        assert_eq!(manifest.cards.len(), 6);
        assert!(atlas_dir.join("manifest.md").exists());
        assert!(atlas_dir.join("manifest.json").exists());
        assert!(atlas_dir.join("00-core-map.md").exists());
        assert!(atlas_dir.join("05-risk-register.md").exists());

        for card in &manifest.cards {
            let content = fs::read_to_string(atlas_dir.join(&card.path)).expect("card content");
            assert_eq!(card.lines, content.lines().count());
            assert_eq!(card.bytes, content.len());
            assert!(content.contains("What this card does not cover"));
            assert!(
                !content.contains("```json"),
                "{}: dense cards must not carry a JSON fence",
                card.path
            );
            assert!(
                content.contains("Freshness: fresh - card matches the loaded snapshot authority.")
            );
            assert!(
                content.contains(&format!("Full payload: {}", full_json_filename(&card.path))),
                "{}: card must point at its canonical payload sibling",
                card.path
            );
            // An empty pack fits every card; nothing is capped...
            assert!(!card.truncated, "{} unexpectedly truncated", card.path);
            assert_eq!(card.dropped_lines, 0);
            // ...but the canonical payload sibling is written REGARDLESS of
            // truncation — the payload is a contract, not an overflow artifact.
            let sibling_name = full_json_filename(&card.path);
            assert_eq!(card.full_path.as_deref(), Some(sibling_name.as_str()));
            let sibling = fs::read_to_string(atlas_dir.join(&sibling_name))
                .expect("canonical payload sibling must exist for every card");
            assert_eq!(card.full_payload_lines, Some(sibling.lines().count()));
            assert_eq!(
                card.payload_hash.as_deref(),
                Some(payload_hash_hex(&sibling).as_str()),
                "{}: manifest hash must equal sha256 of the sibling bytes",
                card.path
            );
            assert_eq!(card.fact_count, 0, "empty pack carries no base facts");
        }

        let manifest_md = fs::read_to_string(atlas_dir.join("manifest.md")).expect("manifest md");
        assert!(manifest_md.contains("Recommended Reading Path"));
        assert!(manifest_md.contains("Saves you from"));
        // No card overflowed, so the manifest carries no partial-card section.
        assert!(!manifest_md.contains("## Partial cards"));
    }

    #[test]
    fn sequential_scopes_retain_independent_atlases_and_catalog_identities() {
        fn scoped_pack(
            root: &Path,
            selector: &str,
            fingerprint: &str,
            marker: &str,
        ) -> ContextPack {
            let mut pack = ContextPack::empty(ProjectIdentity {
                canonical_root: Some(root.display().to_string()),
                branch: Some("main".to_string()),
                commit: Some("abc1234".to_string()),
                snapshot_id: Some("scan-1".to_string()),
            });
            pack.scope = Some(ScopeReport {
                selectors: vec![selector.to_string()],
                matched_files: 1,
                empty: false,
                fingerprint: fingerprint.to_string(),
                named_resolved_from: None,
                resolved_selectors: Vec::new(),
                selector_match_counts: Vec::new(),
            });
            pack.task = Some(TaskReport {
                text: format!("retain {selector}"),
                mode: "rank_within_scope".to_string(),
                authority: "operator".to_string(),
            });
            pack.action.next_safe_commands = vec![marker.to_string()];
            pack
        }

        let tmp = TempDir::new().expect("temp dir");
        let mut project_pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(tmp.path().display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: Some("scan-1".to_string()),
        });
        project_pack.action.next_safe_commands = vec!["PROJECT_SCOPE_MARKER".to_string()];
        let project_manifest =
            materialize_context_atlas(&project_pack, tmp.path(), None).expect("project atlas");
        assert_eq!(project_manifest.identity.atlas_id, "project");

        let first = scoped_pack(
            tmp.path(),
            "path:src/first",
            "scope-first",
            "FIRST_SCOPE_MARKER",
        );
        let first_manifest =
            materialize_context_atlas(&first, tmp.path(), None).expect("first scope atlas");
        let first_id = first_manifest.identity.atlas_id.clone();

        let second = scoped_pack(
            tmp.path(),
            "path:src/second",
            "scope-second",
            "SECOND_SCOPE_MARKER",
        );
        let second_manifest =
            materialize_context_atlas(&second, tmp.path(), None).expect("second scope atlas");
        let second_id = second_manifest.identity.atlas_id.clone();
        assert_ne!(
            first_id, second_id,
            "distinct scopes need distinct identities"
        );

        let atlas_root = atlas_dir_for_project(tmp.path());
        let project_dir = atlas_root.join(CONTEXT_ATLAS_RUNS_DIR).join("project");
        let first_dir = atlas_root.join(CONTEXT_ATLAS_RUNS_DIR).join(&first_id);
        let second_dir = atlas_root.join(CONTEXT_ATLAS_RUNS_DIR).join(&second_id);
        let project_core = fs::read_to_string(project_dir.join("00-core-map.md"))
            .expect("project atlas remains addressable after scoped runs");
        let first_core = fs::read_to_string(first_dir.join("00-core-map.md"))
            .expect("first scope remains addressable");
        let second_core = fs::read_to_string(second_dir.join("00-core-map.md"))
            .expect("second scope remains addressable");
        assert!(project_core.contains("PROJECT_SCOPE_MARKER"));
        assert!(!project_core.contains("FIRST_SCOPE_MARKER"));
        assert!(!project_core.contains("SECOND_SCOPE_MARKER"));
        assert!(first_core.contains("FIRST_SCOPE_MARKER"));
        assert!(!first_core.contains("SECOND_SCOPE_MARKER"));
        assert!(second_core.contains("SECOND_SCOPE_MARKER"));
        assert!(!second_core.contains("FIRST_SCOPE_MARKER"));

        let catalog: ContextAtlasManifest = serde_json::from_str(
            &fs::read_to_string(atlas_root.join("manifest.json")).expect("root catalog"),
        )
        .expect("root catalog parses");
        let retained: BTreeSet<&str> = catalog
            .atlases
            .iter()
            .map(|atlas| atlas.identity.atlas_id.as_str())
            .collect();
        assert_eq!(
            retained,
            BTreeSet::from(["project", first_id.as_str(), second_id.as_str()])
        );
        assert!(
            fs::read_to_string(atlas_root.join("manifest.md"))
                .expect("manifest markdown")
                .contains("## Persisted atlas identities")
        );
    }

    /// loctree-fail hak 2026-05-23 #3 regression: a fat JSON card body
    /// must be truncated to the per-card line cap and append a clearly
    /// labelled tail so the operator never reads a clipped card as
    /// canonical truth.
    #[test]
    fn cap_json_payload_truncates_with_marker_pointing_at_sibling() {
        let big: String = (0..1500)
            .map(|i| format!("  \"row_{i}\": {i},"))
            .collect::<Vec<_>>()
            .join("\n");
        let (capped, dropped) = cap_json_payload(&big, 1000, "01-structural-map.full.json");
        let line_total = capped.lines().count();
        assert!(
            line_total <= 1000,
            "capped payload must fit cap, got {line_total} lines"
        );
        assert!(
            dropped.is_some_and(|d| d > 0),
            "over-cap input must report dropped line count"
        );
        let last = capped.lines().last().unwrap_or("");
        assert!(
            last.starts_with("// truncated:") && last.contains("more line(s)"),
            "tail marker must explain truncation explicitly, got: {last}"
        );
        assert!(
            last.contains("01-structural-map.full.json"),
            "marker must point at the concrete sibling artifact, got: {last}"
        );
    }

    #[test]
    fn cap_json_payload_keeps_small_input_unchanged() {
        let small = "{\n  \"a\": 1\n}".to_string();
        let (out, dropped) = cap_json_payload(&small, 1000, "00-core-map.full.json");
        assert_eq!(out, small);
        assert!(dropped.is_none(), "a fitting payload reports no truncation");
    }

    #[test]
    fn full_json_filename_swaps_md_for_full_json() {
        assert_eq!(
            full_json_filename("01-structural-map.md"),
            "01-structural-map.full.json"
        );
        assert_eq!(
            full_json_filename("00-core-map.md"),
            "00-core-map.full.json"
        );
    }

    /// loctree-fail tail 2026-06-22 regression: an over-cap card body must keep
    /// the complete payload around (for the sibling artifact) and embed an
    /// on-card marker pointing at that concrete file.
    #[test]
    fn render_json_card_overflow_carries_full_payload_and_marker() {
        let pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        let rows: Vec<serde_json::Value> = (0..2000).map(|i| json!({ "row": i })).collect();
        let body = render_json_card(
            &pack,
            "01-structural-map.md",
            "Structural Map",
            "lead",
            "missing",
            json!(rows),
            Vec::new(),
        );
        assert!(
            body.fence_dropped_lines.is_some_and(|dropped| dropped > 0),
            "a 2000-entry payload must overflow the per-card cap"
        );
        assert!(
            body.markdown.contains("01-structural-map.full.json"),
            "card body must point readers at the complete sibling artifact"
        );
        assert!(body.markdown.contains("// truncated:"));
        // The canonical payload is uncapped — the last row survives.
        let canonical = canonical_json_pretty(&body.canonical_payload);
        assert!(
            canonical.contains("\"row\": 1999"),
            "canonical payload must hold the complete, uncapped payload"
        );
        assert!(
            canonical.lines().count() > ATLAS_CARD_JSON_LINE_CAP,
            "complete payload should exceed the per-card cap"
        );
        assert_eq!(
            body.payload_hash,
            payload_hash_hex(&canonical),
            "payload_hash must be the sha256 of the canonical serialization"
        );
    }

    #[test]
    fn render_json_card_surfaces_stale_snapshot_in_header() {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.risk.stale_snapshot = true;

        let body = render_json_card(
            &pack,
            "00-core-map.md",
            "Core Map",
            "lead",
            "missing",
            json!({}),
            Vec::new(),
        );

        assert!(
            body.markdown
                .contains("Freshness: `STALE - card snapshot main@abc1234"),
            "stale atlas cards must carry a loud header flag: {}",
            body.markdown
        );
        assert!(
            body.markdown
                .contains("refresh with `loct context --full` before relying on this card")
        );
    }

    #[test]
    fn render_json_card_surfaces_missing_snapshot_in_header() {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.risk.cache_scope = RiskCacheScope::MissingSnapshot;
        pack.risk.cache_scope_authority = AuthorityLabel::RepoVerified;
        pack.risk.snapshot_health = Some("missing_snapshot".to_string());

        let body = render_json_card(
            &pack,
            "00-core-map.md",
            "Core Map",
            "lead",
            "missing",
            json!({}),
            Vec::new(),
        );

        assert!(
            body.markdown
                .contains("Freshness: `NO SNAPSHOT - run loct scan"),
            "missing atlas cards must carry a no-snapshot header: {}",
            body.markdown
        );
        assert!(
            !body
                .markdown
                .contains("Freshness: `fresh - card matches the loaded snapshot authority.`"),
            "missing snapshot must not render as fresh: {}",
            body.markdown
        );
    }

    /// loctree-fail tail 2026-06-22 regression: the manifest must explicitly
    /// flag partial cards and name the concrete complete artifact, instead of
    /// silently quoting a line count for a clipped card.
    #[test]
    fn manifest_marks_partial_cards_and_points_to_full_artifact() {
        let manifest = ContextAtlasManifest {
            protocol: CONTEXT_ATLAS_PROTOCOL.to_string(),
            status: "atlas_ready".to_string(),
            project: "proj".to_string(),
            snapshot: "main@abc1234".to_string(),
            generated_at: "2026-06-22T00:00:00Z".to_string(),
            atlas_dir: "/tmp/proj/.loctree/context-atlas".to_string(),
            manifest: "manifest.md".to_string(),
            manifest_json: "manifest.json".to_string(),
            recommended_start: "00-core-map.md".to_string(),
            identity: ContextAtlasIdentity::default(),
            atlases: Vec::new(),
            domain_owners: atlas_domain_owners(),
            cards: vec![ContextAtlasCard {
                id: "structural".to_string(),
                title: "Structural Map".to_string(),
                path: "01-structural-map.md".to_string(),
                lines: 1014,
                bytes: 40_000,
                why: "why".to_string(),
                saves_you_from: "saves".to_string(),
                truncated: true,
                dropped_lines: 2589,
                full_path: Some("01-structural-map.full.json".to_string()),
                full_payload_lines: Some(3602),
                payload_hash: Some("deadbeef".to_string()),
                fact_count: 42,
                status: None,
            }],
            message: "msg".to_string(),
        };
        let md = render_manifest(&manifest);
        assert!(
            md.contains("## Partial cards"),
            "manifest must call out partial cards"
        );
        assert!(
            md.contains("01-structural-map.full.json"),
            "manifest must point to the concrete complete artifact"
        );
        assert!(
            md.contains("1014 materialized lines / 3602 full-payload lines"),
            "manifest must quote materialized and full-payload lengths"
        );
        assert!(
            md.contains("Partial-card completeness is stricter"),
            "completeness footer must make partial cards part of read-to-end"
        );
        assert!(
            md.contains("2589"),
            "manifest should quote the dropped-line magnitude"
        );
    }

    #[test]
    fn cli_summary_reports_partial_cards_with_materialized_and_full_payload_lines() {
        let manifest = ContextAtlasManifest {
            protocol: CONTEXT_ATLAS_PROTOCOL.to_string(),
            status: "atlas_ready".to_string(),
            project: "proj".to_string(),
            snapshot: "main@abc1234".to_string(),
            generated_at: "2026-07-01T00:00:00Z".to_string(),
            atlas_dir: "/tmp/proj/.loctree/context-atlas".to_string(),
            manifest: "manifest.md".to_string(),
            manifest_json: "manifest.json".to_string(),
            recommended_start: "00-core-map.md".to_string(),
            identity: ContextAtlasIdentity::default(),
            atlases: Vec::new(),
            domain_owners: atlas_domain_owners(),
            cards: vec![ContextAtlasCard {
                id: "structural".to_string(),
                title: "Structural Map".to_string(),
                path: "01-structural-map.md".to_string(),
                lines: 1008,
                bytes: 40_000,
                why: "why".to_string(),
                saves_you_from: "saves".to_string(),
                truncated: true,
                dropped_lines: 3870,
                full_path: Some("01-structural-map.full.json".to_string()),
                full_payload_lines: Some(4869),
                payload_hash: Some("deadbeef".to_string()),
                fact_count: 42,
                status: None,
            }],
            message: "msg".to_string(),
        };

        let summary = manifest.render_cli_summary();
        assert!(
            summary.contains("1008 materialized lines / 4869 full-payload lines"),
            "CLI summary must not blur materialized card length with full payload: {summary}"
        );
        assert!(
            summary.contains("read complete payload at 01-structural-map.full.json"),
            "CLI summary must send readers to the concrete sibling artifact: {summary}"
        );
    }

    /// Kanon v4 flip of the old partial-card regression: a dense card is
    /// decision-complete BY CONSTRUCTION. An oversized structural payload must
    /// NOT truncate — every edge fact stays inline (grouped form, full set)
    /// and the manifest still records exact full-payload truth.
    #[test]
    fn materialized_atlas_records_partial_card_full_payload_truth() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(tmp.path().display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: Some("scan-1".to_string()),
        });
        pack.structural.files = (0..1200)
            .map(|idx| StructuralFile {
                path: format!("src/dir_{}/generated_{idx}.rs", idx % 40),
                role: StructuralRole::Dependency,
                depth: 1,
                language: "rs".to_string(),
                loc: idx,
                authority: AuthorityLabel::RepoVerified,
            })
            .collect();
        pack.structural.imports = (0..1200)
            .map(|idx| {
                fixture_import(
                    &format!("src/dir_{}/generated_{idx}.rs", idx % 40),
                    &format!("src/hub_{}.rs", idx % 7),
                    vec![],
                    idx + 1,
                )
            })
            .collect();

        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");
        let card = manifest
            .cards
            .iter()
            .find(|card| card.path == "01-structural-map.md")
            .expect("structural card");
        assert!(
            !card.truncated,
            "dense cards are decision-complete by construction — never partial"
        );
        assert_eq!(
            card.fact_count, 1200,
            "every edge fact stays in the receipt"
        );

        let card_content =
            fs::read_to_string(atlas_dir.join(&card.path)).expect("structural card content");
        assert_eq!(
            card.lines,
            card_content.lines().count(),
            "manifest must quote the materialized .md card length"
        );
        assert!(
            !card_content.contains("// truncated:"),
            "dense cards must not carry a truncation marker"
        );
        let parsed = parse_card_facts(&card_content);
        let receipts = receipt_fact_map(&atlas_dir);
        assert_eq!(
            parsed, receipts["01-structural-map.md"],
            "all 1200 edge facts must be reconstructable from the markdown"
        );

        let full_path = card.full_path.as_deref().expect("full sibling path");
        let full_payload =
            fs::read_to_string(atlas_dir.join(full_path)).expect("full sibling payload");
        assert!(
            full_payload.ends_with('\n'),
            "full sibling must be newline-terminated so `wc -l` matches manifest truth"
        );
        assert_eq!(
            card.full_payload_lines,
            Some(full_payload.lines().count()),
            "manifest JSON must quote the full sibling payload length"
        );
        assert_eq!(
            card.payload_hash.as_deref(),
            Some(payload_hash_hex(&full_payload).as_str()),
            "manifest hash must equal sha256 of the sibling bytes"
        );

        let manifest_md = fs::read_to_string(atlas_dir.join("manifest.md")).expect("manifest md");
        assert!(
            !manifest_md.contains("## Partial cards"),
            "no dense card may surface as partial: {manifest_md}"
        );
    }

    /// L1-00: the canonical serialization must not depend on object key
    /// insertion order — otherwise "stable hash between regenerations" is a
    /// serde_json feature-flag lottery (`preserve_order` vs BTreeMap backend).
    #[test]
    fn canonical_serialization_is_key_order_independent() {
        let mut forward = serde_json::Map::new();
        forward.insert("alpha".to_string(), json!({ "z": 1, "a": [2, 3] }));
        forward.insert("beta".to_string(), json!(true));
        let mut reverse = serde_json::Map::new();
        reverse.insert("beta".to_string(), json!(true));
        reverse.insert("alpha".to_string(), json!({ "a": [2, 3], "z": 1 }));

        let canonical_forward = canonical_json_pretty(&serde_json::Value::Object(forward));
        let canonical_reverse = canonical_json_pretty(&serde_json::Value::Object(reverse));
        assert_eq!(
            canonical_forward, canonical_reverse,
            "sorted-key serialization must erase insertion order"
        );
        assert_eq!(
            payload_hash_hex(&canonical_forward),
            payload_hash_hex(&canonical_reverse)
        );
        assert!(
            canonical_forward.ends_with('\n'),
            "canonical serialization is newline-terminated (hash == file bytes)"
        );
        let alpha_pos = canonical_forward.find("\"alpha\"").expect("alpha key");
        let beta_pos = canonical_forward.find("\"beta\"").expect("beta key");
        assert!(alpha_pos < beta_pos, "keys must serialize sorted");
    }

    /// L1-00 acceptance: two materializations of the SAME pack must produce
    /// identical payload hashes for every card — the hash is a regeneration
    /// contract, not a run fingerprint.
    ///
    /// `#[serial]` + env guard: parallel cache-isolation tests toggle
    /// `LOCT_CACHE_DIR`; a flip between the two regenerations would surface
    /// as a payload-hash mismatch that has nothing to do with the contract.
    #[test]
    #[serial_test::serial]
    fn payload_hash_is_stable_across_regenerations() {
        let (_cache_dir, _cache_env) = crate::snapshot::test_env::isolated_cache();
        let tmp = TempDir::new().expect("temp dir");
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(tmp.path().display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: Some("scan-1".to_string()),
        });
        pack.action.next_safe_commands = vec!["cargo test --workspace".to_string()];

        let first = materialize_context_atlas(&pack, tmp.path(), Some(&tmp.path().join("run1")))
            .expect("first materialization");
        let second = materialize_context_atlas(&pack, tmp.path(), Some(&tmp.path().join("run2")))
            .expect("second materialization");

        for (a, b) in first.cards.iter().zip(second.cards.iter()) {
            assert_eq!(a.path, b.path);
            assert!(a.payload_hash.is_some(), "{}: hash must be present", a.path);
            assert_eq!(
                a.payload_hash, b.payload_hash,
                "{}: payload hash must be identical across regenerations",
                a.path
            );
        }
    }

    /// L1-00/L1-01/L1-02 acceptance: cards carry a filled coverage receipt
    /// whose fact ids follow the frozen `kind:path[:detail]` grammar, with
    /// domain ownership per manifest `domain_owners` — hubs + hotspots +
    /// authority + reachability on karta 01, gates on karta 04, karta 05
    /// receipt empty (reference-only card).
    #[test]
    fn coverage_receipts_cover_structural_runtime_and_risk_facts() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());

        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");
        let fact_count_of = |id: &str| {
            manifest
                .cards
                .iter()
                .find(|card| card.id == id)
                .map(|card| card.fact_count)
                .unwrap_or_else(|| panic!("card {id} missing"))
        };
        assert_eq!(fact_count_of("core"), 0, "core is a projection card");
        assert_eq!(
            fact_count_of("structural"),
            8,
            "3 edges + 1 hub + 1 hotspot + 2 authority labels + 1 unreached"
        );
        assert_eq!(fact_count_of("runtime"), 3, "dispatch + env + entry");
        assert_eq!(
            fact_count_of("intent"),
            0,
            "intent card carries no theses without an overlay cache"
        );
        assert_eq!(fact_count_of("verification"), 2, "1 gate + 1 likely test");
        assert_eq!(
            fact_count_of("risk"),
            0,
            "hotspots domain moved to karta 01 (L1-02) — risk receipt is empty"
        );

        let receipts = receipt_fact_map(&atlas_dir);
        let facts_of = |path: &str| -> Vec<String> {
            receipts
                .get(path)
                .unwrap_or_else(|| panic!("receipt entry for {path} missing"))
                .iter()
                .cloned()
                .collect()
        };
        assert_eq!(
            facts_of("01-structural-map.md"),
            vec![
                "authority:LoctreeDerived",
                "authority:RepoVerified",
                "edge:src/hub.rs:src/consumer.rs",
                "edge:src/hub.rs:src/other/consumer_b.rs",
                "edge:src/hub.rs:src/other/consumer_c.rs",
                "hotspots:src/snapshot.rs",
                "hub:src/types.rs",
                "reachability:src/dead.rs"
            ]
        );
        assert_eq!(
            facts_of("02-runtime-map.md"),
            vec![
                "dispatch:src/cli.rs:src/handlers.rs",
                "entry:src/main.rs",
                "env:LOCT_HOME:src/config.rs"
            ]
        );
        assert_eq!(
            facts_of("04-verification-gates.md"),
            vec![
                "gate:cargo clippy --workspace --all-targets -- -D warnings",
                "test:loctree-rs/tests/context_pack_contract.rs"
            ]
        );
        assert_eq!(facts_of("05-risk-register.md"), Vec::<String>::new());
    }

    /// L1-02 acceptance: the manifest carries the `domain_owners` map — each
    /// domain has exactly one owner card, `intent → 03` is forward-declared
    /// for M1-01, and the map survives the JSON manifest (verifier surface).
    #[test]
    fn manifest_domain_owners_declare_single_owner_per_domain() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());
        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");

        let owner_of = |domain: &str| {
            manifest
                .domain_owners
                .get(domain)
                .map(String::as_str)
                .unwrap_or_else(|| panic!("domain {domain} missing from domain_owners"))
        };
        assert_eq!(owner_of("hotspots"), "01-structural-map");
        assert_eq!(owner_of("authority"), "01-structural-map");
        assert_eq!(owner_of("reachability"), "01-structural-map");
        assert_eq!(owner_of("hubs"), "01-structural-map");
        assert_eq!(owner_of("intent"), "03-intent-map");
        assert_eq!(owner_of("safe_next_commands"), "00-core-map");
        assert_eq!(owner_of("freshness"), "05-risk-register");
        assert_eq!(owner_of("gates"), "04-verification-gates");
        assert_eq!(owner_of("dispatch"), "02-runtime-map");

        // Every owner is one of the six materialized cards.
        let stems: BTreeSet<String> = manifest
            .cards
            .iter()
            .map(|card| card.path.trim_end_matches(".md").to_string())
            .collect();
        for (domain, owner) in &manifest.domain_owners {
            assert!(stems.contains(owner), "{domain}: unknown owner {owner}");
        }

        let raw = fs::read_to_string(atlas_dir.join("manifest.json")).expect("manifest.json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("manifest parses");
        assert_eq!(
            value["domain_owners"]["intent"].as_str(),
            Some("03-intent-map"),
            "domain_owners must survive manifest.json serialization"
        );

        let manifest_md = fs::read_to_string(atlas_dir.join("manifest.md")).expect("manifest md");
        assert!(
            manifest_md.contains("## Domain owners"),
            "manifest.md must project the ownership map"
        );
    }

    /// L1-02 acceptance: cross-card domains render on exactly ONE card (the
    /// owner), other cards carry a one-line reference — and the domain-prefixed
    /// fact ids appear only in the owner card's receipt (non-vacuously).
    #[test]
    fn cross_card_domains_render_on_exactly_one_card() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());
        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");

        let contents: Vec<(String, String)> = manifest
            .cards
            .iter()
            .map(|card| {
                let text = fs::read_to_string(atlas_dir.join(&card.path)).expect("card content");
                (card.path.clone(), text.to_lowercase())
            })
            .collect();
        for marker in ["## hubs", "## authority", "## reachability"] {
            let holders: Vec<&str> = contents
                .iter()
                .filter(|(_, text)| text.contains(marker))
                .map(|(path, _)| path.as_str())
                .collect();
            assert_eq!(
                holders,
                vec!["01-structural-map.md"],
                "{marker}: section header must live on exactly one card"
            );
        }

        let receipts = receipt_fact_map(&atlas_dir);
        for domain in ["hotspots", "authority", "reachability"] {
            let prefix = format!("{domain}:");
            for (path, facts) in &receipts {
                let carried: Vec<&String> = facts
                    .iter()
                    .filter(|fact| fact.starts_with(&prefix))
                    .collect();
                if path == "01-structural-map.md" {
                    assert!(
                        !carried.is_empty(),
                        "{domain}: owner receipt must carry domain facts (ownership must not be vacuous)"
                    );
                } else {
                    assert!(
                        carried.is_empty(),
                        "{domain}: fact ids leaked into {path}: {carried:?}"
                    );
                }
            }
        }
    }

    /// L1-01 acceptance: FactSet(markdown) == FactSet(coverage_receipt) for
    /// every card, set equality in both directions — a fact missing from the
    /// card AND a fact rendered without receipt coverage both fail.
    #[test]
    fn card_fact_lines_reconstruct_coverage_receipt() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());
        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");

        let receipts = receipt_fact_map(&atlas_dir);
        for card in &manifest.cards {
            let md = fs::read_to_string(atlas_dir.join(&card.path)).expect("card content");
            assert!(
                !md.contains("```json"),
                "{}: dense card must not carry a JSON fence",
                card.path
            );
            let parsed = parse_card_facts(&md);
            let expected = receipts.get(&card.path).cloned().unwrap_or_default();
            assert_eq!(
                parsed, expected,
                "{}: FactSet(markdown) must equal FactSet(receipt) in both directions",
                card.path
            );
        }
    }

    /// L1-01 anti-teatr (kanon v4): removing ANY base-fact line from a card
    /// must break FactSet parity. Iterates over 3 seeded-random fact lines —
    /// fixed seed, deterministic run.
    #[test]
    fn removing_fact_lines_breaks_receipt_parity() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());
        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");
        let receipts = receipt_fact_map(&atlas_dir);

        let mut fact_lines: Vec<(String, usize)> = Vec::new();
        for card in &manifest.cards {
            let md = fs::read_to_string(atlas_dir.join(&card.path)).expect("card content");
            for (idx, line) in md.lines().enumerate() {
                if !parse_card_facts(line).is_empty() {
                    fact_lines.push((card.path.clone(), idx));
                }
            }
        }
        assert!(
            fact_lines.len() >= 3,
            "fixture must carry at least 3 fact lines, got {}",
            fact_lines.len()
        );

        // Deterministic LCG over a fixed seed — no Date/rand dependency.
        let mut seed: u64 = 0x5EED_CAFE;
        let mut picked: BTreeSet<usize> = BTreeSet::new();
        while picked.len() < 3 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            picked.insert((seed >> 33) as usize % fact_lines.len());
        }
        for &choice in &picked {
            let (card_path, line_idx) = &fact_lines[choice];
            let md = fs::read_to_string(atlas_dir.join(card_path)).expect("card content");
            let mutated: String = md
                .lines()
                .enumerate()
                .filter(|(idx, _)| idx != line_idx)
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
            let parsed = parse_card_facts(&mutated);
            let expected = receipts.get(card_path).cloned().unwrap_or_default();
            assert_ne!(
                parsed, expected,
                "{card_path}: dropping fact line {line_idx} must break FactSet parity"
            );
        }
    }

    /// Kanon v4: unknown card kinds fall back to the generic JSON card —
    /// `render_json_card` stays alive as the fallback, with zero calls for
    /// the six known cards.
    #[test]
    fn unknown_card_kind_falls_back_to_generic_json_card() {
        let pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        let body = render_card_body(&pack, "telemetry", "99-telemetry.md", "Telemetry");
        assert!(
            body.markdown.contains("```json"),
            "fallback must keep the generic JSON fence"
        );
        assert!(body.coverage_receipt.is_empty());
    }

    /// Dense cards must carry the loud freshness header exactly like the old
    /// JSON cards did — staleness is not allowed to get quieter in v4.
    #[test]
    fn dense_cards_surface_stale_snapshot_in_header() {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.risk.stale_snapshot = true;
        let body = render_core_card(&pack, &IntentCardSource::default());
        assert!(
            body.markdown
                .contains("Freshness: STALE - card snapshot main@abc1234"),
            "stale dense cards must carry a loud header flag: {}",
            body.markdown
        );
    }

    /// M1-01 regression: the Identity section once hardcoded three
    /// `unavailable — …` excuse rows claiming the pack carries no intent-layer
    /// revisions. It does — `pack.memory.overlay` has carried all three since
    /// the overlay seam landed — so the card must read them, not apologize.
    #[test]
    fn core_card_reads_intent_layer_revisions_from_the_pack() {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.memory.overlay = Some(crate::aicx::overlay::OverlayRenderState {
            schema_version: "loctree.overlay.intent.v1".to_string(),
            repo_id: "proj".to_string(),
            store_revision: "sr1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            overlay_revision: "ov1:cccccccccccccccccccccccccccccccc".to_string(),
            snapshot_commit: "abc1234".to_string(),
            anchor_catalog_revision: "acr1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            producer_version: "aicx 0.12.2-test".to_string(),
            freshness: crate::aicx::overlay::OverlayFreshness::Fresh,
            key_transition: None,
            theses: Vec::new(),
            scope_paths: Vec::new(),
            refresh_command: "aicx overlay".to_string(),
            refresh_recommended: false,
        });

        let body = render_core_card(&pack, &IntentCardSource::default());
        for expected in [
            "store_revision: sr1:bbbbbbbbbbbb",
            "overlay_revision: ov1:cccccccccccc",
            "anchor_catalog_revision: acr1:aaaaaaaaaaaa",
        ] {
            assert!(
                body.markdown.contains(expected),
                "core card must carry `{expected}`: {}",
                body.markdown
            );
        }
        assert!(
            !body
                .markdown
                .contains("does not carry an AICX store revision"),
            "the retired excuse row must not come back: {}",
            body.markdown
        );
    }

    /// The honest half of the same contract: with no overlay state and no
    /// cached producer document the layer really is cold, and the card names
    /// the refresh instead of a stale-sounding excuse.
    #[test]
    fn core_card_names_the_refresh_when_the_intent_layer_is_cold() {
        let pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        let body = render_core_card(&pack, &IntentCardSource::default());
        assert!(
            body.markdown.contains(
                "store_revision: unavailable — no overlay document (cold store); refresh with `aicx overlay`"
            ),
            "cold layer must point at the refresh: {}",
            body.markdown
        );
    }

    // -----------------------------------------------------------------------
    // M1-01 — 03-intent-map (upgrade of 03-memory-trail)
    // -----------------------------------------------------------------------

    /// Write a contract-valid `loctree.overlay.intent.v1` document into the
    /// I1-01 cache location so the intent card has a producer emission to
    /// consume without spawning anything.
    fn write_mock_overlay(root: &Path, snapshot_commit: &str) {
        let dir = root.join(".loctree");
        fs::create_dir_all(&dir).expect("cache dir");
        let doc = json!({
            "schema": "loctree.overlay.intent.v1",
            "repo_id": "mock-repo",
            "snapshot_commit": snapshot_commit,
            "anchor_catalog_revision": "acr1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "store_revision": "sr1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "overlay_revision": "ov1:cccccccccccccccccccccccccccccccc",
            "producer_version": "aicx 0.9.0-test",
            "entries": [
                {
                    "intent_id": "int1:hubthesis0000001",
                    "target": { "kind": "path", "path": "src/types.rs" },
                    "thesis": "centralizacja shared types utrzymana; rozbicie odrzucone",
                    "status": "current",
                    "authority": "operator_confirmed",
                    "verification_status": "verified",
                    "valid_from": "2026-07-12T10:00:00Z",
                    "refs": [{
                        "evidence_event_id": "ev1:x",
                        "ref": "session:49a84e4c-f24d-4167-9f55-ae0f8123a66f#turn-42"
                    }]
                },
                {
                    "intent_id": "int1:repothesis000001",
                    "target": { "kind": "repo" },
                    "thesis": "force-feed pełnej struktury; on-demand zakazany dla warstwy bazowej",
                    "status": "current",
                    "authority": "agent_derived",
                    "verification_status": "unverified",
                    "valid_from": "2026-07-13T11:00:00Z",
                    "refs": [{ "evidence_event_id": "ev1:y", "ref": "chunk:deadbeef01" }]
                },
                {
                    "intent_id": "int1:antithesis000001",
                    "target": { "kind": "path", "path": "install.sh" },
                    "thesis": "shadow-cleanup kasował binarki hosta — nie przywracać",
                    "status": "disputed",
                    "authority": "operator_confirmed",
                    "verification_status": "verified",
                    "valid_from": "2026-06-30T09:00:00Z",
                    "refs": [{ "evidence_event_id": "ev1:z", "ref": "session:0ac52090-0000-0000-0000-000000000000#turn-7" }]
                },
                {
                    "intent_id": "int1:oldthesis0000001",
                    "target": { "kind": "path", "path": "src/types.rs" },
                    "thesis": "per-plik atomic write uznany za wystarczający",
                    "status": "superseded",
                    "authority": "agent_derived",
                    "verification_status": "verified",
                    "valid_from": "2026-06-04T08:00:00Z",
                    "refs": [{ "evidence_event_id": "ev1:w", "ref": "session:e0568e95-0000-0000-0000-000000000000#turn-3" }]
                },
                {
                    "intent_id": "int1:brokenthesis0001",
                    "target": { "kind": "repo" },
                    "thesis": "producer-side contract violation: current yet refuted",
                    "status": "current",
                    "authority": "inferred",
                    "verification_status": "refuted",
                    "valid_from": "2026-07-01T00:00:00Z",
                    "refs": [{ "evidence_event_id": "ev1:v", "ref": "chunk:feedface02" }]
                }
            ],
            "unresolved_attributions": [
                { "candidate": "below-threshold guess", "confidence": 0.11 }
            ]
        });
        fs::write(
            dir.join("aicx-overlay.v1.json"),
            serde_json::to_string_pretty(&doc).expect("mock overlay json"),
        )
        .expect("mock overlay write");
    }

    /// M1-01 feature path: a warm overlay cache yields spec-grammar theses
    /// pinned per-hub, repo-wide entries, anti-recommendations, superseded
    /// history at the end — and the receipt carries `thesis:<intent_id>`
    /// exactly for what the card renders.
    #[test]
    fn intent_card_renders_theses_pinned_to_hubs_and_repo_wide() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());
        write_mock_overlay(tmp.path(), "abc1234");

        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");

        assert_eq!(manifest.cards.len(), 6, "upgrade, not a new card");
        let card = manifest
            .cards
            .iter()
            .find(|card| card.id == "intent")
            .expect("intent card in manifest");
        assert_eq!(card.path, "03-intent-map.md");
        assert_eq!(card.title, "Intent Map");
        assert!(!atlas_dir.join("03-memory-trail.md").exists());

        let md = fs::read_to_string(atlas_dir.join("03-intent-map.md")).expect("card content");
        assert!(!md.contains("```json"));
        assert!(md.lines().count() <= 200, "card must fit the ≤200 budget");
        assert!(
            !md.contains("intent layer stale"),
            "warm matching cache must not be marked stale: {md}"
        );

        // Hub thesis pinned under the hub path via the overlay attribution.
        assert!(md.contains("## Per-hub — formative decisions (fan-in ≥ 10)"));
        assert!(md.contains("\nsrc/types.rs\n"));
        assert!(
            md.contains(
                "  ✓[V] 2026-07-12 · operator_confirmed · centralizacja shared types utrzymana; rozbicie odrzucone · session 49a84e4c §turn-42"
            ),
            "hub thesis must follow the golden grammar: {md}"
        );
        // Repo-wide thesis keeps the opaque chunk ref verbatim.
        assert!(md.contains("## Repo-wide"));
        assert!(md.contains("  ✓[U] 2026-07-13 · agent_derived · force-feed"));
        assert!(md.contains("· chunk:deadbeef01"));
        // Anti-recommendation renders ✗ and the current+refuted entry is
        // demoted there — ✓[R] never appears.
        assert!(md.contains("## Anti-recommendations (AicxFailure)"));
        assert!(md.contains("✗[V] 2026-06-30 · operator_confirmed · shadow-cleanup"));
        assert!(md.contains("✗[R] 2026-07-01 · inferred · producer-side contract violation"));
        assert!(!md.contains("✓[R]"), "refuted ⊥ current: {md}");
        // Superseded history collected at the end, ⊘-marked.
        let superseded_pos = md.find("## Superseded").expect("superseded section");
        assert!(md[superseded_pos..].contains("⊘[V] 2026-06-04 · agent_derived · per-plik"));
        for section in ["## Per-hub", "## Repo-wide", "## Anti-recommendations"] {
            assert!(
                md.find(section).expect("section") < superseded_pos,
                "{section} must render before the superseded history"
            );
        }

        // Receipt: thesis:<intent_id> for every rendered thesis.
        let receipts = receipt_fact_map(&atlas_dir);
        let expected: BTreeSet<String> = [
            "thesis:int1:hubthesis0000001",
            "thesis:int1:repothesis000001",
            "thesis:int1:antithesis000001",
            "thesis:int1:oldthesis0000001",
            "thesis:int1:brokenthesis0001",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(receipts["03-intent-map.md"], expected);
        assert_eq!(card.fact_count, 5);

        // Payload: raw producer doc (incl. unresolved_attributions — Doktryna
        // 7: payload-only) plus the rendered-theses list the verifier derives
        // fact ids from.
        let payload: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(atlas_dir.join("03-intent-map.full.json")).expect("payload"),
        )
        .expect("payload parses");
        assert!(payload["overlay"]["unresolved_attributions"].is_array());
        assert_eq!(
            payload["rendered_theses"]
                .as_array()
                .expect("rendered_theses")
                .len(),
            5
        );
        assert!(
            !md.contains("below-threshold guess"),
            "unresolved attributions must never reach the card surface"
        );

        assert_eq!(
            manifest.domain_owners.get("intent").map(String::as_str),
            Some("03-intent-map")
        );
    }

    /// M1-01 resilience: a cold store still materializes the card with an
    /// explicit stale line and explicit empty sections — the atlas never
    /// breaks, and the receipt carries no theses.
    #[test]
    fn intent_card_cold_store_is_explicitly_stale_never_broken() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());

        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("cold store must not break the atlas");
        assert_eq!(manifest.cards.len(), 6);

        let md = fs::read_to_string(atlas_dir.join("03-intent-map.md")).expect("card content");
        assert!(
            md.contains("intent layer stale (no overlay cache yet) — refresh: `"),
            "cold store must be explicit, not silent: {md}"
        );
        assert!(md.contains("no registered per-hub decisions — corpus: 0"));
        assert!(md.contains("no repo-wide entries — corpus: 0"));
        assert!(md.contains("no anti-recommendations — corpus: 0"));
        assert!(md.contains("no superseded entries — corpus: 0"));

        let receipts = receipt_fact_map(&atlas_dir);
        assert!(receipts["03-intent-map.md"].is_empty());
    }

    /// M1-01: local truth (snapshot commit) drifting from the cached overlay
    /// yields the explicit stale marker while the last correct theses keep
    /// rendering — last correct data beats silence.
    #[test]
    fn intent_card_snapshot_drift_is_stale_but_keeps_last_correct_theses() {
        let tmp = TempDir::new().expect("temp dir");
        let atlas_dir = tmp.path().join("context-atlas");
        let pack = rich_pack(tmp.path());
        write_mock_overlay(tmp.path(), "deadbee");

        materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");
        let md = fs::read_to_string(atlas_dir.join("03-intent-map.md")).expect("card content");
        assert!(
            md.contains("intent layer stale (snapshot moved (cache deadbee, local abc1234))"),
            "commit drift must surface as an explicit stale marker: {md}"
        );
        assert!(
            md.contains("✓[V] 2026-07-12 · operator_confirmed"),
            "stale layer still serves the last correct theses: {md}"
        );
    }

    fn pack_with_corpus_file(root: &Path) -> ContextPack {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(root.display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: Some("scan-1".to_string()),
        });
        pack.structural.files.push(StructuralFile {
            path: "src/lib.rs".to_string(),
            role: StructuralRole::Target,
            depth: 0,
            language: "rs".to_string(),
            loc: 12,
            authority: AuthorityLabel::RepoVerified,
        });
        pack
    }

    #[test]
    fn w4_02_atlas_status_reflects_snapshot() {
        let tmp = TempDir::new().expect("temp dir");

        let mut hollow = ContextPack::empty(ProjectIdentity {
            canonical_root: Some(tmp.path().display().to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        hollow.risk.cache_scope = RiskCacheScope::MissingSnapshot;
        hollow.risk.cache_scope_authority = AuthorityLabel::RepoVerified;
        hollow.risk.snapshot_health = Some("missing_snapshot".to_string());
        let hollow_dir = tmp.path().join("hollow-atlas");
        let hollow_manifest = materialize_context_atlas(&hollow, tmp.path(), Some(&hollow_dir))
            .expect("hollow atlas should still materialize");
        assert_eq!(hollow_manifest.status, ATLAS_STATUS_HOLLOW);
        assert_ne!(hollow_manifest.status, ATLAS_STATUS_READY);
        assert!(
            hollow_manifest.message.contains("loct scan"),
            "hollow atlas must instruct `loct scan`: {}",
            hollow_manifest.message
        );
        assert!(
            !hollow_manifest.pointer_payload()["status"]
                .as_str()
                .unwrap_or("")
                .eq(ATLAS_STATUS_READY),
            "MCP pointer must not claim atlas_ready without a snapshot"
        );
        for card in &hollow_manifest.cards {
            assert_eq!(
                card.status.as_deref(),
                Some(CARD_STATUS_UNREAD_UNUSABLE),
                "{} must be unread/unusable when hollow",
                card.path
            );
        }
        let hollow_md =
            fs::read_to_string(hollow_dir.join("manifest.md")).expect("hollow manifest md");
        assert!(hollow_md.contains("unread/unusable"));
        assert!(hollow_md.contains("`loct scan`") || hollow_md.contains("loct scan"));
        assert!(!hollow_md.contains(&format!("Atlas status: `{ATLAS_STATUS_READY}`")));

        let mut stale = pack_with_corpus_file(tmp.path());
        stale.risk.stale_snapshot = true;
        let stale_dir = tmp.path().join("stale-atlas");
        let stale_manifest = materialize_context_atlas(&stale, tmp.path(), Some(&stale_dir))
            .expect("stale atlas should materialize");
        assert_eq!(stale_manifest.status, ATLAS_STATUS_STALE);
        assert_ne!(stale_manifest.status, ATLAS_STATUS_READY);

        let ready = pack_with_corpus_file(tmp.path());
        let ready_dir = tmp.path().join("ready-atlas");
        let ready_manifest = materialize_context_atlas(&ready, tmp.path(), Some(&ready_dir))
            .expect("ready atlas should materialize");
        assert_eq!(ready_manifest.status, ATLAS_STATUS_READY);
        for card in &ready_manifest.cards {
            assert_eq!(card.status, None, "{} should be readable", card.path);
        }
    }

    #[test]
    fn w4_02_manifest_identity_list_bounded() {
        let atlases: Vec<ContextAtlasReference> = (0..25)
            .map(|idx| ContextAtlasReference {
                identity: ContextAtlasIdentity {
                    atlas_id: format!("scope-{idx:02}"),
                    kind: "scope".to_string(),
                    scope_fingerprint: Some(format!("fp-{idx:02}")),
                    selectors: vec![format!("path:src/{idx:02}")],
                    task: Some(format!("task {idx:02}")),
                },
                snapshot: "main@abc1234".to_string(),
                generated_at: "2026-09-19T00:00:00Z".to_string(),
                atlas_dir: format!("/tmp/atlas/{idx:02}"),
                manifest: format!("/tmp/atlas/{idx:02}/manifest.md"),
                manifest_json: format!("/tmp/atlas/{idx:02}/manifest.json"),
            })
            .collect();
        let manifest = ContextAtlasManifest {
            protocol: CONTEXT_ATLAS_PROTOCOL.to_string(),
            status: ATLAS_STATUS_READY.to_string(),
            project: "proj".to_string(),
            snapshot: "main@abc1234".to_string(),
            generated_at: "2026-09-19T00:00:00Z".to_string(),
            atlas_dir: "/tmp/proj/.loctree/context-atlas".to_string(),
            manifest: "/tmp/proj/.loctree/context-atlas/manifest.md".to_string(),
            manifest_json: "/tmp/proj/.loctree/context-atlas/manifest.json".to_string(),
            recommended_start: "/tmp/proj/.loctree/context-atlas/00-core-map.md".to_string(),
            identity: ContextAtlasIdentity {
                atlas_id: "scope-00".to_string(),
                kind: "scope".to_string(),
                ..ContextAtlasIdentity::default()
            },
            atlases,
            domain_owners: atlas_domain_owners(),
            cards: Vec::new(),
            message: "msg".to_string(),
        };

        let md = render_manifest(&manifest);
        assert!(md.contains("`scope-00`"));
        assert!(md.contains("`scope-19`"));
        assert!(
            !md.contains("`scope-20`"),
            "markdown table must cap at {ATLAS_IDENTITY_LIST_LIMIT} identities: {md}"
        );
        assert!(
            md.contains("… and 5 more"),
            "markdown must name omitted identities: {md}"
        );
        assert!(md.contains(ATLAS_IDENTITIES_FULL_JSON));

        let payload = manifest.pointer_payload();
        assert_eq!(
            payload["atlases"].as_array().map(Vec::len),
            Some(ATLAS_IDENTITY_LIST_LIMIT)
        );
        assert_eq!(payload["atlases_omitted"], json!(5));
        assert_eq!(
            payload["atlases_full_path"],
            json!(ATLAS_IDENTITIES_FULL_JSON)
        );

        let tmp = TempDir::new().expect("temp dir");
        let mut persisted = manifest;
        persisted.manifest = tmp.path().join("manifest.md").display().to_string();
        persisted.manifest_json = tmp.path().join("manifest.json").display().to_string();
        write_manifest_files(&persisted).expect("persist bounded catalog");
        let full: Vec<ContextAtlasReference> = serde_json::from_str(
            &fs::read_to_string(tmp.path().join(ATLAS_IDENTITIES_FULL_JSON))
                .expect("manifest.full.json"),
        )
        .expect("full identity list parses");
        assert_eq!(full.len(), 25, "full list lives in .full.json");
        let on_disk: ContextAtlasManifest = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("manifest.json")).expect("manifest.json"),
        )
        .expect("catalog parses");
        assert_eq!(
            on_disk.atlases.len(),
            25,
            "on-disk catalog keeps the full identity list for reload"
        );
    }

    #[test]
    fn w4_02_mirror_writes_are_atomic() {
        let tmp = TempDir::new().expect("temp dir");
        let dest = tmp.path().join("card.md");
        write_atlas_file_atomic(&dest, b"# hello\ncomplete\n").expect("atomic write");
        assert_eq!(
            fs::read_to_string(&dest).expect("read dest"),
            "# hello\ncomplete\n"
        );
        let leftover_tmps: Vec<_> = fs::read_dir(tmp.path())
            .expect("list dest dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".loctree-atlas-")
            })
            .collect();
        assert!(
            leftover_tmps.is_empty(),
            "tmp+rename must not leave staging files: {leftover_tmps:?}"
        );

        let root = tmp.path().join("shared-project");
        fs::create_dir_all(&root).expect("project root");
        std::thread::scope(|scope| {
            let root_a = root.clone();
            scope.spawn(move || {
                let mut pack = pack_with_corpus_file(&root_a);
                pack.scope = Some(ScopeReport {
                    selectors: vec!["path:src/a".to_string()],
                    matched_files: 1,
                    empty: false,
                    fingerprint: "scope-a".to_string(),
                    named_resolved_from: None,
                    resolved_selectors: Vec::new(),
                    selector_match_counts: Vec::new(),
                });
                pack.action.next_safe_commands = vec!["FIRST_THREAD_MARKER".to_string()];
                materialize_context_atlas(&pack, &root_a, None).expect("thread a");
            });
            let root_b = root.clone();
            scope.spawn(move || {
                let mut pack = pack_with_corpus_file(&root_b);
                pack.scope = Some(ScopeReport {
                    selectors: vec!["path:src/b".to_string()],
                    matched_files: 1,
                    empty: false,
                    fingerprint: "scope-b".to_string(),
                    named_resolved_from: None,
                    resolved_selectors: Vec::new(),
                    selector_match_counts: Vec::new(),
                });
                pack.action.next_safe_commands = vec!["SECOND_THREAD_MARKER".to_string()];
                materialize_context_atlas(&pack, &root_b, None).expect("thread b");
            });
        });

        let atlas_root = atlas_dir_for_project(&root);
        for name in [
            "00-core-map.md",
            "01-structural-map.md",
            "02-runtime-map.md",
            "03-intent-map.md",
            "04-verification-gates.md",
            "05-risk-register.md",
            "manifest.md",
        ] {
            let text = fs::read_to_string(atlas_root.join(name))
                .unwrap_or_else(|err| panic!("{name} must be a complete file: {err}"));
            assert!(
                text.starts_with("# "),
                "{name} must not be a torn write: {}",
                text.chars().take(80).collect::<String>()
            );
        }
        let catalog_raw =
            fs::read_to_string(atlas_root.join("manifest.json")).expect("root catalog json");
        let catalog: ContextAtlasManifest =
            serde_json::from_str(&catalog_raw).expect("root catalog must be complete JSON");
        assert!(
            catalog.atlases.len() >= 2,
            "both scoped identities should land in the catalog: {:?}",
            catalog
                .atlases
                .iter()
                .map(|atlas| atlas.identity.atlas_id.as_str())
                .collect::<Vec<_>>()
        );
        let core = fs::read_to_string(atlas_root.join("00-core-map.md")).expect("flat core");
        assert!(
            core.contains("FIRST_THREAD_MARKER") || core.contains("SECOND_THREAD_MARKER"),
            "flat compatibility view must be one complete identity, not a mix of fragments: {core}"
        );
        assert!(
            !(core.contains("FIRST_THREAD_MARKER") && core.contains("SECOND_THREAD_MARKER")),
            "flat core card must not mix both writers: {core}"
        );
    }
}
