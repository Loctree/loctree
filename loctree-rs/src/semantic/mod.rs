//! Layer 3: RuntimeSemanticAnalyzer, per-runtime semantic interpretation of
//! the symbol model emitted by Layer 1/2 sensors.
//!
//! See LOCTREE_NEXT.md for the doctrinal split between sensors (parsers
//! deciding "this is a function") and semantic analyzers (deciding "this
//! function is reachable through case dispatch and classified as `usage`
//! help-printer idiom").
//!
//! Tree-sitter, when adopted, lives in Layer 1; it does not enter this module.

use crate::types::{FileAnalysis, Language};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub mod c_family;
pub mod idioms;
pub(crate) mod io;
pub mod make;
pub mod python;
pub mod rust;
pub mod shell;
pub mod tauri;
pub use idioms::{IdiomEntry, IdiomRegistry};
pub use make::MakeSemantics;
pub use python::PythonRuntimeSemantics;
pub use rust::RustRuntimeSemantics;
pub use shell::ShellSemantics;
pub use tauri::TauriSemantics;

/// Run every Layer 3 runtime semantic analyzer over `files` and return the
/// aggregated [`SemanticFacts`].
///
/// `workspace_root` is consulted for `<root>/.loctree/idioms/*.toml`
/// overrides; missing override directories degrade gracefully to embedded
/// defaults. Errors during analysis are logged to stderr and the partial
/// result is preserved — semantic enrichment never fails the scan.
///
/// Layer 1 sensors emit `language` strings (`"shell"`, `"make"`, ...) so this
/// helper is safe to call on any `FileAnalysis` slice; analyzers self-filter
/// by language. For repos with no shell/make files the returned facts are
/// empty (no idiom tags, no dispatch edges, no env contracts).
pub fn compute_semantic_facts(files: &[FileAnalysis], workspace_root: &Path) -> SemanticFacts {
    let registry = match IdiomRegistry::load_with_overrides(workspace_root) {
        Ok(reg) => reg,
        Err(err) => {
            eprintln!(
                "[loctree][warn] semantic: idiom registry failed to load ({err}); falling back to embedded defaults"
            );
            IdiomRegistry::load_defaults().unwrap_or_default()
        }
    };

    let mut facts = SemanticFacts::default();

    if let Err(err) = ShellSemantics.analyze(files, &registry, &mut facts) {
        eprintln!("[loctree][warn] semantic: ShellSemantics aborted: {err}");
    }
    if let Err(err) = MakeSemantics.analyze(files, &registry, &mut facts) {
        eprintln!("[loctree][warn] semantic: MakeSemantics aborted: {err}");
    }
    if let Err(err) = PythonRuntimeSemantics.analyze(files, &registry, &mut facts) {
        eprintln!("[loctree][warn] semantic: PythonRuntimeSemantics aborted: {err}");
    }
    if let Err(err) = TauriSemantics.analyze(files, &registry, &mut facts) {
        eprintln!("[loctree][warn] semantic: TauriSemantics aborted: {err}");
    }
    if let Err(err) = RustRuntimeSemantics.analyze(files, &registry, &mut facts) {
        eprintln!("[loctree][warn] semantic: RustRuntimeSemantics aborted: {err}");
    }

    facts
}

/// Stable symbol identifier across files. Format: `<file>::<symbol>`.
///
/// Future: replace with structural ID once SymbolModel ranges are stable.
pub type SymbolId = String;

/// A per-runtime analyzer that augments the symbol model with semantic facts
/// (idiom tags, reachability, dispatch edges, env contracts).
///
/// Analyzers must be pure with respect to their inputs: same `files` and same
/// `registry` produce the same `SemanticFacts`. This keeps snapshots
/// reproducible.
pub trait RuntimeSemanticAnalyzer: Send + Sync {
    fn language(&self) -> Language;

