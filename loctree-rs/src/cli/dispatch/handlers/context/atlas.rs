use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context_render::current_iso_timestamp;
use crate::pack::ContextPack;

pub const CONTEXT_ATLAS_PROTOCOL: &str = "loctree.context_atlas.v1";
pub const CONTEXT_ATLAS_DIR: &str = "context-atlas";

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
    /// True when the card body was capped to the per-card line budget and a
    /// complete sibling artifact was written. The manifest and on-card markers
    /// surface this so an agent never reads a clipped card as canonical truth.
    #[serde(default)]
    pub truncated: bool,
    /// Lines dropped from the card body when capped (0 when the card is whole).
    #[serde(default)]
    pub dropped_lines: usize,
    /// Relative filename of the complete sibling artifact (e.g.
    /// `01-structural-map.full.json`) written when the card was truncated.
    #[serde(default)]
    pub full_path: Option<String>,
    /// Line count of the complete sibling JSON payload. Present only for
    /// truncated cards, where `lines` is the materialized `.md` card length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_payload_lines: Option<usize>,
}

impl ContextAtlasManifest {
    pub fn pointer_payload(&self) -> serde_json::Value {
        json!({
            "protocol": self.protocol,
            "status": self.status,
            "project": self.project,
            "snapshot": self.snapshot,
            "atlas_dir": self.atlas_dir,
            "manifest": self.manifest,
            "manifest_json": self.manifest_json,
            "recommended_start": self.recommended_start,
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
        out.push_str("Status: ready\n");
        out.push_str(&format!("Project: {}\n", self.project));
        out.push_str(&format!("Snapshot: {}\n", self.snapshot));
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

pub fn atlas_dir_for_project(project_root: &Path) -> PathBuf {
    project_root.join(ATLAS_REPO_DIR).join(CONTEXT_ATLAS_DIR)
}

pub fn materialize_context_atlas(
    pack: &ContextPack,
    project_root: &Path,
    atlas_dir: Option<&Path>,
) -> io::Result<ContextAtlasManifest> {
    let atlas_dir = atlas_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| atlas_dir_for_project(project_root));
    fs::create_dir_all(&atlas_dir)?;

    let project = pack
        .project
        .canonical_root
        .clone()
        .unwrap_or_else(|| project_root.display().to_string());
    let snapshot = snapshot_label(pack);
    let generated_at = current_iso_timestamp();

    let specs = vec![
        CardSpec {
            id: "core",
            title: "Core Map",
            filename: "00-core-map.md",
            why: "Repo identity, current risk, authority labels, safe next commands.",
            saves: "wrong project state, stale assumptions, unsafe first actions",
            body: render_core_card(pack),
        },
        CardSpec {
            id: "structural",
            title: "Structural Map",
            filename: "01-structural-map.md",
            why: "Files, symbols, imports, consumers, entrypoints; read before edits/refactors.",
            saves: "missed consumers, wrong impact, blind dependency edits",
            body: render_json_card(
                pack,
                "01-structural-map.md",
                "Structural Map",
                "This card contains dependency and symbol topology for the selected scope.",
                "This Structural Map does not include runtime behavior, env contracts, or verification gates. Read `02-runtime-map.md` and `04-verification-gates.md` before changing behavior.",
                json!(&pack.structural),
            ),
        },
        CardSpec {
            id: "runtime",
            title: "Runtime Map",
            filename: "02-runtime-map.md",
            why: "Runtime behavior, framework hints, env contracts, reachability.",
            saves: "wrong tests, hidden runtime coupling, config mistakes",
            body: render_json_card(
                pack,
                "02-runtime-map.md",
                "Runtime Map",
                "This card contains runtime signals derived from semantic facts and framework bridges.",
                "This Runtime Map does not include prior decisions or full risk history. Read `03-memory-trail.md` when continuing work and `05-risk-register.md` before release decisions.",
                json!(&pack.runtime),
            ),
        },
        CardSpec {
            id: "memory",
            title: "Memory Trail",
            filename: "03-memory-trail.md",
            why: "Prior decisions, outcomes, tasks, and AICX continuity when available.",
            saves: "repeated work, forgotten decisions, reimplemented tasks",
            body: render_json_card(
                pack,
                "03-memory-trail.md",
                "Memory Trail",
                "This card contains continuity memory from AICX if the overlay is available.",
                "This Memory Trail does not replace repo-verified facts. Re-check structural/runtime cards before editing.",
                json!(&pack.memory),
            ),
        },
        CardSpec {
            id: "verification",
            title: "Verification Gates",
            filename: "04-verification-gates.md",
            why: "Commands and likely tests most relevant to validate changes.",
            saves: "wrong validation path, skipped downstream checks, false confidence",
            body: render_verification_card(pack),
        },
        CardSpec {
            id: "risk",
            title: "Risk Register",
            filename: "05-risk-register.md",
            why: "Hotspots, cache/snapshot health, stale assumptions, next risk-reducing actions.",
            saves: "release blockers, high fan-in surprises, stale-cache decisions",
            body: render_json_card(
                pack,
                "05-risk-register.md",
                "Risk Register",
                "This card collects risk signals and recommended actions for the current context scope.",
                "This Risk Register does not include full source content. Use `loct slice`/`loct impact` for exact file-level surgery.",
                json!({ "risk": &pack.risk, "action": &pack.action, "authority": &pack.authority }),
            ),
        },
    ];

    let mut cards = Vec::new();
    for spec in specs {
        let path = atlas_dir.join(spec.filename);
        fs::write(&path, spec.body.content.as_bytes())?;
        // When the body was capped, write the complete payload to a concrete
        // sibling artifact so the on-card/manifest markers point at a real file
        // (not just the whole-pack `loct context --full --json` regeneration).
        let (truncated, dropped_lines, full_path, full_payload_lines) = match spec.body.overflow {
            Some(overflow) => {
                let full_artifact = atlas_dir.join(&overflow.full_filename);
                let mut full_json = overflow.full_json;
                if !full_json.ends_with('\n') {
                    full_json.push('\n');
                }
                fs::write(&full_artifact, full_json.as_bytes())?;
                (
                    true,
                    overflow.dropped,
                    Some(overflow.full_filename),
                    Some(overflow.full_payload_lines),
                )
            }
            None => (false, 0, None, None),
        };
        cards.push(ContextAtlasCard {
            id: spec.id.to_string(),
            title: spec.title.to_string(),
            path: spec.filename.to_string(),
            lines: line_count(&spec.body.content),
            bytes: spec.body.content.len(),
            why: spec.why.to_string(),
            saves_you_from: spec.saves.to_string(),
            truncated,
            dropped_lines,
            full_path,
            full_payload_lines,
        });
    }

    let manifest_path = atlas_dir.join("manifest.md");
    let manifest_json_path = atlas_dir.join("manifest.json");
    let receipt_path = atlas_dir.join("receipt.json");

    let mut manifest = ContextAtlasManifest {
        protocol: CONTEXT_ATLAS_PROTOCOL.to_string(),
        status: "atlas_ready".to_string(),
        project,
        snapshot,
        generated_at,
        atlas_dir: atlas_dir.display().to_string(),
        manifest: manifest_path.display().to_string(),
        manifest_json: manifest_json_path.display().to_string(),
        recommended_start: atlas_dir.join("00-core-map.md").display().to_string(),
        cards,
        message: "This atlas contains the repo understanding an agent would otherwise rediscover manually. Start with manifest.md, then read the recommended cards; broad repo-level answers are incomplete until core, structural, and runtime are read.".to_string(),
    };

    let manifest_md = render_manifest(&manifest);
    fs::write(&manifest_path, manifest_md.as_bytes())?;
    manifest.manifest = manifest_path.display().to_string();
    fs::write(
        &manifest_json_path,
        serde_json::to_string_pretty(&manifest).map_err(io::Error::other)?,
    )?;
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&json!({
            "protocol": CONTEXT_ATLAS_PROTOCOL,
            "generated_at": manifest.generated_at,
            "project": manifest.project,
            "snapshot": manifest.snapshot,
            "cards": manifest.cards.iter().map(|card| &card.path).collect::<Vec<_>>()
        }))
        .map_err(io::Error::other)?,
    )?;

    Ok(manifest)
}

struct CardSpec {
    id: &'static str,
    title: &'static str,
    filename: &'static str,
    why: &'static str,
    saves: &'static str,
    body: CardBody,
}

/// A rendered atlas card body plus the optional overflow receipt produced when
/// the JSON payload exceeded the per-card cap.
struct CardBody {
    /// Card content written to `<filename>` (capped to the per-card budget).
    content: String,
    /// Present only when the payload was capped; carries the complete payload
    /// so the materializer can write a concrete `<stem>.full.json` sibling.
    overflow: Option<CardOverflow>,
}

struct CardOverflow {
    /// Complete (uncapped) pretty JSON payload.
    full_json: String,
    /// Line count of the complete sibling JSON payload.
    full_payload_lines: usize,
    /// Lines dropped from the card body.
    dropped: usize,
    /// Concrete sibling artifact filename (e.g. `01-structural-map.full.json`).
    full_filename: String,
}

fn snapshot_label(pack: &ContextPack) -> String {
    let branch = pack.project.branch.as_deref().unwrap_or("unknown");
    let commit = pack.project.commit.as_deref().unwrap_or("unknown");
    format!("{}@{}", branch, commit)
}

fn atlas_freshness_line(pack: &ContextPack) -> String {
    if pack.risk.stale_snapshot {
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

fn render_manifest(manifest: &ContextAtlasManifest) -> String {
    let mut out = String::new();
    out.push_str("# Loctree Context Atlas\n\n");
    out.push_str(&format!("Project: `{}`\n", manifest.project));
    out.push_str(&format!("Snapshot: `{}`\n", manifest.snapshot));
    out.push_str(&format!("Generated: `{}`\n\n", manifest.generated_at));
    out.push_str("This atlas is precomputed repository understanding. It contains the repo map an agent would otherwise have to rediscover manually through search/open cycles.\n\n");
    out.push_str("Tokens are cheaper than wrong assumptions.\n\n");
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
    out.push_str("\n## Completeness\n\n");
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
    if !card.truncated {
        return format!("{} lines", card.lines);
    }

    match card.full_payload_lines {
        Some(full_payload_lines) => format!(
            "{} materialized lines / {} full-payload lines ⚠ partial",
            card.lines, full_payload_lines
        ),
        None => format!("{} materialized lines ⚠ partial", card.lines),
    }
}

fn render_core_card(pack: &ContextPack) -> CardBody {
    render_json_card(
        pack,
        "00-core-map.md",
        "Core Map",
        "This card tells you where you are, what is risky, and what actions are safe next.",
        "This Core Map does not include dependency consumers, runtime entrypoints, or prior decisions. For code changes, read `01-structural-map.md` next.",
        json!({
            "schema_version": pack.schema_version,
            "project": &pack.project,
            "risk": &pack.risk,
            "action": &pack.action,
            "authority": &pack.authority,
        }),
    )
}

fn render_verification_card(pack: &ContextPack) -> CardBody {
    render_json_card(
        pack,
        "04-verification-gates.md",
        "Verification Gates",
        "This card lists likely verification commands and tests derived from the current context.",
        "This Verification Gates card does not prove correctness by itself. Run the commands before release or submit.",
        json!({
            "next_safe_commands": &pack.action.next_safe_commands,
            "likely_tests": &pack.action.likely_tests,
            "risk": &pack.risk,
        }),
    )
}

/// Hard cap on lines for each Atlas card body (JSON payload only — frame
/// lines like the header / lead / footer are added on top). Mirrors the
/// 2026-05-21 operator decision that no single card should pass 1000 lines
/// and surface the truncation honestly when it does.
///
/// loctree-feedback hak 2026-05-23 #3: the Memory Trail card was emitting
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
) -> CardBody {
    let full_json = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
    let full_filename = full_json_filename(filename);
    let (json, dropped) = cap_json_payload(&full_json, ATLAS_CARD_JSON_LINE_CAP, &full_filename);
    let content = format!(
        "# {}\n\nProject: `{}`\nSnapshot: `{}`\nFreshness: `{}`\n\n{}\n\n```json\n{}\n```\n\n## What this card does not cover\n\n{}\n",
        title,
        pack.project.canonical_root.as_deref().unwrap_or("unknown"),
        snapshot_label(pack),
        atlas_freshness_line(pack),
        lead,
        json,
        missing
    );
    let overflow = dropped.map(|dropped| CardOverflow {
        full_payload_lines: line_count(&full_json),
        full_json,
        dropped,
        full_filename,
    });
    CardBody { content, overflow }
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
    use crate::pack::{AuthorityLabel, ProjectIdentity, StructuralFile, StructuralRole};
    use tempfile::TempDir;

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
                content
                    .contains("Freshness: `fresh - card matches the loaded snapshot authority.`")
            );
            // An empty pack fits every card; nothing is capped and no sibling
            // `.full.json` artifact is written.
            assert!(!card.truncated, "{} unexpectedly truncated", card.path);
            assert_eq!(card.dropped_lines, 0);
            assert!(card.full_path.is_none());
            let sibling = atlas_dir.join(full_json_filename(&card.path));
            assert!(
                !sibling.exists(),
                "no sibling artifact expected for a whole card: {}",
                sibling.display()
            );
        }

