//! Layer 3 semantic analyzer for Python.
//!
//! Inputs:  FileAnalysis (Layer 1 sensor) + IdiomRegistry (T0)
//! Outputs: SemanticFacts (T0 contract)
//!
//! Five passes:
//!   1. Idiom classification (per-symbol, by name and alias)
//!   2. Framework decorator reachability (FastAPI / Flask / pytest /
//!      Pydantic / Click / Typer / Celery / Django) emits dispatch edges
//!   3. pytest test discovery (`test_*.py`, `*_test.py`, anything under
//!      `tests/` or `test/` directories) marks `def test_*` functions reached
//!   4. Module-level entrypoint detection (`if __name__ == "__main__":`) marks
//!      `main` reached when present
//!   5. Reachability propagation (idiom roles + decorator handlers + pytest
//!      discovery + main entrypoint)
//!
//! Layer 1 sensor at `analyzer/py/` is read-only from this module — we only
//! consume `FileAnalysis::exports` and re-read source content via the validated
//! semantic I/O layer.
//!
//! Out-of-scope (deferred to later cuts):
//!   - Type-aware reachability (Pyright / mypy integration)
//!   - AsyncIO event-loop reachability beyond decorator surface
//!   - Django URL conf parsing (urls.py path() / re_path())
//!   - Tree-sitter migration (Cut 2.5+; current implementation is regex)

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::semantic::{
    DispatchEdge, DispatchKind, EnvContract, EnvContractOccurrence, IdiomRegistry, IdiomTag,
    ReachReason, RuntimeRole, RuntimeSemanticAnalyzer, SemanticFacts, SymbolId, TagSource,
};
use crate::types::{FileAnalysis, Language};

/// `FileAnalysis::language` is a string. Layer 1 emits "py" via
/// `analyzer/classify.rs` for every Python source file. We accept the legacy
/// "python" form too in case a custom upstream emits it.
const LANG_STRS: &[&str] = &["py", "python"];

pub struct PythonRuntimeSemantics;

impl RuntimeSemanticAnalyzer for PythonRuntimeSemantics {
    fn language(&self) -> Language {
        Language::Python
    }

    fn analyze(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) -> anyhow::Result<()> {
        for file in files {
            if !is_python(&file.language) {
                continue;
            }

            self.classify_idioms(file, registry, out);

            // Decorator + entrypoint passes need source content. A file that
            // vanished between scan and analysis is a Living Tree race, not a
            // bug — skip it silently. Validation failures other than NotFound
            // still propagate as errors.
            let Some(content) = crate::semantic::io::try_read_validated_semantic_input(&file.path)?
            else {
                continue;
            };
            let stripped = strip_comments_and_strings(&content);

            self.extract_decorator_dispatch(file, &stripped, out);
            self.discover_pytest_tests(file, out);
            self.detect_main_block(file, &stripped, out);
            self.extract_env_contracts(file, &stripped, out);
        }

        self.compute_reachability(files, out);
        Ok(())
    }
}

