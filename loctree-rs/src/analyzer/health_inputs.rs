//! One health vector, one number.
//!
//! [`super::health_score::calculate_health_score`] has been the single scoring
//! formula since ffc13063 ("polarize markdown report onto one scorer"). The
//! split-brain came back anyway, and the reason is structural: the formula is
//! pure, so the drift cannot live in it — it lives in the *argument*.
//!
//! Every wave that hardened a defect gate landed in exactly one producer:
//!
//! * W1-03 — namesake duplicate groups stop counting (`findings.rs` only).
//! * W2-01 — twin parrots score only where the graded dead detector
//!   independently calls the symbol a defect (`findings.rs` only).
//! * `counts_as_dead_defect` — the confidence/entrypoint fence, copied
//!   verbatim into `audit_report.rs`, never into `for_ai.rs`.
//!
//! `for_ai.rs` kept scoring the ungated sensor rows. Measured on df35a677 over
//! one snapshot of this repository: `loct findings --summary` said 85,
//! `loct --for-ai` said 72. Same commit, same files, same formula — five
//! disagreeing input fields:
//!
//! | field                 | findings (gated) | for-ai (raw) |
//! |-----------------------|------------------|--------------|
//! | `breaking_cycles`     | 5 (classified)   | 16 (every strict cycle) |
//! | `structural_cycles`   | 1 (classified)   | 0 (lazy cycles) |
//! | `duplicate_exports`   | 56 (class-gated) | 183 |
//! | `twins_same_language` | class-gated      | ungated |
//! | `twins_dead_parrots`  | evidence-gated   | ungated |
//!
//! So the gates live here, once. Every surface that reports a health number
//! builds its [`HealthMetrics`] through this module, from the snapshot — which
//! means the next gate a wave adds cannot land on one surface only.
//!
//! Surfaces whose collector genuinely cannot observe the full vector (the audit
//! collector sees cycles, dead exports and twins — not barrels, cascades or
//! duplicate exports) do not get to emit a silent third number: they carry
//! [`AUDIT_BASIS`] next to the figure so the difference is named, not guessed.
//!
//! # What this module does *not* guarantee
//!
//! One layer of the same lesson is still open, and measuring it is cheaper than
//! rediscovering it. This module makes every surface apply the same *gates*. It
//! does not make them read the same *inputs*: `dead_candidates`,
//! `raw_duplicate_symbols` and `cascade_imports` arrive as arguments, collected
//! by whichever scan the calling surface ran.
//!
//! Measured on 257a5f82 over one snapshot of this repository, after
//! `analyzer/output.rs` (the HTML report gauge) was moved onto this module:
//!
//! | field                 | findings | report gauge | agrees? |
//! |-----------------------|---------:|-------------:|---------|
//! | `breaking_cycles`     |        5 |            5 | computed here |
//! | `structural_cycles`   |        1 |            1 | computed here |
//! | `twins_same_language` |        2 |            2 | computed here |
//! | `barrel_chaos_count`  |       99 |           99 | computed here |
//! | `dead_exports`        |        0 |            4 | **passed in** |
//! | `twins_dead_parrots`  |        0 |            4 | **passed in** |
//! | `duplicate_exports`   |       56 |           46 | **passed in** |
//! | `cascade_imports`     |       23 |          145 | **passed in** |
//!
//! The split falls exactly along that line: every field this module derives
//! from the snapshot agrees, and every field handed to it as an argument does
//! not. `findings.rs` reads a persisted snapshot; `output.rs` runs inside a
//! scan and reaches the canonical builder with its own dead-export scan (a
//! different [`DeadFilterConfig`](crate::analyzer::dead_parrots::DeadFilterConfig))
//! and its own notion of a cascade. 85 vs 79 on that commit.
//!
//! Closing it means giving the collectors one origin, not adding another gate
//! here — a gate cannot repair a disagreement about what was counted.

