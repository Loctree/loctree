//! Layer 3 semantic analyzer for Makefiles.
//!
//! The Layer 1 Makefile sensor extracts raw targets, variables, and include
//! directives. This layer decides which targets are runtime entrypoints, which
//! helpers are internal, and where recipe bodies dispatch into shell.

use crate::semantic::{
    Classifier, DispatchEdge, DispatchKind, IdiomRegistry, IdiomTag, ReachReason, RuntimeRole,
    RuntimeSemanticAnalyzer, SemanticFacts, SymbolId, TagSource,
};
use crate::types::{FileAnalysis, Language};
use std::collections::HashSet;

const LANG_STR: &str = "make";

pub struct MakeSemantics;

impl RuntimeSemanticAnalyzer for MakeSemantics {
    fn language(&self) -> Language {
        Language::Makefile
    }

    fn analyze(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) -> anyhow::Result<()> {
        for file in files {
            if file.language != LANG_STR && file.language != "makefile" {
                continue;
            }

            let content = crate::semantic::io::read_validated_semantic_input(&file.path)?;
            let phony = parse_phony_directives(&content);

            self.classify_exports(file, &phony, registry, out);
            self.emit_recipe_shell_markers(file, &content, out);
        }

        Ok(())
    }
}

impl MakeSemantics {
    fn classify_exports(
        &self,
        file: &FileAnalysis,
        phony: &HashSet<String>,
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) {
        for export in &file.exports {
            if export.kind == "var" {
                continue;
            }

            let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);

            if export.kind == "special_target" {
                if let Some(entry) = registry.lookup(Language::Makefile, &export.name) {
                    push_tag(
                        out,
                        symbol_id,
                        IdiomTag {
                            name: entry.name.clone(),
                            classifier: entry.classifier.clone(),
                            runtime_role: entry.runtime_role.clone(),
                            source: TagSource::EmbeddedDefault,
                            reasoning: entry.reasoning.clone(),
                        },
                    );
                }
                continue;
            }

            if phony.contains(&export.name) {
                push_tag(
                    out,
                    symbol_id.clone(),
                    IdiomTag {
                        name: ".PHONY".into(),
                        classifier: Classifier::PublicEntrypoint,
                        runtime_role: RuntimeRole::PublicEntrypoint,
                        source: TagSource::InferredFromCode,
                        reasoning: format!(
                            "Target '{}' is listed in a .PHONY directive; operator-invoked make target.",
                            export.name
                        ),
                    },
                );
                mark_reached(out, symbol_id, ReachReason::PhonyMakeTarget);
                continue;
            }

            if export.name.starts_with('_') {
                push_tag(
                    out,
                    symbol_id,
                    IdiomTag {
                        name: "internal_target".into(),
                        classifier: Classifier::Custom("internal_target".into()),
                        runtime_role: RuntimeRole::Internal,
                        source: TagSource::InferredFromCode,
                        reasoning: "Underscore-prefixed Make target, treated as internal helper."
                            .into(),
                    },
                );
                continue;
            }

            if let Some(entry) = registry.lookup(Language::Makefile, &export.name) {
                push_tag(
                    out,
                    symbol_id.clone(),
                    IdiomTag {
                        name: entry.name.clone(),
                        classifier: entry.classifier.clone(),
                        runtime_role: entry.runtime_role.clone(),
                        source: TagSource::EmbeddedDefault,
                        reasoning: entry.reasoning.clone(),
                    },
                );

                if entry.runtime_role == RuntimeRole::PublicEntrypoint {
                    mark_reached(
                        out,
                        symbol_id,
                        ReachReason::IdiomRuntimeRole(RuntimeRole::PublicEntrypoint),
                    );
                }
            }
        }
    }

    fn emit_recipe_shell_markers(
        &self,
        file: &FileAnalysis,
        content: &str,
        out: &mut SemanticFacts,
    ) {
        for (_target, line_no, called_symbol) in extract_recipe_shell_calls(content) {
            out.dispatch_edges.push(DispatchEdge {
                from_file: file.path.clone(),
                from_line: line_no as u32,
                dispatch_kind: DispatchKind::RecipeShellCall,
                handler_symbol: called_symbol,
                handler_file: None,
            });
        }
    }
}