    fn analyze(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticFacts {
    pub idiom_tags: HashMap<SymbolId, Vec<IdiomTag>>,
    pub reachability: ReachabilityClaims,
    pub dispatch_edges: Vec<DispatchEdge>,
    pub env_contracts: Vec<EnvContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdiomTag {
    pub name: String,
    pub classifier: Classifier,
    pub runtime_role: RuntimeRole,
    pub source: TagSource,
    pub reasoning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Classifier {
    HelpPrinter,
    ErrorExit,
    PrimaryEntrypoint,
    UserFacingEntrypoint,
    PublicEntrypoint,
    LibraryHelper,
    Metadata,
    EnvVar,
    EnvContract,
    SourceLibraryApi,
    DispatchHandler,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeRole {
    UserFacing,
    PrimaryEntrypoint,
    PublicEntrypoint,
    LibraryHelper,
    EnvInput,
    Metadata,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagSource {
    EmbeddedDefault,
    UserOverride,
    InferredFromCode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReachabilityClaims {
    pub reached_symbols: HashSet<SymbolId>,
    pub unreached_symbols: HashSet<SymbolId>,
    pub reasons: HashMap<SymbolId, ReachReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReachReason {
    DirectImport,
    DispatchHandler {
        from_symbol: String,
        dispatch_kind: DispatchKind,
    },
    SourceInclude {
        from_file: String,
    },
    PhonyMakeTarget,
    RecipeShellCall {
        recipe_owner: String,
    },
    IdiomRuntimeRole(RuntimeRole),
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchEdge {
    pub from_file: String,
    pub from_line: u32,
    pub dispatch_kind: DispatchKind,
    pub handler_symbol: String,
    pub handler_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchKind {
    CaseStatement,
    /// Reserved for genuine `&fn` / callback-table dispatch (signal handlers,
    /// `register_callback(handler)`, GTK signal connections, etc.). Decorator-
    /// based registration must use one of the more specific kinds below.
    FunctionPointer,
    EvalString,
    RecipeShellCall,
    TauriInvoke,
    TauriEvent,
    /// Web framework decorator route: FastAPI / Flask / Starlette / Litestar /
    /// Django / aiohttp / axum / actix.
    HttpRoute,
    /// CLI subcommand registration: Typer / Click / clap / argparse-subparsers.
    CliCommand,
    /// Lifecycle / signal / event handlers: FastAPI `on_event`, Django
    /// `receiver`, pytest fixtures, Pydantic validators (anything that fires on
    /// an event instead of an HTTP request).
    EventHandler,
    /// Background-task registration: Celery / arq / Dramatiq / RQ.
    TaskTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvContract {
    pub name: String,
    pub used_in_files: Vec<String>,
    pub required_for: Vec<String>,
    /// Per-call-site detail for the env var: which file/line reads it, through
    /// which access pattern, with what default. Older snapshots and analyzers
    /// that don't populate occurrences will deserialize as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<EnvContractOccurrence>,
}

/// Single read-site for an environment variable: enough provenance for an
/// agent to grep the file, decide whether the variable is required, and choose
/// a default when running the project locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvContractOccurrence {
    pub file: String,
    pub line: u32,
    /// How the variable is read: `os.getenv`, `os.environ[]`, `os.environ.get`,
    /// `pydantic_settings`, `dynaconf`, `std::env::var`, `process.env`, etc.
    pub access_kind: String,
    /// Default value if any, serialised as the literal text from source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// True when the read pattern provides no default (i.e. the project will
    /// raise if the variable is unset).
    pub required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SemanticFactsRef;

    #[test]
    fn idiom_registry_loads_embedded_defaults() {
        let reg = IdiomRegistry::load_defaults().expect("defaults parse");
        assert!(reg.lookup(Language::Shell, "usage").is_some());
        assert!(reg.lookup(Language::Shell, "die").is_some());
        assert!(reg.lookup(Language::Rust, "#[tauri::command]").is_some());
        assert!(reg.lookup(Language::Typescript, "invoke").is_some());
        assert!(
            reg.lookup(Language::Shell, "abort").is_some(),
            "alias resolution"
        );
        assert!(reg.lookup(Language::Makefile, ".PHONY").is_some());
        assert!(reg.lookup(Language::Shell, "nonexistent_xyz").is_none());
    }

    #[test]
    fn idiom_registry_override_replaces_by_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let idioms_dir = tmp.path().join(".loctree").join("idioms");
        std::fs::create_dir_all(&idioms_dir).expect("mkdir");
        std::fs::write(
            idioms_dir.join("custom.toml"),
            r#"
[[idiom]]
name = "usage"
classifier = "HelpPrinter"
runtime_role = "Internal"
language = "shell"
reasoning = "Test override."
"#,
        )
        .expect("write");

        let reg = IdiomRegistry::load_with_overrides(tmp.path()).expect("load");
        let entry = reg.lookup(Language::Shell, "usage").expect("usage present");
        assert_eq!(
            entry.runtime_role,
            RuntimeRole::Internal,
            "override replaced runtime_role"
        );
        assert_eq!(entry.reasoning, "Test override.");
    }

    #[test]
    fn semantic_facts_serde_roundtrip() {
        let mut facts = SemanticFacts::default();
        facts.idiom_tags.insert(
            "scripts/install.sh::usage".into(),
            vec![IdiomTag {
                name: "usage".into(),
                classifier: Classifier::HelpPrinter,
                runtime_role: RuntimeRole::UserFacing,
                source: TagSource::EmbeddedDefault,
                reasoning: "roundtrip".into(),
            }],
        );
        facts
            .reachability
            .reached_symbols
            .insert("Makefile::all".into());
        facts.reachability.reasons.insert(
            "Makefile::all".into(),
            ReachReason::IdiomRuntimeRole(RuntimeRole::PublicEntrypoint),
        );
        facts.dispatch_edges.push(DispatchEdge {
            from_file: "scripts/install.sh".into(),
            from_line: 42,
            dispatch_kind: DispatchKind::CaseStatement,
            handler_symbol: "install_impl".into(),
            handler_file: Some("scripts/install.sh".into()),
        });
        facts.env_contracts.push(EnvContract {
            name: "LOCT_CACHE_DIR".into(),
            used_in_files: vec!["src/cache.rs".into()],
            required_for: vec!["cache override".into()],
            occurrences: vec![EnvContractOccurrence {
                file: "src/cache.rs".into(),
                line: 7,
                access_kind: "std::env::var".into(),
                default: None,
                required: true,
            }],
        });

        let json = serde_json::to_string(&facts).expect("ser");
        let _back: SemanticFacts = serde_json::from_str(&json).expect("de");
    }

    #[test]
    fn classifier_custom_variant_serde() {
        let classifier = Classifier::Custom("library_internal".into());
        let json = serde_json::to_string(&classifier).expect("ser");
        let back: Classifier = serde_json::from_str(&json).expect("de");
        assert_eq!(classifier, back);
    }

    #[test]
    fn semantic_facts_ref_serde_roundtrip() {
        let facts_ref = SemanticFactsRef {
            idiom_tag_count: 2,
            has_dispatch_edges: true,
            has_env_contracts: false,
        };
        let json = serde_json::to_string(&facts_ref).expect("ser");
        let back: SemanticFactsRef = serde_json::from_str(&json).expect("de");
        assert_eq!(facts_ref, back);
    }

    #[test]
    fn idiom_registry_serde_roundtrip() {
        let registry = IdiomRegistry::load_defaults().expect("defaults parse");
        let json = serde_json::to_string(&registry).expect("ser");
        let back: IdiomRegistry = serde_json::from_str(&json).expect("de");

        let entry = back
            .lookup(Language::Shell, "fail")
            .expect("alias survives roundtrip");
        assert_eq!(entry.classifier, Classifier::ErrorExit);
    }
}
