//! ParsedCommand - result of command-line argument parsing.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use super::global::GlobalOptions;
use super::types::Command;

/// Result of parsing command-line arguments.
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// The parsed command
    pub command: Command,

    /// Global options
    pub global: GlobalOptions,

    /// Whether this was parsed from legacy flags (triggers deprecation warning)
    pub from_legacy: bool,

    /// If from legacy, the original invocation for the warning message
    pub legacy_invocation: Option<String>,

    /// If from legacy, the suggested new invocation
    pub suggested_invocation: Option<String>,
}

impl ParsedCommand {
    /// Create a new ParsedCommand for a modern invocation.
    pub fn new(command: Command, global: GlobalOptions) -> Self {
        Self {
            command,
            global,
            from_legacy: false,
            legacy_invocation: None,
            suggested_invocation: None,
        }
    }

    /// Create a new ParsedCommand for a legacy invocation.
    pub fn from_legacy(
        command: Command,
        global: GlobalOptions,
        legacy_invocation: String,
        suggested_invocation: String,
    ) -> Self {
        Self {
            command,
            global,
            from_legacy: true,
            legacy_invocation: Some(legacy_invocation),
            suggested_invocation: Some(suggested_invocation),
        }
    }

    /// Emit deprecation warning to stderr if this is a legacy invocation.
    ///
    /// Respects the `--quiet` flag by not emitting if quiet is set.
    pub fn emit_deprecation_warning(&self) {
        if self.from_legacy
            && !self.global.quiet
            && let (Some(old), Some(new)) = (&self.legacy_invocation, &self.suggested_invocation)
        {
            eprintln!(
                "[loct][deprecated] '{}' -> '{}'. This alias will be removed in v1.0.",
                old, new
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::command::options::DeadOptions;

    #[test]
    fn test_parsed_command_deprecation_warning() {
        let cmd = ParsedCommand::from_legacy(
            Command::Dead(DeadOptions::default()),
            GlobalOptions::default(),
            "loct -A --dead".to_string(),
            "loct dead".to_string(),
        );
        assert!(cmd.from_legacy);
        assert_eq!(cmd.legacy_invocation, Some("loct -A --dead".to_string()));
    }
}