impl PythonRuntimeSemantics {
    fn classify_idioms(
        &self,
        file: &FileAnalysis,
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) {
        for export in &file.exports {
            let Some(entry) = registry.lookup(Language::Python, &export.name) else {
                continue;
            };
            let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
            out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                name: entry.name.clone(),
                classifier: entry.classifier.clone(),
                runtime_role: entry.runtime_role.clone(),
                source: TagSource::EmbeddedDefault,
                reasoning: entry.reasoning.clone(),
            });
        }
    }

    fn extract_decorator_dispatch(
        &self,
        file: &FileAnalysis,
        stripped_content: &str,
        out: &mut SemanticFacts,
    ) {
        for edge in parse_decorator_handlers(stripped_content, &file.path) {
            out.dispatch_edges.push(edge);
        }
    }

    fn discover_pytest_tests(&self, file: &FileAnalysis, out: &mut SemanticFacts) {
        if !is_pytest_test_file(&file.path) {
            return;
        }
        for export in &file.exports {
            // pytest collects functions whose name starts with `test_`, plus
            // `Test*` classes. We tag both — the runner reaches them without
            // any import edge from production code.
            let is_test_func = export.kind == "function" && export.name.starts_with("test_");
            let is_test_class = export.kind == "class" && export.name.starts_with("Test");
            if !is_test_func && !is_test_class {
                continue;
            }
            let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
            out.reachability.reached_symbols.insert(symbol_id.clone());
            out.reachability
                .reasons
                .entry(symbol_id)
                .or_insert(ReachReason::IdiomRuntimeRole(RuntimeRole::PublicEntrypoint));
        }
    }

    fn extract_env_contracts(
        &self,
        file: &FileAnalysis,
        stripped_content: &str,
        out: &mut SemanticFacts,
    ) {
        for occurrence in parse_env_var_reads(stripped_content, &file.path) {
            merge_env_occurrence(out, occurrence);
        }
    }

    fn detect_main_block(
        &self,
        file: &FileAnalysis,
        stripped_content: &str,
        out: &mut SemanticFacts,
    ) {
        if !RE_DUNDER_MAIN_GUARD.is_match(stripped_content) {
            return;
        }
        // The `if __name__ == "__main__":` guard turns the module into a
        // script. Any `main` symbol the file exports is reachable through
        // interpreter invocation even without an importer.
        for export in &file.exports {
            if export.name == "main" {
                let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
                out.reachability.reached_symbols.insert(symbol_id.clone());
                out.reachability
                    .reasons
                    .entry(symbol_id)
                    .or_insert(ReachReason::IdiomRuntimeRole(
                        RuntimeRole::PrimaryEntrypoint,
                    ));
            }
        }
    }

    fn compute_reachability(&self, files: &[FileAnalysis], out: &mut SemanticFacts) {
        // Mark dispatch_edge handler symbols as reached when they appear in
        // the file's own exports. Decorator handlers are always defined in the
        // same file the decorator lives in.
        //
        // The Python analyzer emits one of `HttpRoute`, `CliCommand`,
        // `EventHandler`, `TaskTarget`, or (for unrecognised callbacks)
        // `FunctionPointer` per decorator/handler pair — every one of these
        // marks the handler reachable through external invocation.
        let edges = out.dispatch_edges.clone();
        for edge in &edges {
            if !matches!(
                edge.dispatch_kind,
                DispatchKind::HttpRoute
                    | DispatchKind::CliCommand
                    | DispatchKind::EventHandler
                    | DispatchKind::TaskTarget
                    | DispatchKind::FunctionPointer
            ) {
                continue;
            }
            for f in files {
                if !is_python(&f.language) {
                    continue;
                }
                if f.path != edge.from_file {
                    continue;
                }
                if !f.exports.iter().any(|e| e.name == edge.handler_symbol) {
                    continue;
                }
                let symbol_id = format!("{}::{}", f.path, edge.handler_symbol);
                out.reachability.reached_symbols.insert(symbol_id.clone());
                out.reachability.reasons.insert(
                    symbol_id,
                    ReachReason::DispatchHandler {
                        from_symbol: edge.from_file.clone(),
                        dispatch_kind: edge.dispatch_kind.clone(),
                    },
                );
            }
        }

        // Idiom-tagged symbols whose runtime role implies external invocation
        // are reached even without a static import edge.
        let tag_pairs: Vec<(SymbolId, RuntimeRole)> = out
            .idiom_tags
            .iter()
            .flat_map(|(id, tags)| {
                tags.iter()
                    .map(move |t| (id.clone(), t.runtime_role.clone()))
            })
            .collect();
        for (symbol_id, role) in tag_pairs {
            if matches!(
                role,
                RuntimeRole::UserFacing
                    | RuntimeRole::PrimaryEntrypoint
                    | RuntimeRole::PublicEntrypoint
                    | RuntimeRole::EnvInput
                    | RuntimeRole::LibraryHelper
            ) {
                out.reachability.reached_symbols.insert(symbol_id.clone());
                out.reachability
                    .reasons
                    .entry(symbol_id)
                    .or_insert(ReachReason::IdiomRuntimeRole(role));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers — testable without disk access.
// ---------------------------------------------------------------------------

fn is_python(language: &str) -> bool {
    LANG_STRS.contains(&language)
}

/// Return true when the path looks like a pytest test file: filename matches
/// `test_*.py` or `*_test.py`, OR any path segment is `tests` / `test`.
fn is_pytest_test_file(path: &str) -> bool {
    let p = Path::new(path);
    let filename = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if filename.starts_with("test_") && filename.ends_with(".py") {
        return true;
    }
    if filename.ends_with("_test.py") {
        return true;
    }
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "tests" || s == "test"
    })
}

static RE_DECORATOR_LINE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?P<indent>\s*)@(?P<expr>.+)$").expect("valid decorator regex"));

static RE_DEF_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?P<indent>\s*)(?:async\s+)?def\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .expect("valid def regex")
});

static RE_DUNDER_MAIN_GUARD: Lazy<Regex> = Lazy::new(|| {
    // Match `if __name__ == "__main__":` with either quote flavour and any
    // whitespace flexibility. We strip strings before regex runs, but keep
    // the literal __main__ text by NOT stripping ALL strings — see
    // strip_comments_and_strings for the partial strategy.
    Regex::new(r#"(?m)^\s*if\s+__name__\s*==\s*['"]__main__['"]\s*:"#)
        .expect("valid dunder main regex")
});

// ---------------------------------------------------------------------------
// Env contract extraction
// ---------------------------------------------------------------------------

// Rust's regex crate does not support backreferences, so we cannot match
// `(['"])...\1` to enforce balanced quotes. Two patterns per call shape
// (double-quoted, single-quoted) keeps things simple and unambiguous.

/// Match `os.getenv("FOO")` or `os.getenv("FOO", "default")`.
/// Captures: 1 = env var name, 2 = remainder (presence detects default).
static RE_OS_GETENV_DQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\.getenv\(\s*"([A-Z_][A-Z0-9_]*)"\s*(,[^)]*)?\)"#)
        .expect("valid os.getenv regex (double-quoted)")
});

static RE_OS_GETENV_SQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\.getenv\(\s*'([A-Z_][A-Z0-9_]*)'\s*(,[^)]*)?\)"#)
        .expect("valid os.getenv regex (single-quoted)")
});

/// Match `os.environ.get("FOO")` or `os.environ.get("FOO", "default")`.
static RE_OS_ENVIRON_GET_DQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\.environ\.get\(\s*"([A-Z_][A-Z0-9_]*)"\s*(,[^)]*)?\)"#)
        .expect("valid os.environ.get regex (double-quoted)")
});

static RE_OS_ENVIRON_GET_SQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\.environ\.get\(\s*'([A-Z_][A-Z0-9_]*)'\s*(,[^)]*)?\)"#)
        .expect("valid os.environ.get regex (single-quoted)")
});

/// Match `os.environ["FOO"]` (required: KeyError on miss).
static RE_OS_ENVIRON_INDEX_DQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\.environ\[\s*"([A-Z_][A-Z0-9_]*)"\s*\]"#)
        .expect("valid os.environ[] regex (double-quoted)")
});

static RE_OS_ENVIRON_INDEX_SQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\.environ\[\s*'([A-Z_][A-Z0-9_]*)'\s*\]"#)
        .expect("valid os.environ[] regex (single-quoted)")
});

/// Match `Field(env="FOO")` or `Field(..., env="FOO", ...)` for pydantic-settings.
static RE_PYDANTIC_FIELD_ENV_DQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"Field\([^)]*?env\s*=\s*"([A-Z_][A-Z0-9_]*)"[^)]*\)"#)
        .expect("valid pydantic Field env= regex (double-quoted)")
});

