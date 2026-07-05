//! Lightweight shell script analyzer (bash/sh/zsh/fish).
//!
//! Regex-based parser that extracts function definitions, exported variables,
//! and `source`/`.` import statements. Mirrors the minimal-viable shape of
//! `analyzer/go.rs` — no shell grammar, just structural signals for the
//! dependency graph.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportResolutionKind};

// ---- regex cache (once_cell for thread-safe lazy init) ----

static RE_FUNC: Lazy<Regex> = Lazy::new(|| {
    // Matches:   function foo() {   |   foo() {   |   function foo {
    Regex::new(r"^\s*(?:function\s+([A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*\))")
        .expect("valid shell function regex")
});

static RE_EXPORT: Lazy<Regex> = Lazy::new(|| {
    // Matches:   export FOO=...   |   export FOO   |   declare -x FOO=...
    Regex::new(r"^\s*(?:export|declare\s+-x)\s+([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid shell export regex")
});

static RE_SOURCE: Lazy<Regex> = Lazy::new(|| {
    // Matches:   source path   |   . path   (but not `. ` alone or `..`)
    Regex::new(r#"^\s*(?:source|\.)\s+["']?([^"'\s#;]+)["']?"#).expect("valid shell source regex")
});

static RE_IDENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\b").expect("valid shell ident regex"));

static RE_COMMAND_SUBST: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\(\s*([A-Za-z_][A-Za-z0-9_]*)").expect("valid command substitution regex")
});

/// Detect if the first line of `content` is a shell shebang.
/// Used for extensionless file classification.
pub fn has_shell_shebang(content: &str) -> bool {
    let Some(first_line) = content.lines().next() else {
        return false;
    };
    if !first_line.starts_with("#!") {
        return false;
    }
    // Accept `#!/bin/sh`, `#!/usr/bin/env bash`, `#!/usr/bin/fish`, etc.
    first_line.contains("bash")
        || first_line.contains("zsh")
        || first_line.contains("fish")
        || first_line.ends_with("/sh")
        || first_line.contains("/sh ")
        || first_line.contains("env sh")
}

/// Analyze a shell script file with regex-based structural extraction.
pub fn analyze_shell_file(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);
    analysis.imports = parse_imports(content);
    analysis.exports = parse_exports_and_funcs(content);
    analysis.local_uses = collect_local_uses(content);
    analysis
}

fn parse_imports(content: &str) -> Vec<ImportEntry> {
    let mut imports: Vec<ImportEntry> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        // Skip shebang and comment-only lines
        let trimmed = strip_comment(line).trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        // Must match "source X" or ". X" (not "./x" standalone, though `. ./x`
        // would pass — regex requires whitespace after `.`). Guard against
        // the bash `..` no-op sequence.
        if let Some(caps) = RE_SOURCE.captures(&trimmed)
            && let Some(m) = caps.get(1)
        {
            let path = m.as_str().trim();
            if path.is_empty() || path == "." || path == ".." {
                continue;
            }
            if imports.iter().any(|i| i.source == path) {
                continue;
            }
            let mut entry = ImportEntry::new(path.to_string(), ImportKind::Static);
            entry.line = Some(idx + 1);
            entry.resolution = ImportResolutionKind::Unknown;
            imports.push(entry);
        }
    }
    imports
}

fn parse_exports_and_funcs(content: &str) -> Vec<ExportSymbol> {
    let mut exports: Vec<ExportSymbol> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        // Strip inline comments to avoid matching `echo "# export FOO"` etc.
        let effective = strip_comment(line);

        if let Some(caps) = RE_FUNC.captures(effective) {
            // group 1 = `function foo`, group 2 = `foo()`
            let name_opt = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str().to_string());
            if let Some(name) = name_opt
                && !name.is_empty()
                && !exports
                    .iter()
                    .any(|e| e.name == name && e.kind == "function")
            {
                exports.push(ExportSymbol::new(name, "function", "named", Some(idx + 1)));
            }
        }

        if let Some(caps) = RE_EXPORT.captures(effective)
            && let Some(m) = caps.get(1)
        {
            let name = m.as_str().to_string();
            if !exports.iter().any(|e| e.name == name && e.kind == "env") {
                exports.push(ExportSymbol::new(name, "env", "named", Some(idx + 1)));
            }
        }
    }
    exports
}

fn collect_local_uses(content: &str) -> Vec<String> {
    let mut uses = Vec::new();

    for line in content.lines() {
        let effective = strip_comment(line).trim();
        if effective.is_empty() || RE_FUNC.is_match(effective) {
            continue;
        }
        collect_shell_command_uses_from_line(effective, &mut uses);
    }

    uses
}