fn push_tag(out: &mut SemanticFacts, symbol_id: SymbolId, tag: IdiomTag) {
    out.idiom_tags.entry(symbol_id).or_default().push(tag);
}

fn mark_reached(out: &mut SemanticFacts, symbol_id: SymbolId, reason: ReachReason) {
    out.reachability.reached_symbols.insert(symbol_id.clone());
    out.reachability.reasons.insert(symbol_id, reason);
}

fn parse_phony_directives(content: &str) -> HashSet<String> {
    let mut phony = HashSet::new();
    for logical in logical_make_lines(content) {
        let line = strip_make_comment(&logical);
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(".PHONY:") else {
            continue;
        };

        for target in rest.split_whitespace() {
            if !target.is_empty() {
                phony.insert(target.to_string());
            }
        }
    }
    phony
}

fn extract_recipe_shell_calls(content: &str) -> Vec<(String, usize, String)> {
    let mut calls = Vec::new();
    let mut current_targets: Vec<String> = Vec::new();
    let mut pending_recipe: Option<(String, usize, String)> = None;

    for (idx, raw) in content.lines().enumerate() {
        let line_no = idx + 1;
        if raw.starts_with('\t') {
            if let Some(target) = current_targets.first().cloned() {
                let body = raw.trim_start_matches('\t');
                let cleaned = without_continuation(body);

                if let Some((pending_target, start_line, pending_body)) = pending_recipe.as_mut() {
                    pending_body.push(' ');
                    pending_body.push_str(cleaned.trim());
                    if !continues_line(body) {
                        emit_shell_calls(calls.as_mut(), pending_target, *start_line, pending_body);
                        pending_recipe = None;
                    }
                } else if continues_line(body) {
                    pending_recipe = Some((target, line_no, cleaned.trim().to_string()));
                } else {
                    emit_shell_calls(calls.as_mut(), &target, line_no, cleaned);
                }
            }
            continue;
        }

        if let Some((pending_target, start_line, pending_body)) = pending_recipe.take() {
            emit_shell_calls(calls.as_mut(), &pending_target, start_line, &pending_body);
        }

        let stripped = strip_make_comment(raw);
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            continue;
        }

        current_targets = parse_target_names(trimmed);
    }

    if let Some((pending_target, start_line, pending_body)) = pending_recipe {
        emit_shell_calls(calls.as_mut(), &pending_target, start_line, &pending_body);
    }

    calls
}

fn emit_shell_calls(
    calls: &mut Vec<(String, usize, String)>,
    target: &str,
    line_no: usize,
    body: &str,
) {
    for symbol in extract_command_symbols(body) {
        calls.push((target.to_string(), line_no, symbol));
    }
}

fn extract_command_symbols(body: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for (idx, segment) in split_shell_segments(body).into_iter().enumerate() {
        // Make recipe modifiers (`@` silent, `-` ignore-errors, `+` always-run)
        // only legally appear at the very start of a recipe line. Trimming
        // them on every shell-split segment turned `... && -sS foo` into a
        // bogus `sS` command. Only the first segment may carry them.
        let symbol = first_command_symbol(segment, idx == 0);
        let Some(symbol) = symbol else {
            continue;
        };
        if !symbols.contains(&symbol) {
            symbols.push(symbol);
        }
    }
    symbols
}

fn split_shell_segments(body: &str) -> Vec<&str> {
    body.split([';', '|'])
        .flat_map(|segment| segment.split("&&"))
        .flat_map(|segment| segment.split("||"))
        .collect()
}

