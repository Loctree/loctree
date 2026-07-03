//! Command detection configuration and exclusion lists.
//!
//! This module contains the configuration structures for Tauri command detection,
//! including lists of DOM APIs, non-invoke functions, and invalid command names
//! that should be excluded from analysis.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::HashSet;

/// Configuration for command detection in JS/TS analysis.
///
/// Contains exclusion lists to filter out false positives when detecting
/// Tauri invoke calls.
#[derive(Clone, Debug)]
pub struct CommandDetectionConfig {
    pub dom_exclusions: HashSet<String>,
    pub non_invoke_exclusions: HashSet<String>,
    pub invalid_command_names: HashSet<String>,
    /// Event emit/listen wrapper function names from `.loctree/config.toml`
    /// (`event_wrappers = ["emit_compat", ...]`). Calls to these functions
    /// with a string-literal first argument count as event emit/listen sites.
    pub event_wrappers: HashSet<String>,
}

/// Known DOM APIs to exclude from Tauri command detection
pub(super) const DOM_EXCLUSIONS: &[&str] = &[
    "execCommand",
    "queryCommandState",
    "queryCommandEnabled",
    "queryCommandSupported",
    "queryCommandValue",
];

/// Functions that ARE NOT Tauri invokes - ignore completely (project heuristics)
/// These happen to match "invoke" or "Command" but are not actual Tauri calls
pub(super) const NON_INVOKE_EXCLUSIONS: &[&str] = &[
    // React hooks that happen to have "Command" in name
    "useVoiceCommands",
    "useAssistantToolCommands",
    "useNewVisitVoiceCommands",
    "useAiTopicCommands",
    // VSCode command registration (not Tauri)
    "registerCommand",
    "registerTextEditorCommand",
    // Build tools / CLI commands (not Tauri)
    "runGitCommand",
    "executeCommand",
    "buildCommandString",
    "buildCommandArgs",
    "classifyCommand",
    // Internal tracking/context functions
    "onCommandContext",
    "enqueueCommandContext",
    "setLastCommand",
    "setCommandError",
    "recordCommandInvokeStart",
    "recordCommandInvokeFinish",
    "handleInvokeFailure",
    "isCommandMissingError",
    "isRetentionCommandMissing",
    // Collection/analysis utilities
    "collectInvokeCommands",
    "collectUsedCommandsFromRoamLogs",
    "extractInvokeCommandsFromText",
    "scanCommandsInFiles",
    "parseBackendCommands",
    "buildSessionCommandPayload",
    // Mention/slash command handlers (UI, not Tauri)
    "onMentionCommand",
    "onSlashCommand",
    // Mock/test utilities
    "invokeFallbackMock",
    "resolveMockCommand",
];

/// Command names that are clearly not Tauri commands (CLI tools / tests)
pub(super) const INVALID_COMMAND_NAMES: &[&str] = &[
    // CLI tools / shell commands
    "node", "npm", "pnpm", "yarn", "bun", "cargo", "rustc", "rustup", "git", "gh", "python",
    "python3", "pip", "brew", "apt", "yum", "sh", "bash", "zsh", "curl", "wget", "docker",
    "kubectl", // Generic/test names
    "test", "mock", "stub", "fake",
];

impl CommandDetectionConfig {
    pub fn new(
        dom_exclusions: &[String],
        non_invoke_exclusions: &[String],
        invalid_command_names: &[String],
    ) -> Self {
        let mut dom: HashSet<String> = DOM_EXCLUSIONS.iter().map(|s| s.to_string()).collect();
        dom.extend(dom_exclusions.iter().cloned());

        let mut non_invoke: HashSet<String> = NON_INVOKE_EXCLUSIONS
            .iter()
            .map(|s| s.to_string())
            .collect();
        non_invoke.extend(non_invoke_exclusions.iter().cloned());

        let mut invalid: HashSet<String> = INVALID_COMMAND_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect();
        invalid.extend(invalid_command_names.iter().cloned());

        Self {
            dom_exclusions: dom,
            non_invoke_exclusions: non_invoke,
            invalid_command_names: invalid,
            event_wrappers: HashSet::new(),
        }
    }

    /// Attach configured event wrapper function names (W3-b).
    pub fn with_event_wrappers(mut self, event_wrappers: &[String]) -> Self {
        self.event_wrappers.extend(event_wrappers.iter().cloned());
        self
    }
}

impl Default for CommandDetectionConfig {
    fn default() -> Self {
        Self::new(&[], &[], &[])
    }
}
