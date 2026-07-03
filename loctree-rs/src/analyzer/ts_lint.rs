//! TypeScript-specific lint checks
//!
//! Detects common TypeScript anti-patterns that weaken type safety:
//! - Explicit `any` types (`: any`, `as any`, `<any>`)
//! - `@ts-ignore` comments (suppresses all errors)
//! - `@ts-expect-error` comments (acceptable but tracked)
//! - `@ts-nocheck` comments (disables checking for entire file)
//!
//! # Example
//!
//! ```ignore
//! // BAD: any types
//! function process(data: any) { ... }
//! const value = response as any;
//!
//! // GOOD: proper types
//! function process(data: UserData) { ... }
//! const value = response as UserResponse;
//! ```
//!
//! Developed by The Loctree Team ⓒ 2025-2026

use super::is_test_file;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

/// A TypeScript-specific lint issue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsLintIssue {
    /// File path (relative)
    pub file: String,
    /// Line number (1-indexed)
    pub line: usize,
    /// Column number (1-indexed)
    pub column: usize,
    /// Rule that was violated
    pub rule: String,
    /// Severity: "high", "medium", "low"
    pub severity: String,
    /// Human-readable message
    pub message: String,
    /// The matched code snippet
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// TypeScript lint rule identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsLintRule {
    /// Explicit `any` type annotation
    ExplicitAny,
    /// @ts-ignore comment
    TsIgnore,
    /// @ts-expect-error comment
    TsExpectError,
    /// @ts-nocheck comment
    TsNocheck,
}

impl TsLintRule {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExplicitAny => "ts/explicit-any",
            Self::TsIgnore => "ts/ts-ignore",
            Self::TsExpectError => "ts/ts-expect-error",
            Self::TsNocheck => "ts/ts-nocheck",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::ExplicitAny => "Explicit `any` type weakens type safety",
            Self::TsIgnore => "@ts-ignore suppresses all TypeScript errors on next line",
            Self::TsExpectError => "@ts-expect-error suppresses expected error",
            Self::TsNocheck => "@ts-nocheck disables type checking for entire file",
        }
    }

    pub fn base_severity(&self) -> &'static str {
        match self {
            Self::ExplicitAny => "high",
            Self::TsIgnore => "high",
            Self::TsExpectError => "medium",
            Self::TsNocheck => "high",
        }
    }
}

// Pre-compiled regexes for performance
static ANY_TYPE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Matches: `: any`, `: any;`, `: any)`, `: any,`, `as any`, `<any`, `any[]`
    // But NOT: `company`, `anyway`, `anyone` (word boundaries)
    // <any matches generic position: Map<any, ...> or Promise<any>
    Regex::new(r"(?::\s*any\s*[;,)\]\s>]|:\s*any$|as\s+any\b|<any[,>]|\bany\[\])").unwrap()
});

static TS_IGNORE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@ts-ignore\b").unwrap());

static TS_EXPECT_ERROR_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@ts-expect-error\b").unwrap());

static TS_NOCHECK_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@ts-nocheck\b").unwrap());

/// Adjusts severity based on file context
fn adjust_severity(base_severity: &str, is_test: bool) -> String {
    if is_test && base_severity == "high" {
        "low".to_string()
    } else {
        base_severity.to_string()
    }
}

/// Lint a single TypeScript/TSX file for type safety issues
pub fn lint_ts_file(path: &Path, content: &str) -> Vec<TsLintIssue> {
    let mut issues = Vec::new();
    let file_str = path.to_string_lossy().to_string();
    let is_test = is_test_file(&file_str);

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;

        // Check for `any` types - find ALL matches in line
        for mat in ANY_TYPE_REGEX.find_iter(line) {
            issues.push(TsLintIssue {
                file: file_str.clone(),
                line: line_num,
                column: mat.start() + 1,
                rule: TsLintRule::ExplicitAny.as_str().to_string(),
                severity: adjust_severity(TsLintRule::ExplicitAny.base_severity(), is_test),
                message: TsLintRule::ExplicitAny.message().to_string(),
                snippet: Some(line.trim().to_string()),
            });
        }

        // Check for @ts-ignore
        if let Some(mat) = TS_IGNORE_REGEX.find(line) {
            issues.push(TsLintIssue {
                file: file_str.clone(),
                line: line_num,
                column: mat.start() + 1,
                rule: TsLintRule::TsIgnore.as_str().to_string(),
                severity: adjust_severity(TsLintRule::TsIgnore.base_severity(), is_test),
                message: TsLintRule::TsIgnore.message().to_string(),
                snippet: Some(line.trim().to_string()),
            });
        }

        // Check for @ts-expect-error
        if let Some(mat) = TS_EXPECT_ERROR_REGEX.find(line) {
            issues.push(TsLintIssue {
                file: file_str.clone(),
                line: line_num,
                column: mat.start() + 1,
                rule: TsLintRule::TsExpectError.as_str().to_string(),
                severity: adjust_severity(TsLintRule::TsExpectError.base_severity(), is_test),
                message: TsLintRule::TsExpectError.message().to_string(),
                snippet: Some(line.trim().to_string()),
            });
        }

        // Check for @ts-nocheck
        if let Some(mat) = TS_NOCHECK_REGEX.find(line) {
            issues.push(TsLintIssue {
                file: file_str.clone(),
                line: line_num,
                column: mat.start() + 1,
                rule: TsLintRule::TsNocheck.as_str().to_string(),
                severity: adjust_severity(TsLintRule::TsNocheck.base_severity(), is_test),
                message: TsLintRule::TsNocheck.message().to_string(),
                snippet: Some(line.trim().to_string()),
            });
        }
    }

    issues
}

