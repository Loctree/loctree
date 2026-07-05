//! Filter functions for dead export detection

use globset::GlobSet;
use serde::{Deserialize, Serialize};

use crate::semantic::{Classifier, ReachReason, SemanticFacts};
use crate::types::{ExportSymbol, FileAnalysis};

use super::{DeadExport, DeadFilterConfig};

/// Counters captured while applying semantic-fact suppression to a
/// `Vec<DeadExport>`. Surfaced in `Findings` so operators can audit how much
/// noise the Layer 3 analyzers absorbed without changing core dead-export
/// detection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SuppressionCounts {
    /// Symbol dropped because Layer 3 classified it as an idiom (`usage`,
    /// `die`, sourced helper, public entrypoint, env var, …).
    pub idiom_suppressed: u32,
    /// Symbol dropped because it was a Make `.PHONY` target or other Make
    /// metadata classified as `Metadata` / `PublicEntrypoint`.
    pub make_metadata_suppressed: u32,
    /// Symbol dropped because Layer 3 reachability marked it as reached via
    /// a shell `case ... esac` dispatch handler.
    pub shell_reachable_by_dispatch: u32,
}

impl SuppressionCounts {
    pub fn total(&self) -> u32 {
        self.idiom_suppressed + self.make_metadata_suppressed + self.shell_reachable_by_dispatch
    }
}

/// Drop dead-export candidates that Layer 3 semantic analysis reclassified as
/// reached or idiomatic, and bump the matching counter for each drop.
///
/// No-op when `facts` is empty (no shell or make in the scan) — preserves
/// behaviour for repos without a Layer 3 analyzer.
pub fn apply_semantic_suppression(
    candidates: Vec<DeadExport>,
    facts: &SemanticFacts,
    counts: &mut SuppressionCounts,
) -> Vec<DeadExport> {
    if facts.idiom_tags.is_empty() && facts.reachability.reached_symbols.is_empty() {
        return candidates;
    }

    candidates
        .into_iter()
        .filter(|candidate| keep_after_semantic(candidate, facts, counts))
        .collect()
}

fn keep_after_semantic(
    candidate: &DeadExport,
    facts: &SemanticFacts,
    counts: &mut SuppressionCounts,
) -> bool {
    let symbol_id = format!("{}::{}", candidate.file, candidate.symbol);

    if facts.reachability.reached_symbols.contains(&symbol_id) {
        match facts.reachability.reasons.get(&symbol_id) {
            Some(ReachReason::DispatchHandler { .. }) => {
                counts.shell_reachable_by_dispatch += 1;
            }
            Some(ReachReason::PhonyMakeTarget) => {
                counts.make_metadata_suppressed += 1;
            }
            Some(ReachReason::SourceInclude { .. })
            | Some(ReachReason::IdiomRuntimeRole(_))
            | Some(ReachReason::DirectImport)
            | Some(ReachReason::RecipeShellCall { .. })
            | Some(ReachReason::Unknown)
            | None => {
                counts.idiom_suppressed += 1;
            }
        }
        return false;
    }

    if let Some(tags) = facts.idiom_tags.get(&symbol_id) {
        let is_idiom = tags.iter().any(|tag| {
            matches!(
                tag.classifier,
                Classifier::HelpPrinter
                    | Classifier::ErrorExit
                    | Classifier::LibraryHelper
                    | Classifier::EnvVar
                    | Classifier::EnvContract
                    | Classifier::Metadata
                    | Classifier::PublicEntrypoint
                    | Classifier::PrimaryEntrypoint
                    | Classifier::UserFacingEntrypoint
                    | Classifier::SourceLibraryApi
                    | Classifier::DispatchHandler
            )
        });

        if is_idiom {
            let make_metadata = tags.iter().any(|tag| {
                matches!(
                    tag.classifier,
                    Classifier::Metadata | Classifier::PublicEntrypoint
                )
            });
            if make_metadata && is_make_path(&candidate.file) {
                counts.make_metadata_suppressed += 1;
            } else {
                counts.idiom_suppressed += 1;
            }
            return false;
        }
    }

    true
}

