//! Named environment reads discovered directly from source files.
//!
//! This is deliberately an inventory, not a value reader: only variable names,
//! source locations, and access classes leave this module. Dynamic lookups stay
//! omitted rather than being guessed.
//!
//! # Evidence ladder (Rust)
//!
//! Rust hides its env reads behind three different shapes, and each carries a
//! different strength of proof. The name-shape requirement tightens as the
//! proof weakens, so a weak signal can never invent an env var out of an
//! unrelated constant:
//!
//! 1. **Direct** — `std::env::var("X")` / `env::var_os("X")`. The callee *is*
//!    the env API, so any SCREAMING_SNAKE literal is accepted.
//! 2. **Accessor wrapper** — a call whose own identifier says `env`
//!    (`effective_env_string("X", ..)`, `env_bool("X", ..)`, `getenv("X")`).
//!    The callee name is the proof; write-shaped accessors (`set_`, `remove_`,
//!    `unset_`, …) are rejected so a mutation site never masquerades as a read.
//! 3. **Key registry** — a `const` / `static` array of key names
//!    (`const PROMOTED_SETTINGS_KEYS: &[&str] = &["APP_MODE", ..]`). This is
//!    the "promoted key" shape: the key never reaches `env::var` in that file,
//!    it is routed through a settings brain. Weakest proof, so it demands both
//!    a binding named like a key registry (`ENV` / `KEY` / `SETTING` / `VAR`)
//!    **and** a key literal carrying at least one underscore.
//!
//! Detection is line-oriented and therefore comment/string-blind by design —
//! this is an inventory that must not miss live contract keys, and callers
//! label the result as an inventory, never as reachability proof.

use once_cell::sync::Lazy;
use regex::Regex;
use std::path::{Component, Path};

use crate::snapshot::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEnvRead {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub access_kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourceEnvInventory {
    pub reads: Vec<SourceEnvRead>,
    pub classes: Vec<String>,
    pub files_scanned: usize,
}

static RUST_ENV_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:std::)?env::(?P<kind>var|var_os)\s*\(\s*\"(?P<name>[A-Z][A-Z0-9_]*)\""#)
        .expect("valid Rust environment-read regex")
});

/// Tier 2: a call whose own identifier contains `env`, taking a
/// SCREAMING_SNAKE literal first argument. Catches project-local accessors
/// (`effective_env_string`, `env_bool`, `env_f32`, `require_env`, `getenv`)
/// that `env::var` regexes never see. `std::env::var(..)` cannot match here —
/// `env` is followed by `::`, not `(`.
static RUST_ENV_WRAPPER_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        // `\b` + a possibly-empty prefix: the `env` fragment may open the
        // identifier (`env_bool`) or sit inside it (`effective_env_string`),
        // but the match must still start at an identifier boundary so a
        // suffix of an unrelated word never becomes the callee. `dot` records
        // whether this was a method call — see [`is_child_env_builder`].
        r#"(?P<dot>\.)?\b(?P<call>[A-Za-z0-9_]*(?i:env)[A-Za-z0-9_]*)\s*\(\s*\"(?P<name>[A-Z][A-Z0-9_]*)\""#,
    )
    .expect("valid Rust env-accessor-wrapper regex")
});

/// Same two tiers, but for the call rustfmt broke across lines:
///
/// ```text
/// stt_engine: effective_env_string(
///     "CODESCRIBE_STT_ENGINE",
/// ```
///
/// Long argument lists get wrapped as a matter of routine, so a scanner that
/// only ever looks at one line at a time misses a large share of the real
/// wrappers — and silently, which is the worst way to miss anything.
static RUST_ENV_READ_OPEN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:std::)?env::(?P<kind>var|var_os)\s*\(\s*$")
        .expect("valid split std::env read-opener regex")
});

static RUST_ENV_WRAPPER_OPEN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?P<dot>\.)?\b(?P<call>[A-Za-z0-9_]*(?i:env)[A-Za-z0-9_]*)\s*\(\s*$")
        .expect("valid split env-accessor-wrapper opener regex")
});

