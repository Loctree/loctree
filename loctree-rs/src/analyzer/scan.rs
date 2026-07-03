use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::fs_utils::{GitIgnoreChecker, gather_files};
use crate::types::{FileAnalysis, Options};
use loctree_ast::{LangExtractor as _, Parsers as TsParsers};

use super::cargo_manifest::{
    find_nearest_crate_manifest, parse_crate_manifest, parse_workspace_root,
};
use super::classify::{detect_language, detect_language_from_filename, file_kind, resource_kind};
use super::css::analyze_css_file;
use super::dart::analyze_dart_file;
use super::html_analyzer::analyze_html_file;
use super::js::analyze_js_file;
use super::makefile::{analyze_makefile, is_makefile_name, resolve_makefile_include};
use super::py::{analyze_py_file, python_stdlib_set};
use super::resolvers::{
    TsPathResolver, find_rust_crate_root, resolve_js_relative, resolve_python_relative,
    resolve_rust_import,
};
use super::rust::analyze_rust_file;
use super::shell::{analyze_shell_file, has_shell_shebang, resolve_shell_source};
use super::zig::{analyze_zig_file, resolve_zig_import};
use crate::analyzer::ast_js::CommandDetectionConfig;

/// Known binary file extensions that should be skipped
// `svg` is deliberately NOT here: it is text/XML that rg matches literally, so
// treating it as binary made `find --literal/--regex` under-report versus rg
// (W2-02 adversarial probe). It rides the scan-only resource path instead.
const BINARY_EXTENSIONS: &[&str] = &[
    "dat", "bz2", "gz", "zip", "tar", "tgz", "pack", "png", "jpg", "jpeg", "gif", "bmp", "ico",
    "webp", "woff", "woff2", "ttf", "eot", "otf", "exe", "dll", "so", "dylib", "node", "wasm",
    "bin", "o", "a", "lib", "pyc", "pyo", "pdf", "doc", "docx", "xls", "xlsx", "mp3", "mp4", "avi",
    "mov", "wav",
];

/// Source code extensions that should never be treated as binary
const SOURCE_CODE_EXTENSIONS: &[&str] = &[
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "py",
    "rb",
    "go",
    "dart",
    "svelte",
    "vue",
    "astro",
    "kt",
    "kts",
    "css",
    "scss",
    "less",
    "html",
    "json",
    "yaml",
    "yml",
    "toml",
    "md",
    "txt",
    // Text resources that should not trip binary sniffing.
    "storyboard",
    "xib",
    "properties",
    "xml",
    "svg",
    // v0.9.0 lightweight parsers.
    "sh",
    "bash",
    "zsh",
    "fish",
    "mk",
    "make",
    "zig",
    "zon",
    "swift",
];

/// Check if a file is likely binary based on extension or magic bytes
fn is_binary_file(path: &Path) -> bool {
    // Check extension first
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();

        // Known binary extensions - definitely binary
        if BINARY_EXTENSIONS.contains(&ext_lower.as_str()) {
            return true;
        }

        // Known source code extensions - never binary (even with UTF-8 chars)
        if SOURCE_CODE_EXTENSIONS.contains(&ext_lower.as_str()) {
            return false;
        }
    }

    // Check magic bytes only for unknown extensions
    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buffer = [0u8; 512];
        if let Ok(n) = file.read(&mut buffer)
            && n > 0
        {
            // Only null bytes indicate true binary (executables, images, etc.)
            // UTF-8 encoded text files may have non-ASCII but never null bytes
            let null_count = buffer[..n].iter().filter(|&&b| b == 0).count();
            if null_count > 0 {
                return true;
            }
        }
    }

    false
}

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

/// Bundles the non-path parameters for [`analyze_file`].
pub(crate) struct AnalyzeContext<'a> {
    pub root_canon: &'a Path,
    pub extensions: Option<&'a HashSet<String>>,
    pub ts_resolver: Option<&'a TsPathResolver>,
    pub py_roots: &'a [PathBuf],
    pub py_stdlib: &'a HashSet<String>,
    pub symbol: Option<&'a str>,
    pub custom_command_macros: &'a [String],
    pub command_cfg: &'a CommandDetectionConfig,
}