static RE_PYDANTIC_FIELD_ENV_SQ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"Field\([^)]*?env\s*=\s*'([A-Z_][A-Z0-9_]*)'[^)]*\)"#)
        .expect("valid pydantic Field env= regex (single-quoted)")
});

#[derive(Debug, Clone)]
struct EnvOccurrenceCandidate {
    name: String,
    file: String,
    line: u32,
    access_kind: &'static str,
    default: Option<String>,
    required: bool,
}

fn parse_env_var_reads(stripped_content: &str, file_path: &str) -> Vec<EnvOccurrenceCandidate> {
    let mut out = Vec::new();
    for (idx, line) in stripped_content.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        // os.getenv — required iff no second positional arg.
        for re in [&*RE_OS_GETENV_DQ, &*RE_OS_GETENV_SQ] {
            for caps in re.captures_iter(line) {
                let name = caps[1].to_string();
                let default_arg = caps.get(2).map(|m| m.as_str().trim().to_string());
                let (default_text, required) = match default_arg {
                    None => (None, true),
                    Some(arg) => (Some(strip_leading_comma(&arg)), false),
                };
                out.push(EnvOccurrenceCandidate {
                    name,
                    file: file_path.to_string(),
                    line: line_no,
                    access_kind: "os.getenv",
                    default: default_text,
                    required,
                });
            }
        }
        // os.environ.get — required iff no second positional arg.
        for re in [&*RE_OS_ENVIRON_GET_DQ, &*RE_OS_ENVIRON_GET_SQ] {
            for caps in re.captures_iter(line) {
                let name = caps[1].to_string();
                let default_arg = caps.get(2).map(|m| m.as_str().trim().to_string());
                let (default_text, required) = match default_arg {
                    None => (None, true),
                    Some(arg) => (Some(strip_leading_comma(&arg)), false),
                };
                out.push(EnvOccurrenceCandidate {
                    name,
                    file: file_path.to_string(),
                    line: line_no,
                    access_kind: "os.environ.get",
                    default: default_text,
                    required,
                });
            }
        }
        // os.environ[X] — always required (raises KeyError).
        for re in [&*RE_OS_ENVIRON_INDEX_DQ, &*RE_OS_ENVIRON_INDEX_SQ] {
            for caps in re.captures_iter(line) {
                out.push(EnvOccurrenceCandidate {
                    name: caps[1].to_string(),
                    file: file_path.to_string(),
                    line: line_no,
                    access_kind: "os.environ[]",
                    default: None,
                    required: true,
                });
            }
        }
        // pydantic Field(env="FOO", ...) — treat as required unless `default=`
        // is also present in the same expression (best-effort substring check).
        for re in [&*RE_PYDANTIC_FIELD_ENV_DQ, &*RE_PYDANTIC_FIELD_ENV_SQ] {
            for caps in re.captures_iter(line) {
                let has_default = line.contains("default=") || line.contains("default_factory=");
                out.push(EnvOccurrenceCandidate {
                    name: caps[1].to_string(),
                    file: file_path.to_string(),
                    line: line_no,
                    access_kind: "pydantic_field_env",
                    default: None,
                    required: !has_default,
                });
            }
        }
    }
    out
}

fn strip_leading_comma(arg: &str) -> String {
    arg.trim_start_matches(',').trim().to_string()
}

fn merge_env_occurrence(out: &mut SemanticFacts, candidate: EnvOccurrenceCandidate) {
    let occurrence = EnvContractOccurrence {
        file: candidate.file.clone(),
        line: candidate.line,
        access_kind: candidate.access_kind.to_string(),
        default: candidate.default,
        required: candidate.required,
    };
    match out
        .env_contracts
        .iter_mut()
        .find(|c| c.name == candidate.name)
    {
        Some(contract) => {
            if !contract.used_in_files.contains(&candidate.file) {
                contract.used_in_files.push(candidate.file.clone());
            }
            // Avoid duplicate occurrences when a variable is read multiple
            // times on the same line (rare but possible).
            let already = contract.occurrences.iter().any(|o| {
                o.file == occurrence.file
                    && o.line == occurrence.line
                    && o.access_kind == occurrence.access_kind
            });
            if !already {
                contract.occurrences.push(occurrence);
            }
        }
        None => {
            out.env_contracts.push(EnvContract {
                name: candidate.name,
                used_in_files: vec![candidate.file],
                required_for: Vec::new(),
                occurrences: vec![occurrence],
            });
        }
    }
}

/// Detect framework-decorator -> def pairs and emit one DispatchEdge per pair.
///
/// Strategy: walk lines, accumulate decorator lines (`@expr`) at any indent,
/// and when we hit a `def name(...)` at the SAME indent, if any accumulated
/// decorator matched a known framework pattern, emit an edge whose handler is
/// `name`. Reset accumulator on any non-decorator non-def non-blank line.
fn parse_decorator_handlers(stripped_content: &str, file_path: &str) -> Vec<DispatchEdge> {
    let mut edges = Vec::new();
    let mut pending: Vec<(usize, String, String)> = Vec::new(); // (line_no, indent, expr)

    for (idx, raw) in stripped_content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim_start();
        if trimmed.is_empty() {
            continue; // blank lines do not break the decorator stack
        }

        if let Some(caps) = RE_DECORATOR_LINE.captures(raw) {
            let indent = caps.name("indent").map(|m| m.as_str()).unwrap_or("");
            let expr = caps
                .name("expr")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            pending.push((line_no, indent.to_string(), expr));
            continue;
        }

        if let Some(caps) = RE_DEF_LINE.captures(raw) {
            let def_indent = caps.name("indent").map(|m| m.as_str()).unwrap_or("");
            let name = caps.name("name").map(|m| m.as_str()).unwrap_or("");
            if name.is_empty() {
                pending.clear();
                continue;
            }

            // Decorators apply at the same indentation as the def.
            let matching: Vec<&(usize, String, String)> = pending
                .iter()
                .filter(|(_, indent, _)| indent == def_indent)
                .collect();

            for (dec_line, _, expr) in &matching {
                let Some(kind) = classify_decorator_dispatch(expr) else {
                    continue;
                };
                edges.push(DispatchEdge {
                    from_file: file_path.to_string(),
                    from_line: *dec_line as u32,
                    dispatch_kind: kind,
                    handler_symbol: name.to_string(),
                    handler_file: Some(file_path.to_string()),
                });
            }
            pending.clear();
            continue;
        }

        // Any other non-blank, non-decorator, non-def line breaks the stack.
        pending.clear();
    }
    edges
}

