//! Project temporal path identities relative to one admitted scan root.
//! This is a typed projection, never a replacement across serialized JSON or source text.
use crate::semantic::ReachReason;
use crate::snapshot::Snapshot;
use crate::symbols::SymbolGraph;
use std::path::{Component, Path, PathBuf};

struct ExportPaths {
    root: PathBuf,
    canonical_root: PathBuf,
}

impl ExportPaths {
    fn relative(&self, value: &Path) -> Option<PathBuf> {
        if !value.is_absolute() {
            return None;
        }
        let relative = value
            .strip_prefix(&self.root)
            .or_else(|_| value.strip_prefix(&self.canonical_root))
            .ok()?;
        if relative
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return None;
        }
        // Verify the existing ancestry while the export is alive. This admits
        // missing owned targets too, but never a symlink escaping the export.
        let candidate = self.canonical_root.join(relative);
        let mut ancestor = candidate.as_path();
        loop {
            match ancestor.symlink_metadata() {
                Ok(_) => {
                    if !ancestor
                        .canonicalize()
                        .ok()?
                        .starts_with(&self.canonical_root)
                    {
                        return None;
                    }
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ancestor = ancestor.parent()?;
                }
                Err(_) => return None,
            }
        }
        Some(if relative.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            relative.to_path_buf()
        })
    }

    fn path(&self, value: &mut PathBuf) {
        if let Some(relative) = self.relative(value) {
            *value = relative;
        }
    }

    fn text(&self, value: &mut String) {
        if let Some(relative) = self.relative(Path::new(value)) {
            *value = relative.to_string_lossy().into_owned();
        }
    }

    fn optional(&self, value: &mut Option<String>) {
        if let Some(value) = value {
            self.text(value);
        }
    }

    fn texts(&self, values: &mut [String]) {
        for value in values {
            self.text(value);
        }
    }

    fn symbol(&self, value: &mut String) {
        if let Some((file, suffix)) = value.split_once("::")
            && let Some(relative) = self.relative(Path::new(file))
        {
            *value = format!("{}::{suffix}", relative.display());
        }
    }

    fn symbols(&self, graph: &mut SymbolGraph) {
        for node in &mut graph.symbols {
            self.symbol(&mut node.id.0);
            if let Some(file) = &mut node.file {
                self.path(file);
            }
        }
        for occurrence in &mut graph.occurrences {
            self.symbol(&mut occurrence.symbol_id.0);
            self.path(&mut occurrence.file);
        }
        for edge in &mut graph.edges {
            self.symbol(&mut edge.from.0);
            self.symbol(&mut edge.to.0);
        }
        for projection in &mut graph.file_projection {
            self.path(&mut projection.file);
            for id in projection
                .defined
                .iter_mut()
                .chain(&mut projection.referenced)
            {
                self.symbol(&mut id.0);
            }
        }
    }
}