/// The continuation line of a broken call: the key literal must lead it, so an
/// unrelated later argument can never be mistaken for the variable name.
static RUST_LEADING_KEY_LITERAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^\s*\"(?P<name>[A-Z][A-Z0-9_]*)\""#).expect("valid leading key-literal regex")
});

/// Tier 3 opener: `const NAME: <ty> = [` / `= &[`. The binding name is
/// filtered by [`is_key_registry_binding`] before any literal is harvested.
static RUST_KEY_REGISTRY_OPEN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(?:const|static)\s+(?P<binding>[A-Z][A-Z0-9_]*)\s*:[^=]*=\s*&?\s*\[")
        .expect("valid Rust key-registry opener regex")
});

/// Tier 3 payload: a SCREAMING_SNAKE literal carrying at least one underscore.
/// The underscore requirement is what keeps `"GET"` / `"OK"` / `"UTF8"` out of
/// the env catalogue when the weakest tier is the only evidence.
static RUST_KEY_REGISTRY_LITERAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"\"(?P<name>[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)\""#)
        .expect("valid key-registry literal regex")
});

/// Identifier segments that mark an accessor as a *writer*. Matched
/// segment-wise on `_` so `env_settings` (contains `set`) stays a read while
/// `set_env_var` is rejected.
const ENV_WRITE_SEGMENTS: &[&str] = &[
    "set", "unset", "remove", "delete", "del", "clear", "export", "write", "store", "put", "save",
    "persist", "restore", "seed", "inject", "reset",
];

/// Binding-name fragments that make a `const` array plausible as a registry of
/// environment / settings keys.
const KEY_REGISTRY_BINDING_FRAGMENTS: &[&str] = &["ENV", "KEY", "SETTING", "VAR"];

fn is_env_write_accessor(call: &str) -> bool {
    call.split('_')
        .any(|segment| ENV_WRITE_SEGMENTS.contains(&segment.to_ascii_lowercase().as_str()))
}

/// `command.env("KEY", "value")` / `.envs(..)` is the `std::process::Command`
/// builder: it *populates* a child environment, it does not consume a contract.
/// Every genuine reader wrapper carries a more descriptive name than the bare
/// `env`, so rejecting the bare method keeps child-process setup out of the
/// read side without costing a single real reader.
fn is_child_env_builder(call: &str, is_method_call: bool) -> bool {
    is_method_call && matches!(call.to_ascii_lowercase().as_str(), "env" | "envs")
}

fn is_key_registry_binding(binding: &str) -> bool {
    KEY_REGISTRY_BINDING_FRAGMENTS
        .iter()
        .any(|fragment| binding.contains(fragment))
}

static SWIFT_ENV_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?:ProcessInfo\s*\.\s*processInfo\s*\.\s*)?environment\s*\[\s*\"(?P<name>[A-Z][A-Z0-9_]*)\"\s*\]"#,
    )
    .expect("valid Swift environment-read regex")
});

static JS_ENV_DOT_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"process\s*\.\s*env\s*\.\s*(?P<name>[A-Z][A-Z0-9_]*)"#)
        .expect("valid JS process.env dot-read regex")
});

static JS_ENV_INDEX_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"process\s*\.\s*env\s*\[\s*['\"](?P<name>[A-Z][A-Z0-9_]*)['\"]\s*\]"#)
        .expect("valid JS process.env index-read regex")
});

/// Python literal env reads: `os.environ.get("X")`, `os.environ["X"]`,
/// `os.getenv("X")`. H5a — env-truth's source-read inventory previously
/// scanned rust/swift/js only, so a Python-heavy repo reported 0 read
/// sites and every declaration looked orphaned. Dynamic keys
/// (`environ.get(var)`) stay out of the inventory by design; the semantic
/// layer records those separately.
static PY_ENV_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?:os\s*\.\s*)?(?:environ\s*\.\s*get|getenv)\s*\(\s*['\"](?P<name>[A-Z][A-Z0-9_]*)['\"]"#,
    )
    .expect("valid Python env-read regex")
});