/// True when `lower` starts with `prefix` AND the next character is not part
/// of the same identifier (i.e. a real word boundary). Prevents false matches
/// like `app.get_data(...)` against the `app.get` prefix.
fn matches_decorator_prefix(lower: &str, prefix: &str) -> bool {
    if !lower.starts_with(prefix) {
        return false;
    }
    match lower.as_bytes().get(prefix.len()) {
        // Exact match — end of string is a valid boundary.
        None => true,
        // Boundary char must NOT be `[A-Za-z0-9_]`. ASCII byte check is safe
        // because every prefix in the FastAPI list is pure ASCII.
        Some(&b) => !(b.is_ascii_alphanumeric() || b == b'_'),
    }
}

/// Classify a decorator expression into its [`DispatchKind`].
///
/// Returns `None` when the decorator is unrecognised (importing application
/// code, type-only decorators, etc.). Recognised expressions are mapped to a
/// taxonomy that downstream consumers (`runtime.dispatch_edges`) can use to
/// distinguish HTTP routes from CLI subcommands from background tasks — in
/// the previous regime every recognised decorator collapsed onto
/// `DispatchKind::FunctionPointer`, which made the runtime block ambiguous.
///
/// The list is drawn from `analyzer/py/decorators.rs::is_framework_decorator`
/// (Layer 1 reference) and extended for Pydantic validators that the Layer 1
/// helper does not currently flag.
fn classify_decorator_dispatch(expr: &str) -> Option<DispatchKind> {
    // Decorator expressions can be bare (`pytest.fixture`), call-form
    // (`pytest.fixture(...)`), or aliased (`fixture`). We match on
    // case-folded substring presence. The expression has its leading `@`
    // already stripped by RE_DECORATOR_LINE.
    let lower = expr.to_lowercase();

    // FastAPI / Starlette / Litestar HTTP route registrations.
    for prefix in [
        "app.get",
        "app.post",
        "app.put",
        "app.delete",
        "app.patch",
        "app.head",
        "app.options",
        "router.get",
        "router.post",
        "router.put",
        "router.delete",
        "router.patch",
        "router.head",
        "router.options",
        "router.websocket",
        "api_router.",
        "app.websocket",
    ] {
        if matches_decorator_prefix(&lower, prefix) {
            return Some(DispatchKind::HttpRoute);
        }
    }

    // FastAPI lifecycle / middleware / exception handler decorators.
    for prefix in ["app.on_event", "app.middleware", "app.exception_handler"] {
        if matches_decorator_prefix(&lower, prefix) {
            return Some(DispatchKind::EventHandler);
        }
    }

    // Flask routes — covers `app.route`, `blueprint.route`, and `bp.route`.
    if lower.starts_with("app.route")
        || lower.starts_with("blueprint.route")
        || lower.contains(".route(")
    {
        return Some(DispatchKind::HttpRoute);
    }

    // Click / Typer / app.command CLI registries.
    if lower.starts_with("click.command")
        || lower.starts_with("click.group")
        || lower.starts_with("typer.")
        || lower.starts_with("app.command")
        || lower.contains(".command(")
        || lower.contains(".callback(")
    {
        return Some(DispatchKind::CliCommand);
    }

    // Celery / arq / dramatiq / RQ background workers.
    if lower.starts_with("celery.task")
        || lower.starts_with("app.task")
        || lower.starts_with("shared_task")
        || lower.starts_with("cron(")
        || lower.starts_with("func(")
    {
        return Some(DispatchKind::TaskTarget);
    }

    // pytest fixtures, marks, parametrize — fire on collection, not via import.
    if lower.starts_with("pytest.fixture")
        || lower.starts_with("fixture(")
        || lower.starts_with("fixture\n")
        || lower == "fixture"
        || lower.starts_with("pytest.mark")
        || lower.starts_with("pytest.parametrize")
    {
        return Some(DispatchKind::EventHandler);
    }

    // Pydantic v1 + v2 validators — fire on model construction.
    if lower.starts_with("validator")
        || lower.starts_with("root_validator")
        || lower.starts_with("field_validator")
        || lower.starts_with("model_validator")
        || lower.starts_with("computed_field")
    {
        return Some(DispatchKind::EventHandler);
    }

    // Django auth gates / signal receivers / admin registry.
    if lower.starts_with("admin.register") {
        return Some(DispatchKind::EventHandler);
    }
    if lower.starts_with("receiver") {
        return Some(DispatchKind::EventHandler);
    }
    if lower.starts_with("login_required")
        || lower.starts_with("permission_required")
        || lower.starts_with("staff_member_required")
        || lower.starts_with("user_passes_test")
    {
        return Some(DispatchKind::EventHandler);
    }

    // Generic event / callback patterns.
    if lower.starts_with("on_event") || lower.starts_with("event_handler") {
        return Some(DispatchKind::EventHandler);
    }
    if lower.starts_with("callback") || lower.starts_with("hook") || lower.starts_with("register") {
        return Some(DispatchKind::FunctionPointer);
    }

    None
}

