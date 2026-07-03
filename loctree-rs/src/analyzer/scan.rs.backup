use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::fs_utils::{GitIgnoreChecker, gather_files};
use crate::types::{FileAnalysis, Options};

use super::classify::{detect_language, file_kind};
use super::css::analyze_css_file;
use super::dart::analyze_dart_file;
use super::js::analyze_js_file;
use super::py::{analyze_py_file, python_stdlib_set};
use super::resolvers::{
    TsPathResolver, find_rust_crate_root, resolve_js_relative, resolve_python_relative,
    resolve_rust_import,
};
use super::rust::analyze_rust_file;
use crate::analyzer::ast_js::CommandDetectionConfig;

/// Build a globset from user patterns.
pub fn build_globset(patterns: &[String]) -> Option<GlobSet> {
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

pub fn opt_globset(globs: &[String]) -> Option<GlobSet> {
    build_globset(globs).and_then(|g| if g.is_empty() { None } else { Some(g) })
}

pub fn strip_excluded(files: &[String], exclude: &Option<GlobSet>) -> Vec<String> {
    match exclude {
        None => files.to_vec(),
        Some(set) => files.iter().filter(|p| !set.is_match(p)).cloned().collect(),
    }
}

pub fn matches_focus(files: &[String], focus: &Option<GlobSet>) -> bool {
    match focus {
        None => true,
        Some(set) => files.iter().any(|p| set.is_match(p)),
    }
}

fn is_ident_like(raw: &str) -> bool {
    raw.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Resolve event names declared as constants across files. This mutates analyses in-place.
pub fn resolve_event_constants_across_files(analyses: &mut [FileAnalysis]) {
    let mut consts_by_path: HashMap<String, HashMap<String, String>> = HashMap::new();
    for a in analyses.iter() {
        if !a.event_consts.is_empty() {
            consts_by_path.insert(a.path.clone(), a.event_consts.clone());
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut unique: HashMap<String, String> = HashMap::new();
    for map in consts_by_path.values() {
        for (name, val) in map {
            *counts.entry(name.clone()).or_insert(0) += 1;
            unique.entry(name.clone()).or_insert(val.clone());
        }
    }
    unique.retain(|k, _| counts.get(k) == Some(&1));

    for analysis in analyses.iter_mut() {
        for ev in analysis
            .event_emits
            .iter_mut()
            .chain(analysis.event_listens.iter_mut())
        {
            let raw = match ev.raw_name.clone() {
                Some(r) if is_ident_like(&r) => r,
                _ => continue,
            };

            let resolved = if let Some(val) = analysis.event_consts.get(&raw) {
                Some(val.clone())
            } else {
                let mut found: Option<String> = None;
                for imp in &analysis.imports {
                    if let Some(resolved_path) = &imp.resolved_path {
                        for sym in &imp.symbols {
                            let alias = sym.alias.as_ref().unwrap_or(&sym.name);
                            if alias == &raw
                                && let Some(map) = consts_by_path.get(resolved_path)
                                && let Some(val) = map.get(&sym.name)
                            {
                                found = Some(val.clone());
                            }
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }
                found.or_else(|| unique.get(&raw).cloned())
            };

            if let Some(val) = resolved {
                ev.name = val;
                if ev.kind.starts_with("emit_ident") {
                    ev.kind = "emit_const".to_string();
                } else if ev.kind.starts_with("listen_ident") {
                    ev.kind = "listen_const".to_string();
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_file(
    path: &Path,
    root_canon: &Path,
    extensions: Option<&HashSet<String>>,
    ts_resolver: Option<&TsPathResolver>,
    py_roots: &[PathBuf],
    py_stdlib: &HashSet<String>,
    symbol: Option<&str>,
    custom_command_macros: &[String],
    command_cfg: &CommandDetectionConfig,
) -> io::Result<FileAnalysis> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root_canon) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "analyzed file escapes provided root",
        ));
    }

    // nosemgrep:rust.actix.path-traversal.tainted-path.tainted-path - canonicalized and bounded to root_canon above
    let content = std::fs::read_to_string(&canonical)?;
    let relative = canonical
        .strip_prefix(root_canon)
        .unwrap_or(&canonical)
        .to_string_lossy()
        .to_string();
    let loc = content.lines().count();
    let ext = canonical
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    let mut analysis = match ext.as_str() {
        "rs" => analyze_rust_file(&content, relative, custom_command_macros),
        "css" => analyze_css_file(&content, relative),
        "py" => analyze_py_file(
            &content, &canonical, root_canon, extensions, relative, py_roots, py_stdlib,
        ),
        "go" => crate::analyzer::go::analyze_go_file(&content, relative),
        "dart" => analyze_dart_file(&content, relative),
        _ => analyze_js_file(
            &content,
            &canonical,
            root_canon,
            extensions,
            ts_resolver,
            relative,
            command_cfg,
        ),
    };

    if let Some(sym) = symbol {
        for (i, line) in content.lines().enumerate() {
            if line.contains(sym) {
                analysis.matches.push(crate::types::SymbolMatch {
                    line: i + 1,
                    context: line.trim().to_string(),
                });
            }
        }
    }

    analysis.loc = loc;
    analysis.language = detect_language(&ext);
    let (kind, is_test, is_generated) = file_kind(&analysis.path);
    analysis.kind = kind;
    analysis.is_test = is_test;
    analysis.is_generated = is_generated;

    // Resolve Rust imports
    if ext == "rs" {
        let crate_root = find_rust_crate_root(&canonical);
        if let Some(ref crate_root) = crate_root {
            for imp in analysis.imports.iter_mut() {
                if imp.resolved_path.is_none() {
                    imp.resolved_path =
                        resolve_rust_import(&imp.source, &canonical, crate_root, root_canon);
                }
            }
        }
    }

    // Resolve other language imports (relative paths)
    for imp in analysis.imports.iter_mut() {
        if imp.resolved_path.is_none() && imp.source.starts_with('.') {
            let resolved = match ext.as_str() {
                "py" => resolve_python_relative(&imp.source, &canonical, root_canon, extensions),
                "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "css" | "svelte" | "vue" => {
                    ts_resolver
                        .and_then(|r| r.resolve(&imp.source, extensions))
                        .or_else(|| {
                            resolve_js_relative(&canonical, root_canon, &imp.source, extensions)
                        })
                }
                _ => None,
            };
            imp.resolved_path = resolved;
        }
    }

    Ok(analysis)
}

/// Expand gather_files with gitignore handling. Returns the list of files and the visited set.
#[allow(dead_code)]
pub fn collect_files(
    root_path: &Path,
    options: &Options,
) -> io::Result<(Vec<PathBuf>, HashSet<PathBuf>)> {
    let git_checker = if options.use_gitignore {
        GitIgnoreChecker::new(root_path)
    } else {
        None
    };

    let mut files = Vec::new();
    let mut visited = HashSet::new();
    gather_files(
        root_path,
        options,
        0,
        git_checker.as_ref(),
        &mut visited,
        &mut files,
    )?;
    Ok((files, visited))
}

pub fn python_stdlib() -> HashSet<String> {
    python_stdlib_set().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_globset_empty() {
        let patterns: Vec<String> = vec![];
        let result = build_globset(&patterns);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_globset_whitespace_only() {
        let patterns = vec!["  ".to_string(), "\t".to_string()];
        let result = build_globset(&patterns);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_globset_valid_patterns() {
        let patterns = vec!["*.ts".to_string(), "src/**/*.js".to_string()];
        let result = build_globset(&patterns);
        assert!(result.is_some());
        let gs = result.unwrap();
        assert!(gs.is_match("foo.ts"));
        assert!(gs.is_match("src/components/Button.js"));
        assert!(!gs.is_match("foo.rs"));
    }

    #[test]
    fn test_build_globset_invalid_pattern_skipped() {
        let patterns = vec!["*.ts".to_string(), "[invalid".to_string()];
        let result = build_globset(&patterns);
        assert!(result.is_some());
    }

    #[test]
    fn test_opt_globset_empty() {
        let globs: Vec<String> = vec![];
        let result = opt_globset(&globs);
        assert!(result.is_none());
    }

    #[test]
    fn test_opt_globset_valid() {
        let globs = vec!["*.rs".to_string()];
        let result = opt_globset(&globs);
        assert!(result.is_some());
    }

    #[test]
    fn test_strip_excluded_none() {
        let files = vec!["a.ts".to_string(), "b.ts".to_string()];
        let result = strip_excluded(&files, &None);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_strip_excluded_some() {
        let files = vec![
            "a.ts".to_string(),
            "b.test.ts".to_string(),
            "c.ts".to_string(),
        ];
        let exclude = opt_globset(&["*.test.ts".to_string()]);
        let result = strip_excluded(&files, &exclude);
        assert_eq!(result.len(), 2);
        assert!(!result.contains(&"b.test.ts".to_string()));
    }

    #[test]
    fn test_matches_focus_none() {
        let files = vec!["a.ts".to_string()];
        assert!(matches_focus(&files, &None));
    }

    #[test]
    fn test_matches_focus_some_match() {
        let files = vec!["src/components/Button.tsx".to_string()];
        let focus = opt_globset(&["src/components/**".to_string()]);
        assert!(matches_focus(&files, &focus));
    }

    #[test]
    fn test_matches_focus_some_no_match() {
        let files = vec!["src/utils/helpers.ts".to_string()];
        let focus = opt_globset(&["src/components/**".to_string()]);
        assert!(!matches_focus(&files, &focus));
    }

    #[test]
    fn test_is_ident_like_valid() {
        assert!(is_ident_like("foo"));
        assert!(is_ident_like("FOO_BAR"));
        assert!(is_ident_like("_private"));
        assert!(is_ident_like("$jquery"));
        assert!(is_ident_like("foo123"));
    }

    #[test]
    fn test_is_ident_like_invalid() {
        assert!(!is_ident_like("foo-bar"));
        assert!(!is_ident_like("foo.bar"));
        assert!(!is_ident_like("foo bar"));
        assert!(!is_ident_like("foo:bar"));
    }

    #[test]
    fn test_python_stdlib_not_empty() {
        let stdlib = python_stdlib();
        assert!(!stdlib.is_empty());
        assert!(stdlib.contains("os"));
        assert!(stdlib.contains("sys"));
        assert!(stdlib.contains("json"));
    }

    #[test]
    fn test_resolve_event_constants_empty() {
        let mut analyses: Vec<FileAnalysis> = vec![];
        resolve_event_constants_across_files(&mut analyses);
        assert!(analyses.is_empty());
    }

    #[test]
    fn test_resolve_event_constants_no_events() {
        let mut analyses = vec![FileAnalysis {
            path: "src/app.ts".to_string(),
            ..Default::default()
        }];
        resolve_event_constants_across_files(&mut analyses);
        assert!(analyses[0].event_emits.is_empty());
    }

    #[test]
    fn test_resolve_event_constants_from_local_consts() {
        use std::collections::HashMap;
        let mut consts = HashMap::new();
        consts.insert("USER_EVENT".to_string(), "user:updated".to_string());

        let mut analyses = vec![FileAnalysis {
            path: "src/app.ts".to_string(),
            event_emits: vec![crate::types::EventRef {
                raw_name: Some("USER_EVENT".to_string()),
                name: "USER_EVENT".to_string(),
                line: 10,
                kind: "emit_ident".to_string(),
                awaited: false,
                payload: None,
            }],
            event_consts: consts,
            ..Default::default()
        }];
        resolve_event_constants_across_files(&mut analyses);
        assert_eq!(analyses[0].event_emits[0].name, "user:updated");
        assert_eq!(analyses[0].event_emits[0].kind, "emit_const");
    }

    #[test]
    fn test_resolve_event_constants_from_imports() {
        use std::collections::HashMap;
        let mut consts_file = HashMap::new();
        consts_file.insert("EVENT_NAME".to_string(), "imported:event".to_string());

        let mut analyses = vec![
            FileAnalysis {
                path: "src/constants.ts".to_string(),
                event_consts: consts_file,
                ..Default::default()
            },
            FileAnalysis {
                path: "src/app.ts".to_string(),
                imports: vec![crate::types::ImportEntry {
                    source: "./constants".to_string(),
                    source_raw: "./constants".to_string(),
                    kind: crate::types::ImportKind::Static,
                    resolved_path: Some("src/constants.ts".to_string()),
                    is_bare: false,
                    symbols: vec![crate::types::ImportSymbol {
                        name: "EVENT_NAME".to_string(),
                        alias: None,
                        is_default: false,
                    }],
                    resolution: crate::types::ImportResolutionKind::Local,
                    is_type_checking: false,
                    is_lazy: false,
                    is_crate_relative: false,
                    is_super_relative: false,
                    is_self_relative: false,
                    raw_path: String::new(),
                }],
                event_listens: vec![crate::types::EventRef {
                    raw_name: Some("EVENT_NAME".to_string()),
                    name: "EVENT_NAME".to_string(),
                    line: 20,
                    kind: "listen_ident".to_string(),
                    awaited: false,
                    payload: None,
                }],
                ..Default::default()
            },
        ];
        resolve_event_constants_across_files(&mut analyses);
        assert_eq!(analyses[1].event_listens[0].name, "imported:event");
        assert_eq!(analyses[1].event_listens[0].kind, "listen_const");
    }

    #[test]
    fn test_resolve_event_constants_from_unique_global() {
        use std::collections::HashMap;
        let mut consts_file = HashMap::new();
        consts_file.insert("UNIQUE_CONST".to_string(), "unique:value".to_string());

        let mut analyses = vec![
            FileAnalysis {
                path: "src/constants.ts".to_string(),
                event_consts: consts_file,
                ..Default::default()
            },
            FileAnalysis {
                path: "src/app.ts".to_string(),
                event_emits: vec![crate::types::EventRef {
                    raw_name: Some("UNIQUE_CONST".to_string()),
                    name: "UNIQUE_CONST".to_string(),
                    line: 15,
                    kind: "emit_ident".to_string(),
                    awaited: false,
                    payload: None,
                }],
                ..Default::default()
            },
        ];
        resolve_event_constants_across_files(&mut analyses);
        assert_eq!(analyses[1].event_emits[0].name, "unique:value");
    }

    #[test]
    fn test_resolve_event_constants_non_ident_skipped() {
        let mut analyses = vec![FileAnalysis {
            path: "src/app.ts".to_string(),
            event_emits: vec![crate::types::EventRef {
                raw_name: Some("not-an-ident!".to_string()),
                name: "not-an-ident!".to_string(),
                line: 10,
                kind: "emit_ident".to_string(),
                awaited: false,
                payload: None,
            }],
            ..Default::default()
        }];
        resolve_event_constants_across_files(&mut analyses);
        // Should not change since raw_name is not ident-like
        assert_eq!(analyses[0].event_emits[0].name, "not-an-ident!");
    }

    #[test]
    fn test_resolve_event_constants_with_alias() {
        use std::collections::HashMap;
        let mut consts_file = HashMap::new();
        consts_file.insert("ORIGINAL".to_string(), "aliased:event".to_string());

        let mut analyses = vec![
            FileAnalysis {
                path: "src/constants.ts".to_string(),
                event_consts: consts_file,
                ..Default::default()
            },
            FileAnalysis {
                path: "src/app.ts".to_string(),
                imports: vec![crate::types::ImportEntry {
                    source: "./constants".to_string(),
                    source_raw: "./constants".to_string(),
                    kind: crate::types::ImportKind::Static,
                    resolved_path: Some("src/constants.ts".to_string()),
                    is_bare: false,
                    symbols: vec![crate::types::ImportSymbol {
                        name: "ORIGINAL".to_string(),
                        alias: Some("ALIASED".to_string()),
                        is_default: false,
                    }],
                    resolution: crate::types::ImportResolutionKind::Local,
                    is_type_checking: false,
                    is_lazy: false,
                    is_crate_relative: false,
                    is_super_relative: false,
                    is_self_relative: false,
                    raw_path: String::new(),
                }],
                event_emits: vec![crate::types::EventRef {
                    raw_name: Some("ALIASED".to_string()),
                    name: "ALIASED".to_string(),
                    line: 30,
                    kind: "emit_ident".to_string(),
                    awaited: false,
                    payload: None,
                }],
                ..Default::default()
            },
        ];
        resolve_event_constants_across_files(&mut analyses);
        assert_eq!(analyses[1].event_emits[0].name, "aliased:event");
    }
}