use std::collections::HashSet;

use crate::analyzer::barrels::analyze_barrel_chaos;
use crate::analyzer::cycles::{ClassifiedCycle, CycleClassification, find_cycles_with_lazy};
use crate::analyzer::dead_parrots::DeadExport;
use crate::analyzer::health_score::HealthMetrics;
use crate::analyzer::twins::{
    ExactTwin, TwinCategory, categorize_twin, detect_exact_twins, filter_idiom_twins,
    find_dead_parrots, omit_from_duplicate_groups,
};
use crate::semantic::SemanticFacts;
use crate::snapshot::Snapshot;
use crate::types::FileAnalysis;

/// The one shape-evidence twin computation every surface must share.
///
/// `detect_exact_twins` finds exact-duplicate export groups; Layer 3 then
/// drops the ones semantic analysis calls an idiom (two `usage` printers in
/// two scripts, a shared constructor name a framework dictates, …). Every
/// surface that reports on twins — `findings.rs`, `output.rs`,
/// `analysis_reports.rs`, the CLI `analysis.rs` handlers — must apply BOTH
/// steps over the SAME `files`/`include_tests` input, or the twin count
/// drifts by which surface happened to skip the idiom filter or read a
/// stale/partial file slice (F6: `detect_exact_twins` was called from 9+
/// sites, each choosing its own `include_tests` and its own file slice —
/// `findings.rs` read `scan_results.global_analyses` while this module read
/// `snapshot.files`, two different views of what should be one sensor).
///
/// `semantic_facts` is `None` on a surface that genuinely has none computed
/// yet at that pipeline stage (e.g. `output::process_root_context` runs
/// before semantic facts exist) — the idiom filter is then correctly skipped
/// rather than faked, matching what that surface already did before this
/// converged the call sites.
pub fn canonical_exact_twins(
    files: &[FileAnalysis],
    semantic_facts: Option<&SemanticFacts>,
    include_tests: bool,
) -> Vec<ExactTwin> {
    let raw = detect_exact_twins(files, include_tests);
    match semantic_facts {
        Some(facts) => filter_idiom_twins(raw, facts),
        None => raw,
    }
}

/// What the canonical structural vector scores. Quoted by any surface that
/// reports the number so a reader can tell two figures apart without reading
/// the source.
pub const STRUCTURAL_BASIS: &str =
    "cycles, dead exports, twins, barrel chaos, cascades and duplicate exports";

/// What the audit collector can observe. Strictly narrower than
/// [`STRUCTURAL_BASIS`] — barrels, cascades and duplicate exports are not in
/// `AuditFindings` at all, so an audit health figure is a different metric,
/// not a competing measurement of the same one.
pub const AUDIT_BASIS: &str = "cycles, dead exports and twins only";

/// Does this dead-export verdict belong in the HIGH severity dimension?
///
/// The reported `dead_parrots` list is the sensor and keeps every candidate.
/// The score is an aggregate of *defects*, and two kinds of candidate are not
/// defects no matter how the list reads:
///
/// * `confidence: low` — the detector is telling us it could not resolve the
///   references, not that there are none. Swift is the sharp case: `import` is
///   module-level, so a missing import edge carries no information at all.
/// * `entrypoint: true` — a declared runtime entry (Swift `@main`, Cargo
///   `[[bin]]`, a shebang script) has no caller *by definition*. The entry-point
///   fence already keeps these out of delete quick-wins; scoring them would
///   penalize a repository for having a way to start.
///
/// This gate lived in two producers and was missing from the third. It lives
/// here now, and the producers import it.
pub fn counts_as_dead_defect(candidate: &DeadExport) -> bool {
    !candidate.entrypoint && matches!(candidate.confidence.as_str(), "high" | "very-high")
}