/// Backwards-compatible boolean wrapper retained for existing call sites and
/// tests that only need to know whether the decorator is recognised at all.
#[cfg(test)]
fn is_framework_decorator_expr(expr: &str) -> bool {
    classify_decorator_dispatch(expr).is_some()
}

/// Strip Python comments (`#` to end-of-line, respecting string boundaries)
/// and replace triple-quoted string bodies with blank lines (preserving line
/// numbers) so the decorator scanner does not match inside docstrings.
///
/// Single- and double-quoted strings on a single line are left intact — they
/// rarely contain decorator-shaped text, and stripping them would force a
/// full Python tokenizer. The cost of a false-positive `@app.get` inside a
/// single-line string literal is bounded: we'd emit an unreachable
/// DispatchEdge that subsequent reachability propagation simply ignores
/// (no exported symbol named `def` follows it).
fn strip_comments_and_strings(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_triple_single: bool = false;
    let mut in_triple_double: bool = false;

    for line in content.lines() {
        let line_to_emit = if in_triple_single || in_triple_double {
            // Inside a triple-quoted string — blank the line and look for the
            // closing triple-quote. Use the quote-aware scanner so that a
            // single-line `"..."` literal in the docstring body cannot
            // accidentally toggle state via embedded `'''`.
            if in_triple_single && line_toggles_triple(line, "'''").is_some() {
                in_triple_single = false;
            } else if in_triple_double && line_toggles_triple(line, "\"\"\"").is_some() {
                in_triple_double = false;
            }
            String::new()
        } else {
            // Detect opening triple-quote on this line, possibly after content.
            // The scanner respects single-line `'...'` and `"..."` boundaries
            // so e.g. `print("'''")` is no longer mis-detected as opening a
            // triple-single docstring.
            let mut working = strip_line_comment(line);
            if let Some(idx) = line_toggles_triple(&working, "'''") {
                // Check if it's also closed on the same line. Inside a
                // triple-quote the contents are opaque text so naive `.find`
                // is correct here. ASCII triple markers stay on UTF-8
                // boundaries, so byte-slicing below is safe.
                let after = &working[idx + 3..];
                if let Some(close_offset) = after.find("'''") {
                    let close = idx + 3 + close_offset;
                    let mut new_str = String::with_capacity(working.len());
                    new_str.push_str(&working[..idx]);
                    new_str.push_str(&working[close + 3..]);
                    working = new_str;
                } else {
                    in_triple_single = true;
                    working.truncate(idx);
                }
            } else if let Some(idx) = line_toggles_triple(&working, "\"\"\"") {
                let after = &working[idx + 3..];
                if let Some(close_offset) = after.find(r#"""""#) {
                    let close = idx + 3 + close_offset;
                    let mut new_str = String::with_capacity(working.len());
                    new_str.push_str(&working[..idx]);
                    new_str.push_str(&working[close + 3..]);
                    working = new_str;
                } else {
                    in_triple_double = true;
                    working.truncate(idx);
                }
            }
            working
        };
        out.push_str(&line_to_emit);
        out.push('\n');
    }
    out
}

/// Scan `line` for the literal triple-quote sequence `want` (`'''` or `"""`)
/// while respecting single-line `'...'` and `"..."` string boundaries.
///
/// Returns `Some(byte_offset)` of the first match that lies OUTSIDE any
/// single-line string literal. Triples of the *other* quote style are skipped
/// past so they cannot leave us mid-string and trigger spurious quote-toggle.
///
/// Both `'''` and `"""` are pure ASCII so all byte-offset arithmetic stays on
/// UTF-8 boundaries even when the surrounding line contains multi-byte chars.
fn line_toggles_triple(line: &str, want: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let want_bytes = want.as_bytes();
    debug_assert_eq!(want_bytes.len(), 3);
    let mut i = 0;
    let mut in_dq = false;
    let mut in_sq = false;
    let mut esc = false;
    while i < bytes.len() {
        let c = bytes[i];
        if esc {
            esc = false;
            i += 1;
            continue;
        }
        if c == b'\\' && (in_sq || in_dq) {
            esc = true;
            i += 1;
            continue;
        }
        if !in_dq && !in_sq && i + 3 <= bytes.len() {
            let triple = &bytes[i..i + 3];
            if triple == want_bytes {
                return Some(i);
            }
            // Skip non-target triple to avoid the toggle below interpreting
            // its first quote as opening a single-line string.
            if triple == b"'''" || triple == b"\"\"\"" {
                i += 3;
                continue;
            }
        }
        if !in_sq && c == b'"' {
            in_dq = !in_dq;
            i += 1;
            continue;
        }
        if !in_dq && c == b'\'' {
            in_sq = !in_sq;
            i += 1;
            continue;
        }
        i += 1;
    }
    None
}

