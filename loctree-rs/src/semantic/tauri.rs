//! Layer 3 semantic analyzer for Tauri command and event contracts.
//!
//! Layer 1 sensors already extract raw `CommandRef` and `EventRef` values from
//! Rust and frontend files. This module promotes those raw facts into the shared
//! semantic contract: reachability, dispatch edges, idiom tags, and cross-runtime
//! gap classifications.

use std::collections::{HashMap, HashSet};

use heck::ToSnakeCase;

use crate::semantic::{
    Classifier, DispatchEdge, DispatchKind, IdiomRegistry, IdiomTag, ReachReason, RuntimeRole,
    RuntimeSemanticAnalyzer, SemanticFacts, SymbolId, TagSource,
};
use crate::types::{CommandRef, EventRef, FileAnalysis, Language};

pub use crate::snapshot::{CommandBridge, EventBridge};

pub struct TauriSemantics;

impl RuntimeSemanticAnalyzer for TauriSemantics {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn analyze(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) -> anyhow::Result<()> {
        let command_index = CommandIndex::from_files(files);
        self.classify_commands(files, registry, out, &command_index);
        self.classify_events(files, registry, out);
        Ok(())
    }
}

impl TauriSemantics {
    fn classify_commands(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
        index: &CommandIndex,
    ) {
        let handler_idiom = registry.lookup(Language::Rust, "#[tauri::command]");
        let invoke_idiom = registry
            .lookup(Language::Typescript, "invoke")
            .or_else(|| registry.lookup(Language::Javascript, "invoke"));
        let registered_idiom = registry.lookup(Language::Rust, "generate_handler!");

        let mut reached_handlers: HashSet<SymbolId> = HashSet::new();

        for file in files {
            for call in &file.command_calls {
                let call_key = command_key(call);
                let call_symbol = invoke_symbol_id(file, call);
                if let Some(entry) = invoke_idiom {
                    push_tag(
                        out,
                        call_symbol.clone(),
                        idiom_tag_from_entry(entry, TagSource::EmbeddedDefault),
                    );
                }

                let Some(handlers) = index.handlers_by_key.get(&call_key) else {
                    push_tag(
                        out,
                        call_symbol,
                        inferred_tag(
                            "frontend-orphan",
                            Classifier::Custom("frontend-orphan".into()),
                            RuntimeRole::Internal,
                            format!(
                                "Frontend invokes '{}' but no matching #[tauri::command] handler was found.",
                                display_command(call)
                            ),
                        ),
                    );
                    continue;
                };

                for handler in handlers {
                    out.dispatch_edges.push(DispatchEdge {
                        from_file: file.path.clone(),
                        from_line: call.line as u32,
                        dispatch_kind: DispatchKind::TauriInvoke,
                        handler_symbol: handler.command.name.clone(),
                        handler_file: Some(handler.file.clone()),
                    });

                    let symbol_id = handler_symbol_id(handler);
                    reached_handlers.insert(symbol_id.clone());
                    out.reachability.reached_symbols.insert(symbol_id.clone());
                    out.reachability.reasons.insert(
                        symbol_id,
                        ReachReason::DispatchHandler {
                            from_symbol: file.path.clone(),
                            dispatch_kind: DispatchKind::TauriInvoke,
                        },
                    );
                }
            }
        }

        for handler in &index.handlers {
            let symbol_id = handler_symbol_id(handler);
            if let Some(entry) = handler_idiom {
                push_tag(
                    out,
                    symbol_id.clone(),
                    idiom_tag_from_entry(entry, TagSource::EmbeddedDefault),
                );
            }
            if index.registered_handlers.contains(&handler.command.name)
                && let Some(entry) = registered_idiom
            {
                push_tag(
                    out,
                    symbol_id.clone(),
                    idiom_tag_from_entry(entry, TagSource::EmbeddedDefault),
                );
            }
            if !reached_handlers.contains(&symbol_id) {
                push_tag(
                    out,
                    symbol_id.clone(),
                    inferred_tag(
                        "dead-likely-tauri",
                        Classifier::Custom("dead-likely-tauri".into()),
                        RuntimeRole::Internal,
                        format!(
                            "Tauri handler '{}' has no frontend invoke() site in the current scan.",
                            handler.command.name
                        ),
                    ),
                );
                out.reachability.unreached_symbols.insert(symbol_id.clone());
                out.reachability
                    .reasons
                    .entry(symbol_id)
                    .or_insert(ReachReason::Unknown);
            }
        }
    }

