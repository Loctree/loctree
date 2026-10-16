//! Static, symbol-scoped registration for Hatch's custom build-hook loader.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::snapshot::Snapshot;

const INTERFACE: &str = "hatchling.builders.hooks.plugin.interface.BuildHookInterface";

/// Shared by every dead-truth consumer, not a filename exemption. Registration
/// may be conditional or ambiguous; it prevents a deletion verdict, not proves
/// that a build succeeded or that every export in the module is used.
pub(super) fn runtime_entrypoint_symbols(snapshot: &Snapshot) -> HashSet<(String, String)> {
    let mut symbols = HashSet::new();
    for root in &snapshot.metadata.roots {
        let Ok(root) = Path::new(root).canonicalize() else {
            continue;
        };
        let mut indexed = HashMap::new();
        let mut projects = HashSet::new();
        for file in &snapshot.files {
            if !file.path.ends_with(".py") {
                continue;
            }
            let Some(path) = contained_file(&root, Path::new(&file.path)) else {
                continue;
            };
            indexed.insert(path.clone(), file.path.replace('\\', "/"));
            for parent in path.ancestors().skip(1) {
                if !parent.starts_with(&root) {
                    break;
                }
                projects.insert(parent.to_path_buf());
            }
        }
        let parsers = loctree_ast::Parsers::new_default();
        for project in projects {
            let Some(build) = build_config(&project) else {
                continue;
            };
            let mut paths = HashSet::new();
            collect_hook_path(&build, &mut paths);
            if let Some(targets) = build.get("targets").and_then(toml::Value::as_table) {
                for target in targets.values() {
                    collect_hook_path(target, &mut paths);
                }
            }
            for path in paths {
                let Some(path) = contained_file(&project, Path::new(&path)) else {
                    continue;
                };
                let Some(relative) = indexed.get(&path) else {
                    continue;
                };
                let Ok(source) = std::fs::read(&path) else {
                    continue;
                };
                for name in hook_symbols(&parsers, &source) {
                    symbols.insert((relative.clone(), name));
                }
            }
        }
    }
    symbols
}

fn contained_file(root: &Path, relative: &Path) -> Option<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    let path = root.join(relative).canonicalize().ok()?;
    if !path.is_file() || !path.starts_with(root) {
        return None;
    }
    // Neither a nested repository nor a symlink into one inherits the parent's
    // manifest identity. The root's own .git marker is expected and allowed.
    if path
        .ancestors()
        .skip(1)
        .take_while(|p| *p != root)
        .any(|p| p.join(".git").exists())
    {
        return None;
    }
    Some(path)
}

fn read_toml(root: &Path, name: &str) -> Option<toml::Value> {
    let path = contained_file(root, Path::new(name))?;
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn build_config(root: &Path) -> Option<toml::Value> {
    let pyproject = read_toml(root, "pyproject.toml");
    let hatch = read_toml(root, "hatch.toml");
    // HatchMetadata uses a shallow update: hatch.toml's build replaces the
    // entire tool.hatch.build value, but an unrelated key does not replace it.
    if let Some(build) = hatch.as_ref().and_then(|config| config.get("build")) {
        return Some(build.clone());
    }
    pyproject?.get("tool")?.get("hatch")?.get("build").cloned()
}

fn collect_hook_path(config: &toml::Value, paths: &mut HashSet<String>) {
    let Some(custom) = config
        .get("hooks")
        .and_then(|hooks| hooks.get("custom"))
        .and_then(toml::Value::as_table)
    else {
        return;
    };
    let path = match custom.get("path") {
        None => "hatch_build.py",
        Some(toml::Value::String(path)) if !path.is_empty() => path,
        _ => return,
    };
    paths.insert(path.to_string());
}

fn text<'a>(node: tree_sitter::Node<'_>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn module_declarations<'a>(node: tree_sitter::Node<'a>, out: &mut Vec<tree_sitter::Node<'a>>) {
    match node.kind() {
        "class_definition"
        | "function_definition"
        | "import_statement"
        | "import_from_statement" => out.push(node),
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                module_declarations(child, out);
            }
        }
    }
}