/// Strip a `#` comment from a line, respecting single- and double-quote
/// boundaries so `print("# not a comment")` keeps its hash.
fn strip_line_comment(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    for c in line.chars() {
        if escape {
            result.push(c);
            escape = false;
            continue;
        }
        match c {
            '\\' => {
                result.push(c);
                escape = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
                result.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                result.push(c);
            }
            '#' if !in_single && !in_double => {
                break;
            }
            _ => result.push(c),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExportSymbol;

    fn py_file(path: &str, exports: &[(&str, &str)]) -> FileAnalysis {
        let mut fa = FileAnalysis::new(path.to_string());
        fa.language = "py".to_string();
        for (name, kind) in exports {
            fa.exports.push(ExportSymbol::new(
                (*name).to_string(),
                kind,
                "named",
                Some(1),
            ));
        }
        fa
    }

    fn write_tmp(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).expect("write tmp file");
        path.to_string_lossy().into_owned()
    }

    // ---- decorator prefix matching ----

    #[test]
    fn decorator_prefix_does_not_match_longer_identifier() {
        // Exact match — boundary char is end-of-string.
        assert!(matches_decorator_prefix("app.get", "app.get"));
        // Prefix followed by `(` — boundary char is non-identifier.
        assert!(matches_decorator_prefix("app.get(\"/users\")", "app.get"));
        // Prefix followed by whitespace — also a boundary.
        assert!(matches_decorator_prefix("app.get ", "app.get"));
        // Overmatch guard: `app.get_data` must NOT match `app.get`.
        assert!(!matches_decorator_prefix("app.get_data(\"/x\")", "app.get"));
        assert!(!matches_decorator_prefix(
            "router.delete_all(\"/x\")",
            "router.delete"
        ));
        // Negative: completely different.
        assert!(!matches_decorator_prefix("foo.bar", "app.get"));
    }

    // ---- triple-quote scanner ----

    #[test]
    fn triple_quote_detection_ignores_marker_inside_single_line_string() {
        // Naive `.find("'''")` would falsely detect the embedded `'''`
        // inside the `"..."` literal and toggle into triple-mode.
        let input = "print(\"'''\")\nx = 1\n";
        let stripped = strip_comments_and_strings(input);
        // Both lines should be preserved — neither blanked by triple-mode.
        assert!(
            stripped.contains("x = 1"),
            "code after the misleading line must survive: {stripped:?}"
        );
        assert!(
            stripped.contains("print("),
            "the print line itself must survive: {stripped:?}"
        );

        // Single-line triple-single docstring is correctly stripped.
        let single_line_doc = "'''docstring'''\ndef foo(): pass\n";
        let stripped = strip_comments_and_strings(single_line_doc);
        assert!(stripped.contains("def foo()"));
        assert!(!stripped.contains("docstring"));

        // Multi-line triple-double docstring blanks the middle.
        let multi = "\"\"\"\nbody1\nbody2\n\"\"\"\ndef bar(): pass\n";
        let stripped = strip_comments_and_strings(multi);
        assert!(!stripped.contains("body1"));
        assert!(!stripped.contains("body2"));
        assert!(stripped.contains("def bar()"));

        // Triple of OTHER style inside a `"..."` literal must not toggle.
        let mixed = "s = \"contains \\\"\\\"\\\" not a triple\"\nlet_us_continue = 1\n";
        let stripped = strip_comments_and_strings(mixed);
        assert!(stripped.contains("let_us_continue = 1"));
    }

    // ---- idiom classification ----

    #[test]
    fn idiom_classifies_dunder_main_and_init_and_app_router() {
        let analyzer = PythonRuntimeSemantics;
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let files = vec![py_file(
            "/synthetic/pkg/module.py",
            &[
                ("__init__", "function"),
                ("__main__", "function"),
                ("main", "function"),
                ("app", "var"),
                ("router", "var"),
                ("info", "function"),
                ("warning", "function"),
            ],
        )];
        let mut out = SemanticFacts::default();
        analyzer
            .analyze(&files, &registry, &mut out)
            .expect("analyze");

        let names: Vec<&String> = out.idiom_tags.values().flatten().map(|t| &t.name).collect();
        assert!(names.iter().any(|n| n.as_str() == "__init__"));
        assert!(names.iter().any(|n| n.as_str() == "__main__"));
        assert!(names.iter().any(|n| n.as_str() == "main"));
        assert!(names.iter().any(|n| n.as_str() == "app"));
        assert!(names.iter().any(|n| n.as_str() == "router"));
        assert!(names.iter().any(|n| n.as_str() == "info"));
        assert!(names.iter().any(|n| n.as_str() == "warning"));
        assert!(
            out.idiom_tags.len() >= 7,
            "expected at least 7 idiom-tagged symbols, got {}",
            out.idiom_tags.len()
        );
    }

    #[test]
    fn idiom_alias_resolution_matches_warn_and_underscore_variants() {
        let analyzer = PythonRuntimeSemantics;
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let files = vec![py_file(
            "/synthetic/log.py",
            &[
                ("warn", "function"),  // alias of warning
                ("_warn", "function"), // alias of warning
                ("_error", "function"),
                ("fatal", "function"), // alias of critical
            ],
        )];
        let mut out = SemanticFacts::default();
        analyzer
            .analyze(&files, &registry, &mut out)
            .expect("analyze");

        let names: Vec<&String> = out.idiom_tags.values().flatten().map(|t| &t.name).collect();
        assert!(
            names.iter().any(|n| n.as_str() == "warning"),
            "warn alias should resolve to warning"
        );
        assert!(names.iter().filter(|n| n.as_str() == "warning").count() >= 1);
        assert!(
            names.iter().any(|n| n.as_str() == "error"),
            "_error alias should resolve to error"
        );
        assert!(
            names.iter().any(|n| n.as_str() == "critical"),
            "fatal alias should resolve to critical"
        );
    }

    // ---- pytest discovery ----

    #[test]
    fn pytest_test_function_in_test_file_marked_reached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "test_widget.py",
            r#"def test_creates_widget():
    assert True

def helper():
    return 1
"#,
        );
        let fa = py_file(
            &path,
            &[("test_creates_widget", "function"), ("helper", "function")],
        );
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let test_id = format!("{path}::test_creates_widget");
        let helper_id = format!("{path}::helper");
        assert!(
            out.reachability.reached_symbols.contains(&test_id),
            "test_creates_widget in test_*.py must be reached; reached={:?}",
            out.reachability.reached_symbols
        );
        assert!(
            !out.reachability.reached_symbols.contains(&helper_id),
            "helper() is not a test_* function and not under test discovery"
        );
    }

    #[test]
    fn pytest_test_class_marked_reached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "test_classes.py",
            r#"class TestSuite:
    def test_thing(self):
        pass
