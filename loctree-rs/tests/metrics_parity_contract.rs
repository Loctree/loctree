use loctree::metrics::{
    import_edge_counts, importer_counts_direct, incoming_import_metrics,
    top_hubs_by_importers_direct,
};
use loctree::snapshot::{GraphEdge, Snapshot};
use loctree::types::FileAnalysis;

fn multi_edge_snapshot() -> Snapshot {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    for path in ["types.rs", "a.rs", "b.rs"] {
        snapshot.files.push(FileAnalysis::new(path.to_string()));
    }
    snapshot.edges.push(GraphEdge {
        from: "a.rs".to_string(),
        to: "types.rs".to_string(),
        label: "TypeA".to_string(),
    });
    snapshot.edges.push(GraphEdge {
        from: "a.rs".to_string(),
        to: "types.rs".to_string(),
        label: "TypeB".to_string(),
    });
    snapshot.edges.push(GraphEdge {
        from: "b.rs".to_string(),
        to: "types.rs".to_string(),
        label: "TypeC".to_string(),
    });
    snapshot
}

#[test]
fn canonical_metrics_name_unique_importers_and_raw_edges() {
    let snapshot = multi_edge_snapshot();
    let metrics = incoming_import_metrics(&snapshot);
    let types = metrics.get("types.rs").expect("types.rs metric");

    assert_eq!(types.importers_direct, 2);
    assert_eq!(types.import_edges, 3);
    assert_eq!(importer_counts_direct(&snapshot)["types.rs"], 2);
    assert_eq!(import_edge_counts(&snapshot)["types.rs"], 3);
}

#[test]
fn hub_ranking_uses_unique_direct_importers_not_raw_edges() {
    let snapshot = multi_edge_snapshot();
    let hubs = top_hubs_by_importers_direct(&snapshot, 1);

    assert_eq!(hubs[0].file, "types.rs");
    assert_eq!(hubs[0].importers_direct, 2);
    assert_eq!(hubs[0].import_edges, 3);
}

#[test]
fn cli_mcp_and_health_surfaces_use_the_canonical_metric_source() {
    // In-crate surfaces stay compile-time (always shipped with this crate).
    let pack = include_str!("../src/pack.rs");
    let context_scope = include_str!("../src/cli/dispatch/handlers/context/scope.rs");

    assert!(pack.contains("importer_counts_direct(snapshot)"));
    assert!(pack.contains("top_hubs_by_importers_direct(snapshot, limit)"));
    assert!(context_scope.contains("top_hubs_by_importers_direct_filtered"));

    // Sibling-crate surfaces are read at runtime: the engine release mirror
    // ships loctree-rs without loctree-lsp/loctree-mcp, and a compile-time
    // include_str! would break the whole test target there. Absent file ⇒
    // that surface is not part of this checkout ⇒ nothing to assert; present
    // file MUST still honor the canonical metric source.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let health_path = manifest_dir.join("../loctree-lsp/src/health.rs");
    let mcp_path = manifest_dir.join("../loctree-mcp/src/main.rs");
    let health = std::fs::read_to_string(&health_path).ok();
    let mcp = std::fs::read_to_string(&mcp_path).ok();

    if let Some(health) = &health {
        assert!(health.contains("importer_counts_direct(snapshot)"));
        assert!(health.contains("top_hubs_by_importers_direct(snapshot"));
    }
    if let Some(mcp) = &mcp {
        assert!(mcp.contains("repository_metrics(&snapshot)"));
        assert!(mcp.contains("top_hubs_by_importers_direct(&snapshot, 5)"));
    }

    let mut surfaces = vec![
        ("pack", pack.to_string()),
        ("context_scope", context_scope.to_string()),
    ];
    if let Some(health) = health {
        surfaces.push(("health", health));
    }
    if let Some(mcp) = mcp {
        surfaces.push(("mcp", mcp));
    }
    for (surface, source) in surfaces {
        assert!(
            !source.contains("entry(edge.to.clone()).or_insert(0) += 1")
                && !source.contains("entry(&edge.to).or_default() += 1"),
            "{surface} must not reintroduce raw-edge importer counting"
        );
    }
}