/// Plan 19 Stage 1 parser knob. Read from `LOCTREE_PARSER` env override or the
/// `analyzer.parser` field in `.loctree/config.toml` via
/// [`crate::config::LoctreeConfig::parser_strategy`]. We keep this resolution
/// inside `scan` (rather than threading the value through `AnalyzeContext`) so
/// every existing call site stays compatible while Stage 1 is opt-in.
fn ts_parser_strategy_active() -> bool {
    if let Ok(env) = std::env::var("LOCTREE_PARSER") {
        return matches!(
            env.trim().to_ascii_lowercase().as_str(),
            "ts" | "tree-sitter" | "treesitter"
        );
    }
    false
}

fn is_scan_only_resource_extension(ext: &str) -> bool {
    matches!(
        ext,
        "md" | "markdown"
            | "mdx"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "storyboard"
            | "xib"
            | "properties"
            | "xml"
            | "svg"
            | "txt"
    )
}

/// Plan 19 Stage 1 — tree-sitter dispatch for JS/TS cold-scan files.
///
/// Produces a `FileAnalysis` populated from `loctree-ast` extractors:
/// `imports`, `exports`, and `symbol_usages` (call sites). Tauri command
/// metadata, dynamic imports, and reexports remain Stage 2 follow-ups; the
/// returned analysis carries empty values for those fields, which is
/// faithful to the parser strategy promise (opt-in, parity not yet 100%).
///
/// The parity contract is documented in
/// `internal-artifacts/reports/lsp/19-cross-lang-stage-1.md`.
pub fn ts_dispatch_js(content: &str, path: &Path, relative: String) -> FileAnalysis {
    use crate::types::{
        ExportSymbol as LExport, ImportEntry as LImport, ImportKind, ImportResolutionKind,
        ImportSymbol as LImportSym, SymbolUsage,
    };

    let parsers = TsParsers::new_default();
    let tree = match parsers.parse_path(path, content.as_bytes()) {
        Ok(t) => t,
        Err(_) => {
            return FileAnalysis {
                path: relative,
                ..FileAnalysis::default()
            };
        }
    };

    // Pick the right extractor by language tag, mirroring the registry.
    let extracted_exports;
    let extracted_imports;
    let extracted_calls;
    match tree.lang {
        "typescript" => {
            let ext = loctree_ast::TsExtractor;
            extracted_exports = ext.extract_exports(&tree);
            extracted_imports = ext.extract_imports(&tree);
            extracted_calls = ext.extract_calls(&tree);
        }
        "tsx" => {
            let ext = loctree_ast::extractors::ts::TsxExtractor;
            extracted_exports = ext.extract_exports(&tree);
            extracted_imports = ext.extract_imports(&tree);
            extracted_calls = ext.extract_calls(&tree);
        }
        "javascript" => {
            let ext = loctree_ast::JsExtractor;
            extracted_exports = ext.extract_exports(&tree);
            extracted_imports = ext.extract_imports(&tree);
            extracted_calls = ext.extract_calls(&tree);
        }
        _ => {
            return FileAnalysis {
                path: relative,
                ..FileAnalysis::default()
            };
        }
    }

    let exports: Vec<LExport> = extracted_exports
        .into_iter()
        .map(|e| LExport::new(e.name, e.kind.as_str(), e.export_type.as_str(), e.line))
        .collect();

    let imports: Vec<LImport> = extracted_imports
        .into_iter()
        .map(|i| {
            let symbols: Vec<LImportSym> = i
                .symbols
                .iter()
                .map(|b| LImportSym {
                    name: b.imported.clone().unwrap_or_else(|| b.local_name.clone()),
                    alias: b.imported.as_ref().map(|_| b.local_name.clone()),
                    is_default: b.is_default,
                })
                .collect();
            let kind = if symbols.is_empty() {
                ImportKind::SideEffect
            } else {
                ImportKind::Static
            };
            let mut entry = LImport::new(i.source.clone(), kind);
            entry.line = i.line;
            entry.symbols = symbols;
            entry.is_bare = !i.source.starts_with('.') && !i.source.starts_with('/');
            entry.resolution = if entry.is_bare {
                ImportResolutionKind::Unknown
            } else {
                ImportResolutionKind::Local
            };
            entry
        })
        .collect();

    let symbol_usages: Vec<SymbolUsage> = extracted_calls
        .into_iter()
        .map(|c| SymbolUsage {
            name: c.name,
            line: c.line,
            context: c.callee,
        })
        .collect();

    FileAnalysis {
        path: relative,
        imports,
        exports,
        symbol_usages,
        ..FileAnalysis::default()
    }
}