"#,
        );
        let fa = py_file(&path, &[("TestSuite", "class"), ("test_thing", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let class_id = format!("{path}::TestSuite");
        assert!(
            out.reachability.reached_symbols.contains(&class_id),
            "TestSuite class in test file must be reached"
        );
    }

    #[test]
    fn pytest_discovery_skips_non_test_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "production.py",
            r#"def test_something():
    return True
"#,
        );
        let fa = py_file(&path, &[("test_something", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let id = format!("{path}::test_something");
        assert!(
            !out.reachability.reached_symbols.contains(&id),
            "test_something in production.py must NOT be reached by pytest discovery"
        );
    }

    // ---- decorator dispatch ----

    #[test]
    fn fastapi_route_decorator_marks_handler_reached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "routes.py",
            r#"from fastapi import APIRouter

router = APIRouter()

@router.get("/items/{item_id}")
def get_item(item_id: int):
    return {"id": item_id}

@router.post("/items")
async def create_item(payload: dict):
    return payload

def unused_helper():
    return 0
"#,
        );
        let fa = py_file(
            &path,
            &[
                ("router", "var"),
                ("get_item", "function"),
                ("create_item", "function"),
                ("unused_helper", "function"),
            ],
        );
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let get_id = format!("{path}::get_item");
        let create_id = format!("{path}::create_item");
        let helper_id = format!("{path}::unused_helper");

        assert!(
            out.reachability.reached_symbols.contains(&get_id),
            "get_item should be reached via @router.get decorator; reached={:?}",
            out.reachability.reached_symbols
        );
        assert!(
            out.reachability.reached_symbols.contains(&create_id),
            "create_item should be reached via @router.post decorator"
        );
        assert!(
            !out.reachability.reached_symbols.contains(&helper_id),
            "unused_helper has no decorator and no idiom — must NOT be reached"
        );

        let edge_handlers: Vec<&str> = out
            .dispatch_edges
            .iter()
            .map(|e| e.handler_symbol.as_str())
            .collect();
        assert!(
            edge_handlers.contains(&"get_item"),
            "expected DispatchEdge for get_item; got {:?}",
            out.dispatch_edges
        );
        assert!(edge_handlers.contains(&"create_item"));
    }

    #[test]
    fn pytest_fixture_decorator_marks_fixture_reached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Place outside any tests/ dir so pytest discovery does NOT fire,
        // proving the decorator path is what reaches the symbol.
        let path = write_tmp(
            tmp.path(),
            "conftest_fragment.py",
            r#"import pytest

@pytest.fixture
def db_connection():
    return None

@pytest.fixture(scope="session")
def app_client():
    return None
"#,
        );
        let fa = py_file(
            &path,
            &[("db_connection", "function"), ("app_client", "function")],
        );
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let db_id = format!("{path}::db_connection");
        let client_id = format!("{path}::app_client");
        assert!(
            out.reachability.reached_symbols.contains(&db_id),
            "db_connection should be reached via @pytest.fixture"
        );
        assert!(
            out.reachability.reached_symbols.contains(&client_id),
            "app_client should be reached via @pytest.fixture(scope=session)"
        );
    }

    #[test]
    fn pydantic_field_validator_marks_method_reached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "models.py",
            r#"from pydantic import BaseModel, field_validator

class Item(BaseModel):
    name: str

    @field_validator("name")
    def validate_name(cls, v):
        return v.strip()