fn hook_symbols(parsers: &loctree_ast::Parsers, source: &[u8]) -> HashSet<String> {
    let Ok(parsed) = parsers.parse_language("python", source) else {
        return HashSet::new();
    };
    let mut declarations = Vec::new();
    module_declarations(parsed.tree.root_node(), &mut declarations);
    let mut imports = HashMap::new();
    let mut classes = Vec::new();
    let mut hooks = HashSet::new();
    for node in declarations {
        match node.kind() {
            "import_statement" | "import_from_statement" => {
                let module = node.child_by_field_name("module_name");
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if Some(child) == module || child.kind() == "relative_import" {
                        continue;
                    }
                    let (name, alias) = if child.kind() == "aliased_import" {
                        let Some(name) = child.child_by_field_name("name") else {
                            continue;
                        };
                        (
                            text(name, source),
                            child.child_by_field_name("alias").map(|n| text(n, source)),
                        )
                    } else {
                        (text(child, source), None)
                    };
                    if let Some(module) = module {
                        imports.insert(
                            alias.unwrap_or(name).to_string(),
                            format!("{}.{name}", text(module, source)),
                        );
                    } else if let Some(alias) = alias {
                        imports.insert(alias.to_string(), name.to_string());
                    } else {
                        let first = name.split('.').next().unwrap_or(name);
                        imports.insert(first.to_string(), first.to_string());
                    }
                }
            }
            "function_definition" => {
                if node
                    .child_by_field_name("name")
                    .is_some_and(|name| text(name, source) == "get_build_hook")
                {
                    hooks.insert("get_build_hook".to_string());
                }
            }
            "class_definition" => {
                let Some(name) = node.child_by_field_name("name") else {
                    continue;
                };
                let mut bases = Vec::new();
                if let Some(superclasses) = node.child_by_field_name("superclasses") {
                    let mut cursor = superclasses.walk();
                    for base in superclasses.named_children(&mut cursor) {
                        if matches!(base.kind(), "identifier" | "attribute") {
                            bases.push(text(base, source).to_string());
                        }
                    }
                }
                classes.push((text(name, source).to_string(), bases));
            }
            _ => {}
        }
    }
    let mut subclasses = HashSet::new();
    loop {
        let before = subclasses.len();
        for (name, bases) in &classes {
            if bases.iter().any(|base| {
                if subclasses.contains(base) {
                    return true;
                }
                let (first, suffix) = base.split_once('.').unwrap_or((base, ""));
                imports.get(first).is_some_and(|import| {
                    let qualified = if suffix.is_empty() {
                        import.clone()
                    } else {
                        format!("{import}.{suffix}")
                    };
                    qualified == INTERFACE
                })
            }) {
                subclasses.insert(name.clone());
            }
        }
        if before == subclasses.len() {
            break;
        }
    }
    hooks.extend(subclasses);
    hooks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_snapshot_registers_only_hook_symbols() {
        let root = tempfile::tempdir().expect("temporary project");
        let source = b"from hatchling.builders.hooks.plugin.interface import BuildHookInterface\nclass Hook(BuildHookInterface): pass\nclass Unrelated: pass\n";
        std::fs::write(
            root.path().join("pyproject.toml"),
            "[tool.hatch.build.hooks.custom]\n",
        )
        .expect("write manifest");
        std::fs::write(root.path().join("hatch_build.py"), source).expect("write hook");
        let canonical = root.path().canonicalize().expect("canonical project root");
        assert!(contained_file(&canonical, Path::new("hatch_build.py")).is_some());
        assert!(build_config(&canonical).is_some());
        let mut paths = HashSet::new();
        collect_hook_path(&build_config(&canonical).expect("build table"), &mut paths);
        assert_eq!(paths, HashSet::from(["hatch_build.py".into()]));
        let mut snapshot = Snapshot::new(vec![root.path().display().to_string()]);
        snapshot
            .files
            .push(crate::types::FileAnalysis::new("hatch_build.py".into()));
        assert_eq!(
            runtime_entrypoint_symbols(&snapshot),
            HashSet::from([("hatch_build.py".into(), "Hook".into())])
        );
    }

    #[test]
    fn indexed_foreign_repository_still_cannot_inherit_parent_registration() {
        let root = tempfile::tempdir().expect("temporary project");
        std::fs::create_dir_all(root.path().join("nested/.git")).expect("nested repository");
        std::fs::write(
            root.path().join("pyproject.toml"),
            "[tool.hatch.build.hooks.custom]\npath = 'nested/hatch_build.py'\n",
        )
        .expect("write parent manifest");
        std::fs::write(root.path().join("nested/hatch_build.py"), "from hatchling.builders.hooks.plugin.interface import BuildHookInterface\nclass Hook(BuildHookInterface): pass\n").expect("write foreign hook");
        let mut snapshot = Snapshot::new(vec![root.path().display().to_string()]);
        snapshot.files.push(crate::types::FileAnalysis::new(
            "nested/hatch_build.py".into(),
        ));
        assert!(runtime_entrypoint_symbols(&snapshot).is_empty());
    }

    #[test]
    fn static_subclass_discovery_resolves_aliases_and_inheritance_not_strings() {
        let parsers = loctree_ast::Parsers::new_default();
        let source = br#"
from hatchling.builders.hooks.plugin.interface import BuildHookInterface as Hook
import hatchling.builders.hooks.plugin.interface as api
class First(Hook): pass
class Second(api.BuildHookInterface): pass
class Third(First): pass
class Unrelated: pass
example = 'class Fake(Hook): pass'
def unrelated():
    class Nested(Hook): pass
"#;
        assert_eq!(
            hook_symbols(&parsers, source),
            HashSet::from(["First".into(), "Second".into(), "Third".into()])
        );
        assert!(
            hook_symbols(
                &parsers,
                b"class BuildHookInterface: pass\nclass Fake(BuildHookInterface): pass\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn factory_is_a_module_level_definition_not_a_mention() {
        let parsers = loctree_ast::Parsers::new_default();
        assert_eq!(
            hook_symbols(
                &parsers,
                b"def get_build_hook():\n    return make_dynamic_hook()\n"
            ),
            HashSet::from(["get_build_hook".into()])
        );
        assert!(hook_symbols(&parsers, b"# def get_build_hook():\nexample = 'def get_build_hook():'\nclass Unrelated:\n    def get_build_hook(self): pass\n").is_empty());
    }
}
