use std::fs;
use std::io;
use std::path::Path;

use report_leptos::types::{ContextAtlasCardInfo, ContextAtlasInfo};

use super::ReportSection;
use super::assets::{
    COSE_BASE_JS, CYTOSCAPE_COSE_BILKENT_JS, CYTOSCAPE_DAGRE_JS, CYTOSCAPE_JS, DAGRE_JS,
    LAYOUT_BASE_JS,
};

/// Attempt to load a materialized Context Atlas pointer.
///
/// Atlas now always lives at `<repo_root>/.loctree/context-atlas/manifest.json`
/// (Plan 01 — atlas-per-repo). The `artifacts_dir` here is the directory
/// containing `report.html`, which may be either:
///   - `<repo_root>/.loctree/` itself (auto flow drops report next to atlas), or
///   - a global cache bucket (deprecated fallback — atlas not reachable).
///
/// Strategy: if artifacts_dir ends in `.loctree`, look directly; otherwise
/// walk ancestors searching for `<ancestor>/.loctree/context-atlas/manifest.json`.
/// Returns `None` when atlas not materialized or unreachable.
fn load_atlas_info(artifacts_dir: &Path) -> Option<ContextAtlasInfo> {
    let manifest_json = if artifacts_dir.ends_with(".loctree") {
        let candidate = artifacts_dir.join("context-atlas").join("manifest.json");
        if candidate.exists() {
            candidate
        } else {
            return None;
        }
    } else {
        artifacts_dir.ancestors().find_map(|ancestor| {
            let candidate = ancestor
                .join(".loctree")
                .join("context-atlas")
                .join("manifest.json");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })?
    };
    let content = fs::read_to_string(&manifest_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let cards = value
        .get("cards")
        .and_then(|c| c.as_array())
        .map(|cards| {
            cards
                .iter()
                .filter_map(|card| {
                    Some(ContextAtlasCardInfo {
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
        .unwrap_or_default();
    Some(ContextAtlasInfo {
        atlas_dir: value.get("atlas_dir")?.as_str()?.to_string(),
        manifest: value.get("manifest")?.as_str()?.to_string(),
        manifest_json: value.get("manifest_json")?.as_str()?.to_string(),
        recommended_start: value.get("recommended_start")?.as_str()?.to_string(),
        message: value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        cards,
    })
}

/// Render HTML report using Leptos SSR
pub(crate) fn render_html_report(path: &Path, sections: &[ReportSection]) -> io::Result<()> {
    // Only write JS assets if there's an actual parent directory (not empty path)
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        write_js_assets(dir)?;
    }

    // Convert loctree types to report-leptos types via JSON serialization
    // JSON bridge enables clean type separation between the analyzer and renderer
    let json = serde_json::to_string(sections).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to serialize sections: {}", e),
        )
    })?;

    let mut leptos_sections: Vec<report_leptos::types::ReportSection> = serde_json::from_str(&json)
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to deserialize to Leptos types: {}", e),
            )
        })?;

    // Attach materialized Context Atlas pointer (when `loct auto` produced
    // `<artifacts_dir>/context-atlas/manifest.json` next to the report).
    if let Some(atlas_info) = path.parent().and_then(load_atlas_info) {
        for section in &mut leptos_sections {
            section.context_atlas = Some(atlas_info.clone());
        }
    }

    // Configure JS asset paths (relative to output file)
    // These match the files written by write_js_assets below
    let js_assets = report_leptos::JsAssets {
        cytoscape_path: "loctree-cytoscape.min.js".into(),
        dagre_path: "loctree-dagre.min.js".into(),
        cytoscape_dagre_path: "loctree-cytoscape-dagre.js".into(),
        layout_base_path: "loctree-layout-base.js".into(),
        cose_base_path: "loctree-cose-base.js".into(),
        cytoscape_cose_bilkent_path: "loctree-cytoscape-cose-bilkent.js".into(),
        ..Default::default()
    };

    // Check if this project has Tauri command data
    let has_tauri = sections.iter().any(|s| {
        !s.missing_handlers.is_empty()
            || !s.unused_handlers.is_empty()
            || !s.unregistered_handlers.is_empty()
            || !s.command_bridges.is_empty()
            || s.command_counts.0 > 0
            || s.command_counts.1 > 0
    });

    let html = report_leptos::render_report(&leptos_sections, &js_assets, has_tauri);
    fs::write(path, html)
}