fn collect_shell_command_uses_from_line(line: &str, uses: &mut Vec<String>) {
    for caps in RE_COMMAND_SUBST.captures_iter(line) {
        if let Some(m) = caps.get(1) {
            push_shell_use(m.as_str(), uses);
        }
    }

    collect_trap_handler_use(line, uses);

    for segment in split_shell_command_segments(line) {
        let mut segment = strip_shell_leading_control(segment.trim());
        if segment.is_empty() {
            continue;
        }

        // case arms: `start) start_impl "$@" ;;`
        if let Some((_, after_case_label)) = segment.split_once(')') {
            for ident in RE_IDENT.find_iter(after_case_label).map(|m| m.as_str()) {
                if !is_shell_keyword_or_builtin(ident) {
                    push_shell_use(ident, uses);
                }
            }
            continue;
        }

        if let Some(assignment_rhs) = strip_leading_assignments(segment) {
            segment = assignment_rhs;
        }

        if let Some(first) = RE_IDENT.find(segment).map(|m| m.as_str()) {
            push_shell_use(first, uses);
        }
    }
}

fn collect_trap_handler_use(line: &str, uses: &mut Vec<String>) {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("trap ") else {
        return;
    };
    let Some(first_arg) = rest.split_whitespace().next() else {
        return;
    };
    let handler = first_arg.trim_matches(['"', '\'']);
    let Some(handler_match) = RE_IDENT.find(handler).filter(|m| m.start() == 0) else {
        return;
    };
    if handler_match.end() == handler.len() {
        push_shell_use(handler, uses);
    }
}

fn split_shell_command_segments(line: &str) -> impl Iterator<Item = &str> {
    line.split([';', '|', '&'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
}

fn strip_shell_leading_control(mut segment: &str) -> &str {
    loop {
        let trimmed = segment.trim_start();
        let Some(first) = RE_IDENT.find(trimmed).filter(|m| m.start() == 0) else {
            return trimmed;
        };
        let word = first.as_str();
        if matches!(
            word,
            "if" | "then"
                | "elif"
                | "else"
                | "do"
                | "while"
                | "until"
                | "time"
                | "command"
                | "builtin"
                | "exec"
                | "!"
                | "coproc"
        ) {
            segment = &trimmed[first.end()..];
            continue;
        }
        return trimmed;
    }
}

fn strip_leading_assignments(segment: &str) -> Option<&str> {
    let mut rest = segment.trim_start();
    let mut stripped = false;

    while let Some(eq_pos) = rest.find('=') {
        let lhs = rest[..eq_pos].trim();
        if lhs.is_empty()
            || lhs
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == ' '))
        {
            break;
        }
        let next_space = rest[eq_pos + 1..].find(char::is_whitespace);
        let Some(next_space) = next_space else {
            return Some("");
        };
        rest = rest[eq_pos + 1 + next_space..].trim_start();
        stripped = true;
    }

    if stripped { Some(rest) } else { None }
}

fn push_shell_use(name: &str, uses: &mut Vec<String>) {
    if !name.is_empty() && !is_shell_keyword_or_builtin(name) && !uses.iter().any(|u| u == name) {
        uses.push(name.to_string());
    }
}

fn is_shell_keyword_or_builtin(name: &str) -> bool {
    matches!(
        name,
        "alias"
            | "bg"
            | "break"
            | "case"
            | "cd"
            | "command"
            | "continue"
            | "declare"
            | "do"
            | "done"
            | "echo"
            | "elif"
            | "else"
            | "esac"
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "false"
            | "fi"
            | "for"
            | "function"
            | "getopts"
            | "hash"
            | "if"
            | "in"
            | "local"
            | "printf"
            | "pwd"
            | "read"
            | "readonly"
            | "return"
            | "select"
            | "set"
            | "shift"
            | "source"
            | "test"
            | "then"
            | "time"
            | "trap"
            | "true"
            | "type"
            | "typeset"
            | "ulimit"
            | "umask"
            | "unalias"
            | "unset"
            | "until"
            | "wait"
            | "while"
    )
}

#[cfg(test)]
fn shell_local_uses_contain(line: &str, name: &str) -> bool {
    let mut uses = Vec::new();
    collect_shell_command_uses_from_line(line, &mut uses);
    uses.iter().any(|u| u == name)
}

/// Strip `# ...` line comments while respecting single/double quotes.
/// (Not bulletproof against heredocs or `$#`, but good enough for structural
/// extraction.)
fn strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = line.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        match ch {
            '\\' => {
                idx += 2;
                continue;
            }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                // Guard $#, ${#var}, ${var#pat}
                if idx > 0 {
                    let prev = bytes[idx - 1] as char;
                    if prev == '$' || prev == '{' {
                        idx += 1;
                        continue;
                    }
                }
                return &line[..idx];
            }
            _ => {}
        }
        idx += 1;
    }
    line
}