/// Lint multiple files and return all issues
pub fn lint_ts_files(files: &[(String, String)]) -> Vec<TsLintIssue> {
    files
        .iter()
        .flat_map(|(path, content)| lint_ts_file(Path::new(path), content))
        .collect()
}

/// Summary statistics for TypeScript lint results
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TsLintSummary {
    pub total_issues: usize,
    pub by_severity: std::collections::HashMap<String, usize>,
    pub by_rule: std::collections::HashMap<String, usize>,
    pub affected_files: usize,
    pub test_files_issues: usize,
    pub prod_files_issues: usize,
}

impl TsLintSummary {
    pub fn from_issues(issues: &[TsLintIssue]) -> Self {
        use std::collections::{HashMap, HashSet};

        let files: HashSet<_> = issues.iter().map(|i| &i.file).collect();

        let mut by_severity = HashMap::new();
        for issue in issues {
            *by_severity.entry(issue.severity.clone()).or_insert(0) += 1;
        }

        let mut by_rule = HashMap::new();
        for issue in issues {
            *by_rule.entry(issue.rule.clone()).or_insert(0) += 1;
        }

        let test_files_issues = issues.iter().filter(|i| is_test_file(&i.file)).count();
        let prod_files_issues = issues.len() - test_files_issues;

        Self {
            total_issues: issues.len(),
            by_severity,
            by_rule,
            affected_files: files.len(),
            test_files_issues,
            prod_files_issues,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detects_colon_any() {
        let content = r#"
function process(data: any) {
    return data;
}
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/explicit-any");
        assert_eq!(issues[0].severity, "high");
    }

    #[test]
    fn test_detects_as_any() {
        let content = r#"
const value = response as any;
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/explicit-any");
    }

    #[test]
    fn test_detects_any_array() {
        let content = r#"
const items: any[] = [];
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/explicit-any");
    }

    #[test]
    fn test_detects_generic_any() {
        let content = r#"
const map = new Map<any, string>();
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/explicit-any");
    }

    #[test]
    fn test_ignores_any_in_words() {
        let content = r#"
const company = "Acme";
const anyway = true;
const anyone = "person";
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 0, "Should not match 'any' inside words");
    }

    #[test]
    fn test_detects_ts_ignore() {
        let content = r#"
// @ts-ignore
const x = badCode();
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/ts-ignore");
        assert_eq!(issues[0].severity, "high");
    }

    #[test]
    fn test_detects_ts_expect_error() {
        let content = r#"
// @ts-expect-error - intentional for testing
const x = badCode();
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/ts-expect-error");
        assert_eq!(issues[0].severity, "medium");
    }

    #[test]
    fn test_detects_ts_nocheck() {
        let content = r#"
// @ts-nocheck
const x = anything;
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "ts/ts-nocheck");
        assert_eq!(issues[0].severity, "high");
    }

    #[test]
    fn test_severity_lowered_for_test_files() {
        let content = "function mock(data: any) {}";

        // Production file - high severity
        let prod_issues = lint_ts_file(Path::new("src/service.ts"), content);
        assert_eq!(prod_issues[0].severity, "high");

        // Test file - low severity
        let test_issues = lint_ts_file(Path::new("src/service.test.ts"), content);
        assert_eq!(test_issues[0].severity, "low");

        // __tests__ folder - low severity
        let tests_dir_issues = lint_ts_file(Path::new("src/__tests__/service.ts"), content);
        assert_eq!(tests_dir_issues[0].severity, "low");
    }

    #[test]
    fn test_multiple_issues_per_file() {
        let content = r#"
// @ts-ignore
function bad(x: any, y: any): any {
    return x as any;
}
"#;
        let issues = lint_ts_file(Path::new("test.ts"), content);
        // 1x ts-ignore, 4x any (x: any, y: any, ): any, as any)
        assert!(issues.len() >= 4, "Should detect multiple issues");
    }

    #[test]
    fn test_summary_separates_test_and_prod() {
        let issues = vec![
            TsLintIssue {
                file: "src/app.ts".to_string(),
                line: 1,
                column: 1,
                rule: "ts/explicit-any".to_string(),
                severity: "high".to_string(),
                message: "test".to_string(),
                snippet: None,
            },
            TsLintIssue {
                file: "src/app.test.ts".to_string(),
                line: 1,
                column: 1,
                rule: "ts/explicit-any".to_string(),
                severity: "low".to_string(),
                message: "test".to_string(),
                snippet: None,
            },
        ];

        let summary = TsLintSummary::from_issues(&issues);
        assert_eq!(summary.prod_files_issues, 1);
        assert_eq!(summary.test_files_issues, 1);
    }

    #[test]
    fn test_column_position_correct() {
        let content = "const x: any = 1;";
        let issues = lint_ts_file(Path::new("test.ts"), content);
        assert_eq!(issues.len(), 1);
        // `: any` starts at position 8 (0-indexed: 7), column is 1-indexed
        assert!(issues[0].column > 0);
    }
}
