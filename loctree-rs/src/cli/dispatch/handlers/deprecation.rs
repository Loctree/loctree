//! Deprecation warnings for truly redundant commands
//!
//! ## Design Philosophy (Hybrid CLI)
//! - Human-friendly commands (`dead`, `cycles`, `twins`, `health`, `audit`) are STABLE
//! - jq-style queries (`.dead_parrots`, `.cycles`) are for power-users/CI
//! - Only truly redundant aliases are deprecated:
//!   - `zombie` → use `dead` (same output)
//!   - `sniff` → use `loct findings` (same output)
//!
//! Removal of redundant commands planned for 0.9.0.

/// Print a deprecation warning to stderr (does not break piped output)
pub fn warn_deprecated(old_cmd: &str, new_cmd: &str) {
    eprintln!(
        "[DEPRECATED] 'loct {}' will be removed in 0.9. Use: {}",
        old_cmd, new_cmd
    );
}