/// Resolve a shell `source`/`.` import relative to the file's directory.
/// Returns an absolute-ish path string (canonicalized when possible).
pub fn resolve_shell_source(spec: &str, file_path: &Path, _root: &Path) -> Option<String> {
    let parent = file_path.parent()?;
    let candidate: PathBuf = if Path::new(spec).is_absolute() {
        PathBuf::from(spec)
    } else {
        parent.join(spec)
    };
    if candidate.exists() {
        let canon = candidate.canonicalize().unwrap_or(candidate);
        return Some(canon.to_string_lossy().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shell_shebangs() {
        assert!(has_shell_shebang("#!/bin/bash\nfoo"));
        assert!(has_shell_shebang("#!/usr/bin/env bash"));
        assert!(has_shell_shebang("#!/usr/bin/env zsh"));
        assert!(has_shell_shebang("#!/usr/bin/fish"));
        assert!(has_shell_shebang("#!/bin/sh"));
        assert!(!has_shell_shebang("#!/usr/bin/env python"));
        assert!(!has_shell_shebang("no shebang here"));
        assert!(!has_shell_shebang(""));
    }

    #[test]
    fn parses_functions() {
        let src = r#"#!/bin/bash

function greet() {
    echo "hello"
}

run_thing() {
    echo "run"
}

# This is just a comment: fake() {
not_a_function_arg=1
"#;
        let analysis = analyze_shell_file(src, "test.sh".to_string());
        let names: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "function")
            .map(|e| e.name.clone())
            .collect();
        assert!(names.contains(&"greet".to_string()));
        assert!(names.contains(&"run_thing".to_string()));
        assert!(!names.contains(&"fake".to_string()));
    }

    #[test]
    fn parses_exports() {
        let src = r#"
export PATH=/usr/bin
export MY_VAR="value"
declare -x DECLARED_VAR=42
local not_exported=1
"#;
        let analysis = analyze_shell_file(src, "test.sh".to_string());
        let names: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "env")
            .map(|e| e.name.clone())
            .collect();
        assert!(names.contains(&"PATH".to_string()));
        assert!(names.contains(&"MY_VAR".to_string()));
        assert!(names.contains(&"DECLARED_VAR".to_string()));
        assert!(!names.contains(&"not_exported".to_string()));
    }

    #[test]
    fn parses_sources() {
        let src = r#"
source ./common.sh
. utils.sh
source "/etc/profile.d/my.sh"
# source fake.sh   (commented)
echo "source pipe.sh"  # not a source
"#;
        let analysis = analyze_shell_file(src, "test.sh".to_string());
        let sources: Vec<_> = analysis.imports.iter().map(|i| i.source.clone()).collect();
        assert!(sources.contains(&"./common.sh".to_string()));
        assert!(sources.contains(&"utils.sh".to_string()));
        assert!(sources.contains(&"/etc/profile.d/my.sh".to_string()));
        assert!(!sources.contains(&"fake.sh".to_string()));
    }

    #[test]
    fn collects_local_function_calls() {
        let src = r#"
usage() {
    echo "usage"
}

_vetcoders_spawn_script() {
    echo spawn
}

main() {
    usage
    if _vetcoders_spawn_script "$@"; then
        return 0
    fi
}

main "$@"
"#;
        let analysis = analyze_shell_file(src, "vetcoders.sh".to_string());

        assert!(analysis.local_uses.contains(&"usage".to_string()));
        assert!(
            analysis
                .local_uses
                .contains(&"_vetcoders_spawn_script".to_string())
        );
        assert!(analysis.local_uses.contains(&"main".to_string()));
    }

    #[test]
    fn collects_shell_command_positions_without_substring_matches() {
        let src = r#"
run_task() { echo run; }
case_dispatch() { echo case; }
render() { echo render; }

if run_task "$@"; then
    echo ok
fi

case "$cmd" in
    start) case_dispatch "$@" ;;
esac

result="$(render --json)"
trap cleanup EXIT
foo_bar
"#;
        let analysis = analyze_shell_file(src, "dispatch.sh".to_string());

        assert!(analysis.local_uses.contains(&"run_task".to_string()));
        assert!(analysis.local_uses.contains(&"case_dispatch".to_string()));
        assert!(analysis.local_uses.contains(&"render".to_string()));
        assert!(analysis.local_uses.contains(&"cleanup".to_string()));
        assert!(!analysis.local_uses.contains(&"foo".to_string()));
        assert!(shell_local_uses_contain("if run_task; then", "run_task"));
        assert!(!shell_local_uses_contain("foo_bar", "foo"));
    }

    #[test]
    fn strip_comment_respects_quotes() {
        assert_eq!(strip_comment("echo hello # comment"), "echo hello ");
        assert_eq!(strip_comment("echo 'a # b'"), "echo 'a # b'");
        assert_eq!(strip_comment("echo \"a # b\""), "echo \"a # b\"");
        assert_eq!(strip_comment("echo $# args"), "echo $# args");
        assert_eq!(strip_comment("no comment"), "no comment");
    }
}
