//! Configuration file support for loctree.
//!
//! Loads optional `.loctree/config.toml` from project root.

use serde::Deserialize;
use std::path::Path;

/// Root configuration structure
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct LoctreeConfig {
    pub tauri: TauriConfig,
    /// Enable library mode to filter example/demo/fixture files
    #[serde(default)]
    pub library_mode: bool,
    /// Additional glob patterns for library example files
    #[serde(default)]
    pub library_example_globs: Vec<String>,
    /// Plan 19 Stage 1 — analyzer parser knob.
    ///
    /// `oxc` (default) keeps the existing OXC-backed JS/TS cold-scan path.
    /// `ts` opts in to the tree-sitter extractor surface from `loctree-ast`.
    /// Other values are tolerated for forward compatibility but treated as
    /// `oxc` until a corresponding extractor lands.
    #[serde(default)]
    pub analyzer: AnalyzerConfig,
    /// Event emit/listen wrapper function names (W3-b).
    ///
    /// Functions whose first string-literal argument is an event name, e.g.
    /// `event_wrappers = ["emit_compat", "emitCompat"]`. Calls like
    /// `emit_compat("visit_deleted", payload)` count as emitters of the
    /// literal event. Wrappers whose name contains `listen` (or starts with
    /// `on` + uppercase) are treated as listeners instead of emitters.
    /// Complements same-file wrapper autodetection on the Rust side.
    #[serde(default)]
    pub event_wrappers: Vec<String>,
}

/// Cold-scan parser strategy. `oxc` is the default and tracks current
/// behavior; `ts` switches JS/TS dispatch over to the Plan 19 Stage 1
/// tree-sitter extractor pipeline. Per-language opt-in is intentionally
/// deferred to Stage 2 — Stage 1 is workspace-wide on/off.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct AnalyzerConfig {
    /// Parser knob name. Any unknown value falls back to `oxc`.
    #[serde(default = "default_parser_strategy")]
    pub parser: String,
}

fn default_parser_strategy() -> String {
    "oxc".to_string()
}

/// Tauri-specific configuration
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct TauriConfig {
    /// Additional attribute macros that generate Tauri commands.
    /// Example: `["api_cmd_tauri", "gitbutler_command"]`
    #[serde(default)]
    pub command_macros: Vec<String>,
    /// Extra DOM API names to exclude from Tauri command detection.
    #[serde(default)]
    pub dom_exclusions: Vec<String>,
    /// Extra function names to exclude from Tauri invoke detection.
    #[serde(default)]
    pub non_invoke_exclusions: Vec<String>,
    /// Extra invalid command names (CLI/test helpers) to ignore.
    #[serde(default)]
    pub invalid_command_names: Vec<String>,
}

impl LoctreeConfig {
    /// Load config from `.loctree/config.toml` in the given root directory.
    /// Returns default config if file doesn't exist or is invalid.
    pub fn load(root: &Path) -> Self {
        let config_path = crate::snapshot::project_config_dir(root).join("config.toml");
        Self::load_from_path(&config_path)
    }

    /// Load config from a specific path.
    pub fn load_from_path(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("[loctree][warn] Failed to parse {}: {}", path.display(), e);
                    Self::default()
                }
            },
            Err(e) => {
                eprintln!("[loctree][warn] Failed to read {}: {}", path.display(), e);
                Self::default()
            }
        }
    }

    /// Check if any custom Tauri command macros are configured.
    pub fn has_custom_command_macros(&self) -> bool {
        !self.tauri.command_macros.is_empty()
    }

    /// Plan 19 Stage 1 — resolve the cold-scan parser strategy.
    ///
    /// Order of precedence:
    /// 1. `LOCTREE_PARSER` env var (operator override / smoke testing).
    /// 2. `analyzer.parser` field in `.loctree/config.toml`.
    /// 3. Default `oxc`.
    ///
    /// Unknown values silently fall back to `oxc` so config typos never
    /// degrade analyzer behavior.
    pub fn parser_strategy(&self) -> &'static str {
        if let Ok(env) = std::env::var("LOCTREE_PARSER") {
            return normalize_parser_strategy(&env);
        }
        normalize_parser_strategy(&self.analyzer.parser)
    }
}

