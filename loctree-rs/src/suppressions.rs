//! Suppression system for false positives in loctree analysis.
//!
//! Allows users to mark findings as "reviewed and OK" so they don't
//! appear in subsequent runs.
//!
//! Suppressions are stored in `.loctree/suppressions.toml`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Type of finding that can be suppressed
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuppressionType {
    /// Exact twin (same symbol exported from multiple files)
    Twins,
    /// Dead parrot (export with 0 imports)
    DeadParrot,
    /// Dead export (unused export)
    DeadExport,
    /// Circular import
    Circular,
}

impl std::fmt::Display for SuppressionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SuppressionType::Twins => write!(f, "twins"),
            SuppressionType::DeadParrot => write!(f, "dead_parrot"),
            SuppressionType::DeadExport => write!(f, "dead_export"),
            SuppressionType::Circular => write!(f, "circular"),
        }
    }
}

impl std::str::FromStr for SuppressionType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "twins" | "twin" => Ok(SuppressionType::Twins),
            "dead_parrot" | "dead-parrot" | "parrot" => Ok(SuppressionType::DeadParrot),
            "dead_export" | "dead-export" | "dead" => Ok(SuppressionType::DeadExport),
            "circular" | "cycle" => Ok(SuppressionType::Circular),
            _ => Err(format!("Unknown suppression type: {}", s)),
        }
    }
}

/// A single suppression entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suppression {
    /// Type of finding
    #[serde(rename = "type")]
    pub suppression_type: SuppressionType,
    /// Symbol name to suppress
    pub symbol: String,
    /// Optional: specific file path (if not set, suppresses all locations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Reason for suppression (for documentation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When this suppression was added
    pub added: String,
}

/// Collection of all suppressions
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Suppressions {
    #[serde(default, rename = "suppress")]
    pub items: Vec<Suppression>,
}

impl Suppressions {
    /// Load suppressions from `.loctree/suppressions.toml`
    pub fn load(root: &Path) -> Self {
        let path = crate::snapshot::project_config_dir(root).join("suppressions.toml");
        Self::load_from_path(&path)
    }

    /// Load from a specific path
    pub fn load_from_path(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(suppressions) => suppressions,
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

    /// Save suppressions to `.loctree/suppressions.toml`
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let dir = crate::snapshot::project_config_dir(root);
        std::fs::create_dir_all(&dir)?;

        let path = dir.join("suppressions.toml");
        let content = toml::to_string_pretty(self).map_err(std::io::Error::other)?;

        // Add header comment
        let with_header = format!(
            "# Loctree suppressions - findings marked as reviewed/OK\n\
             # Edit manually or use: loct suppress <type> <symbol>\n\
             # Clear all: loct suppress --clear\n\n{}",
            content
        );

        std::fs::write(&path, with_header)?;
        eprintln!("[loctree] Saved suppressions to {}", path.display());
        Ok(())
    }

    /// Add a new suppression
    pub fn add(
        &mut self,
        suppression_type: SuppressionType,
        symbol: String,
        file: Option<String>,
        reason: Option<String>,
    ) {
        // Check if already exists
        if self.is_suppressed(&suppression_type, &symbol, file.as_deref()) {
            return;
        }

        self.items.push(Suppression {
            suppression_type,
            symbol,
            file,
            reason,
            added: Utc::now().format("%Y-%m-%d").to_string(),
        });
    }

    /// Remove a suppression
    pub fn remove(&mut self, suppression_type: &SuppressionType, symbol: &str) -> bool {
        let before = self.items.len();
        self.items
            .retain(|s| !(&s.suppression_type == suppression_type && s.symbol == symbol));
        self.items.len() < before
    }

    /// Clear all suppressions
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Check if a finding is suppressed
    pub fn is_suppressed(
        &self,
        suppression_type: &SuppressionType,
        symbol: &str,
        file: Option<&str>,
    ) -> bool {
        self.items.iter().any(|s| {
            if &s.suppression_type != suppression_type || s.symbol != symbol {
                return false;
            }
            // If suppression has no file, it matches all files
            // If suppression has a file, it must match
            match (&s.file, file) {
                (None, _) => true,              // Suppress all locations
                (Some(sf), Some(f)) => sf == f, // Must match specific file
                (Some(_), None) => false,       // Suppression is file-specific but no file given
            }
        })
    }

    /// Get all suppressed symbols for a type
    pub fn suppressed_symbols(&self, suppression_type: &SuppressionType) -> HashSet<String> {
        self.items
            .iter()
            .filter(|s| &s.suppression_type == suppression_type)
            .map(|s| s.symbol.clone())
            .collect()
    }

    /// Count suppressions by type
    pub fn count_by_type(&self, suppression_type: &SuppressionType) -> usize {
        self.items
            .iter()
            .filter(|s| &s.suppression_type == suppression_type)
            .count()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total count
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_suppression_type_parsing() {
        assert_eq!(
            "twins".parse::<SuppressionType>().unwrap(),
            SuppressionType::Twins
        );
        assert_eq!(
            "dead_parrot".parse::<SuppressionType>().unwrap(),
            SuppressionType::DeadParrot
        );
        assert_eq!(
            "dead-export".parse::<SuppressionType>().unwrap(),
            SuppressionType::DeadExport
        );
    }

    #[test]
    fn test_add_and_check_suppression() {
        let mut suppressions = Suppressions::default();

        suppressions.add(
            SuppressionType::Twins,
            "Message".to_string(),
            None,
            Some("FE/BE mirror".to_string()),
        );

        assert!(suppressions.is_suppressed(&SuppressionType::Twins, "Message", None));
        assert!(suppressions.is_suppressed(
            &SuppressionType::Twins,
            "Message",
            Some("src/types.ts")
        ));
        assert!(!suppressions.is_suppressed(&SuppressionType::Twins, "Other", None));
        assert!(!suppressions.is_suppressed(&SuppressionType::DeadParrot, "Message", None));
    }

    #[test]
    fn test_file_specific_suppression() {
        let mut suppressions = Suppressions::default();

        suppressions.add(
            SuppressionType::DeadParrot,
            "unusedFunc".to_string(),
            Some("src/utils.ts".to_string()),
            None,
        );

        // Should match specific file
        assert!(suppressions.is_suppressed(
            &SuppressionType::DeadParrot,
            "unusedFunc",
            Some("src/utils.ts")
        ));
        // Should NOT match other file
        assert!(!suppressions.is_suppressed(
            &SuppressionType::DeadParrot,
            "unusedFunc",
            Some("src/other.ts")
        ));
    }

    #[test]
    fn test_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let mut suppressions = Suppressions::default();

        suppressions.add(
            SuppressionType::Twins,
            "Message".to_string(),
            None,
            Some("OK".to_string()),
        );

        suppressions.save(tmp.path()).unwrap();

        let loaded = Suppressions::load(tmp.path());
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].symbol, "Message");
    }

    #[test]
    fn test_remove_suppression() {
        let mut suppressions = Suppressions::default();

        suppressions.add(SuppressionType::Twins, "A".to_string(), None, None);
        suppressions.add(SuppressionType::Twins, "B".to_string(), None, None);

        assert!(suppressions.remove(&SuppressionType::Twins, "A"));
        assert_eq!(suppressions.items.len(), 1);
        assert_eq!(suppressions.items[0].symbol, "B");
    }
}