static PY_ENV_INDEX_READ: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"os\s*\.\s*environ\s*\[\s*['\"](?P<name>[A-Z][A-Z0-9_]*)['\"]\s*\]"#)
        .expect("valid Python environ index-read regex")
});

pub(crate) fn collect_source_env_reads(
    project_root: &Path,
    snapshot: &Snapshot,
) -> SourceEnvInventory {
    let mut inventory = SourceEnvInventory {
        classes: vec![
            "javascript_process_env".to_string(),
            "python_os_environ".to_string(),
            "rust_env_accessor_wrapper".to_string(),
            "rust_key_registry_const".to_string(),
            "rust_std_env".to_string(),
            "swift_process_environment".to_string(),
        ],
        ..SourceEnvInventory::default()
    };

    for file in &snapshot.files {
        let class = match file.language.as_str() {
            "rs" | "rust" => "rust",
            "swift" => "swift",
            "js" | "jsx" | "ts" | "tsx" | "javascript" | "typescript" => "javascript",
            "py" | "python" => "python",
            _ => continue,
        };
        let Some(path) = safe_project_path(project_root, &file.path) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        inventory.files_scanned += 1;
        collect_file_reads(class, &file.path, &source, &mut inventory.reads);
    }

    inventory.reads.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.access_kind.cmp(&b.access_kind))
    });
    inventory.reads.dedup();
    inventory
}

fn safe_project_path(project_root: &Path, relative: &str) -> Option<std::path::PathBuf> {
    let candidate = Path::new(relative);
    if candidate.is_absolute()
        || candidate.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(project_root.join(candidate))
}

fn collect_file_reads(class: &str, file: &str, source: &str, reads: &mut Vec<SourceEnvRead>) {
    // Tier-3 state: the key-registry `const` block currently open, if any.
    // Registries routinely span dozens of lines, so the opener/closer pair is
    // tracked across the line loop instead of being re-detected per line.
    let mut open_registry: Option<String> = None;
    // A call broken across lines by rustfmt: `(access_kind, call line)` waiting
    // for its key literal on the very next line.
    let mut pending_call: Option<(String, u32)> = None;
    for (index, line) in source.lines().enumerate() {
        let line_number = (index + 1) as u32;
        match class {
            "rust" => collect_rust_line(
                file,
                line,
                line_number,
                &mut open_registry,
                &mut pending_call,
                reads,
            ),
            "swift" => push_captures(
                &SWIFT_ENV_READ,
                line,
                file,
                line_number,
                |_| Some("ProcessInfo.environment[]".to_string()),
                reads,
            ),
            "javascript" => {
                push_captures(
                    &JS_ENV_DOT_READ,
                    line,
                    file,
                    line_number,
                    |_| Some("process.env.NAME".to_string()),
                    reads,
                );
                push_captures(
                    &JS_ENV_INDEX_READ,
                    line,
                    file,
                    line_number,
                    |_| Some("process.env[]".to_string()),
                    reads,
                );
            }
            "python" => {
                push_captures(
                    &PY_ENV_READ,
                    line,
                    file,
                    line_number,
                    |_| Some("os.environ.get()".to_string()),
                    reads,
                );
                push_captures(
                    &PY_ENV_INDEX_READ,
                    line,
                    file,
                    line_number,
                    |_| Some("os.environ[]".to_string()),
                    reads,
                );
            }
            _ => {}
        }
    }
}

