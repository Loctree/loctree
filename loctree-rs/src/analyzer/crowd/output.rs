//! Terminal output formatting for crowd detection

use super::types::{Crowd, CrowdIssue};

/// Shorten a path to fit within max_len while keeping it identifiable
/// Shows "...parent/file.ext" format for long paths
fn shorten_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }

    // Get path segments
    let parts: Vec<&str> = path.split('/').collect();

    if parts.len() <= 2 {
        // Short path, just truncate
        return format!("...{}", &path[path.len().saturating_sub(max_len - 3)..]);
    }

    // Try to show last 2-3 segments
    let filename = parts.last().unwrap_or(&"");
    let parent = parts.get(parts.len() - 2).unwrap_or(&"");
    let grandparent = parts.get(parts.len() - 3);

    let short = if let Some(gp) = grandparent {
        format!(".../{}/{}/{}", gp, parent, filename)
    } else {
        format!(".../{}/{}", parent, filename)
    };

    if short.len() <= max_len {
        short
    } else {
        format!(".../{}/{}", parent, filename)
    }
}

/// Format a crowd for terminal display
pub fn format_crowd(crowd: &Crowd, _verbose: bool) -> String {
    let mut lines = Vec::new();

    // Header
    let score_label = if crowd.score > 7.0 {
        "(HIGH - needs attention!)"
    } else if crowd.score > 4.0 {
        "(MEDIUM - review suggested)"
    } else {
        "(LOW - probably fine)"
    };

    // Show context type if detected
    let context_label = crowd
        .context_type
        .map(|ct| format!(" [{}]", ct))
        .unwrap_or_default();

    lines.push(format!("CROWD: \"{}\"{}", crowd.pattern, context_label));
    lines.push(format!(
        "Crowd Score: {:.1}/10 {}",
        crowd.score, score_label
    ));
    lines.push(String::new());

    // Members
    lines.push(format!("FILES IN CROWD ({} files)", crowd.members.len()));

    let max_importers = crowd
        .members
        .iter()
        .map(|m| m.importer_count)
        .max()
        .unwrap_or(1);

    for member in &crowd.members {
        let bar_len = if max_importers > 0 {
            (member.importer_count * 12 / max_importers.max(1)).max(1)
        } else {
            1
        };
        let bar = "█".repeat(bar_len);

        // Show shortened path that's still unique (last 2-3 path segments)
        let display_path = shorten_path(&member.file, 50);
        lines.push(format!(
            "  {:<50} {} {} importers",
            display_path, bar, member.importer_count
        ));
    }

    // Issues
    if !crowd.issues.is_empty() {
        lines.push(String::new());
        lines.push("=== ISSUES DETECTED ===".to_string());

        for issue in &crowd.issues {
            match issue {
                CrowdIssue::NameCollision { files } => {
                    lines.push(format!(
                        "  - Name collision: {} files with similar names",
                        files.len()
                    ));
                }
                CrowdIssue::UsageAsymmetry { primary, underused } => {
                    lines.push(format!(
                        "  - Usage asymmetry: {} is primary, {} underused",
                        primary,
                        underused.len()
                    ));
                }
                CrowdIssue::ExportOverlap { files, overlap: _ } => {
                    lines.push(format!(
                        "  - Export overlap: {} files export similar things",
                        files.len()
                    ));
                }
                CrowdIssue::Fragmentation { categories } => {
                    lines.push(format!(
                        "  - Fragmentation: functionality split across {} categories",
                        categories.len()
                    ));
                }
            }
        }
    }

    lines.join("\n")
}

/// Format multiple crowds summary
pub fn format_crowds_summary(crowds: &[Crowd]) -> String {
    if crowds.is_empty() {
        return "No crowds detected.".to_string();
    }

    let mut lines = vec![format!("Found {} potential crowds:\n", crowds.len())];

    for crowd in crowds {
        lines.push(format_crowd(crowd, false));
        lines.push(String::new());
    }

    lines.join("\n")
}