        let manifest_md = fs::read_to_string(atlas_dir.join("manifest.md")).expect("manifest md");
        assert!(manifest_md.contains("Recommended Reading Path"));
        assert!(manifest_md.contains("Saves you from"));
        // No card overflowed, so the manifest carries no partial-card section.
        assert!(!manifest_md.contains("## Partial cards"));
    }

    /// loctree-feedback hak 2026-05-23 #3 regression: a fat JSON card body
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

    /// loctree-feedback tail 2026-06-22 regression: an over-cap card body must keep
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
        );
        let overflow = body
            .overflow
            .expect("a 2000-entry payload must overflow the per-card cap");
        assert_eq!(overflow.full_filename, "01-structural-map.full.json");
        assert!(overflow.dropped > 0);
        assert!(
            overflow.full_payload_lines > ATLAS_CARD_JSON_LINE_CAP,
            "complete payload line count must preserve full sibling size"
        );
        assert!(
            body.content.contains("01-structural-map.full.json"),
            "card body must point readers at the complete sibling artifact"
        );
        assert!(body.content.contains("// truncated:"));
        // The complete payload is uncapped — the last row survives in the sibling.
        assert!(
            overflow.full_json.contains("\"row\": 1999"),
            "sibling artifact must hold the complete, uncapped payload"
        );
        assert!(
            overflow.full_json.lines().count() > ATLAS_CARD_JSON_LINE_CAP,
            "complete payload should exceed the per-card cap"
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
        );

        assert!(
            body.content
                .contains("Freshness: `STALE - card snapshot main@abc1234"),
            "stale atlas cards must carry a loud header flag: {}",
            body.content
        );
        assert!(
            body.content
                .contains("refresh with `loct context --full` before relying on this card")
        );
    }

    /// loctree-feedback tail 2026-06-22 regression: the manifest must explicitly
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
                path: format!("src/generated_{idx}.rs"),
                role: StructuralRole::Dependency,
                depth: 1,
                language: "rs".to_string(),
                loc: idx,
                authority: AuthorityLabel::RepoVerified,
            })
            .collect();

        let manifest = materialize_context_atlas(&pack, tmp.path(), Some(&atlas_dir))
            .expect("atlas should materialize");
        let card = manifest
            .cards
            .iter()
            .find(|card| card.path == "01-structural-map.md")
            .expect("structural card");
        assert!(card.truncated, "oversized structural card must be partial");

        let card_content =
            fs::read_to_string(atlas_dir.join(&card.path)).expect("structural card content");
        assert_eq!(
            card.lines,
            card_content.lines().count(),
            "manifest must quote the materialized .md card length"
        );
        assert!(
            card_content.contains("// truncated:"),
            "materialized card must carry an in-card truncation marker"
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

        let expected = format!(
            "{} materialized lines / {} full-payload lines",
            card.lines,
            card.full_payload_lines.expect("full payload lines")
        );
        let manifest_md = fs::read_to_string(atlas_dir.join("manifest.md")).expect("manifest md");
        assert!(
            manifest_md.contains(&expected),
            "manifest markdown must make the materialized/full split explicit: {manifest_md}"
        );
        assert!(
            manifest.render_cli_summary().contains(&expected),
            "CLI summary must make the materialized/full split explicit"
        );
    }
}