/// Write JS assets to output directory
fn write_js_assets(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    // Core Cytoscape library
    let js_path = dir.join("loctree-cytoscape.min.js");
    if !js_path.exists() {
        fs::write(&js_path, CYTOSCAPE_JS)?;
    }
    // Dagre layout library (dependency for cytoscape-dagre)
    let dagre_path = dir.join("loctree-dagre.min.js");
    if !dagre_path.exists() {
        fs::write(&dagre_path, DAGRE_JS)?;
    }
    // Cytoscape-dagre extension (hierarchical layout)
    let cy_dagre_path = dir.join("loctree-cytoscape-dagre.js");
    if !cy_dagre_path.exists() {
        fs::write(&cy_dagre_path, CYTOSCAPE_DAGRE_JS)?;
    }
    // layout-base (dependency for cose-base)
    let layout_base_path = dir.join("loctree-layout-base.js");
    if !layout_base_path.exists() {
        fs::write(&layout_base_path, LAYOUT_BASE_JS)?;
    }
    // cose-base (dependency for cytoscape-cose-bilkent)
    let cose_base_path = dir.join("loctree-cose-base.js");
    if !cose_base_path.exists() {
        fs::write(&cose_base_path, COSE_BASE_JS)?;
    }
    // Cytoscape-cose-bilkent extension (improved force-directed layout)
    let cy_cose_bilkent_path = dir.join("loctree-cytoscape-cose-bilkent.js");
    if !cy_cose_bilkent_path.exists() {
        fs::write(&cy_cose_bilkent_path, CYTOSCAPE_COSE_BILKENT_JS)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_html_report;
    use crate::analyzer::dist::{DeadBundleExport, DistAnalysisLevel, DistFileImpact, DistResult};
    use crate::analyzer::report::{AiInsight, DupSeverity, RankedDup, ReportSection};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn renders_basic_report() {
        let tmp_dir = tempdir().expect("tmp dir");
        let out_path = tmp_dir.path().join("report.html");

        let dup = RankedDup {
            name: "Foo".into(),
            files: vec!["a.ts".into(), "b.ts".into()],
            locations: vec![],
            score: 2,
            prod_count: 2,
            dev_count: 0,
            canonical: "a.ts".into(),
            canonical_line: None,
            refactors: vec!["b.ts".into()],
            severity: DupSeverity::SamePackage,
            is_cross_lang: false,
            packages: vec![],
            reason: String::new(),
        };

        let section = ReportSection {
            root: "test-root".into(),
            files_analyzed: 2,
            total_loc: 100,
            reexport_files_count: 1,
            dynamic_imports_count: 1,
            ranked_dups: vec![dup],
            cascades: vec![("a.ts".into(), "b.ts".into())],
            circular_imports: vec![],
            lazy_circular_imports: vec![],
            dynamic: vec![("dyn.ts".into(), vec!["./lazy".into()])],
            analyze_limit: 5,
            generated_at: None,
            schema_name: None,
            schema_version: None,
            loctree_version: None,
            missing_handlers: Vec::new(),
            unregistered_handlers: Vec::new(),
            unused_handlers: Vec::new(),
            command_counts: (0, 0),
            command_bridges: Vec::new(),
            open_base: None,
            tree: None,
            graph: None,
            graph_warning: None,
            insights: vec![AiInsight {
                title: "Hint".into(),
                severity: "medium".into(),
                message: "Message".into(),
            }],
            git_branch: None,
            git_commit: None,
            priority_tasks: Vec::new(),
            hub_files: Vec::new(),
            hotspots: Vec::new(),
            crowds: Vec::new(),
            dead_exports: Vec::new(),
            dist: Some(DistResult {
                src_dir: "src".into(),
                source_map_paths: vec!["dist/app.js.map".into()],
                source_maps: 1,
                source_exports: 2,
                bundled_exports: 1,
                dead_exports: vec![DeadBundleExport {
                    file: "src/b.ts".into(),
                    line: 12,
                    name: "Ghost".into(),
                    kind: "function".into(),
                }],
                reduction: "50%".into(),
                symbol_level: true,
                analysis_level: DistAnalysisLevel::Symbol,
                tree_shaken_exports: 1,
                tree_shaken_pct: 50,
                coverage_pct: 50,
                impacted_files: vec![DistFileImpact {
                    file: "src/b.ts".into(),
                    source_exports: 1,
                    bundled_exports: 0,
                    tree_shaken_exports: 1,
                    status: "fully-shaken".into(),
                }],
                chunks: Vec::new(),
                candidate_counts: std::collections::BTreeMap::new(),
                candidates: Vec::new(),
            }),
            twins_data: None,
            coverage_gaps: Vec::new(),
            health_score: None,
            refactor_plan: None,
            context_atlas: None,
        };

        render_html_report(&out_path, &[section]).expect("render html");
        let html = fs::read_to_string(&out_path).expect("read html");

        // Verify key parts exist in the Leptos-rendered output
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Loctree Report")); // Title in new example-app design

        // The output format might differ slightly from legacy, check for content
        assert!(html.contains("Hint"));
        assert!(html.contains("Foo"));
        assert!(html.contains("test-root"));
        assert!(html.contains("Bundle distribution"));
        assert!(html.contains("Ghost"));
    }

    #[test]
    fn escapes_html_entities() {
        let tmp_dir = tempdir().expect("tmp dir");
        let out_path = tmp_dir.path().join("report.html");
        let malicious = r#"<script>alert('x')</script>"#;
        let section = ReportSection {
            root: malicious.into(),
            files_analyzed: 0,
            total_loc: 0,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: Vec::new(),
            cascades: Vec::new(),
            circular_imports: Vec::new(),
            lazy_circular_imports: Vec::new(),
            dynamic: Vec::new(),
            analyze_limit: 1,
            generated_at: None,
            schema_name: None,
            schema_version: None,
            loctree_version: None,
            missing_handlers: Vec::new(),
            unregistered_handlers: Vec::new(),
            unused_handlers: Vec::new(),
            command_counts: (0, 0),
            command_bridges: Vec::new(),
            open_base: None,
            tree: None,
            graph: None,
            graph_warning: None,
            insights: Vec::new(),
            git_branch: None,
            git_commit: None,
            priority_tasks: Vec::new(),
            hub_files: Vec::new(),
            hotspots: Vec::new(),
            crowds: Vec::new(),
            dead_exports: Vec::new(),
            dist: None,
            twins_data: None,
            coverage_gaps: Vec::new(),
            health_score: None,
            refactor_plan: None,
            context_atlas: None,
        };

        render_html_report(&out_path, &[section]).expect("render html");
        let html = fs::read_to_string(&out_path).expect("read html");

        // Security: raw script must not appear
        assert!(
            !html.contains(malicious),
            "XSS: raw script tag should be escaped"
        );

        // Leptos escapes content automatically
        // We check that both opening and closing tags are safely escaped
        assert!(html.contains("&lt;script&gt;") && html.contains("&lt;/script&gt;"));
    }
}