fn is_make_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("/makefile")
        || lower.ends_with("\\makefile")
        || lower == "makefile"
        || lower.ends_with(".mk")
        || lower.ends_with(".make")
}

pub(super) fn should_skip_dead_export_check(
    analysis: &FileAnalysis,
    config: &DeadFilterConfig,
    example_globs: Option<&GlobSet>,
) -> bool {
    let path = &analysis.path;
    let lower_path = path.to_ascii_lowercase();

    // Artifact fence (shared classification, see analyzer::classify):
    // vendored/minified/generated/template files are never dead-export
    // candidates; fixtures follow the include_tests convention.
    match crate::analyzer::classify::artifact_class(path, None) {
        crate::analyzer::classify::ArtifactClass::Product => {}
        crate::analyzer::classify::ArtifactClass::Fixture => {
            if !config.include_tests {
                return true;
            }
        }
        _ => return true,
    }

    // Go exports are primarily public API; static import graph is insufficient (FP-heavy)
    // Skip dead-export detection for Go to avoid noise.
    if analysis.language == "go" {
        return true;
    }

    // JSX runtime files - exports consumed by TypeScript/Babel compiler, not by imports
    // Files matching: *jsx-runtime*, jsx-runtime.js, jsx-runtime/index.js, jsx-dev-runtime.js, etc.
    if (analysis.language == "ts" || analysis.language == "js")
        && (lower_path.contains("jsx-runtime")
            || lower_path.contains("jsx_runtime")
            || lower_path.contains("jsx-dev-runtime"))
    {
        return true;
    }

    // Test files and fixtures
    if analysis.is_test && !config.include_tests {
        return true;
    }

    // Flutter generated/plugin registrant files
    if path.ends_with("generated_plugin_registrant.dart")
        || path.contains("/generated_plugin_registrant.dart")
    {
        return true;
    }

    // Test-related directories
    const TEST_DIRS: &[&str] = &[
        "stories",
        "__tests__",
        "__mocks__",
        "__fixtures__",
        "/cypress/",
        "/e2e/",
        "/playwright/",
        "/test/",
        "/tests/",
        "/spec/",
    ];
    if TEST_DIRS.iter().any(|d| lower_path.contains(d)) && !config.include_tests {
        return true;
    }

    // Example/demo/fixture packages (library-mode noise)
    const EXAMPLE_DIRS: &[&str] = &[
        "/examples/",
        "/example/",
        "/samples/",
        "/sample/",
        "/demo/",
        "/demos/",
        "/playground/",
        "/showcase/",
    ];
    if EXAMPLE_DIRS.iter().any(|d| lower_path.contains(d)) {
        return true;
    }
    if config.library_mode {
        if let Some(globs) = example_globs
            && (globs.is_match(path) || globs.is_match(&lower_path))
        {
            return true;
        }
        const LIBRARY_NOISE_DIRS: &[&str] = &[
            "/kitchen-sink/",
            "/kitchensink/",
            "/sandbox/",
            "/sandboxes/",
            "/cookbook/",
            "/gallery/",
            "/examples-",
            "/examples_",
            "/docs/examples/",
            "/documentation/examples/",
        ];
        if LIBRARY_NOISE_DIRS.iter().any(|d| lower_path.contains(d))
            || lower_path.starts_with("examples/")
            || lower_path.starts_with("example/")
            || lower_path.starts_with("demo/")
            || lower_path.contains("/examples/")
        {
            return true;
        }
    }
    // Lowercase check for testfixtures (common in codemods)
    if lower_path.contains("testfixtures") {
        return true;
    }

    // TypeScript declaration files (.d.ts) - only contain type declarations
    if path.ends_with(".d.ts") {
        return true;
    }

    // Dart/Flutter generated artifacts
    if path.ends_with(".g.dart")
        || path.ends_with(".freezed.dart")
        || path.ends_with(".gr.dart")
        || path.ends_with(".pb.dart")
        || path.ends_with(".pbjson.dart")
        || path.ends_with(".pbenum.dart")
        || path.ends_with(".pbserver.dart")
        || path.ends_with(".config.dart")
    {
        return true;
    }

    // Config files loaded dynamically by build tools (Vite, Jest, Cypress, etc.)
    if path.contains(".config.") || path.ends_with(".config.ts") || path.ends_with(".config.js") {
        return true;
    }

    if !config.include_helpers {
        const HELPER_DIRS: &[&str] = &["/scripts/", "/script/", "/tools/", "/docs/"];
        if HELPER_DIRS.iter().any(|d| path.contains(d))
            || path.starts_with("scripts/")
            || path.starts_with("script/")
            || path.starts_with("tools/")
            || path.starts_with("docs/")
        {
            return true;
        }

        if analysis.language == "shell" && is_shell_operator_glue_path(path) {
            return true;
        }
    }

    // Framework routing/entry point conventions
    // SvelteKit: +page.ts, +layout.ts, +server.ts, +page.server.ts, hooks.*.ts
    // Next.js: page.tsx, layout.tsx, route.ts (in app/ directory)
    const FRAMEWORK_ENTRY_PATTERNS: &[&str] = &[
        "+page.",
        "+layout.",
        "+server.",
        "+error.",
        "/page.tsx",
        "/page.ts",
        "/layout.tsx",
        "/layout.ts",
        "/route.ts",
        "/route.tsx",
        "/error.tsx",
        "/loading.tsx",
        "/not-found.tsx",
        // SvelteKit hooks (auto-loaded by framework)
        "hooks.client.",
        "hooks.server.",
        "/hooks.",
    ];
    if FRAMEWORK_ENTRY_PATTERNS.iter().any(|p| path.contains(p)) {
        return true;
    }

    // Virtual/module-runtime entrypoints (framework-provided consumers)
    const RUNTIME_PATTERNS: &[&str] =
        &["/.svelte-kit/", "/runtime/", "/app/router/", "/app/routes/"];
    if (analysis.language == "ts" || analysis.language == "js")
        && RUNTIME_PATTERNS.iter().any(|p| path.contains(p))
    {
        return true;
    }

    // Library barrels and public API surfaces (avoid flagging public exports)
    if (path.ends_with("/index.ts")
        || path.ends_with("/index.tsx")
        || path.ends_with("/index.js")
        || path.ends_with("/index.mjs")
        || path.ends_with("/index.cjs")
        || path.ends_with("/mod.ts")
        || path.ends_with("/mod.js"))
        && (analysis.language == "ts" || analysis.language == "js")
        && (path.contains("/packages/") || path.contains("/libs/") || path.contains("/library/"))
    {
        return true;
    }

    false
}