/// Shell control-flow keywords that introduce a nested command but are not
/// themselves commands. Splitting a recipe on `;`/`|`/`&&`/`||` leaves these
/// as the first token of their segment; without filtering they leak into
/// `dispatch_edges` as fake commands (`then`, `fi`, `do`, …).
///
/// Treatment:
/// - `STRIP_KEYWORDS` — strip and look at the next token in the same segment
///   (the real command lives there: `then real_cmd …`, `else fallback …`).
/// - `SKIP_SEGMENT_KEYWORDS` — drop the entire segment; the real command is in
///   the following segment after `;` (`if [ -d x ]; then real_cmd; fi`).
const STRIP_KEYWORDS: &[&str] = &["then", "else", "elif", "do", "in", "time"];
const SKIP_SEGMENT_KEYWORDS: &[&str] = &[
    "if", "while", "until", "for", "case", "select", "fi", "done", "esac",
];

/// Standalone shell-syntax tokens that surface as segment leads but are not
/// commands. `[` opens a `test` expression; `(` and `{` open subshell/group
/// blocks whose first real command lives in the next token. We also reject
/// stray closers and POSIX `test` artifacts the caller can't otherwise
/// disambiguate from a path-shaped symbol.
fn is_shell_syntax_token(token: &str) -> bool {
    matches!(
        token,
        "[" | "[[" | "]" | "]]" | "(" | "((" | ")" | "))" | "{" | "}" | ";" | "&" | ":"
    )
}

/// A token only counts as a command name when it can plausibly resolve to an
/// executable: it must start with an alphanumeric, `_`, `.`, or `/` (so
/// `./script.sh`, `bin/cmd`, `_helper` all work), and contain at least one
/// command-name-shaped character before any noise. This kills shell
/// artifacts like `true)`, `pattern)`, `--flag`, and the historical `sS`
/// false positive while letting real commands through.
fn looks_like_command_name(token: &str) -> bool {
    let trimmed = token.trim_matches(['"', '\'']);
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '_' || first == '.' || first == '/') {
        return false;
    }
    // Reject tokens that end in `)`, `]`, `}` — those are case-pattern tails
    // (`true)`, `*.txt)`) or syntax fragments, not commands.
    if matches!(trimmed.chars().last(), Some(')' | ']' | '}')) {
        return false;
    }
    true
}

fn first_command_symbol(segment: &str, allow_recipe_modifiers: bool) -> Option<String> {
    let mut words = segment.split_whitespace().peekable();

    // First token may carry Make recipe modifiers. Subsequent segments
    // (after `;`/`&&`/etc.) cannot — those modifiers are line-level.
    let raw = words.next()?;
    let mut token = if allow_recipe_modifiers {
        raw.trim_start_matches(['@', '-', '+'])
    } else {
        raw
    };

    // Strip leading env-var assignments (`FOO=bar baz`) — assignments are
    // not commands, the command sits after them.
    while looks_like_assignment(token) {
        token = words.next()?;
    }

    // Strip shell control keywords. `then real_cmd …` → command is `real_cmd`.
    // `if [ -d x ]; then …` → entire segment is skipped (real command is in
    // the next segment after the `;`).
    loop {
        if SKIP_SEGMENT_KEYWORDS.contains(&token) {
            return None;
        }
        if STRIP_KEYWORDS.contains(&token) {
            token = words.next()?;
            continue;
        }
        break;
    }

    // Drop bare shell syntax tokens (`[`, `(`, `{`, …). The real command
    // follows them on the same segment for things like `[ -d x ] && cmd`,
    // though our split already handles `&&` — what reaches here is segments
    // whose lead is pure syntax.
    while is_shell_syntax_token(token) {
        token = words.next()?;
    }

    if token.is_empty()
        || token.starts_with('#')
        || token.starts_with("$(")
        || token.starts_with("${")
        || token.contains('=')
        || !looks_like_command_name(token)
    {
        return None;
    }

    Some(
        token
            .trim_matches('"')
            .trim_matches('\'')
            .rsplit('/')
            .next()
            .unwrap_or(token)
            .to_string(),
    )
}

fn looks_like_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
}

fn parse_target_names(trimmed: &str) -> Vec<String> {
    if is_variable_assignment(trimmed) {
        return Vec::new();
    }

    let Some((head, _tail)) = split_make_rule(trimmed) else {
        return Vec::new();
    };

    head.split_whitespace()
        .filter(|name| !name.is_empty() && !name.contains('='))
        .map(ToString::to_string)
        .collect()
}