    fn classify_events(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) {
        let emit_idiom = registry
            .lookup(Language::Typescript, "emit")
            .or_else(|| registry.lookup(Language::Javascript, "emit"));
        let listen_idiom = registry
            .lookup(Language::Typescript, "listen")
            .or_else(|| registry.lookup(Language::Javascript, "listen"));

        let mut emits: HashMap<String, Vec<EventSite>> = HashMap::new();
        let mut listens: HashMap<String, Vec<EventSite>> = HashMap::new();

        for file in files {
            for event in &file.event_emits {
                let symbol_id = event_symbol_id(file, "emit", event);
                if let Some(entry) = emit_idiom {
                    push_tag(
                        out,
                        symbol_id,
                        idiom_tag_from_entry(entry, TagSource::EmbeddedDefault),
                    );
                }
                emits
                    .entry(event.name.clone())
                    .or_default()
                    .push(EventSite {
                        file: file.path.clone(),
                        line: event.line,
                    });
            }

            for event in &file.event_listens {
                let symbol_id = event_symbol_id(file, "listen", event);
                if let Some(entry) = listen_idiom {
                    push_tag(
                        out,
                        symbol_id,
                        idiom_tag_from_entry(entry, TagSource::EmbeddedDefault),
                    );
                }
                listens
                    .entry(event.name.clone())
                    .or_default()
                    .push(EventSite {
                        file: file.path.clone(),
                        line: event.line,
                    });
            }
        }

        for (event_name, emit_sites) in emits {
            let Some(listen_sites) = listens.get(&event_name) else {
                continue;
            };
            for emit in &emit_sites {
                for listen in listen_sites {
                    out.dispatch_edges.push(DispatchEdge {
                        from_file: emit.file.clone(),
                        from_line: emit.line as u32,
                        dispatch_kind: DispatchKind::TauriEvent,
                        handler_symbol: format!("event:{event_name}"),
                        handler_file: Some(listen.file.clone()),
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CommandSite {
    file: String,
    command: CommandRef,
}

#[derive(Debug, Clone)]
struct CommandIndex {
    handlers: Vec<CommandSite>,
    handlers_by_key: HashMap<String, Vec<CommandSite>>,
    registered_handlers: HashSet<String>,
}

impl CommandIndex {
    fn from_files(files: &[FileAnalysis]) -> Self {
        let mut handlers = Vec::new();
        let mut handlers_by_key: HashMap<String, Vec<CommandSite>> = HashMap::new();
        let mut registered_handlers = HashSet::new();

        for file in files {
            registered_handlers.extend(file.tauri_registered_handlers.iter().cloned());
            for handler in &file.command_handlers {
                let site = CommandSite {
                    file: file.path.clone(),
                    command: handler.clone(),
                };
                handlers_by_key
                    .entry(command_key(handler))
                    .or_default()
                    .push(site.clone());
                if let Some(exposed) = handler.exposed_name.as_ref()
                    && exposed != &handler.name
                {
                    let mut exposed_ref = handler.clone();
                    exposed_ref.name = exposed.clone();
                    handlers_by_key
                        .entry(command_key(&exposed_ref))
                        .or_default()
                        .push(site.clone());
                }
                handlers.push(site);
            }
        }

        Self {
            handlers,
            handlers_by_key,
            registered_handlers,
        }
    }
}

#[derive(Debug, Clone)]
struct EventSite {
    file: String,
    line: usize,
}

fn command_key(command: &CommandRef) -> String {
    let plugin = command.plugin_name.as_deref().unwrap_or("");
    format!("{plugin}:{}", normalize_cmd_name(&command.name))
}

fn normalize_cmd_name(name: &str) -> String {
    let mut buffered = String::new();
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            buffered.push(ch);
        } else {
            buffered.push('_');
        }
    }
    buffered
        .to_snake_case()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn handler_symbol_id(handler: &CommandSite) -> SymbolId {
    format!("{}::{}", handler.file, handler.command.name)
}

fn invoke_symbol_id(file: &FileAnalysis, call: &CommandRef) -> SymbolId {
    format!("{}::invoke:{}", file.path, display_command(call))
}

fn event_symbol_id(file: &FileAnalysis, role: &str, event: &EventRef) -> SymbolId {
    format!("{}::{role}:{}", file.path, event.name)
}

fn display_command(command: &CommandRef) -> String {
    match command.plugin_name.as_ref() {
        Some(plugin) => format!("plugin:{plugin}|{}", command.name),
        None => command.name.clone(),
    }
}

fn idiom_tag_from_entry(entry: &crate::semantic::IdiomEntry, source: TagSource) -> IdiomTag {
    IdiomTag {
        name: entry.name.clone(),
        classifier: entry.classifier.clone(),
        runtime_role: entry.runtime_role.clone(),
        source,
        reasoning: entry.reasoning.clone(),
    }
}

fn inferred_tag(
    name: impl Into<String>,
    classifier: Classifier,
    runtime_role: RuntimeRole,
    reasoning: impl Into<String>,
) -> IdiomTag {
    IdiomTag {
        name: name.into(),
        classifier,
        runtime_role,
        source: TagSource::InferredFromCode,
        reasoning: reasoning.into(),
    }
}

fn push_tag(out: &mut SemanticFacts, symbol_id: SymbolId, tag: IdiomTag) {
    out.idiom_tags.entry(symbol_id).or_default().push(tag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{IdiomRegistry, RuntimeSemanticAnalyzer};
    use crate::types::CommandRef;

    fn command(name: &str, line: usize) -> CommandRef {
        CommandRef {
            name: name.into(),
            exposed_name: None,
            line,
            generic_type: None,
            payload: None,
            plugin_name: None,
        }
    }

    fn file(path: &str, language: &str) -> FileAnalysis {
        FileAnalysis {
            path: path.into(),
            language: language.into(),
            ..FileAnalysis::default()
        }
    }

    #[test]
    fn registered_command_reached_by_frontend_invoke() {
        let mut rust = file("src-tauri/src/main.rs", "rust");
        rust.command_handlers.push(command("greet_user", 3));
        rust.tauri_registered_handlers.push("greet_user".into());

        let mut ts = file("src/App.tsx", "typescript");
        ts.command_calls.push(command("greetUser", 7));

        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();
        TauriSemantics
            .analyze(&[rust, ts], &registry, &mut facts)
            .expect("semantic analysis");

        assert!(
            facts
                .reachability
                .reached_symbols
                .contains("src-tauri/src/main.rs::greet_user")
        );
        assert!(facts.dispatch_edges.iter().any(|edge| {
            edge.dispatch_kind == DispatchKind::TauriInvoke
                && edge.from_file == "src/App.tsx"
                && edge.handler_symbol == "greet_user"
        }));
    }

    #[test]
    fn orphan_frontend_invoke_is_classified() {
        let mut ts = file("src/App.tsx", "typescript");
        ts.command_calls.push(command("missing_handler", 12));

        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();
        TauriSemantics
            .analyze(&[ts], &registry, &mut facts)
            .expect("semantic analysis");

        let tags = facts
            .idiom_tags
            .get("src/App.tsx::invoke:missing_handler")
            .expect("orphan tag");
        assert!(tags.iter().any(|tag| tag.name == "frontend-orphan"));
    }

    #[test]
    fn ghost_backend_command_is_dead_likely() {
        let mut rust = file("src-tauri/src/main.rs", "rust");
        rust.command_handlers.push(command("unused_handler", 8));

        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();
        TauriSemantics
            .analyze(&[rust], &registry, &mut facts)
            .expect("semantic analysis");

        let symbol = "src-tauri/src/main.rs::unused_handler";
        assert!(facts.reachability.unreached_symbols.contains(symbol));
        assert!(
            facts
                .idiom_tags
                .get(symbol)
                .expect("handler tags")
                .iter()
                .any(|tag| tag.name == "dead-likely-tauri")
        );
    }

    #[test]
    fn event_emit_listen_pair_becomes_dispatch_edge() {
        let mut emitter = file("src/events.ts", "typescript");
        emitter.event_emits.push(EventRef {
            raw_name: Some("patient-updated".into()),
            name: "patient-updated".into(),
            line: 4,
            kind: "emit_literal".into(),
            awaited: false,
            payload: None,
            is_dynamic: false,
        });

        let mut listener = file("src/listener.ts", "typescript");
        listener.event_listens.push(EventRef {
            raw_name: Some("patient-updated".into()),
            name: "patient-updated".into(),
            line: 9,
            kind: "listen_literal".into(),
            awaited: false,
            payload: None,
            is_dynamic: false,
        });

        let registry = IdiomRegistry::load_defaults().expect("defaults");
        let mut facts = SemanticFacts::default();
        TauriSemantics
            .analyze(&[emitter, listener], &registry, &mut facts)
            .expect("semantic analysis");

        assert!(facts.dispatch_edges.iter().any(|edge| {
            edge.dispatch_kind == DispatchKind::TauriEvent
                && edge.from_file == "src/events.ts"
                && edge.handler_file.as_deref() == Some("src/listener.ts")
                && edge.handler_symbol == "event:patient-updated"
        }));
    }
}