/// Read a file as UTF-8 after re-asserting that its canonical form is a
/// descendant of `allowed_root`.
///
/// SaaS-safety helper for [`analyze_file`]: callers have typically already
/// validated `path` against `allowed_root` upstream, but Semgrep's
/// `tainted-path` analysis only follows local data-flow. The
/// [`crate::fs_utils::SanitizedPath`] gate inside
/// `read_to_string_within` re-runs canonicalize + `starts_with`
/// immediately before `fs::read_to_string` so the boundary guard sits at
/// the same call site as the I/O sink.
fn read_file_within_root(allowed_root: &Path, path: &Path) -> io::Result<String> {
    crate::fs_utils::read_to_string_within(allowed_root, path)
}

pub(crate) fn analyze_file(path: &Path, ctx: &AnalyzeContext) -> io::Result<FileAnalysis> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(ctx.root_canon) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "analyzed file escapes provided root",
        ));
    }

    // Check if file is binary and skip with warning
    if is_binary_file(&canonical) {
        eprintln!(
            "[loctree][warn] Skipping binary file: {}",
            canonical.display()
        );
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "binary file skipped",
        ));
    }

    // SaaS-safety: `ctx.root_canon` arrives from `--root`, `LOCT_CACHE_DIR`,
    // or the MCP payload, so even though `canonical` was checked against it
    // 20-odd lines above, that guard is invisible to Semgrep's local
    // `tainted-path` data-flow analysis. `read_file_within_root` re-runs
    // canonicalize + `starts_with` immediately before the `read_to_string`
    // sink so the boundary guard sits at the same call site as the I/O.
    let content = match read_file_within_root(ctx.root_canon, &canonical) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            eprintln!(
                "[loctree][warn] Skipping file with invalid UTF-8: {}",
                canonical.display()
            );
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    let relative = canonical
        .strip_prefix(ctx.root_canon)
        .unwrap_or(&canonical)
        .to_string_lossy()
        .to_string();
    let loc = content.lines().count();
    let ext = canonical
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    // Filename-aware resolution: Makefile-family files have no extension, so
    // we look at the basename before falling back to the extension map. Same
    // goes for extensionless executable entrypoints with known source shebangs.
    let filename = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let is_makefile = is_makefile_name(filename);
    let is_loctree_config = crate::fs_utils::is_loctree_config_filename(filename);
    let is_extensionless_shell = ext.is_empty() && has_shell_shebang(&content);
    let shebang_ext = if ext.is_empty() {
        content
            .lines()
            .next()
            .and_then(crate::fs_utils::shebang_source_extension)
    } else {
        None
    };
    let dispatch_ext = shebang_ext.unwrap_or(ext.as_str());

    let mut analysis = if is_loctree_config {
        FileAnalysis::new(relative.clone())
    } else if is_scan_only_resource_extension(dispatch_ext) && resource_kind(&relative).is_some() {
        FileAnalysis::new(relative)
    } else if is_makefile || dispatch_ext == "mk" || dispatch_ext == "make" {
        analyze_makefile(&content, relative)
    } else if dispatch_ext == "sh"
        || dispatch_ext == "bash"
        || dispatch_ext == "zsh"
        || dispatch_ext == "fish"
        || is_extensionless_shell
    {
        analyze_shell_file(&content, relative)
    } else if dispatch_ext == "zig" || dispatch_ext == "zon" {
        analyze_zig_file(&content, relative)
    } else if dispatch_ext == "rb" {
        FileAnalysis::new(relative)
    } else {
        match dispatch_ext {
            "rs" => analyze_rust_file(&content, relative, ctx.custom_command_macros),
            "css" => analyze_css_file(&content, relative),
            "py" => analyze_py_file(
                &content,
                &canonical,
                ctx.root_canon,
                ctx.extensions,
                relative,
                ctx.py_roots,
                ctx.py_stdlib,
            ),
            "go" => crate::analyzer::go::analyze_go_file(&content, relative),
            "swift" | "m" | "mm" | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" => {
                crate::analyzer::c_family_syntax::analyze_c_family_file(
                    &content,
                    relative,
                    dispatch_ext,
                )
            }
            "dart" => analyze_dart_file(&content, relative),
            "kt" | "kts" => FileAnalysis::new(relative),
            "html" | "htm" => analyze_html_file(
                &content,
                &canonical,
                ctx.root_canon,
                ctx.extensions,
                ctx.ts_resolver,
                relative,
                ctx.command_cfg,
            ),
            _ => {
                if ts_parser_strategy_active()
                    && matches!(
                        ext.as_str(),
                        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "cts" | "mts"
                    )
                {
                    ts_dispatch_js(&content, &canonical, relative)
                } else {
                    analyze_js_file(
                        &content,
                        &canonical,
                        ctx.root_canon,
                        ctx.extensions,
                        ctx.ts_resolver,
                        relative,
                        ctx.command_cfg,
                    )
                }
            }
        }
    };

    if let Some(sym) = ctx.symbol {
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
    analysis.language = if !dispatch_ext.is_empty() {
        detect_language(dispatch_ext)
    } else if is_makefile {
        "make".to_string()
    } else if is_extensionless_shell {
        "shell".to_string()
    } else {
        detect_language_from_filename(filename)
    };
    let (kind, is_test, is_generated) = file_kind(&analysis.path);
    analysis.kind = kind;
    analysis.resource_kind = resource_kind(&analysis.path).map(str::to_string);
    analysis.is_test = is_test;
    analysis.is_generated = is_generated;

    if filename == "Cargo.toml" {
        if let Ok(workspace) = parse_workspace_root(&canonical) {
            for member in workspace.resolved_members {
                analysis.cargo_targets.extend(member.targets);
            }
        } else if let Ok(manifest) = parse_crate_manifest(&canonical) {
            analysis.crate_membership = Some(manifest.package_name);
            analysis.cargo_targets = manifest.targets;
        }
    } else if ext == "rs"
        && let Some(manifest_path) = find_nearest_crate_manifest(&canonical, ctx.root_canon)
        && let Ok(manifest) = parse_crate_manifest(&manifest_path)
    {
        analysis.crate_membership = Some(manifest.package_name);
    }

    // Resolve Rust imports and reexports
    if ext == "rs" {
        let crate_root = find_rust_crate_root(&canonical);
        if let Some(ref crate_root) = crate_root {
            // Resolve imports
            for imp in analysis.imports.iter_mut() {
                if imp.resolved_path.is_none() {
                    imp.resolved_path =
                        resolve_rust_import(&imp.source, &canonical, crate_root, ctx.root_canon);
                }
            }
            // Resolve reexports (pub use statements)
            // This is critical for dead code detection - reexported symbols are NOT dead
            for re in analysis.reexports.iter_mut() {
                if re.resolved.is_none() {
                    re.resolved =
                        resolve_rust_import(&re.source, &canonical, crate_root, ctx.root_canon);
                }
            }
        }
    }

    // Resolve other language imports (relative paths).
    // For shell/make/zig the source may or may not start with `.` — their
    // resolvers accept any best-effort relative path.
    for imp in analysis.imports.iter_mut() {
        if imp.resolved_path.is_some() {
            continue;
        }

        // Shell/make/zig: try filesystem-relative resolution regardless of
        // leading dot. These never recurse into node_modules or stdlib trees.
        if is_makefile || ext == "mk" || ext == "make" {
            imp.resolved_path = resolve_makefile_include(&imp.source, &canonical, ctx.root_canon);
            if imp.resolved_path.is_some() {
                imp.resolution = crate::types::ImportResolutionKind::Local;
            }
            continue;
        }
        if ext == "sh" || ext == "bash" || ext == "zsh" || ext == "fish" || is_extensionless_shell {
            imp.resolved_path = resolve_shell_source(&imp.source, &canonical, ctx.root_canon);
            if imp.resolved_path.is_some() {
                imp.resolution = crate::types::ImportResolutionKind::Local;
            }
            continue;
        }
        if ext == "zig" || ext == "zon" {
            // `std`/`builtin`/`root` are already marked Stdlib at parse time.
            if matches!(imp.source.as_str(), "std" | "builtin" | "root") {
                continue;
            }
            imp.resolved_path = resolve_zig_import(&imp.source, &canonical, ctx.root_canon);
            if imp.resolved_path.is_some() {
                imp.resolution = crate::types::ImportResolutionKind::Local;
            }
            continue;
        }

        if imp.source.starts_with('.') {
            let resolved = match ext.as_str() {
                "py" => {
                    resolve_python_relative(&imp.source, &canonical, ctx.root_canon, ctx.extensions)
                }
                "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "css" | "svelte" | "vue"
                | "astro" | "html" | "htm" => ctx
                    .ts_resolver
                    .and_then(|r| r.resolve(&imp.source, ctx.extensions))
                    .or_else(|| {
                        resolve_js_relative(&canonical, ctx.root_canon, &imp.source, ctx.extensions)
                    }),
                _ => None,
            };
            imp.resolved_path = resolved;
        }
    }

    Ok(analysis)
}

