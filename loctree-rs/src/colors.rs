//! Terminal color utilities for CLI output.
//!
//! Provides ANSI color codes and semantic helpers for consistent
//! colorized output across all loctree commands.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::io::IsTerminal;

use crate::types::ColorMode;

// ============================================================================
// ANSI Color Codes
// ============================================================================

pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";

pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RESET: &str = "\x1b[0m";

// Bright variants
pub const BRIGHT_RED: &str = "\x1b[91m";
pub const BRIGHT_GREEN: &str = "\x1b[92m";
pub const BRIGHT_YELLOW: &str = "\x1b[93m";
pub const BRIGHT_CYAN: &str = "\x1b[96m";

// ============================================================================
// Color State
// ============================================================================

/// Determines if colors should be used based on ColorMode and terminal detection.
pub fn is_enabled(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::stdout().is_terminal(),
    }
}

/// Colorizer that can be passed around to format functions.
#[derive(Clone, Copy)]
pub struct Painter {
    enabled: bool,
}

impl Painter {
    pub fn new(mode: ColorMode) -> Self {
        Self {
            enabled: is_enabled(mode),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    // === Semantic colors ===

    /// Error, critical, dead code - RED
    pub fn error(&self, s: &str) -> String {
        self.wrap(s, RED)
    }

    /// Warning, cycles, caution - YELLOW
    pub fn warn(&self, s: &str) -> String {
        self.wrap(s, YELLOW)
    }

    /// Success, OK, healthy - GREEN
    pub fn ok(&self, s: &str) -> String {
        self.wrap(s, GREEN)
    }

    /// Info, neutral - BLUE
    pub fn info(&self, s: &str) -> String {
        self.wrap(s, BLUE)
    }

    /// File paths - CYAN
    pub fn path(&self, s: &str) -> String {
        self.wrap(s, CYAN)
    }

    /// Headers, titles - BOLD
    pub fn header(&self, s: &str) -> String {
        self.wrap(s, BOLD)
    }

    /// Secondary info, hints - DIM
    pub fn dim(&self, s: &str) -> String {
        self.wrap(s, DIM)
    }

    /// Symbols, identifiers - MAGENTA
    pub fn symbol(&self, s: &str) -> String {
        self.wrap(s, MAGENTA)
    }

    /// Numbers, counts - BRIGHT_CYAN
    pub fn number(&self, n: impl std::fmt::Display) -> String {
        self.wrap(&n.to_string(), BRIGHT_CYAN)
    }

    // === Status indicators ===

    /// [OK] prefix
    pub fn status_ok(&self, msg: &str) -> String {
        format!("{} {}", self.ok("[OK]"), msg)
    }

    /// [WARN] prefix
    pub fn status_warn(&self, msg: &str) -> String {
        format!("{} {}", self.warn("[WARN]"), msg)
    }

    /// [ERROR] prefix
    pub fn status_error(&self, msg: &str) -> String {
        format!("{} {}", self.error("[ERROR]"), msg)
    }

    /// [INFO] prefix
    pub fn status_info(&self, msg: &str) -> String {
        format!("{} {}", self.info("[INFO]"), msg)
    }

    // === Severity levels ===

    /// Critical severity - bright red
    pub fn critical(&self, s: &str) -> String {
        self.wrap(s, BRIGHT_RED)
    }

    /// High severity - red
    pub fn high(&self, s: &str) -> String {
        self.wrap(s, RED)
    }

    /// Medium severity - yellow
    pub fn medium(&self, s: &str) -> String {
        self.wrap(s, YELLOW)
    }

    /// Low severity - dim
    pub fn low(&self, s: &str) -> String {
        self.wrap(s, DIM)
    }

    // === Raw color access ===

    pub fn wrap(&self, s: &str, code: &str) -> String {
        if self.enabled {
            format!("{code}{s}{RESET}")
        } else {
            s.to_string()
        }
    }

    pub fn wrap_both(&self, s: &str, code1: &str, code2: &str) -> String {
        if self.enabled {
            format!("{code1}{code2}{s}{RESET}")
        } else {
            s.to_string()
        }
    }
}

// ============================================================================
// Convenience Functions (for quick one-off coloring)
// ============================================================================

/// Quick color wrapper - returns colored string if enabled
pub fn paint(s: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("{code}{s}{RESET}")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_painter_disabled() {
        let p = Painter { enabled: false };
        assert_eq!(p.error("test"), "test");
        assert_eq!(p.ok("test"), "test");
        assert_eq!(p.path("test"), "test");
    }

    #[test]
    fn test_painter_enabled() {
        let p = Painter { enabled: true };
        assert_eq!(p.error("test"), "\x1b[31mtest\x1b[0m");
        assert_eq!(p.ok("test"), "\x1b[32mtest\x1b[0m");
        assert_eq!(p.path("test"), "\x1b[36mtest\x1b[0m");
    }

    #[test]
    fn test_status_prefixes() {
        let p = Painter { enabled: true };
        assert!(p.status_ok("done").contains("[OK]"));
        assert!(p.status_warn("caution").contains("[WARN]"));
        assert!(p.status_error("failed").contains("[ERROR]"));
    }

    #[test]
    fn test_color_mode_detection() {
        assert!(is_enabled(ColorMode::Always));
        assert!(!is_enabled(ColorMode::Never));
        // Auto depends on terminal, can't reliably test
    }
}
