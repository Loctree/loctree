use serde_json::json;

use super::dead_parrots::DeadExport;
use super::report::{CommandGap, RankedDup};
use crate::snapshot::Snapshot;

pub struct SarifInputs<'a> {
    pub duplicate_exports: &'a [RankedDup],
    pub missing_handlers: &'a [CommandGap],
    pub unused_handlers: &'a [CommandGap],
    pub dead_exports: &'a [DeadExport],
    /// Circular imports: each cycle is a Vec of file paths
    pub circular_imports: &'a [Vec<String>],
    pub pipeline_summary: &'a serde_json::Value,
    /// Snapshot for enrichment metadata (blast radius, consumer count, etc.)
    pub snapshot: Option<&'a Snapshot>,
}

/// Calculate blast radius: how many files depend on this file
fn compute_blast_radius(snapshot: &Snapshot, file_path: &str) -> usize {
    snapshot
        .edges
        .iter()
        .filter(|edge| edge.from == file_path)
        .count()
}

/// Calculate consumer count: how many files import this specific file
fn compute_consumer_count(snapshot: &Snapshot, file_path: &str) -> usize {
    snapshot
        .edges
        .iter()
        .filter(|edge| {
            edge.to == file_path
                || edge.to.ends_with(&format!("/{}", file_path))
                || (file_path.contains('/') && edge.to.contains(file_path))
        })
        .count()
}

/// Map loctree confidence to SARIF-compatible confidence level
fn map_confidence_level(confidence: &str) -> &'static str {
    match confidence.to_lowercase().as_str() {
        "certain" => "CERTAIN",
        "high" => "HIGH",
        "medium" | "low" => "SMELL",
        _ => "SMELL",
    }
}