/// Walk one Rust line through all three tiers of the evidence ladder.
///
/// `open_registry` carries the tier-3 block state between lines: `Some(binding)`
/// while a key-registry array is open, `None` otherwise.
fn collect_rust_line(
    file: &str,
    line: &str,
    line_number: u32,
    open_registry: &mut Option<String>,
    pending_call: &mut Option<(String, u32)>,
    reads: &mut Vec<SourceEnvRead>,
) {
    // Continuation of a call opened on the previous line. Exactly one line of
    // lookahead: the key is rustfmt's first argument or it is not the key.
    if let Some((access_kind, call_line)) = pending_call.take()
        && let Some(captures) = RUST_LEADING_KEY_LITERAL.captures(line)
        && let Some(name) = captures.name("name")
    {
        reads.push(SourceEnvRead {
            name: name.as_str().to_string(),
            file: file.to_string(),
            line: call_line,
            access_kind,
        });
    }

    // Tier 1 — the env API itself.
    push_captures(
        &RUST_ENV_READ,
        line,
        file,
        line_number,
        |captures| {
            let kind = captures.name("kind")?.as_str();
            Some(format!("std::env::{kind}"))
        },
        reads,
    );

    // Tier 2 — an accessor whose own name says `env`, minus the writers.
    push_captures(
        &RUST_ENV_WRAPPER_READ,
        line,
        file,
        line_number,
        |captures| {
            let call = captures.name("call")?.as_str();
            if is_env_write_accessor(call)
                || is_child_env_builder(call, captures.name("dot").is_some())
            {
                return None;
            }
            Some(format!("{call}()"))
        },
        reads,
    );

    // Arm the one-line lookahead when this line ends on an open env call.
    if let Some(captures) = RUST_ENV_READ_OPEN.captures(line)
        && let Some(kind) = captures.name("kind")
    {
        *pending_call = Some((format!("std::env::{}", kind.as_str()), line_number));
    } else if let Some(captures) = RUST_ENV_WRAPPER_OPEN.captures(line)
        && let Some(call) = captures.name("call")
    {
        let call = call.as_str();
        if !is_env_write_accessor(call)
            && !is_child_env_builder(call, captures.name("dot").is_some())
        {
            *pending_call = Some((format!("{call}()"), line_number));
        }
    }

    // Tier 3 — `const`/`static` key registries (the "promoted key" shape).
    if open_registry.is_none()
        && let Some(captures) = RUST_KEY_REGISTRY_OPEN.captures(line)
        && let Some(binding) = captures.name("binding")
        && is_key_registry_binding(binding.as_str())
    {
        *open_registry = Some(binding.as_str().to_string());
    }
    if let Some(binding) = open_registry.clone() {
        for captures in RUST_KEY_REGISTRY_LITERAL.captures_iter(line) {
            let Some(name) = captures.name("name") else {
                continue;
            };
            reads.push(SourceEnvRead {
                name: name.as_str().to_string(),
                file: file.to_string(),
                line: line_number,
                access_kind: format!("const {binding}[]"),
            });
        }
        if line.contains("];") {
            *open_registry = None;
        }
    }
}

