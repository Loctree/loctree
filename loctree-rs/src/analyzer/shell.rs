//! Lightweight shell script analyzer (bash/sh/zsh/fish).
//!
//! Regex-based parser that extracts function definitions, exported variables,
//! and `source`/`.` import statements. Mirrors the minimal-viable shape of
//! `analyzer/go.rs` — no shell grammar, just structural signals for the
//! dependency graph.

use std::collections::HashMap;
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
    Regex::new(r##"^\s*(?:source|\.)\s+(?:"([^"#;]+)"|'([^'#;]+)'|([^"'\s#;]+))"##)
        .expect("valid shell source regex")
});

static RE_IDENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\b").expect("valid shell ident regex"));

static RE_COMMAND_SUBST: Lazy<Regex> = Lazy::new(|| {
    // Matches:   $(cmd)   |   <(cmd)   |   >(cmd)
    Regex::new(r"(?:\$|<|>)\(\s*([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid command or process substitution regex")
});

static RE_VAR_ASSIGN: Lazy<Regex> = Lazy::new(|| {
    // Matches:   VAR=val   |   export VAR=val   |   local VAR=val   |   readonly VAR=val   |   declare [-x] VAR=val
    Regex::new(r#"^\s*(?:(?:export|local|readonly|declare(?:\s+-[A-Za-z]+)?|typeset)\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$"#)
        .expect("valid shell var assign regex")
});

static RE_VAR_REF: Lazy<Regex> = Lazy::new(|| {
    // Matches:   $VAR   |   ${VAR}
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid shell var ref regex")
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
    for imp in &analysis.imports {
        if matches!(imp.kind, ImportKind::Dynamic) {
            analysis.dynamic_imports.push(imp.source.clone());
        }
    }
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
        if let Some(caps) = RE_SOURCE.captures(&trimmed) {
            let path_opt = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(3))
                .map(|m| m.as_str().trim());
            if let Some(path) = path_opt {
                if path.is_empty() || path == "." || path == ".." {
                    continue;
                }
                if imports.iter().any(|i| i.source == path) {
                    continue;
                }
                let is_dynamic = path.contains('$');
                let mut entry = ImportEntry::new(
                    path.to_string(),
                    if is_dynamic {
                        ImportKind::Dynamic
                    } else {
                        ImportKind::Static
                    },
                );
                entry.line = Some(idx + 1);
                entry.resolution = if is_dynamic {
                    ImportResolutionKind::Dynamic
                } else {
                    ImportResolutionKind::Unknown
                };
                imports.push(entry);
            }
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

fn is_case_arm_label(label: &str) -> bool {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Case labels cannot contain redirection, process substitutions, command substitutions, or assignments
    if trimmed.contains('<')
        || trimmed.contains('>')
        || trimmed.contains('=')
        || trimmed.contains('$')
        || trimmed.contains('`')
    {
        return false;
    }
    // Optional leading '(' for `(pattern)`
    let pat = if let Some(stripped) = trimmed.strip_prefix('(') {
        stripped.trim()
    } else {
        trimmed
    };
    if pat.is_empty() || pat.contains('(') {
        return false;
    }
    true
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
        if let Some((case_label, after_case_label)) = segment.split_once(')')
            && is_case_arm_label(case_label)
        {
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

/// Extract simple constant variable assignments from shell script content.
/// Handles `VAR=val`, `export VAR=val`, `local VAR=val`, `readonly VAR=val`, etc.
/// Constant values with single/double quotes have quotes stripped.
fn extract_variable_assignments(content: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in content.lines() {
        let effective = strip_comment(line).trim();
        if effective.is_empty() {
            continue;
        }
        if let Some(caps) = RE_VAR_ASSIGN.captures(effective) {
            let name = caps.get(1).map(|m| m.as_str().to_string());
            let raw_val = caps.get(2).map(|m| m.as_str().trim());
            if let (Some(name), Some(raw_val)) = (name, raw_val) {
                let val = if (raw_val.starts_with('"')
                    && raw_val.ends_with('"')
                    && raw_val.len() >= 2)
                    || (raw_val.starts_with('\'') && raw_val.ends_with('\'') && raw_val.len() >= 2)
                {
                    &raw_val[1..raw_val.len() - 1]
                } else if raw_val.starts_with("$(") || raw_val.starts_with('`') {
                    // Non-constant command substitution; do not treat as simple constant
                    continue;
                } else {
                    raw_val.split([' ', '\t', ';']).next().unwrap_or(raw_val)
                };
                // Resolve any previously assigned variables inside this value
                let (expanded_val, _) = expand_variables(val, &vars);
                vars.insert(name, expanded_val);
            }
        }
    }
    vars
}

/// Substitute `$VAR` and `${VAR}` using known variables.
/// Returns (expanded_string, all_variables_resolved).
fn expand_variables(spec: &str, vars: &HashMap<String, String>) -> (String, bool) {
    let mut out = String::with_capacity(spec.len());
    let mut last_idx = 0;
    let mut all_resolved = true;

    for caps in RE_VAR_REF.captures_iter(spec) {
        let whole = caps.get(0).unwrap();
        out.push_str(&spec[last_idx..whole.start()]);

        let var_name = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");

        if let Some(val) = vars.get(var_name) {
            out.push_str(val);
        } else {
            all_resolved = false;
            out.push_str(whole.as_str());
        }
        last_idx = whole.end();
    }
    out.push_str(&spec[last_idx..]);
    (out, all_resolved)
}

fn format_resolved_path(path: &Path, root: &Path) -> String {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !root.as_os_str().is_empty() {
        if let Ok(canon_root) = root.canonicalize()
            && let Ok(rel) = canon.strip_prefix(&canon_root)
        {
            return rel.to_string_lossy().to_string();
        }
        if let Ok(rel) = canon.strip_prefix(root) {
            return rel.to_string_lossy().to_string();
        }
    }
    canon.to_string_lossy().to_string()
}

/// Resolve a shell `source`/`.` import relative to the file's directory.
/// Returns an absolute-ish or root-relative path string (canonicalized when possible).
///
/// Supports:
/// 1. Direct path resolution (relative to file_path's directory or absolute, and root-relative).
/// 2. Variable expansion for `$VAR` / `${VAR}` using simple constant assignments in `file_path`.
/// 3. Basename fallback when path is dynamic or unlocated, succeeding if exactly 1 matching file
///    exists in the project tree, and failing closed (None) with an honest unresolved note when ambiguous.
pub fn resolve_shell_source(spec: &str, file_path: &Path, root: &Path) -> Option<String> {
    let clean_spec = spec.trim().trim_matches(['"', '\'']);
    if clean_spec.is_empty() || clean_spec == "." || clean_spec == ".." {
        return None;
    }

    let parent = file_path.parent().map(|p| {
        if p.as_os_str().is_empty() {
            Path::new(".")
        } else {
            p
        }
    });

    // 1. Direct path check if spec contains no variables
    if !clean_spec.contains('$') {
        let candidate: PathBuf = if Path::new(clean_spec).is_absolute() {
            PathBuf::from(clean_spec)
        } else if let Some(parent) = parent {
            parent.join(clean_spec)
        } else {
            PathBuf::from(clean_spec)
        };
        if candidate.exists() {
            return Some(format_resolved_path(&candidate, root));
        }
        if !root.as_os_str().is_empty() {
            let root_candidate = root.join(clean_spec);
            if root_candidate.exists() {
                return Some(format_resolved_path(&root_candidate, root));
            }
        }
    }

    // 2. Variable expansion from known variable assignments in file_path
    let mut expanded = clean_spec.to_string();
    if clean_spec.contains('$') {
        let vars = if file_path.exists() && file_path.is_file() {
            std::fs::read_to_string(file_path)
                .map(|content| extract_variable_assignments(&content))
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let (exp, _) = expand_variables(clean_spec, &vars);
        expanded = exp;

        // Try candidate with expanded path
        if !expanded.contains('$') {
            let candidate: PathBuf = if Path::new(&expanded).is_absolute() {
                PathBuf::from(&expanded)
            } else if let Some(parent) = parent {
                parent.join(&expanded)
            } else {
                PathBuf::from(&expanded)
            };
            if candidate.exists() {
                return Some(format_resolved_path(&candidate, root));
            }
            if !root.as_os_str().is_empty() {
                let root_candidate = root.join(&expanded);
                if root_candidate.exists() {
                    return Some(format_resolved_path(&root_candidate, root));
                }
            }
        }
    }

    // 3. Basename fallback
    let candidate_str = if !expanded.is_empty() {
        &expanded
    } else {
        clean_spec
    };
    let target_basename = Path::new(candidate_str)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if target_basename.is_empty()
        || target_basename.contains('$')
        || target_basename == "."
        || target_basename == ".."
    {
        // Dynamic or invalid filename: fail closed with an honest unresolved note
        eprintln!(
            "[loctree][shell] unresolved source '{}' in {}: dynamic filename",
            spec,
            file_path.display()
        );
        return None;
    }

    let search_root = if root.is_dir() {
        root
    } else if let Some(p) = parent {
        if p.is_dir() { p } else { Path::new(".") }
    } else {
        Path::new(".")
    };

    let mut matches: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(search_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                !name.starts_with('.') && name != "node_modules" && name != "target"
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && entry.file_name() == target_basename {
            matches.push(entry.into_path());
            if matches.len() > 1 {
                break;
            }
        }
    }

    if matches.len() == 1 {
        let matched = matches.remove(0);
        return Some(format_resolved_path(&matched, root));
    } else if matches.len() > 1 {
        // Ambiguous basename: multiple candidate files exist.
        // Fail closed (no edge) with an honest unresolved note.
        eprintln!(
            "[loctree][shell] unresolved source '{}' in {}: ambiguous basename '{}' matches multiple candidates",
            spec,
            file_path.display(),
            target_basename
        );
        return None;
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

    #[test]
    fn w1_03_shell_process_substitution_callsite_counted() {
        let src = r#"
collect_stats() {
    echo "stats"
}

worker_in() {
    echo "in"
}

worker_out() {
    echo "out"
}

case_worker() {
    echo "case"
}

main() {
    while read -r line; do
        echo "$line"
    done < <(collect_stats)

    tee >(worker_out) < <(worker_in)

    case "$1" in
        run) case_worker < <(collect_stats) ;;
    esac
}
"#;
        let analysis = analyze_shell_file(src, "process_subst.sh".to_string());
        assert!(
            analysis.local_uses.contains(&"collect_stats".to_string()),
            "collect_stats called via <(...) must be counted as a local use"
        );
        assert!(
            analysis.local_uses.contains(&"worker_in".to_string()),
            "worker_in called via <(...) must be counted as a local use"
        );
        assert!(
            analysis.local_uses.contains(&"worker_out".to_string()),
            "worker_out called via >(...) must be counted as a local use"
        );
        assert!(
            analysis.local_uses.contains(&"case_worker".to_string()),
            "case_worker called in case arm with process substitution must be counted"
        );
        assert!(
            shell_local_uses_contain("done < <(collect_stats)", "collect_stats"),
            "done < <(collect_stats) must extract collect_stats"
        );
    }

    #[test]
    fn w1_03_dynamic_source_var_expansion_creates_edge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let lib_dir = root.join("lib");
        std::fs::create_dir_all(&lib_dir).expect("create lib dir");

        let foo_path = lib_dir.join("foo.sh");
        std::fs::write(&foo_path, "foo() { echo foo; }\n").expect("write foo.sh");

        let main_sh = root.join("main.sh");
        std::fs::write(
            &main_sh,
            r#"#!/bin/bash
LIB_DIR=lib
source "$LIB_DIR/foo.sh"
foo
"#,
        )
        .expect("write main.sh");

        // 1. Verify dynamic import detection in analyze_shell_file
        let analysis = analyze_shell_file(
            "LIB_DIR=lib\nsource \"$LIB_DIR/foo.sh\"\n",
            "main.sh".to_string(),
        );
        assert_eq!(analysis.imports.len(), 1);
        assert_eq!(analysis.imports[0].source, "$LIB_DIR/foo.sh");
        assert!(matches!(analysis.imports[0].kind, ImportKind::Dynamic));
        assert_eq!(
            analysis.imports[0].resolution,
            ImportResolutionKind::Dynamic
        );
        assert!(
            analysis
                .dynamic_imports
                .contains(&"$LIB_DIR/foo.sh".to_string())
        );

        // 2. Verify variable expansion in resolve_shell_source
        let resolved = resolve_shell_source("$LIB_DIR/foo.sh", &main_sh, root);
        assert!(resolved.is_some(), "expected $LIB_DIR/foo.sh to resolve");
        assert_eq!(resolved.unwrap(), "lib/foo.sh");

        // 3. Verify unambiguous basename fallback
        let bar_dir = root.join("unique_dir");
        std::fs::create_dir_all(&bar_dir).expect("create bar dir");
        let bar_path = bar_dir.join("bar.sh");
        std::fs::write(&bar_path, "bar() { echo bar; }\n").expect("write bar.sh");

        let resolved_bar = resolve_shell_source("$UNKNOWN_VAR/bar.sh", &main_sh, root);
        assert!(
            resolved_bar.is_some(),
            "expected unambiguous bar.sh to resolve via basename fallback"
        );
        assert_eq!(resolved_bar.unwrap(), "unique_dir/bar.sh");

        // 4. Verify ambiguous basename fallback fails closed without edge
        let ambig_dir = root.join("another_dir");
        std::fs::create_dir_all(&ambig_dir).expect("create ambig dir");
        let ambig_foo = ambig_dir.join("foo.sh");
        std::fs::write(&ambig_foo, "foo_other() { echo other; }\n").expect("write second foo.sh");

        let resolved_ambig = resolve_shell_source("$UNKNOWN_VAR/foo.sh", &main_sh, root);
        assert!(
            resolved_ambig.is_none(),
            "ambiguous basename matching multiple files must fail closed without edge"
        );

        // 5. Verify dynamic filename ($module.sh) fails closed without edge
        let resolved_dynamic = resolve_shell_source("$lib_dir/$module.sh", &main_sh, root);
        assert!(
            resolved_dynamic.is_none(),
            "dynamic filename ($module.sh) must fail closed without edge"
        );

        // 6. Verify resolution workflow produces local resolved_path
        let mut import_entry = analysis.imports[0].clone();
        import_entry.resolved_path = resolve_shell_source(&import_entry.source, &main_sh, root);
        if import_entry.resolved_path.is_some() {
            import_entry.resolution = ImportResolutionKind::Local;
        }
        assert_eq!(import_entry.resolved_path, Some("lib/foo.sh".to_string()));
        assert_eq!(import_entry.resolution, ImportResolutionKind::Local);
    }
}