fn split_make_rule(line: &str) -> Option<(&str, &str)> {
    for (idx, ch) in line.char_indices() {
        if ch != ':' {
            continue;
        }

        if line[idx..].starts_with(":=") {
            continue;
        }

        let after_colon = if line[idx..].starts_with("::") {
            &line[idx + 2..]
        } else {
            &line[idx + 1..]
        };
        return Some((&line[..idx], after_colon));
    }
    None
}

fn is_variable_assignment(line: &str) -> bool {
    let Some(first) = line.split_whitespace().next() else {
        return false;
    };
    let rest = line[first.len()..].trim_start();
    rest.starts_with(":=")
        || rest.starts_with("?=")
        || rest.starts_with("+=")
        || rest.starts_with('=')
}

fn logical_make_lines(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for raw in content.lines() {
        let trimmed_end = raw.trim_end();
        if continues_line(trimmed_end) {
            current.push_str(without_continuation(trimmed_end));
            current.push(' ');
            continue;
        }

        current.push_str(trimmed_end);
        lines.push(current.trim().to_string());
        current.clear();
    }

    if !current.trim().is_empty() {
        lines.push(current.trim().to_string());
    }

    lines
}

fn continues_line(line: &str) -> bool {
    line.trim_end().ends_with('\\')
}

fn without_continuation(line: &str) -> &str {
    line.trim_end().trim_end_matches('\\').trim_end()
}