/// Expand gather_files with gitignore handling. Returns the list of files and the visited set.
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
    fn workspace_detection_sets_crate_membership_and_manifest_targets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/api/src")).expect("crate dirs");
        std::fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["crates/*"]
"#,
        )
        .expect("workspace manifest");
        std::fs::write(
            root.join("crates/api/Cargo.toml"),
            r#"[package]
name = "api"
"#,
        )
        .expect("crate manifest");
        let source = root.join("crates/api/src/lib.rs");
        std::fs::write(&source, "pub fn api() {}\n").expect("source");

        let py_stdlib = HashSet::new();
        let command_cfg = CommandDetectionConfig::default();
        let root_canon = root.canonicalize().expect("canonical root");
        let ctx = AnalyzeContext {
            root_canon: &root_canon,
            extensions: None,
            ts_resolver: None,
            py_roots: &[],
            py_stdlib: &py_stdlib,
            symbol: None,
            custom_command_macros: &[],
            command_cfg: &command_cfg,
        };

        let analysis = analyze_file(&source, &ctx).expect("analyze source");
        assert_eq!(analysis.crate_membership.as_deref(), Some("api"));

        let manifest = analyze_file(&root.join("Cargo.toml"), &ctx).expect("analyze workspace");
        assert!(
            manifest
                .cargo_targets
                .iter()
                .any(|target| target.name == "api")
        );
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
                is_dynamic: false,
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
                    line: None,
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
                    is_mod_declaration: false,
                }],
                event_listens: vec![crate::types::EventRef {
                    raw_name: Some("EVENT_NAME".to_string()),
                    name: "EVENT_NAME".to_string(),
                    line: 20,
                    kind: "listen_ident".to_string(),
                    awaited: false,
                    payload: None,
                    is_dynamic: false,
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
                    is_dynamic: false,
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
                is_dynamic: false,
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
                    line: None,
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
                    is_mod_declaration: false,
                }],
                event_emits: vec![crate::types::EventRef {
                    raw_name: Some("ALIASED".to_string()),
                    name: "ALIASED".to_string(),
                    line: 30,
                    kind: "emit_ident".to_string(),
                    awaited: false,
                    payload: None,
                    is_dynamic: false,
                }],
                ..Default::default()
            },
        ];
        resolve_event_constants_across_files(&mut analyses);
        assert_eq!(analyses[1].event_emits[0].name, "aliased:event");
    }
}