/// The canonical structural defect vector — the only shape that may become a
/// [`HealthMetrics`] on a reporting surface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructuralDefects {
    pub breaking_cycles: usize,
    pub structural_cycles: usize,
    pub dead_exports: usize,
    pub twins_dead_parrots: usize,
    pub twins_same_language: usize,
    pub barrel_chaos_count: usize,
    pub duplicate_exports: usize,
    pub cascade_imports: usize,
    pub files: usize,
    pub loc: usize,
}

impl StructuralDefects {
    /// Feed the canonical scorer. Handler dimensions (`missing_handlers`,
    /// `unregistered_handlers`, `unused_high_confidence`) stay zero here on
    /// purpose: `findings.rs` has no access to command-bridge data, so scoring
    /// them on the `--for-ai` surface alone would be the same split-brain in a
    /// new dimension. They remain reported as counts for visibility.
    pub fn metrics(&self) -> HealthMetrics {
        HealthMetrics {
            breaking_cycles: self.breaking_cycles,
            dead_exports: self.dead_exports,
            twins_dead_parrots: self.twins_dead_parrots,
            twins_same_language: self.twins_same_language,
            barrel_chaos_count: self.barrel_chaos_count,
            structural_cycles: self.structural_cycles,
            cascade_imports: self.cascade_imports,
            duplicate_exports: self.duplicate_exports,
            files: self.files,
            loc: self.loc,
            ..Default::default()
        }
    }
}

