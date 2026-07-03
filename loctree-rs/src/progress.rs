//! Progress UI utilities (spinners, status messages)
//!
//! Provides Black-style feedback for CLI operations.

use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Spinner for long-running operations
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    /// Create a new spinner with a message
    pub fn new(message: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
                .template("{spinner:.cyan} {msg}")
                .expect("valid template"),
        );
        bar.set_message(message.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));
        Self { bar }
    }

    /// Update the spinner message
    pub fn set_message(&self, message: &str) {
        self.bar.set_message(message.to_string());
    }

    /// Finish with success message (green)
    pub fn finish_success(&self, message: &str) {
        self.bar.finish_and_clear();
        eprintln!("{} {}", style("[OK]").green().bold(), message);
    }

    /// Finish with warning message (yellow)
    pub fn finish_warning(&self, message: &str) {
        self.bar.finish_and_clear();
        eprintln!("{} {}", style("[!]").yellow().bold(), message);
    }

    /// Finish with error message (red)
    pub fn finish_error(&self, message: &str) {
        self.bar.finish_and_clear();
        eprintln!("{} {}", style("[ERR]").red().bold(), message);
    }

    /// Just clear the spinner without message
    pub fn finish_clear(&self) {
        self.bar.finish_and_clear();
    }
}

/// Print a success message (green)
pub fn success(message: &str) {
    eprintln!("{} {}", style("[OK]").green().bold(), message);
}

/// Print an info message (blue)
pub fn info(message: &str) {
    eprintln!("{} {}", style("[i]").blue().bold(), message);
}

/// Print a warning message (yellow)
pub fn warning(message: &str) {
    eprintln!("{} {}", style("[!]").yellow().bold(), message);
}

/// Print an error message (red)
pub fn error(message: &str) {
    eprintln!("{} {}", style("[ERR]").red().bold(), message);
}

/// Format duration in human-readable form
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{:.2}s", secs)
    } else {
        let mins = secs / 60.0;
        format!("{:.1}m", mins)
    }
}

/// Format a count with proper singular/plural
pub fn format_count(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{} {}", count, singular)
    } else {
        format!("{} {}", count, plural)
    }
}