fn normalize_parser_strategy(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "ts" | "tree-sitter" | "treesitter" => "ts",
        "oxc" | "" => "oxc",
        _ => "oxc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = LoctreeConfig::default();
        assert!(config.tauri.command_macros.is_empty());
        assert!(config.tauri.dom_exclusions.is_empty());
        assert!(config.tauri.non_invoke_exclusions.is_empty());
        assert!(config.tauri.invalid_command_names.is_empty());
        assert!(!config.has_custom_command_macros());
    }

    #[test]
    fn test_load_missing_file() {
        let temp = TempDir::new().expect("temp dir");
        let config = LoctreeConfig::load(temp.path());
        assert!(config.tauri.command_macros.is_empty());
    }

    #[test]
    fn test_load_valid_config() {
        let temp = TempDir::new().expect("temp dir");
        let loctree_dir = temp.path().join(".loctree");
        std::fs::create_dir_all(&loctree_dir).expect("create .loctree");

        let config_path = loctree_dir.join("config.toml");
        let mut file = std::fs::File::create(&config_path).expect("create config");
        writeln!(
            file,
            r#"
event_wrappers = ["emit_compat"]

[tauri]
command_macros = ["api_cmd_tauri", "custom_command"]
dom_exclusions = ["fetch"]
non_invoke_exclusions = ["wrapCommand"]
invalid_command_names = ["npm"]
"#
        )
        .expect("write config");

        let config = LoctreeConfig::load(temp.path());
        assert_eq!(config.tauri.command_macros.len(), 2);
        assert!(
            config
                .tauri
                .command_macros
                .contains(&"api_cmd_tauri".to_string())
        );
        assert!(
            config
                .tauri
                .command_macros
                .contains(&"custom_command".to_string())
        );
        assert!(config.tauri.dom_exclusions.contains(&"fetch".to_string()));
        assert!(
            config
                .tauri
                .non_invoke_exclusions
                .contains(&"wrapCommand".to_string())
        );
        assert!(
            config
                .tauri
                .invalid_command_names
                .contains(&"npm".to_string())
        );
        assert!(config.event_wrappers.contains(&"emit_compat".to_string()));
        assert!(config.has_custom_command_macros());
    }

    #[test]
    fn parser_strategy_defaults_to_oxc() {
        // Avoid env-var leakage when other tests set LOCTREE_PARSER.
        // SAFETY: integration tests that toggle this env serialize via
        // `serial_test`; this unit test runs single-threaded by default.
        unsafe {
            std::env::remove_var("LOCTREE_PARSER");
        }
        let config = LoctreeConfig::default();
        assert_eq!(config.parser_strategy(), "oxc");
    }

    #[test]
    fn parser_strategy_respects_config_field() {
        unsafe {
            std::env::remove_var("LOCTREE_PARSER");
        }
        let temp = TempDir::new().expect("temp dir");
        let loctree_dir = temp.path().join(".loctree");
        std::fs::create_dir_all(&loctree_dir).expect("create .loctree");
        let config_path = loctree_dir.join("config.toml");
        let mut file = std::fs::File::create(&config_path).expect("create config");
        writeln!(
            file,
            r#"
[analyzer]
parser = "ts"
"#
        )
        .expect("write config");
        let config = LoctreeConfig::load(temp.path());
        assert_eq!(config.parser_strategy(), "ts");
    }

    #[test]
    fn parser_strategy_falls_back_on_unknown_value() {
        unsafe {
            std::env::remove_var("LOCTREE_PARSER");
        }
        let mut config = LoctreeConfig::default();
        config.analyzer.parser = "swc".to_string();
        assert_eq!(config.parser_strategy(), "oxc");
    }

    #[test]
    fn test_load_empty_config() {
        let temp = TempDir::new().expect("temp dir");
        let loctree_dir = temp.path().join(".loctree");
        std::fs::create_dir_all(&loctree_dir).expect("create .loctree");

        let config_path = loctree_dir.join("config.toml");
        std::fs::File::create(&config_path).expect("create empty config");

        let config = LoctreeConfig::load(temp.path());
        assert!(config.tauri.command_macros.is_empty());
    }
}