fn push_captures(
    regex: &Regex,
    line: &str,
    file: &str,
    line_number: u32,
    access_kind: impl Fn(&regex::Captures<'_>) -> Option<String>,
    reads: &mut Vec<SourceEnvRead>,
) {
    for captures in regex.captures_iter(line) {
        let Some(name) = captures.name("name").map(|value| value.as_str()) else {
            continue;
        };
        let Some(access_kind) = access_kind(&captures) else {
            continue;
        };
        reads.push(SourceEnvRead {
            name: name.to_string(),
            file: file.to_string(),
            line: line_number,
            access_kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_literal_env_reads_are_inventoried() {
        // H5a: a Python-heavy repo previously reported 0 read sites because
        // the inventory only scanned rust/swift/js.
        let mut reads = Vec::new();
        let source = "import os\nv = os.environ.get(\"SCREENSCRIBE_MODEL\")\nw = os.environ[\"API_BASE\"]\nx = os.getenv(\"CACHE_DIR\", None)\ny = os.environ.get(dynamic_key)\n";
        collect_file_reads("python", "app/config.py", source, &mut reads);
        let names: Vec<&str> = reads.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"SCREENSCRIBE_MODEL"),
            "environ.get literal: {names:?}"
        );
        assert!(names.contains(&"API_BASE"), "environ[] literal: {names:?}");
        assert!(names.contains(&"CACHE_DIR"), "getenv literal: {names:?}");
        assert_eq!(
            reads.len(),
            3,
            "dynamic key must stay out of the inventory: {names:?}"
        );
    }

    #[test]
    fn extracts_names_from_supported_source_classes_without_values() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "src/main.rs",
            "let _ = std::env::var(\"RUST_TOKEN\");\nlet _ = env::var_os(\"CACHE_ROOT\");",
            &mut reads,
        );
        collect_file_reads(
            "swift",
            "Sources/App.swift",
            "let token = ProcessInfo.processInfo.environment[\"SWIFT_TOKEN\"]",
            &mut reads,
        );
        collect_file_reads(
            "javascript",
            "src/config.ts",
            "const a = process.env.API_URL; const b = process.env['BUILD_ID'];",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "RUST_TOKEN",
                "CACHE_ROOT",
                "SWIFT_TOKEN",
                "API_URL",
                "BUILD_ID"
            ]
        );
        assert!(reads.iter().all(|read| !read.access_kind.contains("TOKEN")));
    }

    /// Tier 2. Regression for the 2026-08-16 Codescribe hak: the live STT
    /// contract keys are read through `effective_env_string(..)`, never through
    /// `env::var`, so a `env::var`-only scanner reported them as non-existent.
    #[test]
    fn accessor_wrappers_count_as_reads() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "bridge/src/config.rs",
            "stt_engine: effective_env_string(\"CODESCRIBE_STT_ENGINE\", settings, &env_file),\n\
             let idle = env_u64(\"IDLE_UNLOAD_SECS\", 300);\n",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(names, vec!["CODESCRIBE_STT_ENGINE", "IDLE_UNLOAD_SECS"]);
        assert_eq!(reads[0].access_kind, "effective_env_string()");
        assert_eq!(reads[0].line, 1);
    }

    /// Tier 2 must not turn a mutation into a read — `set_env_var("X", ..)`
    /// writes the process env, it does not consume a contract.
    #[test]
    fn write_shaped_accessors_are_not_reads() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "src/boot.rs",
            "set_env_var(\"FORCED_MODE\", \"on\");\nremove_env(\"LEGACY_FLAG\");\n",
            &mut reads,
        );
        assert!(
            reads.is_empty(),
            "write accessors leaked into the read inventory: {reads:?}"
        );
    }

    /// rustfmt wraps long argument lists as a matter of routine. A one-line
    /// scanner therefore misses real wrappers systematically — in Codescribe it
    /// saw 1 of the 3 `effective_env_string` call sites.
    #[test]
    fn calls_broken_across_lines_are_still_reads() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "bridge/src/config.rs",
            "stt_engine: effective_env_string(\n\
             \x20   \"CODESCRIBE_STT_ENGINE\",\n\
             \x20   settings.stt_engine.clone(),\n\
             ),\n\
             let raw = std::env::var(\n\
             \x20   \"SPLIT_DIRECT_READ\",\n\
             );\n\
             let path = write_env_file(\n\
             \x20   \"MUTATED_KEY\",\n\
             );\n",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["CODESCRIBE_STT_ENGINE", "SPLIT_DIRECT_READ"],
            "the write-shaped wrapper must stay out even when broken across lines"
        );
        // The read is attributed to the call site, matching the single-line case.
        assert_eq!(reads[0].line, 1);
        assert_eq!(reads[0].access_kind, "effective_env_string()");
        assert_eq!(reads[1].line, 5);
    }

    /// Lookahead is exactly one line: a key literal that only appears further
    /// down is a different argument, not the variable being read.
    #[test]
    fn lookahead_does_not_reach_past_the_first_argument() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "src/cfg.rs",
            "let v = env_lookup(\n\
             \x20   ctx,\n\
             \x20   \"SECOND_ARGUMENT\",\n\
             );\n",
            &mut reads,
        );
        assert!(reads.is_empty(), "lookahead overreached: {reads:?}");
    }

    /// Mutation verbs beyond the obvious `set_`/`remove_` pair: `save_to_env`
    /// and `restore_env_for_test` write the environment, and both were counted
    /// as readers on the first pass over the Codescribe tree.
    #[test]
    fn mutation_verbs_are_rejected() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "core/config/loader.rs",
            "self.save_to_env(\"PERSISTED_KEY\", value)?;\n\
             restore_env_for_test(\"RESTORED_KEY\", previous);\n\
             let live = config_runtime_env_var(\"LIVE_KEY\");\n",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["LIVE_KEY"],
            "only the accessor that returns a value is a read"
        );
    }

    /// `Command::env("K", "V")` seeds a child process; calling it a read would
    /// re-introduce the same class of catalogue lie this ladder exists to kill.
    /// A free function named `env(..)` stays a read — only the bare method form
    /// is the builder.
    #[test]
    fn command_env_builder_is_not_a_read() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "bin/corpus.rs",
            "command.env(\"CHILD_MODE\", \"apple\").envs(\"EXTRA_VARS\", &map);\n\
             let v = env(\"OWN_LOOKUP\");\n",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(names, vec!["OWN_LOOKUP"]);
    }

    /// `env_settings(..)` contains the letters `set` but is a reader — the
    /// writer filter matches whole `_` segments, not substrings.
    #[test]
    fn settings_shaped_accessor_stays_a_read() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "src/cfg.rs",
            "let v = env_settings(\"APP_MODE\");\n",
            &mut reads,
        );
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].name, "APP_MODE");
    }

    /// Tier 3. The "promoted key" shape: the key lives in a settings brain and
    /// only ever appears as a literal inside a `const` registry array.
    #[test]
    fn const_key_registry_yields_promoted_keys() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "core/config/settings.rs",
            "pub const PROMOTED_SETTINGS_KEYS: &[&str] = &[\n\
             \x20   \"WHISPER_MODEL\",\n\
             \x20   // comment between entries\n\
             \x20   \"CODESCRIBE_ASR_MODE\",\n\
             \x20   \"CODESCRIBE_CLOUD_CONSENT\",\n\
             ];\n\
             pub const UNRELATED_LIMITS: &[usize] = &[1, 2];\n\
             let ignored = \"NOT_IN_A_REGISTRY\";\n",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "WHISPER_MODEL",
                "CODESCRIBE_ASR_MODE",
                "CODESCRIBE_CLOUD_CONSENT"
            ],
            "registry harvesting must start at the opener and stop at `];`"
        );
        assert_eq!(reads[0].access_kind, "const PROMOTED_SETTINGS_KEYS[]");
    }

    /// Tier 3 is the weakest evidence, so it stays fenced: a `const` array that
    /// is not named like a key registry contributes nothing, and single-word
    /// literals never qualify.
    #[test]
    fn const_arrays_without_registry_shape_contribute_nothing() {
        let mut reads = Vec::new();
        collect_file_reads(
            "rust",
            "src/http.rs",
            "const ALLOWED_METHODS: &[&str] = &[\"GET\", \"POST\", \"CONTENT_TYPE\"];\n\
             const SUPPORTED_ENV_KEYS: &[&str] = &[\"GET\", \"APP_MODE\"];\n",
            &mut reads,
        );

        let names: Vec<&str> = reads.iter().map(|read| read.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["APP_MODE"],
            "only the registry-shaped binding contributes, and only underscored keys"
        );
    }

    #[test]
    fn rejects_paths_outside_the_project() {
        assert!(safe_project_path(Path::new("/tmp/project"), "../secret.rs").is_none());
        assert!(safe_project_path(Path::new("/tmp/project"), "/tmp/secret.rs").is_none());
        assert!(safe_project_path(Path::new("/tmp/project"), "src/main.rs").is_some());
    }
}