fn build_sarif(inputs: SarifInputs) -> serde_json::Value {
    let mut results = Vec::new();

    // Duplicate exports
    for dup in inputs.duplicate_exports {
        for file in &dup.files {
            results.push(json!({
                "ruleId": "duplicate-export",
                "level": "warning",
                "message": {
                    "text": format!("Duplicate export '{}' (canonical: {})", dup.name, dup.canonical)
                },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": file }
                    }
                }]
            }));
        }
    }

    // Missing handlers
    for gap in inputs.missing_handlers {
        for (file, line) in &gap.locations {
            results.push(json!({
                "ruleId": "missing-handler",
                "level": "error",
                "message": {
                    "text": format!("Missing backend handler for command '{}'", gap.name)
                },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                        "region": { "startLine": line }
                    }
                }]
            }));
        }
    }

    // Unused handlers
    for gap in inputs.unused_handlers {
        for (file, line) in &gap.locations {
            results.push(json!({
                "ruleId": "unused-handler",
                "level": "warning",
                "message": {
                    "text": format!("Unused backend handler '{}'", gap.name)
                },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                        "region": { "startLine": line }
                    }
                }]
            }));
        }
    }

    // Dead exports
    for dead in inputs.dead_exports {
        let mut result = json!({
            "ruleId": "dead-export",
            "level": "warning",
            "message": {
                "text": format!("Potential dead export '{}' ({})", dead.symbol, dead.confidence)
            },
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": { "uri": dead.file },
                    "region": { "startLine": dead.line.unwrap_or(1) }
                }
            }]
        });

        // Build properties object with enrichment data
        let mut properties = serde_json::Map::new();

        // Add open_url if present (existing functionality)
        if let Some(ref open_url) = dead.open_url {
            properties.insert("openUrl".to_string(), json!(open_url));
        }

        // Add loctree enrichment fields if snapshot is available
        if let Some(snapshot) = inputs.snapshot {
            let blast_radius = compute_blast_radius(snapshot, &dead.file);
            let consumer_count = compute_consumer_count(snapshot, &dead.file);
            let confidence_level = map_confidence_level(&dead.confidence);

            properties.insert(
                "loctree".to_string(),
                json!({
                    "blast_radius": blast_radius,
                    "is_dead_code": true,
                    "consumer_count": consumer_count,
                    "confidence_level": confidence_level
                }),
            );
        }

        if !properties.is_empty() {
            result["properties"] = json!(properties);
        }

        results.push(result);
    }

    // Circular imports
    for cycle in inputs.circular_imports {
        if cycle.is_empty() {
            continue;
        }
        let cycle_desc = cycle.join(" → ");
        let first_file = &cycle[0];
        results.push(json!({
            "ruleId": "circular-import",
            "level": "warning",
            "message": {
                "text": format!("Circular import detected: {}", cycle_desc)
            },
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": { "uri": first_file }
                }
            }],
            "relatedLocations": cycle.iter().skip(1).enumerate().map(|(idx, file)| {
                json!({
                    "id": idx + 1,
                    "physicalLocation": {
                        "artifactLocation": { "uri": file }
                    },
                    "message": { "text": format!("Part of cycle at position {}", idx + 2) }
                })
            }).collect::<Vec<_>>()
        }));
    }

    // Ghost events
    if let Some(events) = inputs.pipeline_summary.get("events") {
        if let Some(ghosts) = events.get("ghostEmits").and_then(|v| v.as_array()) {
            for ghost in ghosts {
                let name = ghost["name"].as_str().unwrap_or("?");
                let path = ghost["path"].as_str().unwrap_or("?");
                let line = ghost["line"].as_u64().unwrap_or(1);
                let conf = ghost["confidence"].as_str().unwrap_or("low");

                results.push(json!({
                    "ruleId": "ghost-event",
                    "level": "warning",
                    "message": {
                        "text": format!("Ghost event '{}' (emitted but not listened, confidence: {})", name, conf)
                    },
                    "locations": [{
                        "physicalLocation": {
                            "artifactLocation": { "uri": path },
                            "region": { "startLine": line }
                        }
                    }]
                }));
            }
        }

        if let Some(orphans) = events.get("orphanListeners").and_then(|v| v.as_array()) {
            for orphan in orphans {
                let name = orphan["name"].as_str().unwrap_or("?");
                let path = orphan["path"].as_str().unwrap_or("?");
                let line = orphan["line"].as_u64().unwrap_or(1);

                results.push(json!({
                    "ruleId": "orphan-listener",
                    "level": "warning",
                    "message": {
                        "text": format!("Orphan listener for '{}' (no emitter found)", name)
                    },
                    "locations": [{
                        "physicalLocation": {
                            "artifactLocation": { "uri": path },
                            "region": { "startLine": line }
                        }
                    }]
                }));
            }
        }
    }

    let tool = json!({
        "driver": {
            "name": "loctree",
            "informationUri": "https://github.com/Loctree/Loctree",
            "version": env!("CARGO_PKG_VERSION"),
            "rules": [
                { "id": "duplicate-export", "shortDescription": { "text": "Duplicate export detected" } },
                { "id": "missing-handler", "shortDescription": { "text": "Missing backend handler for frontend command" } },
                { "id": "unused-handler", "shortDescription": { "text": "Unused backend handler" } },
                { "id": "dead-export", "shortDescription": { "text": "Export defined but never imported" } },
                { "id": "circular-import", "shortDescription": { "text": "Circular import dependency detected" } },
                { "id": "ghost-event", "shortDescription": { "text": "Event emitted but not listened to" } },
                { "id": "orphan-listener", "shortDescription": { "text": "Event listener without emitter" } }
            ]
        }
    });

    json!({
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": tool,
            "results": results
        }]
    })
}

/// Generate SARIF report as a JSON value (for file output or further processing)
pub fn generate_sarif(inputs: SarifInputs) -> serde_json::Value {
    build_sarif(inputs)
}

/// Generate SARIF report as a pretty-printed JSON string
pub fn generate_sarif_string(inputs: SarifInputs) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&build_sarif(inputs))
}

