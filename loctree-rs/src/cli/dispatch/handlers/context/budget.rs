//! Cut 11 — budget engine for the pill renderer.
//!
//! Hard contract: total markdown ≤ 1000 lines, fixed per-section caps,
//! ranking-aware truncation with a uniform tail line. The engine is a
//! pure-data utility (no I/O); the pill renderer feeds it ranked content
//! and gets back content trimmed to budget.
//!
//! # Section budgets
//!
//! | Section          | Cap | Notes                                     |
//! |------------------|-----|-------------------------------------------|
//! | TL;DR            | 100 | generated last, rendered first            |
//! | Where You Are    | 200 | hubs, cycles, recent activity, risk       |
//! | What's Live      | 200 | idiom tags, dispatch, env, reachability   |
//! | Memory           | 200 | AICX entries (already capped upstream)    |
//! | Action           | 100 | ≤3 next-safe commands + gates + tests     |
//! | Authority Index  | 100 | bucket counts (no enumeration)            |
//! | Total            |1000 | preserved by [`Budget::TOTAL_CEILING`]    |
//!
//! When ranked content exceeds a section's cap, the engine drops the
//! lowest-ranked rows and appends a uniform tail line so the operator
//! always knows where to look for the rest:
//!
//! > `+ N more, run \`loct context --full\` for full data`

/// Hard caps + ceilings used by the pill renderer.
pub struct Budget;

impl Budget {
    /// Total markdown line ceiling for the pill output.
    pub const TOTAL_CEILING: usize = 1000;

    pub const TLDR_CAP: usize = 100;
    pub const WHERE_CAP: usize = 200;
    pub const LIVE_CAP: usize = 200;
    pub const MEMORY_CAP: usize = 200;
    pub const ACTION_CAP: usize = 100;
    pub const AUTHORITY_CAP: usize = 100;

    /// Sum of section caps. Always ≤ [`Self::TOTAL_CEILING`]; the gap is
    /// reserved for the header, footer, and inter-section blank lines so
    /// that the rendered pill still fits under the global ceiling even
    /// when every section is at its individual cap. Test-only invariant
    /// helper today — the renderer relies on individual `*_CAP` constants.
    #[cfg(test)]
    pub const fn sum_of_caps() -> usize {
        Self::TLDR_CAP
            + Self::WHERE_CAP
            + Self::LIVE_CAP
            + Self::MEMORY_CAP
            + Self::ACTION_CAP
            + Self::AUTHORITY_CAP
    }
}

/// Truncate `lines` to `cap`, appending the uniform tail line when content
/// was dropped. The tail line itself counts toward `cap` so the caller can
/// trust the returned vector to fit.
pub fn truncate_with_tail(lines: Vec<String>, cap: usize) -> Vec<String> {
    if lines.len() <= cap {
        return lines;
    }
    if cap == 0 {
        return Vec::new();
    }
    // Reserve one line for the tail message.
    let kept = cap.saturating_sub(1);
    let dropped = lines.len() - kept;
    let mut out: Vec<String> = lines.into_iter().take(kept).collect();
    out.push(format!(
        "+ {dropped} more, run `loct context --full` for full data"
    ));
    out
}

/// Apply the per-section ranking + truncation to a list of items, then
/// render each surviving item via `render`. Items are sorted in-place by
/// `score` descending (higher = more relevant). Used by the pill renderer
/// for the hub table (importers as score); `truncate_with_tail` is used
/// directly when ranking already happened upstream.
pub fn rank_and_render<T>(
    mut items: Vec<T>,
    cap: usize,
    score: impl Fn(&T) -> i64,
    render: impl Fn(&T) -> String,
) -> Vec<String> {
    items.sort_by_key(|item| -score(item));
    let lines: Vec<String> = items.iter().map(&render).collect();
    truncate_with_tail(lines, cap)
}

/// Count rendered markdown lines (1 line per `\n`-terminated segment, with
/// trailing partial line counted). Used for sanity checks in the renderer
/// and the unit tests.
pub fn count_lines(markdown: &str) -> usize {
    if markdown.is_empty() {
        return 0;
    }
    let mut n = markdown.matches('\n').count();
    if !markdown.ends_with('\n') {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_of_caps_fits_under_total_ceiling() {
        // Section caps must fit under the global ceiling with headroom
        // for the header / footer / blank lines that frame the pill.
        let sum = Budget::sum_of_caps();
        assert!(
            sum <= Budget::TOTAL_CEILING,
            "section caps {sum} exceed total ceiling {}",
            Budget::TOTAL_CEILING
        );
        let headroom = Budget::TOTAL_CEILING - sum;
        assert!(
            headroom >= 50,
            "expected ≥50 lines of headroom for header/footer, got {headroom}"
        );
    }

    #[test]
    fn truncate_keeps_everything_when_under_cap() {
        let input = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        let out = truncate_with_tail(input.clone(), 10);
        assert_eq!(out, input);
    }

    #[test]
    fn truncate_appends_tail_when_over_cap() {
        let input: Vec<String> = (0..20).map(|i| format!("row-{i}")).collect();
        let out = truncate_with_tail(input, 5);
        assert_eq!(out.len(), 5);
        assert!(
            out.last().unwrap().starts_with("+ "),
            "tail line must start with '+ '"
        );
        assert!(
            out.last().unwrap().contains("loct context --full"),
            "tail must guide to --full"
        );
        assert!(
            out.last().unwrap().contains("16"),
            "tail must report the dropped count, got: {}",
            out.last().unwrap()
        );
    }

    #[test]
    fn truncate_handles_zero_cap() {
        let input = vec!["a".to_string(), "b".to_string()];
        assert!(truncate_with_tail(input, 0).is_empty());
    }

    #[test]
    fn rank_and_render_sorts_descending_then_caps() {
        struct Row {
            name: &'static str,
            score: i64,
        }
        let rows = vec![
            Row {
                name: "low",
                score: 1,
            },
            Row {
                name: "high",
                score: 10,
            },
            Row {
                name: "mid",
                score: 5,
            },
        ];
        let rendered = rank_and_render(rows, 2, |r| r.score, |r| r.name.to_string());
        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0], "high");
        assert!(
            rendered[1].starts_with("+ ") || rendered[1] == "mid",
            "second slot is mid or tail line, got: {}",
            rendered[1]
        );
    }

    #[test]
    fn count_lines_matches_section_caps() {
        let body = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // 50 lines (no trailing newline) → 50 counted.
        assert_eq!(count_lines(&body), 50);
        let with_nl = format!("{body}\n");
        assert_eq!(count_lines(&with_nl), 50);
        assert_eq!(count_lines(""), 0);
    }
}
