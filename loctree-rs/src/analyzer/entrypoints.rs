use serde_json::json;

use crate::types::FileAnalysis;

pub fn find_entrypoints(analyses: &[FileAnalysis]) -> Vec<(String, Vec<String>)> {
    let mut entrypoints = Vec::new();
    for analysis in analyses {
        if !analysis.entry_points.is_empty() {
            entrypoints.push((analysis.path.clone(), analysis.entry_points.clone()));
        }
    }
    entrypoints.sort_by(|a, b| a.0.cmp(&b.0));
    entrypoints
}

pub fn print_entrypoints(entrypoints: &[(String, Vec<String>)], json_output: bool) {
    if json_output {
        let items: Vec<_> = entrypoints
            .iter()
            .map(|(path, types)| {
                json!({
                    "path": path,
                    "types": types
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "entryPoints": items }))
                .expect("Failed to serialize entry points to JSON")
        );
    } else if entrypoints.is_empty() {
        println!("No entry points detected.");
    } else {
        println!("Entry points ({} found):", entrypoints.len());
        for (path, types) in entrypoints {
            let unique: std::collections::HashSet<_> = types.iter().collect();
            let mut sorted: Vec<_> = unique.into_iter().collect();
            sorted.sort();
            println!(
                "  - {}: {}",
                path,
                sorted
                    .into_iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_file(path: &str, entry_points: Vec<&str>) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            entry_points: entry_points.into_iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_find_entrypoints_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let result = find_entrypoints(&analyses);
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_entrypoints_no_entries() {
        let analyses = vec![
            mock_file("src/utils.ts", vec![]),
            mock_file("src/helpers.ts", vec![]),
        ];
        let result = find_entrypoints(&analyses);
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_entrypoints_single_file() {
        let analyses = vec![
            mock_file("src/main.ts", vec!["createApp"]),
            mock_file("src/utils.ts", vec![]),
        ];
        let result = find_entrypoints(&analyses);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "src/main.ts");
        assert_eq!(result[0].1, vec!["createApp"]);
    }

    #[test]
    fn test_find_entrypoints_multiple_files() {
        let analyses = vec![
            mock_file("src/main.ts", vec!["createApp"]),
            mock_file("src/App.tsx", vec!["ReactDOM.render"]),
            mock_file("src/utils.ts", vec![]),
        ];
        let result = find_entrypoints(&analyses);
        assert_eq!(result.len(), 2);
        // Should be sorted alphabetically
        assert_eq!(result[0].0, "src/App.tsx");
        assert_eq!(result[1].0, "src/main.ts");
    }

    #[test]
    fn test_find_entrypoints_multiple_types() {
        let analyses = vec![mock_file(
            "src/index.ts",
            vec!["createServer", "mountApp", "bootstrap"],
        )];
        let result = find_entrypoints(&analyses);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1.len(), 3);
    }

    #[test]
    fn test_print_entrypoints_empty_no_crash() {
        let entrypoints: Vec<(String, Vec<String>)> = vec![];
        // Should not panic
        print_entrypoints(&entrypoints, false);
        print_entrypoints(&entrypoints, true);
    }

    #[test]
    fn test_print_entrypoints_json_format() {
        let entrypoints = vec![(
            "src/main.ts".to_string(),
            vec!["createApp".to_string(), "bootstrap".to_string()],
        )];
        // Should not panic
        print_entrypoints(&entrypoints, true);
    }

    #[test]
    fn test_print_entrypoints_human_format() {
        let entrypoints = vec![
            ("src/main.ts".to_string(), vec!["createApp".to_string()]),
            ("src/App.tsx".to_string(), vec!["render".to_string()]),
        ];
        // Should not panic
        print_entrypoints(&entrypoints, false);
    }

    #[test]
    fn test_print_entrypoints_deduplicates_types() {
        let entrypoints = vec![(
            "src/main.ts".to_string(),
            vec![
                "createApp".to_string(),
                "createApp".to_string(),
                "bootstrap".to_string(),
            ],
        )];
        // Should not panic, will deduplicate internally
        print_entrypoints(&entrypoints, false);
    }
}
