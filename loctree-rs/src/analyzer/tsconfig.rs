use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::json;

use super::resolvers::{find_tsconfig, parse_tsconfig_value};
use crate::types::FileAnalysis;

fn build_globset(patterns: &[String]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut added = false;
    for pat in patterns {
        if pat.trim().is_empty() {
            continue;
        }
        match Glob::new(pat) {
            Ok(glob) => {
                builder.add(glob);
                added = true;
            }
            Err(err) => eprintln!("[loctree][warn] invalid glob '{}': {}", pat, err),
        }
    }
    if !added { None } else { builder.build().ok() }
}

fn load_tsconfig(root: &Path) -> Option<serde_json::Value> {
    let ts_path = find_tsconfig(root)?;
    let content = std::fs::read_to_string(&ts_path).ok()?;
    parse_tsconfig_value(&content)
}

pub fn summarize_tsconfig(root: &Path, analyses: &[FileAnalysis]) -> serde_json::Value {
    let Some(tsconfig) = load_tsconfig(root) else {
        return json!({"found": false});
    };

    let compiler = tsconfig
        .get("compilerOptions")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let base_url = compiler
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or(".")
        .to_string();
    let base_path = root.join(&base_url);

    let mut alias_entries = Vec::new();
    if let Some(paths) = compiler.get("paths").and_then(|p| p.as_object()) {
        for (alias, targets) in paths.iter() {
            if let Some(first) = targets.as_array().and_then(|arr| arr.first())
                && let Some(target_str) = first.as_str()
            {
                let normalized = target_str.replace('\\', "/");
                let target_dir = normalized.replace("/*", "").replace('*', "");
                let resolved = base_path.join(&target_dir);
                let exists = resolved.exists();
                alias_entries.push(json!({
                    "alias": alias,
                    "target": target_str,
                    "resolved": resolved.display().to_string(),
                    "exists": exists,
                }));
            }
        }
    }

    let include_patterns: Vec<String> = tsconfig
        .get("include")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.replace('\\', "/")))
                .collect()
        })
        .unwrap_or_default();
    let exclude_patterns: Vec<String> = tsconfig
        .get("exclude")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.replace('\\', "/")))
                .collect()
        })
        .unwrap_or_default();

    let include_set = build_globset(&include_patterns);
    let exclude_set = build_globset(&exclude_patterns);

    let mut outside_include = Vec::new();
    let mut excluded_samples = Vec::new();
    for analysis in analyses {
        let rel = analysis.path.replace('\\', "/");
        let path_obj = PathBuf::from(rel.clone());
        let included = include_set
            .as_ref()
            .map(|set| set.is_match(&path_obj))
            .unwrap_or(true);
        let excluded = exclude_set
            .as_ref()
            .map(|set| set.is_match(&path_obj))
            .unwrap_or(false);

        if excluded {
            if excluded_samples.len() < 8 {
                excluded_samples.push(rel.clone());
            }
            continue;
        }
        if include_set.is_some() && !included && outside_include.len() < 8 {
            outside_include.push(rel.clone());
        }
    }

    let unresolved: Vec<_> = alias_entries
        .iter()
        .filter(|entry| {
            !entry
                .get("exists")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    json!({
        "found": true,
        "baseUrl": base_path.display().to_string(),
        "aliasCount": alias_entries.len(),
        "aliases": alias_entries,
        "unresolvedAliases": unresolved,
        "includeCount": include_patterns.len(),
        "excludeCount": exclude_patterns.len(),
        "outsideIncludeSamples": outside_include,
        "excludedSamples": excluded_samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileAnalysis, ImportEntry, ImportKind};
    use tempfile::tempdir;

    #[test]
    fn summarizes_aliases_and_include_exclude() {
        let dir = tempdir().expect("tmpdir");
        let tsconfig = r#"
        {
          "compilerOptions": {
            "baseUrl": ".",
            "paths": {
              "@/*": ["src/*"]
            }
          },
          "include": ["src/**/*"],
          "exclude": ["**/ignored/**"]
        }
        "#;
        let ts_path = dir.path().join("tsconfig.json");
        std::fs::write(&ts_path, tsconfig).expect("write tsconfig");

        let mut analyses = Vec::new();
        let mut a = FileAnalysis::new("src/main.ts".into());
        a.imports
            .push(ImportEntry::new("@/.something".into(), ImportKind::Static));
        analyses.push(a);
        analyses.push(FileAnalysis::new("ignored/file.ts".into()));
        analyses.push(FileAnalysis::new("outside/file.ts".into()));

        let summary = summarize_tsconfig(dir.path(), &analyses);
        assert!(
            summary
                .get("found")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        );
        assert_eq!(summary.get("aliasCount").and_then(|v| v.as_u64()), Some(1));

        let binding: Vec<serde_json::Value> = Vec::new();
        let unresolved = summary
            .get("unresolvedAliases")
            .and_then(|v| v.as_array())
            .unwrap_or(&binding);
        // target directory doesn't exist in tmpdir
        assert_eq!(unresolved.len(), 1);

        let outside_binding: Vec<serde_json::Value> = Vec::new();
        let outside = summary
            .get("outsideIncludeSamples")
            .and_then(|v| v.as_array())
            .unwrap_or(&outside_binding);
        assert!(
            outside
                .iter()
                .any(|v| v.as_str() == Some("outside/file.ts"))
        );

        let excluded_binding: Vec<serde_json::Value> = Vec::new();
        let excluded = summary
            .get("excludedSamples")
            .and_then(|v| v.as_array())
            .unwrap_or(&excluded_binding);
        assert!(
            excluded
                .iter()
                .any(|v| v.as_str() == Some("ignored/file.ts"))
        );
    }
}