fn strip_make_comment(line: &str) -> &str {
    match line.find('#') {
        Some(pos) => &line[..pos],
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::makefile::analyze_makefile;
    use crate::semantic::{Classifier, RuntimeRole};

    fn analyze_content(content: &str) -> (String, SemanticFacts) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("Makefile");
        std::fs::write(&path, content).expect("write makefile");

        let file_path = path.to_string_lossy().to_string();
        let mut file = analyze_makefile(content, file_path.clone());
        file.language = LANG_STR.into();

        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();
        MakeSemantics
            .analyze(&[file], &registry, &mut facts)
            .expect("analyze");
        (file_path, facts)
    }

    fn tags_for<'a>(facts: &'a SemanticFacts, symbol: &str) -> &'a [IdiomTag] {
        facts
            .idiom_tags
            .get(symbol)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[test]
    fn phony_targets_classified_public() {
        let content = r#"
.PHONY: build test clean

build:
test:
clean:
_internal:
"#;
        let (path, facts) = analyze_content(content);

        for name in ["build", "test", "clean"] {
            let symbol = format!("{path}::{name}");
            let tags = tags_for(&facts, &symbol);
            assert!(
                tags.iter()
                    .any(|tag| tag.classifier == Classifier::PublicEntrypoint)
            );
            assert!(facts.reachability.reached_symbols.contains(&symbol));
            assert!(matches!(
                facts.reachability.reasons.get(&symbol),
                Some(ReachReason::PhonyMakeTarget)
            ));
        }

        let internal = format!("{path}::_internal");
        assert!(
            tags_for(&facts, &internal)
                .iter()
                .any(|tag| tag.runtime_role == RuntimeRole::Internal)
        );
    }

    #[test]
    fn variable_assignments_are_make_symbols_not_env_contracts() {
        let content = r#"
VERSION := 0.9.0
FOO ?= bar
BAR = baz
build:
	@echo ok
"#;
        let (_path, facts) = analyze_content(content);

        assert!(
            facts
                .idiom_tags
                .keys()
                .all(|symbol| !symbol.ends_with("::VERSION")
                    && !symbol.ends_with("::FOO")
                    && !symbol.ends_with("::BAR")),
            "variables must not receive target idiom tags"
        );
        assert!(
            facts.env_contracts.is_empty(),
            "Makefile variable assignments are local make symbols, not runtime env reads: {:?}",
            facts.env_contracts
        );
    }

    #[test]
    fn phony_directive_itself_is_metadata() {
        let content = r#"
.PHONY: build
build:
"#;
        let (path, facts) = analyze_content(content);
        let phony = format!("{path}::.PHONY");

        assert!(
            tags_for(&facts, &phony)
                .iter()
                .any(|tag| tag.classifier == Classifier::Metadata
                    && tag.runtime_role == RuntimeRole::Metadata)
        );
    }

    #[test]
    fn recipe_shell_calls_emitted_as_markers() {
        let content = r#"
build:
	@./scripts/_internal_compile.sh
	@info "done"
"#;
        let (_path, facts) = analyze_content(content);
        let symbols: HashSet<_> = facts
            .dispatch_edges
            .iter()
            .map(|edge| edge.handler_symbol.as_str())
            .collect();

        assert!(
            facts
                .dispatch_edges
                .iter()
                .all(|edge| edge.dispatch_kind == DispatchKind::RecipeShellCall)
        );
        assert!(symbols.contains("_internal_compile.sh"));
        assert!(symbols.contains("info"));
    }

    #[test]
    fn target_inside_comment_not_matched() {
        let content = r#"
# build: not really a target
"#;
        let (_path, facts) = analyze_content(content);
        assert!(facts.idiom_tags.is_empty());
        assert!(facts.dispatch_edges.is_empty());
    }

    #[test]
    fn multiple_phony_lines_aggregated() {
        let content = r#"
.PHONY: a b
SOME_VAR := value
.PHONY: c d
a:
b:
c:
d:
"#;
        let (path, facts) = analyze_content(content);

        for name in ["a", "b", "c", "d"] {
            let symbol = format!("{path}::{name}");
            assert!(
                tags_for(&facts, &symbol)
                    .iter()
                    .any(|tag| tag.classifier == Classifier::PublicEntrypoint)
            );
        }
    }

    #[test]
    fn line_continuation_in_phony() {
        let content = r#"
.PHONY: a b \
        c d
a:
b:
c:
d:
"#;
        let (path, facts) = analyze_content(content);

        for name in ["a", "b", "c", "d"] {
            let symbol = format!("{path}::{name}");
            assert!(
                tags_for(&facts, &symbol)
                    .iter()
                    .any(|tag| tag.classifier == Classifier::PublicEntrypoint)
            );
        }
    }

    #[test]
    fn recipe_continuation_emits_command_once_at_start_line() {
        let content = r#"
release:
	@mkdir -p dist && \
		cp target/foo dist/
"#;
        let (_path, facts) = analyze_content(content);

        assert!(
            facts
                .dispatch_edges
                .iter()
                .any(|edge| edge.handler_symbol == "mkdir" && edge.from_line == 3)
        );
        assert!(
            facts
                .dispatch_edges
                .iter()
                .any(|edge| edge.handler_symbol == "cp" && edge.from_line == 3)
        );
    }

    #[test]
    fn double_colon_rules_are_targets() {
        let content = r#"
.PHONY: generate
generate::
	@echo regenerate
"#;
        let (path, facts) = analyze_content(content);
        let symbol = format!("{path}::generate");

        assert!(
            tags_for(&facts, &symbol)
                .iter()
                .any(|tag| tag.classifier == Classifier::PublicEntrypoint)
        );
    }

    #[test]
    fn unrelated_language_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("script.sh");
        std::fs::write(&path, "echo nope\n").expect("write");

        let file = FileAnalysis {
            path: path.to_string_lossy().to_string(),
            language: "shell".into(),
            ..FileAnalysis::default()
        };
        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();

        MakeSemantics
            .analyze(&[file], &registry, &mut facts)
            .expect("analyze");

        assert!(facts.idiom_tags.is_empty());
        assert!(facts.dispatch_edges.is_empty());
        assert!(facts.env_contracts.is_empty());
    }

    #[test]
    fn missing_makefile_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("missing.mk");

        let file = FileAnalysis {
            path: path.to_string_lossy().to_string(),
            language: LANG_STR.into(),
            ..FileAnalysis::default()
        };
        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();

        let err = MakeSemantics
            .analyze(&[file], &registry, &mut facts)
            .expect_err("missing Makefile should not be skipped silently");

        assert!(
            err.to_string().contains("semantic input"),
            "unexpected error: {err}"
        );
        assert!(facts.idiom_tags.is_empty());
        assert!(facts.dispatch_edges.is_empty());
        assert!(facts.env_contracts.is_empty());
    }

    // ------------------------------------------------------------------
    // L4-B: shell syntax filter for recipe dispatch edges
    // ------------------------------------------------------------------

    fn handler_symbols(content: &str) -> Vec<String> {
        let (_path, facts) = analyze_content(content);
        facts
            .dispatch_edges
            .into_iter()
            .map(|edge| edge.handler_symbol)
            .collect()
    }

    #[test]
    fn extract_recipe_shell_calls_filters_shell_syntax() {
        // Conditional recipe — only `rsync` is a real command. Previously
        // emitted `[`, `then`, `fi` as fake dispatch handlers.
        let content = "\
deploy:
\tif [ -d build ]; then rsync -av build/ out/; fi
";
        let symbols = handler_symbols(content);
        assert!(
            symbols.contains(&"rsync".to_string()),
            "real command must be emitted: {symbols:?}"
        );
        for fake in ["[", "then", "fi", "if", "]"] {
            assert!(
                !symbols.contains(&fake.to_string()),
                "shell syntax `{fake}` leaked into dispatch_edges: {symbols:?}"
            );
        }
    }

    #[test]
    fn extract_recipe_shell_calls_filters_for_loop_keywords() {
        // `for X in list; do real_cmd; done` — only `real_cmd` real.
        let content = "\
ship:
\tfor f in $(SRC); do cp $$f out/; done
";
        let symbols = handler_symbols(content);
        assert!(
            symbols.contains(&"cp".to_string()),
            "loop body command must survive: {symbols:?}"
        );
        for fake in ["for", "in", "do", "done"] {
            assert!(
                !symbols.contains(&fake.to_string()),
                "loop keyword `{fake}` leaked: {symbols:?}"
            );
        }
    }

    #[test]
    fn extract_recipe_shell_calls_filters_case_pattern_tails() {
        // `case` arms leave `pattern)` shaped tokens (e.g. `true)`, `*.txt)`)
        // after the split. Those are not commands and must never leak.
        //
        // L4 scope: we filter the pattern tails out. Recovering the per-arm
        // body command (`echo` etc.) from inside the same segment is a more
        // invasive parse and is deferred — the priority for codex's HAK was
        // killing the false positives, not adding new positives.
        let content = "\
classify:
\tcase $$x in true) echo yes ;; false) echo no ;; esac
";
        let symbols = handler_symbols(content);
        for fake in ["true)", "false)", "case", "esac", "in"] {
            assert!(
                !symbols.contains(&fake.to_string()),
                "case-arm artifact `{fake}` leaked: {symbols:?}"
            );
        }
    }

    #[test]
    fn extract_recipe_shell_calls_drops_modifier_only_in_first_segment() {
        // Make recipe modifiers (`-`, `@`, `+`) are line-level: the `-` in
        // `... && -sS foo` is a flag, not an ignore-errors modifier. Trimming
        // it on every segment historically produced bogus `sS` commands.
        let content = "\
fetch:
\t@curl https://example.org && grep -sS pattern
";
        let symbols = handler_symbols(content);
        assert!(
            symbols.contains(&"curl".to_string()),
            "leading curl must survive modifier strip: {symbols:?}"
        );
        assert!(
            symbols.contains(&"grep".to_string()),
            "post-AND command must survive: {symbols:?}"
        );
        assert!(
            !symbols.contains(&"sS".to_string()),
            "stripped flag must not become a command: {symbols:?}"
        );
    }

    #[test]
    fn extract_recipe_shell_calls_preserves_path_qualified_commands() {
        // `./script.sh` and `bin/tool` must still extract to their basenames.
        let content = "\
build:
\t./scripts/build.sh && bin/postprocess output
";
        let symbols = handler_symbols(content);
        assert!(
            symbols.contains(&"build.sh".to_string()),
            "relative script path must survive: {symbols:?}"
        );
        assert!(
            symbols.contains(&"postprocess".to_string()),
            "bin-path command must survive: {symbols:?}"
        );
    }
}