fn is_shell_operator_glue_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    lower.starts_with("ai-hooks/")
        || lower.starts_with("hooks/")
        || lower.contains("/hooks/")
        || lower.starts_with("completions/")
        || lower.contains("/completions/")
}

/// Check if an export from a Svelte file is likely a component API method.
/// Svelte components expose methods via `export function` that are called via `bind:this`:
///   let modal: MyModal;
///   <MyModal bind:this={modal} />
///   modal.show();  // calling the exported function
///
/// These are NOT imported via ES imports, so they appear as "dead" in static analysis.
pub(super) fn is_jsx_runtime_export(export_name: &str, file_path: &str) -> bool {
    // JSX runtime export names defined by React JSX transform spec
    const JSX_RUNTIME_EXPORTS: &[&str] = &["jsx", "jsxs", "jsxDEV", "jsxsDEV", "Fragment"];

    if !JSX_RUNTIME_EXPORTS.contains(&export_name) {
        return false;
    }

    // Check if file is likely a JSX runtime
    // Patterns: jsx-runtime, jsx_runtime, jsx-dev-runtime (React dev mode)
    let lower_path = file_path.to_ascii_lowercase();
    lower_path.contains("jsx-runtime")
        || lower_path.contains("jsx_runtime")
        || lower_path.contains("jsx-dev-runtime")
}

/// Check if an export is likely a Flow type-only export.
/// Flow files (annotated with @flow) export types that are used via Flow's type system,
/// not via regular import statements. These exports don't appear in static import analysis
pub(super) fn is_flow_type_export(export_symbol: &ExportSymbol, analysis: &FileAnalysis) -> bool {
    if !analysis.is_flow_file {
        return false;
    }

    // Flow type exports: type, interface, opaque type
    // These are type-only and won't appear in runtime imports
    matches!(export_symbol.kind.as_str(), "type" | "interface" | "opaque")
}

