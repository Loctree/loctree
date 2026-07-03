use crate::semantic::{Classifier, RuntimeRole};
use crate::types::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const SHELL_DEFAULTS: &str = include_str!("shell.toml");
const MAKE_DEFAULTS: &str = include_str!("make.toml");
const PYTHON_DEFAULTS: &str = include_str!("python.toml");
const TAURI_DEFAULTS: &str = include_str!("tauri.toml");
const RUST_DEFAULTS: &str = include_str!("rust.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdiomEntry {
    pub name: String,
    pub classifier: Classifier,
    pub runtime_role: RuntimeRole,
    pub reasoning: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub language: Language,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdiomRegistry {
    by_language: HashMap<Language, Vec<IdiomEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdiomFile {
    #[serde(default)]
    idiom: Vec<IdiomEntry>,
}

impl IdiomRegistry {
    pub fn load_defaults() -> anyhow::Result<Self> {
        let mut registry = Self::default();
        registry.merge_toml_str(SHELL_DEFAULTS)?;
        registry.merge_toml_str(MAKE_DEFAULTS)?;
        registry.merge_toml_str(PYTHON_DEFAULTS)?;
        registry.merge_toml_str(TAURI_DEFAULTS)?;
        registry.merge_toml_str(RUST_DEFAULTS)?;
        Ok(registry)
    }

    /// Load bundled defaults, then merge `<workspace>/.loctree/idioms/*.toml`.
    ///
    /// Missing override directories are a no-op. Override entries replace
    /// defaults by `(language, name)` so teams can tune idiom meaning without
    /// depending on the source tree at runtime.
    pub fn load_with_overrides(workspace_root: &Path) -> anyhow::Result<Self> {
        let mut registry = Self::load_defaults()?;
        let override_dir = workspace_root.join(".loctree").join("idioms");
        if !override_dir.exists() {
            return Ok(registry);
        }

        let override_paths = crate::semantic::io::list_idiom_override_files(&override_dir)?;

        for path in override_paths {
            let content =
                crate::semantic::io::read_validated_semantic_input(&path.to_string_lossy())?;
            registry.merge_toml_str(&content)?;
        }

        Ok(registry)
    }

    pub fn lookup(&self, language: Language, symbol: &str) -> Option<&IdiomEntry> {
        self.by_language
            .get(&language)?
            .iter()
            .find(|entry| entry.name == symbol || entry.aliases.iter().any(|alias| alias == symbol))
    }

    fn merge_toml_str(&mut self, content: &str) -> anyhow::Result<()> {
        let parsed: IdiomFile = toml::from_str(content)?;
        for entry in parsed.idiom {
            let bucket = self.by_language.entry(entry.language.clone()).or_default();
            if let Some(pos) = bucket
                .iter()
                .position(|existing| existing.name == entry.name)
            {
                bucket[pos] = entry;
            } else {
                bucket.push(entry);
            }
        }
        Ok(())
    }
}