/// Print SARIF report to stdout
pub fn print_sarif(inputs: SarifInputs) -> Result<(), serde_json::Error> {
    match generate_sarif_string(inputs) {
        Ok(json) => {
            println!("{}", json);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::report::{Confidence, DupSeverity};

    fn mock_dup(name: &str, files: Vec<&str>) -> RankedDup {
        RankedDup {
            name: name.to_string(),
            canonical: name.to_lowercase(),
            files: files.into_iter().map(|s| s.to_string()).collect(),
            locations: vec![],
            score: 1,
            prod_count: 0,
            dev_count: 0,
            canonical_line: None,
            refactors: vec![],
            severity: DupSeverity::SamePackage,
            is_cross_lang: false,
            packages: vec![],
            reason: String::new(),
        }
    }

    fn mock_gap(name: &str, locations: Vec<(&str, usize)>) -> CommandGap {
        CommandGap {
            name: name.to_string(),
            confidence: Some(Confidence::High),
            locations: locations
                .into_iter()
                .map(|(f, l)| (f.to_string(), l))
                .collect(),
            implementation_name: None,
            string_literal_matches: vec![],
        }
    }

    fn mock_dead(file: &str, symbol: &str, line: Option<usize>) -> DeadExport {
        DeadExport {
            file: file.to_string(),
            symbol: symbol.to_string(),
            line,
            confidence: "high".to_string(),
            reason: format!(
                "No imports found for '{}'. Checked: resolved imports (0 matches), star re-exports (none), local references (none)",
                symbol
            ),
            open_url: None,
            is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint: false,
        }
    }

    #[test]
    fn test_print_sarif_empty() {
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_duplicates() {
        let dups = vec![mock_dup("Button", vec!["src/a.ts", "src/b.ts"])];
        let inputs = SarifInputs {
            duplicate_exports: &dups,
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_missing_handlers() {
        let missing = vec![mock_gap("get_user", vec![("src/api.ts", 10)])];
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &missing,
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_unused_handlers() {
        let unused = vec![mock_gap("old_handler", vec![("src-tauri/src/lib.rs", 50)])];
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &unused,
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_dead_exports() {
        let dead = vec![
            mock_dead("src/utils.ts", "unusedHelper", Some(10)),
            mock_dead("src/helpers.ts", "oldFunction", None),
        ];
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &dead,
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_circular_imports() {
        let cycles = vec![vec![
            "src/a.ts".to_string(),
            "src/b.ts".to_string(),
            "src/a.ts".to_string(),
        ]];
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &cycles,
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_ghost_events() {
        let summary = json!({
            "events": {
                "ghostEmits": [
                    {"name": "user-updated", "path": "src/user.ts", "line": 20, "confidence": "high"}
                ]
            }
        });
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &summary,
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_orphan_listeners() {
        let summary = json!({
            "events": {
                "orphanListeners": [
                    {"name": "deleted-event", "path": "src/listener.ts", "line": 15}
                ]
            }
        });
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &summary,
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_full() {
        let dups = vec![mock_dup("Component", vec!["src/a.tsx", "src/b.tsx"])];
        let missing = vec![mock_gap("api_call", vec![("src/api.ts", 5)])];
        let unused = vec![mock_gap("legacy_fn", vec![("src-tauri/src/main.rs", 100)])];
        let dead = vec![mock_dead("src/old.ts", "deprecated", Some(1))];
        let cycles = vec![vec!["src/x.ts".to_string(), "src/y.ts".to_string()]];
        let summary = json!({
            "events": {
                "ghostEmits": [{"name": "evt", "path": "a.ts", "line": 1, "confidence": "low"}],
                "orphanListeners": [{"name": "old-evt", "path": "b.ts", "line": 2}]
            }
        });

        let inputs = SarifInputs {
            duplicate_exports: &dups,
            missing_handlers: &missing,
            unused_handlers: &unused,
            dead_exports: &dead,
            circular_imports: &cycles,
            pipeline_summary: &summary,
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_multiple_locations() {
        let missing = vec![mock_gap(
            "shared_command",
            vec![
                ("src/page1.ts", 10),
                ("src/page2.ts", 20),
                ("src/page3.ts", 30),
            ],
        )];
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &missing,
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: None,
        };
        assert!(print_sarif(inputs).is_ok());
    }

    #[test]
    fn test_print_sarif_with_enrichment() {
        use crate::snapshot::{GraphEdge, Snapshot, SnapshotMetadata};

        // Create a simple test snapshot with edges
        let snapshot = Snapshot {
            metadata: SnapshotMetadata::default(),
            files: vec![],
            edges: vec![
                GraphEdge {
                    from: "src/utils.ts".to_string(),
                    to: "src/component.ts".to_string(),
                    label: "import".to_string(),
                },
                GraphEdge {
                    from: "src/other.ts".to_string(),
                    to: "src/utils.ts".to_string(),
                    label: "import".to_string(),
                },
            ],
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        };

        let dead = vec![mock_dead("src/utils.ts", "unusedHelper", Some(10))];
        let inputs = SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &[],
            unused_handlers: &[],
            dead_exports: &dead,
            circular_imports: &[],
            pipeline_summary: &json!({}),
            snapshot: Some(&snapshot),
        };

        let result = generate_sarif(inputs);

        // Verify the result contains loctree enrichment fields
        let results = result["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);

        let props = &results[0]["properties"];
        assert!(props.get("loctree").is_some());

        let loctree = &props["loctree"];
        assert_eq!(loctree["is_dead_code"], true);
        assert_eq!(loctree["confidence_level"], "HIGH");
        // blast_radius: 1 (utils.ts imports component.ts)
        assert_eq!(loctree["blast_radius"], 1);
        // consumer_count: 1 (other.ts imports utils.ts)
        assert_eq!(loctree["consumer_count"], 1);
    }
}
