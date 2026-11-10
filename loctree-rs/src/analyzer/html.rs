use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use report_leptos::types::{ContextAtlasCardInfo, ContextAtlasInfo};

use super::ReportSection;
use super::assets::{
    COSE_BASE_JS, CYTOSCAPE_COSE_BILKENT_JS, CYTOSCAPE_DAGRE_JS, CYTOSCAPE_JS, DAGRE_JS,
    LAYOUT_BASE_JS,
};

/// Resolve HTML Atlas identity solely from the section's explicit source root.
/// Output location and manifest-supplied display paths are not authorities.
/// This projection uses absolute, confined paths without changing the on-disk
/// manifest or the analyzer's public report payload.
fn load_atlas_info(source_root: &Path) -> Option<ContextAtlasInfo> {
    if source_root.as_os_str().is_empty() {
        return None;
    }
    let source_root = source_root.canonicalize().ok()?;
    let atlas_dir = source_root
        .join(".loctree/context-atlas")
        .canonicalize()
        .ok()?;
    if !atlas_dir.starts_with(&source_root) {
        return None;
    }
    let manifest_json = resolve_atlas_path(&atlas_dir, Path::new("manifest.json"))?;
    let content = fs::read_to_string(&manifest_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    // Older manifests have no project field; their exact source-local location
    // remains the identity. Reject a supplied identity for any other repository.
    if let Some(project) = value.get("project") {
        let project = Path::new(project.as_str()?);
        // A manifest identity must stand on its own, independent of report cwd.
        if !project.is_absolute() || project.canonicalize().ok()? != source_root {
            return None;
        }
    }
    let cards: Vec<ContextAtlasCardInfo> = value
        .get("cards")
        .and_then(|c| c.as_array())
        .map(|cards| {
            cards
                .iter()
                .filter_map(|card| {
                    let path = card.get("path")?.as_str()?.to_string();
                    // The manifest's `lines` value is frozen at materialization
                    // time and drifts once cards are regenerated. The card file
                    // is the truth — read it, count for real, and embed the
                    // body so the static report can open the card in-page.
                    // The manifest is data, not authority: a pre-seeded or
                    // malicious manifest could point `path` at any file on disk
                    // and the report would embed it. Only read card files that
                    // resolve INSIDE the context-atlas directory.
                    let card_file = Path::new(&path);
                    let resolved = resolve_atlas_path(&atlas_dir, card_file);
                    let body = read_card_within(&atlas_dir, card_file);
                    let lines = body.as_ref().map(|b| b.lines().count()).unwrap_or_else(|| {
                        card.get("lines").and_then(|v| v.as_u64()).unwrap_or(0) as usize
                    });
                    Some(ContextAtlasCardInfo {
                        id: card.get("id")?.as_str()?.to_string(),
                        title: card.get("title")?.as_str()?.to_string(),
                        path: resolved
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        lines,
                        why: card
                            .get("why")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        body,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let recommended_start = value
        .get("recommended_start")
        .and_then(|path| path.as_str())
        .and_then(|path| resolve_atlas_path(&atlas_dir, Path::new(path)))
        .map(|path| path.display().to_string())
        .or_else(|| {
            cards
                .iter()
                .find(|card| !card.path.is_empty())
                .map(|card| card.path.clone())
        })
        .unwrap_or_default();
    Some(ContextAtlasInfo {
        manifest: resolve_atlas_path(&atlas_dir, Path::new("manifest.md"))
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        atlas_dir: atlas_dir.display().to_string(),
        manifest_json: manifest_json.display().to_string(),
        recommended_start,
        message: value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        cards,
    })
}

/// One confinement boundary for both embedded bodies and copied HTML paths.
fn resolve_atlas_path(atlas_dir: &Path, candidate: &Path) -> Option<PathBuf> {
    let base = atlas_dir.canonicalize().ok()?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
    let resolved = joined.canonicalize().ok()?;
    if resolved.starts_with(&base) && resolved.is_file() {
        Some(resolved)
    } else {
        None
    }
}

/// Read a card body only when the candidate path resolves inside the
/// context-atlas directory. Absolute, symlink or `..` escapes yield `None`.
fn read_card_within(atlas_dir: &Path, candidate: &Path) -> Option<String> {
    fs::read_to_string(resolve_atlas_path(atlas_dir, candidate)?).ok()
}

/// Render HTML report using Leptos SSR
pub(crate) fn render_html_report(path: &Path, sections: &[ReportSection]) -> io::Result<()> {
    // Complete filesystem admission from the original source sections before
    // entering the JSON presentation bridge. Decoded renderer data is never a
    // source of filesystem authority; it receives only these typed projections.
    let source_atlases: Vec<Option<ContextAtlasInfo>> = sections
        .iter()
        .map(|section| load_atlas_info(Path::new(&section.root)))
        .collect();

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

    // Each section owns its Atlas, including explicit absence. Never retain an
    // unverified pointer supplied by the JSON bridge or borrow an output ancestor.
    for (atlas, section) in source_atlases.into_iter().zip(&mut leptos_sections) {
        section.context_atlas = atlas;
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

    // Check if this project has a Tauri backend or configuration (from manifest/config)
    let has_tauri = detect_tauri_for_report(path, sections);

    let html = report_leptos::render_report(&leptos_sections, &js_assets, has_tauri);
    fs::write(path, html)
}

/// Check if a Cargo.toml file declares `tauri` or `tauri-build` as a dependency.
fn cargo_toml_has_tauri(cargo_path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(cargo_path) else {
        return false;
    };
    if let Ok(toml_val) = toml::from_str::<toml::Value>(&content)
        && toml_contains_tauri_dependency(&toml_val)
    {
        return true;
    }
    // Fallback line check in case of custom or invalid TOML syntax
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "[dependencies.tauri]"
            || trimmed == "[dev-dependencies.tauri]"
            || trimmed == "[build-dependencies.tauri]"
            || trimmed.starts_with("[dependencies.tauri.")
            || (trimmed.starts_with("tauri")
                && trimmed.split_once('=').is_some_and(|(k, _)| {
                    let k = k.trim();
                    k == "tauri" || k == "tauri-build"
                }))
    })
}

fn toml_contains_tauri_dependency(val: &toml::Value) -> bool {
    let Some(table) = val.as_table() else {
        return false;
    };
    for dep_key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(deps) = table.get(dep_key).and_then(|v| v.as_table())
            && (deps.contains_key("tauri") || deps.contains_key("tauri-build"))
        {
            return true;
        }
    }
    if let Some(ws) = table.get("workspace").and_then(|v| v.as_table())
        && let Some(deps) = ws.get("dependencies").and_then(|v| v.as_table())
        && (deps.contains_key("tauri") || deps.contains_key("tauri-build"))
    {
        return true;
    }
    if let Some(targets) = table.get("target").and_then(|v| v.as_table()) {
        for (_target_name, target_val) in targets {
            if let Some(target_table) = target_val.as_table() {
                for dep_key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(deps) = target_table.get(dep_key).and_then(|v| v.as_table())
                        && (deps.contains_key("tauri") || deps.contains_key("tauri-build"))
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check if a package.json file declares `@tauri-apps/*` dependencies.
fn package_json_has_tauri(pkg_path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(pkg_path) else {
        return false;
    };
    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&content) {
        for key in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(deps) = json_val.get(key).and_then(|v| v.as_object())
                && (deps.contains_key("@tauri-apps/api")
                    || deps.contains_key("@tauri-apps/cli")
                    || deps.keys().any(|k| k.starts_with("@tauri-apps/")))
            {
                return true;
            }
        }
    }
    false
}

/// Detect whether a directory represents a Tauri project by inspecting its
/// manifests and configurations (tauri.conf.json, src-tauri, Cargo.toml with
/// tauri dependency, or package.json with @tauri-apps).
pub(crate) fn is_tauri_project_dir(raw_root: &Path) -> bool {
    let root = if raw_root.is_file() {
        raw_root.parent().unwrap_or(raw_root)
    } else {
        raw_root
    };

    if !root.exists() {
        return false;
    }

    if root.file_name().is_some_and(|n| n == "src-tauri") {
        return true;
    }

    // 1. Direct tauri config files
    if root.join("tauri.conf.json").is_file()
        || root.join("tauri.conf.json5").is_file()
        || root.join("Tauri.toml").is_file()
    {
        return true;
    }

    // 2. src-tauri directory (standard Tauri layout)
    let src_tauri = root.join("src-tauri");
    if src_tauri.is_dir() {
        return true;
    }
    if src_tauri.join("tauri.conf.json").is_file()
        || src_tauri.join("tauri.conf.json5").is_file()
        || src_tauri.join("Tauri.toml").is_file()
    {
        return true;
    }
    if cargo_toml_has_tauri(&src_tauri.join("Cargo.toml")) {
        return true;
    }

    // 3. Cargo.toml in root
    if cargo_toml_has_tauri(&root.join("Cargo.toml")) {
        return true;
    }

    // 4. package.json in root
    if package_json_has_tauri(&root.join("package.json")) {
        return true;
    }

    // 5. Monorepo / workspace direct subdirectories check (1 level down)
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str == "node_modules"
                    || name_str == "target"
                    || name_str == ".git"
                    || name_str == ".loctree"
                    || name_str == "dist"
                    || name_str == "build"
                {
                    continue;
                }
                if p.join("tauri.conf.json").is_file()
                    || p.join("tauri.conf.json5").is_file()
                    || p.join("Tauri.toml").is_file()
                    || p.join("src-tauri").is_dir()
                    || cargo_toml_has_tauri(&p.join("Cargo.toml"))
                    || cargo_toml_has_tauri(&p.join("src-tauri").join("Cargo.toml"))
                    || package_json_has_tauri(&p.join("package.json"))
                {
                    return true;
                }
            }
        }
    }

    false
}

/// Determine whether this report covers a Tauri project by checking the
/// analyzed section roots and report directory for Tauri manifests and configs.
///
/// Unlike earlier versions (which derived `has_tauri` from non-empty gap sections,
/// creating a circular failure mode where clean Tauri projects lacked the Tauri tab
/// and non-Tauri projects with gaps falsely got the gate), this detection relies
/// strictly on manifest and configuration ground truth.
pub(crate) fn detect_tauri_for_report(path: &Path, sections: &[ReportSection]) -> bool {
    // 1. Check sections root directories
    for s in sections {
        if !s.root.is_empty() {
            let root = Path::new(&s.root);
            if root.exists() {
                if is_tauri_project_dir(root) {
                    return true;
                }
                // If s.root is ".loctree" inside the repo, check its parent repo root
                if root.file_name().is_some_and(|n| n == ".loctree")
                    && let Some(parent) = root.parent()
                    && is_tauri_project_dir(parent)
                {
                    return true;
                }
            }
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                let joined = parent.join(root);
                if joined.exists() && is_tauri_project_dir(&joined) {
                    return true;
                }
            }
        }
    }

    // 2. Check path.parent() (e.g. if path is <root>/report.html or <root>/.loctree/report.html)
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        if is_tauri_project_dir(parent) {
            return true;
        }
        // If report is inside `<repo>/.loctree/report.html`, check the repo root
        if parent.file_name().is_some_and(|n| n == ".loctree")
            && let Some(repo_root) = parent.parent()
            && is_tauri_project_dir(repo_root)
        {
            return true;
        }
    }

    false
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

    fn atlas_test_section(root: &std::path::Path) -> ReportSection {
        ReportSection {
            root: root.display().to_string(),
            files_analyzed: 1,
            total_loc: 1,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: Vec::new(),
            cascades: Vec::new(),
            circular_imports: Vec::new(),
            lazy_circular_imports: Vec::new(),
            dynamic: Vec::new(),
            analyze_limit: 0,
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
        }
    }

    fn write_test_atlas(root: &std::path::Path, canary: &str) -> std::path::PathBuf {
        let atlas = root.join(".loctree/context-atlas");
        fs::create_dir_all(&atlas).expect("create atlas");
        let atlas = atlas.canonicalize().expect("canonical atlas");
        fs::write(atlas.join("00-core-map.md"), format!("# {canary}\nbody\n")).expect("write card");
        fs::write(atlas.join("manifest.md"), "# Atlas\n").expect("write manifest");
        fs::write(
            atlas.join("manifest.json"),
            serde_json::json!({
                "project": root.canonicalize().expect("canonical root"),
                "atlas_dir": atlas,
                "manifest": atlas.join("manifest.md"),
                "manifest_json": atlas.join("manifest.json"),
                "recommended_start": atlas.join("00-core-map.md"),
                "message": canary,
                "cards": [{
                    "id": "core", "title": canary, "path": "00-core-map.md",
                    "lines": 999, "why": "source identity regression"
                }]
            })
            .to_string(),
        )
        .expect("write atlas json");
        atlas
    }

    #[test]
    fn atlas_relocated_report_uses_source_not_foreign_output_ancestor() {
        let source = tempdir().expect("source");
        let foreign = tempdir().expect("foreign output");
        let source_atlas = write_test_atlas(source.path(), "SOURCE_A_CANARY");
        write_test_atlas(foreign.path(), "FOREIGN_B_CANARY");
        let output = foreign.path().join("elsewhere/report.html");
        fs::create_dir_all(output.parent().expect("output parent")).expect("output dir");
        render_html_report(&output, &[atlas_test_section(source.path())]).expect("render");
        let html = fs::read_to_string(output).expect("html");
        assert!(html.contains("# SOURCE_A_CANARY\nbody\n"));
        assert!(!html.contains("FOREIGN_B_CANARY"));
        for target in ["00-core-map.md", "manifest.md"] {
            assert!(html.contains(&format!(
                "data-copy=\"{}\"",
                source_atlas.join(target).display()
            )));
        }
        assert!(html.contains(&format!("data-copy=\"{}\"", source_atlas.display())));
        assert!(html.contains("2 lines"));
        assert!(!html.contains("data-copy=\"00-core-map.md\""));
    }

    #[test]
    fn atlas_absent_source_never_borrows_output_ancestor() {
        let source = tempdir().expect("source");
        let foreign = tempdir().expect("foreign output");
        write_test_atlas(foreign.path(), "FOREIGN_ONLY_CANARY");
        let output = foreign.path().join("report.html");
        render_html_report(&output, &[atlas_test_section(source.path())]).expect("render");
        let html = fs::read_to_string(output).expect("html");
        assert!(!html.contains("FOREIGN_ONLY_CANARY"));
        assert!(!html.contains("Context Atlas Ready"));
        assert!(html.contains("6 cards · template"));
    }

    #[test]
    fn atlas_multiroot_sections_keep_independent_source_identity() {
        let first = tempdir().expect("first");
        let second = tempdir().expect("second");
        let absent = tempdir().expect("absent");
        let first_atlas = write_test_atlas(first.path(), "FIRST_REPO_CANARY");
        let second_atlas = write_test_atlas(second.path(), "SECOND_REPO_CANARY");
        let output = first.path().join("report.html");
        render_html_report(
            &output,
            &[
                atlas_test_section(first.path()),
                atlas_test_section(second.path()),
                atlas_test_section(absent.path()),
            ],
        )
        .expect("render");
        let html = fs::read_to_string(output).expect("html");
        assert!(html.contains("# FIRST_REPO_CANARY\nbody\n"));
        assert!(html.contains("# SECOND_REPO_CANARY\nbody\n"));
        assert_eq!(html.matches("class=\"context-atlas-panel\"").count(), 2);
        for atlas in [first_atlas, second_atlas] {
            assert!(html.contains(&format!(
                "data-copy=\"{}\"",
                atlas.join("00-core-map.md").display()
            )));
        }
    }

    #[test]
    fn atlas_rejects_mismatched_or_invalid_manifest_identity() {
        let source = tempdir().expect("source");
        let other = tempdir().expect("other source");
        let atlas = write_test_atlas(source.path(), "WRONG_IDENTITY_CANARY");
        let manifest = atlas.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest).expect("manifest"))
                .expect("manifest json");
        for identity in [
            serde_json::json!(other.path()),
            serde_json::json!("."),
            serde_json::Value::Null,
        ] {
            value["project"] = identity;
            fs::write(&manifest, value.to_string()).expect("mismatched manifest");
            assert!(super::load_atlas_info(source.path()).is_none());
            let output = source.path().join("report.html");
            render_html_report(&output, &[atlas_test_section(source.path())]).expect("render");
            let html = fs::read_to_string(output).expect("html");
            assert!(!html.contains("WRONG_IDENTITY_CANARY"));
            assert!(!html.contains(&format!("data-copy=\"{}", atlas.display())));
        }
        assert!(super::load_atlas_info(std::path::Path::new("")).is_none());
    }

    #[test]
    fn atlas_json_presentation_payload_cannot_supply_filesystem_authority() {
        let source = tempdir().expect("source");
        let output_dir = tempdir().expect("output");
        let mut section = atlas_test_section(source.path());
        section.context_atlas = Some(super::super::report::ContextAtlasInfo {
            atlas_dir: "/unadmitted/atlas".into(),
            manifest: "/unadmitted/manifest.md".into(),
            manifest_json: "/unadmitted/manifest.json".into(),
            recommended_start: "/unadmitted/card.md".into(),
            message: "FORGED_PRESENTATION_CANARY".into(),
            cards: Vec::new(),
        });
        let output = output_dir.path().join("report.html");
        render_html_report(&output, std::slice::from_ref(&section)).expect("absent atlas");
        let html = fs::read_to_string(&output).expect("html");
        assert!(!html.contains("FORGED_PRESENTATION_CANARY"));
        assert!(!html.contains("/unadmitted/"));

        let atlas = write_test_atlas(source.path(), "REAL_SOURCE_CANARY");
        render_html_report(&output, &[section]).expect("source-local atlas");
        let html = fs::read_to_string(output).expect("html");
        assert!(html.contains("# REAL_SOURCE_CANARY\nbody\n"));
        assert!(html.contains(&format!("data-copy=\"{}\"", atlas.display())));
        assert!(!html.contains("FORGED_PRESENTATION_CANARY"));
        assert!(!html.contains("/unadmitted/"));
    }

    #[test]
    fn atlas_display_paths_cannot_redirect_admitted_source() {
        let source = tempdir().expect("source");
        let foreign = tempdir().expect("foreign");
        let atlas = write_test_atlas(source.path(), "ADMITTED_SOURCE_CANARY");
        let foreign_atlas = write_test_atlas(foreign.path(), "FOREIGN_DISPLAY_CANARY");
        let manifest = atlas.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest).expect("manifest"))
                .expect("manifest json");
        for field in [
            "atlas_dir",
            "manifest",
            "manifest_json",
            "recommended_start",
        ] {
            value[field] = serde_json::json!(foreign_atlas.join("00-core-map.md"));
        }
        fs::write(&manifest, value.to_string()).expect("poisoned display paths");
        let info = super::load_atlas_info(source.path()).expect("admitted source");
        assert_eq!(info.atlas_dir, atlas.display().to_string());
        assert_eq!(
            info.manifest,
            atlas.join("manifest.md").display().to_string()
        );
        assert_eq!(info.manifest_json, manifest.display().to_string());
        assert_eq!(info.recommended_start, info.cards[0].path);
        assert_eq!(
            info.cards[0].body.as_deref(),
            Some("# ADMITTED_SOURCE_CANARY\nbody\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn atlas_symlink_escapes_and_unavailable_paths_have_no_body_or_copy_payload() {
        use std::os::unix::fs::symlink;

        let source = tempdir().expect("source");
        let foreign = tempdir().expect("foreign");
        let atlas = write_test_atlas(source.path(), "LOCAL_CARD");
        let foreign_atlas = write_test_atlas(foreign.path(), "FOREIGN_LINK_CANARY");
        let manifest = atlas.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest).expect("manifest"))
                .expect("manifest json");
        symlink(
            foreign_atlas.join("00-core-map.md"),
            atlas.join("escape.md"),
        )
        .expect("escaping card symlink");
        fs::remove_file(atlas.join("manifest.md")).expect("remove manifest markdown");
        symlink(foreign_atlas.join("manifest.md"), atlas.join("manifest.md"))
            .expect("escaping manifest symlink");
        for path in ["escape.md", "../../missing.md", "absent.md", "."] {
            value["cards"][0]["path"] = serde_json::json!(path);
            value["recommended_start"] = serde_json::json!(path);
            fs::write(&manifest, value.to_string()).expect("invalid pointers");
            let info = super::load_atlas_info(source.path()).expect("local atlas");
            assert!(info.cards[0].body.is_none());
            assert!(info.cards[0].path.is_empty());
            assert!(info.manifest.is_empty());
            assert!(info.recommended_start.is_empty());
            let output = source.path().join("report.html");
            render_html_report(&output, &[atlas_test_section(source.path())]).expect("render");
            let html = fs::read_to_string(output).expect("html");
            assert!(!html.contains("FOREIGN_LINK_CANARY"));
            assert!(
                !html.contains("data-copy=\"\""),
                "empty copy targets: {:?}",
                html.match_indices("data-copy=\"\"")
                    .map(|(offset, _)| &html[offset.saturating_sub(80)..offset + 80])
                    .collect::<Vec<_>>()
            );
            assert!(html.contains("disabled title=\"Copy card path\""));
        }
        // A symlinked manifest JSON cannot import an otherwise valid foreign Atlas.
        fs::remove_file(&manifest).expect("remove manifest json");
        symlink(foreign_atlas.join("manifest.json"), &manifest).expect("manifest json link");
        assert!(super::load_atlas_info(source.path()).is_none());

        // Nor can the entire source-local Atlas directory point outside the source.
        let no_atlas = tempdir().expect("source without atlas");
        fs::create_dir(no_atlas.path().join(".loctree")).expect("local loctree");
        symlink(
            &foreign_atlas,
            no_atlas.path().join(".loctree/context-atlas"),
        )
        .expect("atlas directory link");
        assert!(super::load_atlas_info(no_atlas.path()).is_none());
    }

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

    #[test]
    fn atlas_card_lines_come_from_the_file_not_the_manifest() {
        // The manifest freezes `lines` at materialization time; cards can be
        // regenerated afterwards by other flows. The card file is the truth
        // the report must render — and its body must be embedded so the
        // static page can open the card without filesystem access.
        let tmp = tempfile::tempdir().unwrap();
        let loctree_dir = tmp.path().join(".loctree");
        let atlas_dir = loctree_dir.join("context-atlas");
        std::fs::create_dir_all(&atlas_dir).unwrap();
        std::fs::write(
            atlas_dir.join("00-core-map.md"),
            "# Core Map\nline2\nline3\nline4\nline5\nline6\nline7\n",
        )
        .unwrap();
        std::fs::write(
            atlas_dir.join("manifest.json"),
            serde_json::json!({
                "atlas_dir": atlas_dir.display().to_string(),
                "manifest": atlas_dir.join("manifest.md").display().to_string(),
                "manifest_json": atlas_dir.join("manifest.json").display().to_string(),
                "recommended_start": atlas_dir.join("00-core-map.md").display().to_string(),
                "message": "test atlas",
                "cards": [{
                    "id": "core",
                    "title": "Core Map",
                    "path": "00-core-map.md",
                    "lines": 999,
                    "why": "test"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let info = super::load_atlas_info(tmp.path()).expect("atlas attached");
        assert_eq!(info.cards.len(), 1);
        // 999 is the stale manifest lie; 7 is what the file actually holds.
        assert_eq!(info.cards[0].lines, 7);
        let body = info.cards[0].body.as_deref().expect("body embedded");
        assert!(body.contains("# Core Map"));

        // Unreadable card: keep the manifest count, ship no body.
        std::fs::remove_file(atlas_dir.join("00-core-map.md")).unwrap();
        let info = super::load_atlas_info(tmp.path()).expect("atlas attached");
        assert_eq!(info.cards[0].lines, 999);
        assert!(info.cards[0].body.is_none());
        assert!(info.cards[0].path.is_empty());
        assert!(info.recommended_start.is_empty());
    }

    #[test]
    fn atlas_card_paths_escaping_the_atlas_dir_are_never_embedded() {
        let tmp = tempfile::tempdir().unwrap();
        let loctree_dir = tmp.path().join(".loctree");
        let atlas_dir = loctree_dir.join("context-atlas");
        std::fs::create_dir_all(&atlas_dir).unwrap();

        // A secret OUTSIDE the atlas dir that a poisoned manifest points at,
        // once absolutely and once via `..` traversal.
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, "s3cr3t").unwrap();

        std::fs::write(
            atlas_dir.join("manifest.json"),
            serde_json::json!({
                "atlas_dir": atlas_dir.display().to_string(),
                "manifest": atlas_dir.join("manifest.md").display().to_string(),
                "manifest_json": atlas_dir.join("manifest.json").display().to_string(),
                "recommended_start": atlas_dir.join("00-core-map.md").display().to_string(),
                "message": "poisoned atlas",
                "cards": [
                    {
                        "id": "abs",
                        "title": "Absolute escape",
                        "path": secret.display().to_string(),
                        "lines": 1,
                        "why": "attack"
                    },
                    {
                        "id": "rel",
                        "title": "Dotdot escape",
                        "path": "../../secret.txt",
                        "lines": 1,
                        "why": "attack"
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let info = super::load_atlas_info(tmp.path()).expect("atlas attached");
        assert_eq!(info.cards.len(), 2);
        for card in &info.cards {
            assert!(card.path.is_empty(), "escaping paths must not be copied");
            assert!(
                card.body.is_none(),
                "card '{}' escaped the atlas dir and got embedded",
                card.id
            );
        }
    }

    #[test]
    fn w2_06_fixture_tauri_conf_zero_gaps_shows_tauri_surface() {
        // Acceptance fixture 1: projekt z tauri.conf.json i zero gapów → Tauri surface widoczna
        let tauri_tmp = tempdir().expect("tauri tmp dir");
        let tauri_root = tauri_tmp.path();
        fs::write(
            tauri_root.join("tauri.conf.json"),
            r#"{"build":{},"tauri":{"bundle":{"identifier":"com.test.app"}}}"#,
        )
        .expect("write tauri.conf.json");
        let tauri_out = tauri_root.join("report.html");

        let tauri_section = ReportSection {
            root: tauri_root.display().to_string(),
            files_analyzed: 10,
            total_loc: 500,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: Vec::new(),
            cascades: Vec::new(),
            circular_imports: Vec::new(),
            lazy_circular_imports: Vec::new(),
            dynamic: Vec::new(),
            analyze_limit: 10,
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

        let tauri_sections = [tauri_section];
        render_html_report(&tauri_out, &tauri_sections).expect("render tauri report");
        let tauri_html = fs::read_to_string(&tauri_out).expect("read tauri html");
        let tauri_detected = super::detect_tauri_for_report(&tauri_out, &tauri_sections);

        assert!(
            tauri_detected,
            "tauri-clean fixture must detect Tauri from tauri.conf.json"
        );
        assert!(
            tauri_html.contains("Tauri coverage"),
            "tauri-clean fixture with zero gaps must display Tauri surface in HTML"
        );
        assert!(
            tauri_html.contains("data-tab=\"commands\""),
            "tauri-clean fixture must include commands tab"
        );
    }

    #[test]
    fn w2_06_fixture_nontauri_repo_with_gaps_has_no_tauri_gate() {
        use crate::analyzer::report::CommandGap;

        // Acceptance fixture 2: non-Tauri repo z gapami → brak Tauri gate
        let nontauri_tmp = tempdir().expect("nontauri tmp dir");
        let nontauri_root = nontauri_tmp.path();
        fs::write(
            nontauri_root.join("package.json"),
            r#"{"name":"nontauri-app","dependencies":{"react":"^18.0.0"}}"#,
        )
        .expect("write package.json");
        let nontauri_out = nontauri_root.join("report.html");

        let nontauri_section = ReportSection {
            root: nontauri_root.display().to_string(),
            files_analyzed: 5,
            total_loc: 250,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: Vec::new(),
            cascades: Vec::new(),
            circular_imports: Vec::new(),
            lazy_circular_imports: Vec::new(),
            dynamic: Vec::new(),
            analyze_limit: 10,
            generated_at: None,
            schema_name: None,
            schema_version: None,
            loctree_version: None,
            missing_handlers: vec![
                CommandGap {
                    name: "reattach-workspace".to_string(),
                    implementation_name: None,
                    locations: vec![("src/frontend.js".to_string(), 42)],
                    confidence: None,
                    string_literal_matches: vec![],
                },
                CommandGap {
                    name: "seek-to-timestamp".to_string(),
                    implementation_name: None,
                    locations: vec![("src/frontend.js".to_string(), 60)],
                    confidence: None,
                    string_literal_matches: vec![],
                },
            ],
            unregistered_handlers: Vec::new(),
            unused_handlers: Vec::new(),
            command_counts: (2, 0),
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

        let nontauri_sections = [nontauri_section];
        render_html_report(&nontauri_out, &nontauri_sections).expect("render nontauri report");
        let nontauri_html = fs::read_to_string(&nontauri_out).expect("read nontauri html");
        let nontauri_detected = super::detect_tauri_for_report(&nontauri_out, &nontauri_sections);

        assert!(
            !nontauri_detected,
            "nontauri-gaps fixture must not detect Tauri when no manifest exists"
        );
        assert!(
            !nontauri_html.contains("Tauri coverage"),
            "nontauri-gaps fixture must NOT display Tauri surface in HTML"
        );
        assert!(
            !nontauri_html.contains("data-tab=\"commands\""),
            "nontauri-gaps fixture must NOT include commands tab"
        );
    }

    #[test]
    fn w2_06_tauri_detection_not_derived_from_gaps() {
        use crate::analyzer::report::CommandGap;

        // Fixture 1: Tauri project with tauri.conf.json and ZERO gaps
        let tauri_tmp = tempdir().expect("tauri tmp dir");
        let tauri_root = tauri_tmp.path();
        fs::write(
            tauri_root.join("tauri.conf.json"),
            r#"{"build":{},"tauri":{"bundle":{"identifier":"com.test.app"}}}"#,
        )
        .expect("write tauri.conf.json");
        let tauri_out = tauri_root.join("report.html");

        let tauri_section = ReportSection {
            root: tauri_root.display().to_string(),
            files_analyzed: 10,
            total_loc: 500,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: Vec::new(),
            cascades: Vec::new(),
            circular_imports: Vec::new(),
            lazy_circular_imports: Vec::new(),
            dynamic: Vec::new(),
            analyze_limit: 10,
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

        let tauri_sections = [tauri_section];

        // Render report for tauri-clean fixture
        render_html_report(&tauri_out, &tauri_sections).expect("render tauri report");
        let tauri_html = fs::read_to_string(&tauri_out).expect("read tauri html");

        let tauri_detected = super::detect_tauri_for_report(&tauri_out, &tauri_sections);
        assert!(
            tauri_detected,
            "tauri-clean fixture must detect Tauri from tauri.conf.json"
        );
        assert!(
            tauri_html.contains("Tauri coverage"),
            "tauri-clean fixture with zero gaps must display Tauri surface in HTML"
        );
        assert!(
            tauri_html.contains("data-tab=\"commands\""),
            "tauri-clean fixture must include commands tab"
        );

        // Fixture 2: Non-Tauri repository WITH gap data (e.g. custom JS event false-positive)
        let nontauri_tmp = tempdir().expect("nontauri tmp dir");
        let nontauri_root = nontauri_tmp.path();
        fs::write(
            nontauri_root.join("package.json"),
            r#"{"name":"nontauri-app","dependencies":{"react":"^18.0.0"}}"#,
        )
        .expect("write package.json");
        let nontauri_out = nontauri_root.join("report.html");

        let nontauri_section = ReportSection {
            root: nontauri_root.display().to_string(),
            files_analyzed: 5,
            total_loc: 250,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: Vec::new(),
            cascades: Vec::new(),
            circular_imports: Vec::new(),
            lazy_circular_imports: Vec::new(),
            dynamic: Vec::new(),
            analyze_limit: 10,
            generated_at: None,
            schema_name: None,
            schema_version: None,
            loctree_version: None,
            missing_handlers: vec![
                CommandGap {
                    name: "reattach-workspace".to_string(),
                    implementation_name: None,
                    locations: vec![("src/frontend.js".to_string(), 42)],
                    confidence: None,
                    string_literal_matches: vec![],
                },
                CommandGap {
                    name: "seek-to-timestamp".to_string(),
                    implementation_name: None,
                    locations: vec![("src/frontend.js".to_string(), 60)],
                    confidence: None,
                    string_literal_matches: vec![],
                },
            ],
            unregistered_handlers: Vec::new(),
            unused_handlers: Vec::new(),
            command_counts: (2, 0),
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

        let nontauri_sections = [nontauri_section];

        // Render report for nontauri-gaps fixture
        render_html_report(&nontauri_out, &nontauri_sections).expect("render nontauri report");
        let nontauri_html = fs::read_to_string(&nontauri_out).expect("read nontauri html");

        let nontauri_detected = super::detect_tauri_for_report(&nontauri_out, &nontauri_sections);
        assert!(
            !nontauri_detected,
            "nontauri-gaps fixture must not detect Tauri when no manifest exists"
        );
        assert!(
            !nontauri_html.contains("Tauri coverage"),
            "nontauri-gaps fixture must NOT display Tauri surface in HTML"
        );
        assert!(
            !nontauri_html.contains("data-tab=\"commands\""),
            "nontauri-gaps fixture must NOT include commands tab"
        );

        // Also test manifest variants (src-tauri, Cargo.toml with tauri dependency)
        let src_tauri_tmp = tempdir().expect("src-tauri tmp");
        fs::create_dir_all(src_tauri_tmp.path().join("src-tauri")).expect("create src-tauri");
        assert!(super::is_tauri_project_dir(src_tauri_tmp.path()));

        let cargo_tauri_tmp = tempdir().expect("cargo tauri tmp");
        fs::write(
            cargo_tauri_tmp.path().join("Cargo.toml"),
            "[package]\nname = \"desktop\"\n[dependencies]\ntauri = \"2.0\"\n",
        )
        .expect("write Cargo.toml");
        assert!(super::is_tauri_project_dir(cargo_tauri_tmp.path()));

        // Emit JSON report for runtime proof logging
        let proof = serde_json::json!({
            "fixtures": [
                {
                    "fixture": "tauri-clean",
                    "has_tauri": tauri_detected,
                    "surface_visible": tauri_html.contains("Tauri coverage"),
                    "missing_handlers_count": 0,
                    "unused_handlers_count": 0
                },
                {
                    "fixture": "nontauri-gaps",
                    "has_tauri": nontauri_detected,
                    "surface_visible": nontauri_html.contains("Tauri coverage"),
                    "missing_handlers_count": 2,
                    "unused_handlers_count": 0
                }
            ]
        });
        println!("RUNTIME_PROOF_JSON: {}", proof);
    }
}