/// Build the canonical vector from a snapshot.
///
/// `dead_candidates` is passed in rather than recomputed because operator flags
/// (`--library-mode`, `--confidence high`) legitimately change which candidates
/// exist; the *gate* applied to them must not change, and does not.
///
/// `raw_duplicate_symbols` are the ranked duplicate group names as the scan
/// produced them (`ctx.filtered_ranked` / `section.ranked_dups` — the same
/// list on both surfaces); the namesake/idiom class gate is applied here.
pub fn structural_defects(
    snapshot: &Snapshot,
    dead_candidates: &[DeadExport],
    raw_duplicate_symbols: &[String],
    cascade_imports: usize,
) -> StructuralDefects {
    // === Cycles: classified, not counted raw ===
    // `circular_imports` is every strict cycle the graph walker found. Only
    // the compilability classification tells breaking from structural from
    // diamond, and a diamond is not a defect.
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();
    let (strict_cycles, lazy_cycles) = find_cycles_with_lazy(&edges);
    let mut breaking_cycles = 0usize;
    let mut structural_cycles = 0usize;
    for nodes in strict_cycles {
        match ClassifiedCycle::new(nodes, &edges).classification {
            CycleClassification::HardBidirectional => breaking_cycles += 1,
            CycleClassification::FanPattern => {} // diamond — shared dependency, not a defect
            CycleClassification::ModuleSelfReference
            | CycleClassification::TraitBased
            | CycleClassification::CfgGated
            | CycleClassification::WildcardImport
            | CycleClassification::Unknown => structural_cycles += 1,
        }
    }
    // Lazy cycles are broken by dynamic import at runtime; they are listed,
    // never scored. (`findings.rs` maps them to a `"lazy"` entry which no
    // severity dimension reads.)
    let _ = lazy_cycles;

    // === Dead exports: sensor rows fenced down to defects ===
    let dead_exports = dead_candidates
        .iter()
        .filter(|candidate| counts_as_dead_defect(candidate))
        .count();
    // Name set for the twin-parrot cross-check below: a parrot scores only
    // where the graded detector already called that symbol dead.
    let scored_dead: HashSet<&str> = dead_candidates
        .iter()
        .filter(|candidate| counts_as_dead_defect(candidate))
        .map(|candidate| candidate.symbol.as_str())
        .collect();

    // === Twins: shape evidence gates both the SMELL and the HIGH dimension ===
    // W2-01: the same evidence that keeps a group out of `duplicate_groups`
    // keeps it out of the SMELL dimension. `text` on two SwiftUI views is a
    // name the framework dictates; scoring it as a smell scores the parser.
    let exact_twins =
        canonical_exact_twins(&snapshot.files, snapshot.semantic_facts.as_ref(), false);
    let omitted: HashSet<&str> = exact_twins
        .iter()
        .filter(|twin| omit_from_duplicate_groups(twin))
        .map(|twin| twin.name.as_str())
        .collect();

    let twins_same_language = exact_twins
        .iter()
        .filter(|twin| !omit_from_duplicate_groups(twin))
        .filter(|twin| matches!(categorize_twin(twin), TwinCategory::SameLanguage(_)))
        .count();

    // W2-01: HIGH must not answer "is this symbol unused" twice — once with
    // evidence and once without. `find_dead_parrots` knows only the import
    // count, and in a module-scoped language that number carries no signal:
    // Swift `import` is module-level, so every declaration inside the module
    // has zero import edges whether it is called on every line or never.
    // A twin parrot is scored only where the graded detector — which
    // cross-checks literal occurrences, applies semantic suppression and
    // fences entry points — independently calls it a defect.
    let twins_dead_parrots = find_dead_parrots(&snapshot.files, false, false)
        .dead_parrots
        .iter()
        .filter(|symbol| scored_dead.contains(symbol.name.as_str()))
        .count();

    // === Duplicate exports: namesake groups are not duplicates ===
    let duplicate_exports = raw_duplicate_symbols
        .iter()
        .filter(|symbol| !omitted.contains(symbol.as_str()))
        .count();

    // === Barrel chaos ===
    let barrel = analyze_barrel_chaos(snapshot);
    let barrel_chaos_count =
        barrel.missing_barrels.len() + barrel.deep_chains.len() + barrel.inconsistent_paths.len();

    StructuralDefects {
        breaking_cycles,
        structural_cycles,
        dead_exports,
        twins_dead_parrots,
        twins_same_language,
        barrel_chaos_count,
        duplicate_exports,
        cascade_imports,
        files: snapshot.canonical_file_count(),
        loc: snapshot.files.iter().map(|f| f.loc).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(symbol: &str, confidence: &str, entrypoint: bool) -> DeadExport {
        DeadExport {
            file: "src/lib.rs".to_string(),
            symbol: symbol.to_string(),
            line: Some(1),
            confidence: confidence.to_string(),
            reason: "test".to_string(),
            open_url: None,
            is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint,
        }
    }

    #[test]
    fn dead_gate_keeps_only_graded_non_entrypoint_candidates() {
        assert!(counts_as_dead_defect(&candidate("gone", "high", false)));
        assert!(counts_as_dead_defect(&candidate(
            "gone",
            "very-high",
            false
        )));
        assert!(!counts_as_dead_defect(&candidate("gone", "low", false)));
        assert!(!counts_as_dead_defect(&candidate("main", "high", true)));
    }

    /// The single biggest contributor to the 85-vs-72 split: `for_ai.rs` fed
    /// `section.circular_imports` — every strict cycle the walker found — into
    /// `breaking_cycles`. On this repository that was 16 CERTAIN issues where
    /// the classifier said 5. A cycle is only breaking when it is hard
    /// bidirectional; a fan/diamond is a shared dependency and not a defect at
    /// all, and the rest are compilable smells.
    #[test]
    fn cycles_are_classified_not_counted_raw() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        for path in ["a.ts", "b.ts", "x.ts", "y.ts", "z.ts"] {
            snapshot
                .files
                .push(crate::types::FileAnalysis::new(path.to_string()));
        }
        let mut edge = |from: &str, to: &str| {
            snapshot.edges.push(crate::snapshot::GraphEdge {
                from: from.to_string(),
                to: to.to_string(),
                label: "import".to_string(),
            });
        };
        // Hard bidirectional pair — the only genuine CERTAIN issue here.
        edge("a.ts", "b.ts");
        edge("b.ts", "a.ts");
        // Three-node ring — the classifier reads it as a fan/diamond: a shared
        // dependency, not a defect in any dimension.
        edge("x.ts", "y.ts");
        edge("y.ts", "z.ts");
        edge("z.ts", "x.ts");

        // Two strict cycles exist in this graph. Exactly one is a defect.
        let (strict, _lazy) = find_cycles_with_lazy(
            &snapshot
                .edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(strict.len(), 2, "the graph really does hold two cycles");

        let defects = structural_defects(&snapshot, &[], &[], 0);

        assert_eq!(
            defects.breaking_cycles,
            1,
            "only the hard bidirectional pair is breaking; charging all {} \
             strict cycles to CERTAIN is exactly the drift this module exists \
             to prevent",
            strict.len()
        );
        assert_eq!(
            defects.structural_cycles + defects.breaking_cycles,
            1,
            "the fan is not a defect in any dimension"
        );
    }

    #[test]
    fn metrics_leave_handler_dimensions_unscored() {
        let metrics = StructuralDefects {
            breaking_cycles: 2,
            dead_exports: 3,
            loc: 1_000,
            ..Default::default()
        }
        .metrics();
        assert_eq!(metrics.breaking_cycles, 2);
        assert_eq!(metrics.dead_exports, 3);
        assert_eq!(metrics.missing_handlers, 0);
        assert_eq!(metrics.unregistered_handlers, 0);
        assert_eq!(metrics.unused_high_confidence, 0);
    }

    fn twin_export_file(path: &str, symbol: &str) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            exports: vec![crate::types::ExportSymbol {
                name: symbol.to_string(),
                kind: "function".to_string(),
                export_type: "named".to_string(),
                line: Some(1),
                params: Vec::new(),
                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        }
    }

    /// F6 — `canonical_exact_twins` is the one computation every twin-reading
    /// surface must share, but `output::process_root_context` builds its
    /// report-scoped `Snapshot` before semantic facts exist in that stage of
    /// the pipeline (it sets `semantic_facts: None` explicitly). Routing that
    /// surface through `canonical_exact_twins(&analyses, None, false)` must
    /// not change its output: with no facts to filter idioms by, the shared
    /// helper must degrade to exactly the raw `detect_exact_twins` result —
    /// this pins that degrade-safely contract so the output.rs migration is
    /// behavior-preserving.
    #[test]
    fn canonical_exact_twins_with_no_facts_matches_raw_detect_exact_twins() {
        let files = vec![
            twin_export_file("src/a.rs", "shared_fn"),
            twin_export_file("src/b.rs", "shared_fn"),
        ];

        let raw = detect_exact_twins(&files, false);
        let canonical = canonical_exact_twins(&files, None, false);

        assert_eq!(canonical.len(), raw.len());
        assert_eq!(
            canonical.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            raw.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
        );
    }
    /// N13 (audit): every twins-reporting surface goes through
    /// [`canonical_exact_twins`]. A direct `detect_exact_twins(` call
    /// anywhere else re-opens the F6 drift this module exists to close.
    #[test]
    fn n13_no_surface_calls_detect_exact_twins_directly() {
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = ["analyzer/twins.rs", "analyzer/health_inputs.rs"];
        let mut offenders = Vec::new();
        for entry in walkdir::WalkDir::new(&src_root) {
            let entry = entry.expect("walk src");
            if !entry.file_type().is_file()
                || entry.path().extension().and_then(|e| e.to_str()) != Some("rs")
            {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&src_root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if allowed.iter().any(|a| rel == *a) {
                continue;
            }
            let content = std::fs::read_to_string(entry.path()).expect("read source");
            if content.contains("detect_exact_twins(") {
                offenders.push(rel);
            }
        }
        assert!(
            offenders.is_empty(),
            "surfaces must call canonical_exact_twins, not detect_exact_twins directly: {offenders:?}"
        );
    }
}