/// Check if an export is used in a WeakMap/WeakSet registry pattern.
/// These are common in React and other libraries for storing metadata about objects
/// without causing memory leaks. Exports stored in WeakMap/WeakSet are used dynamically.
pub(super) fn is_weakmap_registry_export(
    _export_symbol: &ExportSymbol,
    analysis: &FileAnalysis,
) -> bool {
    // If a file contains WeakMap/WeakSet usage (detected by AST visitor),
    // conservatively assume all exports might be stored dynamically in the registry.
    // This reduces false positives in React DevTools and similar code where exports
    // are stored in WeakMaps for dynamic lookup.
    analysis.has_weak_collections
}
pub(super) fn is_python_test_export(analysis: &FileAnalysis, exp: &ExportSymbol) -> bool {
    if !analysis.path.ends_with(".py") {
        return false;
    }
    if exp.kind == "class" && exp.name.starts_with("Test") {
        return true;
    }
    if exp.kind == "def" && exp.name.starts_with("test_") {
        return true;
    }
    false
}

pub(super) fn is_python_test_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.ends_with("_test.py")
        || lower.ends_with("_tests.py")
        || lower
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("test_"))
}

/// Check if a TypeScript/JavaScript file contains ambient declaration patterns.
/// Ambient declarations (declare global, declare module, declare namespace) are
/// consumed by the TypeScript compiler, not by imports. Exports inside these
/// blocks are NOT dead code - they extend global types or module augmentations.
///
/// Returns true if the file contains ambient declaration patterns.
pub(super) fn has_ambient_declarations(analysis: &FileAnalysis) -> bool {
    // Only check TypeScript files
    if analysis.language != "ts" {
        return false;
    }

    // If we have raw file content cached, check for ambient patterns
    // Note: This is a heuristic based on file path patterns since we don't
    // have direct access to file content here. A more precise approach would
    // be to track this during AST parsing.

    let path = &analysis.path;
    let lower_path = path.to_ascii_lowercase();

    // Common patterns for files that contain ambient declarations:
    // 1. Files ending with .d.ts are type declaration files (already handled separately)
    // 2. Files in jsx-runtime directories often contain declare global for JSX namespace
    // 3. Files named global*.ts or globals.ts often contain declare global
    // 4. Files with "types" or "typings" in the path often contain ambient declarations

    // Check for jsx-runtime patterns (Vue, React, etc.)
    if lower_path.contains("jsx-runtime") || lower_path.contains("jsx_runtime") {
        return true;
    }

    // Check for common global type definition file names
    let file_name = path
        .rsplit('/')
        .next()
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    if file_name.starts_with("global")
        || file_name == "globals.ts"
        || file_name == "env.d.ts"
        || file_name == "global.d.ts"
        || file_name == "index.d.ts"
        || file_name == "shim.d.ts"
        || file_name == "shims.d.ts"
        || file_name.contains("augment")
    {
        return true;
    }

    // Check for paths that typically contain type augmentations
    if lower_path.contains("/types/") || lower_path.contains("/typings/") {
        return true;
    }

    false
}

/// Check if a specific export is likely an ambient declaration export.
/// This checks if the export is a type/interface that's commonly found in
/// ambient declaration contexts (like JSX namespace interfaces).
pub(super) fn is_ambient_export(export_symbol: &ExportSymbol, analysis: &FileAnalysis) -> bool {
    // Only applies to TypeScript
    if analysis.language != "ts" {
        return false;
    }

    // Check if file has ambient declaration patterns
    if !has_ambient_declarations(analysis) {
        return false;
    }

    // In ambient declaration contexts, type exports are compiler-consumed
    matches!(
        export_symbol.kind.as_str(),
        "interface" | "type" | "namespace"
    )
}