pub(super) fn normalize_owned_paths(snapshot: &mut Snapshot, export: &Path) -> std::io::Result<()> {
    let paths = ExportPaths {
        root: export.to_path_buf(),
        canonical_root: export.canonicalize()?,
    };
    // metadata.roots retains the physical scan provenance. It is deliberately
    // not a working-checkout identity; structural fingerprints include that
    // provenance and are not a cross-export semantic equality assertion.
    for file in &mut snapshot.files {
        paths.text(&mut file.path);
        for import in &mut file.imports {
            paths.text(&mut import.source);
            paths.optional(&mut import.resolved_path);
        }
        for reexport in &mut file.reexports {
            paths.text(&mut reexport.source);
            paths.optional(&mut reexport.resolved);
        }
        paths.texts(&mut file.dynamic_imports);
        for export in &mut file.exports {
            paths.symbol(&mut export.symbol_id.0);
        }
        for target in &mut file.cargo_targets {
            paths.path(&mut target.path);
            paths.path(&mut target.crate_root);
        }
        for casing in &mut file.command_payload_casing {
            paths.text(&mut casing.path);
        }
        if let Some(fragment) = &mut file.symbol_fragment {
            paths.symbols(fragment);
        }
    }
    for edge in &mut snapshot.edges {
        paths.text(&mut edge.from);
        paths.text(&mut edge.to);
    }
    for files in snapshot.export_index.values_mut() {
        paths.texts(files);
    }
    for bridge in &mut snapshot.command_bridges {
        for (file, _) in &mut bridge.frontend_calls {
            paths.text(file);
        }
        if let Some((file, _)) = &mut bridge.backend_handler {
            paths.text(file);
        }
    }
    for bridge in &mut snapshot.event_bridges {
        for (file, _, _) in &mut bridge.emits {
            paths.text(file);
        }
        for (file, _) in &mut bridge.listens {
            paths.text(file);
        }
    }
    for barrel in &mut snapshot.barrels {
        paths.text(&mut barrel.path);
        paths.text(&mut barrel.module_id);
        paths.texts(&mut barrel.targets);
    }
    if let Some(facts) = &mut snapshot.semantic_facts {
        facts.idiom_tags = std::mem::take(&mut facts.idiom_tags)
            .into_iter()
            .map(|(mut id, tags)| {
                paths.symbol(&mut id);
                (id, tags)
            })
            .collect();
        for set in [
            &mut facts.reachability.reached_symbols,
            &mut facts.reachability.unreached_symbols,
        ] {
            *set = std::mem::take(set)
                .into_iter()
                .map(|mut id| {
                    paths.symbol(&mut id);
                    id
                })
                .collect();
        }
        facts.reachability.reasons = std::mem::take(&mut facts.reachability.reasons)
            .into_iter()
            .map(|(mut id, mut reason)| {
                paths.symbol(&mut id);
                match &mut reason {
                    ReachReason::SourceInclude { from_file } => paths.text(from_file),
                    ReachReason::DispatchHandler { from_symbol, .. } => paths.symbol(from_symbol),
                    ReachReason::RecipeShellCall { recipe_owner } => paths.symbol(recipe_owner),
                    _ => {}
                }
                (id, reason)
            })
            .collect();
        for edge in &mut facts.dispatch_edges {
            paths.text(&mut edge.from_file);
            paths.optional(&mut edge.handler_file);
            paths.symbol(&mut edge.handler_symbol);
        }
        for contract in &mut facts.env_contracts {
            paths.texts(&mut contract.used_in_files);
            for occurrence in &mut contract.occurrences {
                paths.text(&mut occurrence.file);
            }
        }
    }
    if let Some(graph) = &mut snapshot.symbol_graph {
        paths.symbols(graph);
    }
    if let Some(config) = &mut snapshot.metadata.resolver_config {
        paths.optional(&mut config.ts_base_url);
        paths.texts(&mut config.py_roots);
        paths.texts(&mut config.rust_crate_roots);
        for values in config.ts_paths.values_mut() {
            paths.texts(values);
        }
    }
    for manifest in &mut snapshot.metadata.manifest_summary {
        paths.text(&mut manifest.root);
    }
    for entry in &mut snapshot.metadata.entrypoints {
        paths.text(&mut entry.path);
    }
    let drift = &mut snapshot.metadata.entrypoint_drift;
    for entry in drift
        .declared_missing
        .iter_mut()
        .chain(&mut drift.declared_without_marker)
        .chain(&mut drift.declared_unresolved)
    {
        paths.text(&mut entry.path);
    }
    for entry in &mut drift.code_only_entrypoints {
        paths.text(&mut entry.path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{ReachReason, SemanticFacts};
    use crate::snapshot::GraphEdge;
    use crate::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind, StringLiteral};

    #[test]
    fn typed_paths_and_symbol_ids_detach_without_rewriting_source() {
        let dir = tempfile::tempdir().expect("export");
        std::fs::write(dir.path().join("entry.ts"), "export const value = 1;").expect("file");
        let absolute = dir.path().join("entry.ts").to_string_lossy().into_owned();
        let canonical = dir
            .path()
            .canonicalize()
            .expect("canonical export")
            .join("entry.ts")
            .to_string_lossy()
            .into_owned();
        let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
        let mut file = FileAnalysis::new(absolute.clone());
        file.exports.push(
            ExportSymbol::new("value".into(), "const", "named", Some(1)).with_symbol_id(&absolute),
        );
        let raw_source = canonical.clone();
        let mut import = ImportEntry::new(canonical.clone(), ImportKind::Static);
        import.resolved_path = Some(canonical.clone());
        file.imports.push(import);
        file.string_literals.push(StringLiteral {
            value: absolute.clone(),
            line: 1,
        });
        snapshot.files.push(file);
        snapshot.files.push(FileAnalysis::new("relative.ts".into()));
        snapshot.edges.push(GraphEdge {
            from: absolute.clone(),
            to: canonical,
            label: absolute.clone(),
        });
        snapshot
            .export_index
            .insert("value".into(), vec![absolute.clone()]);
        let mut facts = SemanticFacts::default();
        let id = format!("{absolute}::value");
        facts.reachability.reached_symbols.insert(id.clone());
        facts.reachability.reasons.insert(
            id,
            ReachReason::SourceInclude {
                from_file: absolute.clone(),
            },
        );
        snapshot.semantic_facts = Some(facts);
        let mut graph = SymbolGraph::default();
        graph
            .file_projection
            .push(crate::symbols::FileSymbolSummary {
                file: PathBuf::from(&absolute),
                defined: vec![crate::symbols::SymbolId::from_parts(
                    &absolute, "var", "value", 7,
                )],
                referenced: vec![crate::symbols::SymbolId::from(
                    "/external/file::var::external::7",
                )],
            });
        snapshot.symbol_graph = Some(graph);
        normalize_owned_paths(&mut snapshot, dir.path()).expect("detach");
        assert_eq!(snapshot.files[0].path, "entry.ts");
        assert_eq!(snapshot.files[1].path, "relative.ts");
        assert_eq!(
            snapshot.files[0].exports[0].symbol_id.as_str(),
            "entry.ts::value"
        );
        assert_eq!(snapshot.files[0].imports[0].source, "entry.ts");
        assert_eq!(
            snapshot.files[0].imports[0].resolved_path.as_deref(),
            Some("entry.ts")
        );
        assert_eq!(snapshot.files[0].imports[0].source_raw, raw_source);
        assert_eq!(snapshot.files[0].string_literals[0].value, absolute);
        assert_eq!(snapshot.edges[0].from, "entry.ts");
        assert_eq!(snapshot.edges[0].to, "entry.ts");
        assert_eq!(snapshot.edges[0].label, absolute);
        assert_eq!(snapshot.export_index["value"], ["entry.ts"]);
        let projection = &snapshot
            .symbol_graph
            .as_ref()
            .expect("symbol graph")
            .file_projection[0];
        assert_eq!(projection.file, Path::new("entry.ts"));
        assert_eq!(
            projection.defined[0].as_str(),
            "entry.ts::var::value::0000000000000007"
        );
        assert_eq!(
            projection.referenced[0].as_str(),
            "/external/file::var::external::7"
        );
        let facts = snapshot.semantic_facts.as_ref().expect("facts");
        assert!(
            facts
                .reachability
                .reached_symbols
                .contains("entry.ts::value")
        );
        assert!(
            matches!(&facts.reachability.reasons["entry.ts::value"], ReachReason::SourceInclude { from_file } if from_file == "entry.ts")
        );
        let before = serde_json::to_value(&snapshot).expect("serialize");
        normalize_owned_paths(&mut snapshot, dir.path()).expect("idempotent");
        assert_eq!(serde_json::to_value(&snapshot).expect("serialize"), before);
    }

    #[test]
    fn exact_root_boundary_preserves_external_paths_and_rejects_symlink_escape() {
        let dir = tempfile::tempdir().expect("export");
        let external = tempfile::tempdir().expect("external");
        std::fs::write(external.path().join("outside.mk"), "X=1").expect("external file");
        let paths = ExportPaths {
            root: dir.path().to_path_buf(),
            canonical_root: dir.path().canonicalize().expect("canonical root"),
        };
        assert!(
            paths
                .relative(&external.path().join("outside.mk"))
                .is_none()
        );
        assert!(
            paths
                .relative(&PathBuf::from(format!(
                    "{}-other/file",
                    dir.path().display()
                )))
                .is_none()
        );
        assert!(paths.relative(&dir.path().join("../outside.mk")).is_none());
        assert_eq!(
            paths.relative(&dir.path().join("missing/target.mk")),
            Some(PathBuf::from("missing/target.mk"))
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(external.path(), dir.path().join("escape"))
                .expect("escape fixture");
            assert!(
                paths
                    .relative(&dir.path().join("escape/outside.mk"))
                    .is_none()
            );
            assert!(
                paths
                    .relative(&dir.path().join("escape/missing.mk"))
                    .is_none()
            );
        }
        let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
        let external_path = external.path().join("outside.mk").display().to_string();
        snapshot.edges.push(GraphEdge {
            from: "Makefile".into(),
            to: external_path.clone(),
            label: String::new(),
        });
        normalize_owned_paths(&mut snapshot, dir.path()).expect("detach");
        assert_eq!(snapshot.edges[0].to, external_path);
    }
}
