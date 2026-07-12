//! Loctree-specific bearer token scope grain.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// Permission scope for a loctree MCP bearer token.
///
/// Current MCP tools are read-only and should use [`Scope::ContextRead`].
/// `ToolExecute` and `CliFull` are reserved for future write-side tools and
/// CLI-parity surfaces; `Admin` implies every scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Scope {
    ContextRead,
    ToolExecute,
    CliFull,
    Admin,
}

impl Scope {
    /// Returns true when `self` grants `required`.
    pub(crate) fn grants(&self, required: &Scope) -> bool {
        matches!(self, Scope::Admin) || self == required
    }

    /// Canonical token used in persisted JSON and error messages.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Scope::ContextRead => "context-read",
            Scope::ToolExecute => "tool-execute",
            Scope::CliFull => "cli-full",
            Scope::Admin => "admin",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scope {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "context-read" | "context_read" | "contextread" | "read" => Ok(Scope::ContextRead),
            "tool-execute" | "tool_execute" | "toolexecute" | "write" => Ok(Scope::ToolExecute),
            "cli-full" | "cli_full" | "clifull" => Ok(Scope::CliFull),
            "admin" => Ok(Scope::Admin),
            other => Err(anyhow!(
                "Unknown scope '{}'. Use: context-read, tool-execute, cli-full, admin",
                other
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_display_and_parse() {
        assert_eq!(Scope::ContextRead.to_string(), "context-read");
        assert_eq!(Scope::ToolExecute.to_string(), "tool-execute");
        assert_eq!(Scope::CliFull.to_string(), "cli-full");
        assert_eq!(Scope::Admin.to_string(), "admin");

        assert_eq!("context-read".parse::<Scope>().unwrap(), Scope::ContextRead);
        assert_eq!("READ".parse::<Scope>().unwrap(), Scope::ContextRead);
        assert_eq!("write".parse::<Scope>().unwrap(), Scope::ToolExecute);
        assert_eq!("cli_full".parse::<Scope>().unwrap(), Scope::CliFull);
        assert_eq!("Admin".parse::<Scope>().unwrap(), Scope::Admin);
        assert!("invalid".parse::<Scope>().is_err());
    }

    #[test]
    fn admin_grants_every_scope() {
        assert!(Scope::Admin.grants(&Scope::ContextRead));
        assert!(Scope::Admin.grants(&Scope::ToolExecute));
        assert!(Scope::Admin.grants(&Scope::CliFull));
        assert!(Scope::Admin.grants(&Scope::Admin));
        assert!(!Scope::ContextRead.grants(&Scope::ToolExecute));
    }
}