/// Check if an export name matches a dynamically generated pattern from exec/eval/compile.
/// These are template placeholders like "get%s", "set%s" that generate symbols at runtime.
///
/// This detects patterns from CPython and similar projects that use exec() with template
/// strings to generate accessor methods, classes, etc.
///
/// Example pattern:
/// ```python
/// exec("def get%s(self): return self._%s" % (name, name))
/// ```
/// When `name` could be "foo", "bar", etc., the exports "getfoo", "getbar" are NOT dead code.
pub(super) fn is_dynamic_exec_template(export_name: &str, analysis: &FileAnalysis) -> bool {
    // Only applies to Python files
    if !analysis.path.ends_with(".py") {
        return false;
    }

    // Check if file has any dynamic exec templates
    if analysis.dynamic_exec_templates.is_empty() {
        return false;
    }

    // Check if the export name could match any template pattern
    for template in &analysis.dynamic_exec_templates {
        // Check against generated prefixes (e.g., "get", "set")
        for prefix in &template.generated_prefixes {
            // Export name starts with the prefix (e.g., "getfoo" starts with "get")
            if export_name.starts_with(prefix) {
                return true;
            }
        }

        // Also check if the template pattern itself matches
        // Template contains patterns like "get%s" or "set{name}"
        // The export could be "get_something" or "set_something"
        let template_lower = template.template.to_lowercase();

        // Common patterns: "def get%s", "def set%s", "class %s"
        if template_lower.contains("def ") {
            // Extract the function name pattern
            if let Some(def_pos) = template_lower.find("def ") {
                let after_def = &template_lower[def_pos + 4..];
                // Find the pattern before the format specifier
                if let Some(format_pos) = after_def.find('%').or(after_def.find('{')) {
                    let pattern_prefix = after_def[..format_pos].trim();
                    if !pattern_prefix.is_empty()
                        && export_name.to_lowercase().starts_with(pattern_prefix)
                    {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Check if a file uses sys.modules monkey-patching.
/// If a file injects itself into sys.modules (e.g., `sys.modules['compat'] = wrapper`),
/// ALL exports from that file are accessible at runtime via the injected module name.
/// Therefore, none of its exports should be flagged as dead code.
///
/// Example:
/// ```python
/// # compat.py
/// import sys
/// class CompatWrapper:
///     # ... wrapper logic
/// sys.modules['compat'] = CompatWrapper(sys.modules[__name__])
/// ```
///
/// Even if `CompatWrapper` has no direct imports, it's accessible via `import compat`.
pub(super) fn has_sys_modules_injection(analysis: &FileAnalysis) -> bool {
    // Only applies to Python files
    if !analysis.path.ends_with(".py") {
        return false;
    }

    // If file has any sys.modules injections, all its exports are "alive"
    !analysis.sys_modules_injections.is_empty()
}

// ============================================================================
// Entry-point fence (W2-a): runtime entries never become delete candidates
// ============================================================================

fn fence_norm_path(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
}

/// Collect every path the snapshot knows to be a runtime entrypoint:
/// - per-file code markers (`fn main`, `__main__`, shebang scripts, ASGI/WSGI
///   apps, …) recorded in `FileAnalysis::entry_points`,
/// - aggregated `metadata.entrypoints`,
/// - manifest-declared roots (Cargo `[[bin]]`/`lib`, package.json
///   `main`/`module`/`bin`/`exports`, pyproject scripts) that resolve to an
///   existing file.
pub fn runtime_entrypoint_paths(
    snapshot: &crate::snapshot::Snapshot,
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();

    for entry in &snapshot.metadata.entrypoints {
        set.insert(fence_norm_path(&entry.path));
    }
    for file in &snapshot.files {
        if !file.entry_points.is_empty() {
            set.insert(fence_norm_path(&file.path));
        }
    }
    for summary in &snapshot.metadata.manifest_summary {
        for declared in crate::snapshot::collect_declared_entrypoints(summary) {
            if declared.resolved && declared.exists {
                set.insert(fence_norm_path(&declared.path));
            }
        }
    }

    set
}

/// Disk probe for entry markers the per-language analyzers may not record:
/// shebang executables and Swift `@main`. Reads at most the file head.
pub fn probe_entrypoint_marker(full_path: &std::path::Path, rel_path: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(full_path) else {
        return false;
    };
    if content.starts_with("#!") {
        return true;
    }
    if rel_path.ends_with(".swift") {
        return content
            .lines()
            .any(|line| line.trim_start().starts_with("@main"));
    }
    false
}