"#,
        );
        let fa = py_file(&path, &[("Item", "class"), ("validate_name", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let id = format!("{path}::validate_name");
        assert!(
            out.reachability.reached_symbols.contains(&id),
            "validate_name should be reached via @field_validator; reached={:?}",
            out.reachability.reached_symbols
        );
    }

    // ---- __main__ block ----

    #[test]
    fn dunder_main_block_marks_main_reached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "script.py",
            r#"def main():
    print("hello")

if __name__ == "__main__":
    main()
"#,
        );
        let fa = py_file(&path, &[("main", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        let id = format!("{path}::main");
        assert!(
            out.reachability.reached_symbols.contains(&id),
            "main() inside if __name__ == '__main__' guard must be reached"
        );
    }

    // ---- robustness ----

    #[test]
    fn unrelated_language_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(tmp.path(), "x.sh", "usage() { echo hi; }\n");
        let mut fa = py_file(&path, &[("usage", "function")]);
        fa.language = "shell".to_string();
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        assert!(out.idiom_tags.is_empty());
        assert!(out.dispatch_edges.is_empty());
        assert!(out.reachability.reached_symbols.is_empty());
    }

    #[test]
    fn missing_file_does_not_panic() {
        let fa = py_file("/nonexistent/missing.py", &[("__init__", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        // Idiom tag still emitted from in-memory exports.
        assert_eq!(out.idiom_tags.len(), 1);
        // No decorator/main content because file read failed.
        assert!(out.dispatch_edges.is_empty());
    }

    #[test]
    fn decorator_inside_docstring_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "doc.py",
            r#"def real_helper():
    """
    Example usage:

        @app.get("/")
        def fake_handler():
            pass
    """
    return True
"#,
        );
        let fa = py_file(&path, &[("real_helper", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        assert!(
            out.dispatch_edges.is_empty(),
            "decorator inside docstring must not produce a dispatch edge; got {:?}",
            out.dispatch_edges
        );
    }

    #[test]
    fn decorator_inside_comment_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write_tmp(
            tmp.path(),
            "comm.py",
            r#"# @app.get("/")
def helper():
    return 1
"#,
        );
        let fa = py_file(&path, &[("helper", "function")]);
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let mut out = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut out)
            .expect("analyze");

        assert!(
            out.dispatch_edges.is_empty(),
            "decorator inside comment must not produce a dispatch edge"
        );
    }

    // ---- helper-level pure tests (no disk) ----

    #[test]
    fn pytest_path_recognition() {
        assert!(is_pytest_test_file("test_widget.py"));
        assert!(is_pytest_test_file("/abs/path/test_widget.py"));
        assert!(is_pytest_test_file("widget_test.py"));
        assert!(is_pytest_test_file("project/tests/conftest.py"));
        assert!(is_pytest_test_file("project/test/sub/anything.py"));
        assert!(!is_pytest_test_file("project/src/widget.py"));
        assert!(!is_pytest_test_file("attestation.py")); // 'test' not as path segment
    }

    #[test]
    fn framework_decorator_recognition() {
        assert!(is_framework_decorator_expr("app.get(\"/\")"));
        assert!(is_framework_decorator_expr("router.post(\"/items\")"));
        assert!(is_framework_decorator_expr("pytest.fixture"));
        assert!(is_framework_decorator_expr(
            "pytest.fixture(scope=\"session\")"
        ));
        assert!(is_framework_decorator_expr("field_validator(\"name\")"));
        assert!(is_framework_decorator_expr(
            "model_validator(mode=\"after\")"
        ));
        assert!(is_framework_decorator_expr("celery.task"));
        assert!(is_framework_decorator_expr("shared_task"));
        assert!(is_framework_decorator_expr("typer.Typer().command()"));
        assert!(!is_framework_decorator_expr("staticmethod"));
        assert!(!is_framework_decorator_expr("property"));
        assert!(!is_framework_decorator_expr("classmethod"));
    }

    #[test]
    fn strip_comments_keeps_hash_inside_string() {
        let stripped = strip_comments_and_strings("print(\"# not a comment\")\n# real\n");
        assert!(stripped.contains("# not a comment"));
        assert!(!stripped.contains("# real"));
    }

    #[test]
    fn strip_triple_quoted_blanks_body_preserves_lines() {
        let original = "before\n\"\"\"\n@app.get(\"/x\")\n\"\"\"\nafter\n";
        let stripped = strip_comments_and_strings(original);
        assert_eq!(
            stripped.lines().count(),
            original.lines().count(),
            "line numbering must be preserved"
        );
        assert!(stripped.contains("before"));
        assert!(stripped.contains("after"));
        assert!(!stripped.contains("@app.get"));
    }

    // ----- decorator dispatch classification -----

    #[test]
    fn classifies_decorator_dispatch_kinds() {
        assert_eq!(
            classify_decorator_dispatch("app.get(\"/x\")"),
            Some(DispatchKind::HttpRoute)
        );
        assert_eq!(
            classify_decorator_dispatch("router.post(\"/y\")"),
            Some(DispatchKind::HttpRoute)
        );
        assert_eq!(
            classify_decorator_dispatch("app.route(\"/y\")"),
            Some(DispatchKind::HttpRoute)
        );
        assert_eq!(
            classify_decorator_dispatch("typer.Typer().command()"),
            Some(DispatchKind::CliCommand)
        );
        assert_eq!(
            classify_decorator_dispatch("click.command()"),
            Some(DispatchKind::CliCommand)
        );
        assert_eq!(
            classify_decorator_dispatch("celery.task"),
            Some(DispatchKind::TaskTarget)
        );
        assert_eq!(
            classify_decorator_dispatch("pytest.fixture"),
            Some(DispatchKind::EventHandler)
        );
        assert_eq!(
            classify_decorator_dispatch("validator(\"foo\")"),
            Some(DispatchKind::EventHandler)
        );
        assert_eq!(
            classify_decorator_dispatch("app.on_event(\"startup\")"),
            Some(DispatchKind::EventHandler)
        );
        assert_eq!(
            classify_decorator_dispatch("callback"),
            Some(DispatchKind::FunctionPointer)
        );
        assert_eq!(classify_decorator_dispatch("staticmethod"), None);
    }

    // ----- env contract extraction -----

    #[test]
    fn extracts_env_contracts_from_os_getenv_and_environ() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let content = r#"
import os

API_KEY = os.getenv("LIBRAXIS_API_KEY")
MODEL = os.getenv("STT_MODEL", "default-stt")

def runtime():
    db_url = os.environ["DATABASE_URL"]
    cache = os.environ.get("CACHE_DIR")
    queue = os.environ.get('REDIS_URL', 'redis://localhost')
"#;
        let path = write_tmp(tmp.path(), "config.py", content);
        let mut fa = FileAnalysis::new(path.clone());
        fa.language = "py".to_string();

        let registry = IdiomRegistry::load_defaults().expect("registry");
        let mut facts = SemanticFacts::default();
        PythonRuntimeSemantics
            .analyze(&[fa], &registry, &mut facts)
            .expect("analyze");

        let names: Vec<&str> = facts
            .env_contracts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        for expected in [
            "LIBRAXIS_API_KEY",
            "STT_MODEL",
            "DATABASE_URL",
            "CACHE_DIR",
            "REDIS_URL",
        ] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }

        // os.environ[X] is required: KeyError on miss.
        let db = facts
            .env_contracts
            .iter()
            .find(|c| c.name == "DATABASE_URL")
            .expect("DATABASE_URL contract");
        assert_eq!(db.occurrences.len(), 1);
        assert_eq!(db.occurrences[0].access_kind, "os.environ[]");
        assert!(db.occurrences[0].required);

        // getenv with default is optional.
        let model = facts
            .env_contracts
            .iter()
            .find(|c| c.name == "STT_MODEL")
            .expect("STT_MODEL");
        assert!(!model.occurrences[0].required);
        assert_eq!(model.occurrences[0].access_kind, "os.getenv");
        assert!(model.occurrences[0].default.is_some());

        // getenv without default is required.
        let key = facts
            .env_contracts
            .iter()
            .find(|c| c.name == "LIBRAXIS_API_KEY")
            .expect("LIBRAXIS_API_KEY");
        assert!(key.occurrences[0].required);

        // environ.get('REDIS_URL', ...) — single-quoted variant must also be picked up.
        let redis = facts
            .env_contracts
            .iter()
            .find(|c| c.name == "REDIS_URL")
            .expect("REDIS_URL");
        assert_eq!(redis.occurrences[0].access_kind, "os.environ.get");
    }
}
