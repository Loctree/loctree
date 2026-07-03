//! React-specific lint checks using OXC AST
//!
//! Detects common React anti-patterns that can cause bugs:
//! - `useEffect` with async operations but no cleanup (race condition risk)
//! - `useEffect` with `setTimeout`/`setInterval` but no cleanup (memory leak)
//! - Potential setState after unmount patterns
//!
//! # Example
//!
//! ```ignore
//! // BAD: Race condition - no cleanup
//! useEffect(() => {
//!     async function fetch() {
//!         const data = await api.get();
//!         setState(data);  // May fire after unmount!
//!     }
//!     fetch();
//! }, []);
//!
//! // GOOD: With cleanup
//! useEffect(() => {
//!     let cancelled = false;
//!     async function fetch() {
//!         const data = await api.get();
//!         if (!cancelled) setState(data);
//!     }
//!     fetch();
//!     return () => { cancelled = true; };
//! }, []);
//! ```
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use serde::{Deserialize, Serialize};
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};

/// A React-specific lint issue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactLintIssue {
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
    /// Suggested fix
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// React lint rule identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactLintRule {
    /// useEffect with async but no cleanup return
    AsyncEffectNoCleanup,
    /// useEffect with setTimeout but no clearTimeout in cleanup
    SetTimeoutNoCleanup,
    /// useEffect with setInterval but no clearInterval in cleanup
    SetIntervalNoCleanup,
    /// useEffect with addEventListener but no removeEventListener
    EventListenerNoCleanup,
}

impl ReactLintRule {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AsyncEffectNoCleanup => "react/async-effect-no-cleanup",
            Self::SetTimeoutNoCleanup => "react/settimeout-no-cleanup",
            Self::SetIntervalNoCleanup => "react/setinterval-no-cleanup",
            Self::EventListenerNoCleanup => "react/eventlistener-no-cleanup",
        }
    }

    pub fn severity(&self) -> &'static str {
        match self {
            Self::AsyncEffectNoCleanup => "high",
            Self::SetTimeoutNoCleanup => "medium",
            Self::SetIntervalNoCleanup => "high",
            Self::EventListenerNoCleanup => "medium",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::AsyncEffectNoCleanup => {
                "useEffect with async operation but no cleanup - race condition risk"
            }
            Self::SetTimeoutNoCleanup => "useEffect with setTimeout but no clearTimeout in cleanup",
            Self::SetIntervalNoCleanup => {
                "useEffect with setInterval but no clearInterval in cleanup - memory leak"
            }
            Self::EventListenerNoCleanup => {
                "useEffect with addEventListener but no removeEventListener in cleanup"
            }
        }
    }

    pub fn suggestion(&self) -> &'static str {
        match self {
            Self::AsyncEffectNoCleanup => {
                "Add a cleanup function: return () => { cancelled = true; };"
            }
            Self::SetTimeoutNoCleanup => {
                "Add cleanup: const timer = setTimeout(...); return () => clearTimeout(timer);"
            }
            Self::SetIntervalNoCleanup => {
                "Add cleanup: const interval = setInterval(...); return () => clearInterval(interval);"
            }
            Self::EventListenerNoCleanup => {
                "Add cleanup: return () => element.removeEventListener(...);"
            }
        }
    }
}

/// Context tracked while visiting useEffect body
#[derive(Debug, Default)]
struct EffectContext {
    /// Found async function or await expression
    has_async: bool,
    /// Found setTimeout call
    has_set_timeout: bool,
    /// Found setInterval call
    has_set_interval: bool,
    /// Found addEventListener call
    has_add_event_listener: bool,
    /// Found return statement (cleanup function)
    has_cleanup_return: bool,
    /// Found clearTimeout in cleanup
    has_clear_timeout: bool,
    /// Found clearInterval in cleanup
    has_clear_interval: bool,
    /// Found removeEventListener in cleanup
    has_remove_event_listener: bool,
    /// Found cancelled/isMounted pattern
    has_cancelled_pattern: bool,
}

/// Visitor for analyzing React files
struct ReactLintVisitor<'a> {
    issues: Vec<ReactLintIssue>,
    source_text: &'a str,
    file_path: String,
}

impl<'a> ReactLintVisitor<'a> {
    fn new(source_text: &'a str, file_path: String) -> Self {
        Self {
            issues: Vec::new(),
            source_text,
            file_path,
        }
    }

    fn span_to_location(&self, span: Span) -> (usize, usize) {
        let offset = span.start as usize;
        let line = self.source_text[..offset]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1;
        let last_newline = self.source_text[..offset]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let column = offset - last_newline + 1;
        (line, column)
    }

    fn add_issue(&mut self, span: Span, rule: ReactLintRule) {
        let (line, column) = self.span_to_location(span);
        self.issues.push(ReactLintIssue {
            file: self.file_path.clone(),
            line,
            column,
            rule: rule.as_str().to_string(),
            severity: rule.severity().to_string(),
            message: rule.message().to_string(),
            suggestion: Some(rule.suggestion().to_string()),
        });
    }

    /// Analyze a useEffect call expression
    fn check_use_effect(&mut self, call: &CallExpression<'a>) {
        // Get the effect callback (first argument)
        let Some(first_arg) = call.arguments.first() else {
            return;
        };

        let effect_body = match first_arg {
            Argument::ArrowFunctionExpression(arrow) => &arrow.body,
            Argument::FunctionExpression(func) => {
                if let Some(body) = &func.body {
                    body
                } else {
                    return;
                }
            }
            _ => return,
        };

        // Analyze the effect body
        let ctx = self.analyze_effect_body(effect_body);

        // Check for issues
        if ctx.has_async && !ctx.has_cleanup_return && !ctx.has_cancelled_pattern {
            self.add_issue(call.span, ReactLintRule::AsyncEffectNoCleanup);
        }

        if ctx.has_set_timeout && !ctx.has_clear_timeout && !ctx.has_cleanup_return {
            self.add_issue(call.span, ReactLintRule::SetTimeoutNoCleanup);
        }

        if ctx.has_set_interval && !ctx.has_clear_interval {
            self.add_issue(call.span, ReactLintRule::SetIntervalNoCleanup);
        }

        if ctx.has_add_event_listener && !ctx.has_remove_event_listener {
            self.add_issue(call.span, ReactLintRule::EventListenerNoCleanup);
        }
    }

    /// Analyze the body of a useEffect callback
    fn analyze_effect_body(&self, body: &FunctionBody<'a>) -> EffectContext {
        let mut ctx = EffectContext::default();

        // Get source text for the body to do pattern matching
        let body_start = body.span.start as usize;
        let body_end = body.span.end as usize;
        let body_text = if body_end <= self.source_text.len() {
            &self.source_text[body_start..body_end]
        } else {
            ""
        };

        // Check for async patterns (simple string matching for reliability)
        // Match "async" keyword in various contexts: async (), async\n, async function
        ctx.has_async = body_text.contains("async") || body_text.contains("await");

        // Check for timer patterns
        ctx.has_set_timeout = body_text.contains("setTimeout");
        ctx.has_set_interval = body_text.contains("setInterval");
        ctx.has_clear_timeout = body_text.contains("clearTimeout");
        ctx.has_clear_interval = body_text.contains("clearInterval");

        // Check for event listener patterns
        ctx.has_add_event_listener = body_text.contains("addEventListener");
        ctx.has_remove_event_listener = body_text.contains("removeEventListener");

        // Check for cleanup return - various patterns:
        // return () => { ... }
        // return () => cleanup()
        // return function() { ... }
        // return cleanup; (function reference)
        ctx.has_cleanup_return = body_text.contains("return ()")
            || body_text.contains("return () =>")
            || body_text.contains("return function")
            || body_text.contains("return async ()")
            || body_text.contains("return async () =>");

        // Also check for return followed by identifier (return cleanup;)
        // but filter out non-function returns like null, undefined, 0
        if !ctx.has_cleanup_return {
            for line in body_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("return ") && !trimmed.starts_with("return;") {
                    // Extract what's being returned
                    let returned = trimmed["return ".len()..].trim_end_matches(';').trim();
                    // Skip non-function literals
                    let lowered = returned.to_ascii_lowercase();
                    if lowered == "null"
                        || lowered == "undefined"
                        || lowered == "0"
                        || lowered == "false"
                        || lowered == "true"
                        || returned.is_empty()
                    {
                        continue;
                    }
                    // Likely a cleanup function reference
                    ctx.has_cleanup_return = true;
                    break;
                }
            }
        }

        // Check for cancelled/isMounted pattern
        ctx.has_cancelled_pattern = body_text.contains("cancelled")
            || body_text.contains("isMounted")
            || body_text.contains("isMount")
            || body_text.contains("aborted")
            || body_text.contains("AbortController");

        ctx
    }
}

impl<'a> Visit<'a> for ReactLintVisitor<'a> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        // Detect useEffect/useLayoutEffect calls
        if let Expression::Identifier(ident) = &call.callee {
            let name = ident.name.as_str();
            if name == "useEffect" || name == "useLayoutEffect" {
                self.check_use_effect(call);
            }
        }

        // Continue visiting children
        oxc_ast_visit::walk::walk_call_expression(self, call);
    }
}

/// Analyze a single file for React lint issues
pub fn analyze_react_file(
    content: &str,
    path: &Path,
    relative_path: String,
) -> Vec<ReactLintIssue> {
    // Only analyze React-like files (tsx, jsx, ts, js)
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !matches!(ext, "tsx" | "jsx" | "ts" | "js") {
        return Vec::new();
    }

    // Quick check: does the file use React hooks?
    if !content.contains("useEffect") && !content.contains("useLayoutEffect") {
        return Vec::new();
    }

    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path)
        .unwrap_or_default()
        .with_typescript(true)
        .with_jsx(ext == "tsx" || ext == "jsx");

    let ret = Parser::new(&allocator, content, source_type).parse();

    // Skip files with parse errors
    if !ret.errors.is_empty() {
        return Vec::new();
    }

    let mut visitor = ReactLintVisitor::new(content, relative_path);
    visitor.visit_program(&ret.program);

    visitor.issues
}

/// Analyze multiple files and collect all React lint issues
pub fn analyze_react_files(files: &[(String, String, String)]) -> Vec<ReactLintIssue> {
    files
        .iter()
        .flat_map(|(content, path_str, relative)| {
            let path = Path::new(path_str);
            analyze_react_file(content, path, relative.clone())
        })
        .collect()
}

/// Summary of React lint results
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReactLintSummary {
    pub total_issues: usize,
    pub by_severity: std::collections::HashMap<String, usize>,
    pub by_rule: std::collections::HashMap<String, usize>,
    pub affected_files: usize,
}

impl ReactLintSummary {
    pub fn from_issues(issues: &[ReactLintIssue]) -> Self {
        use std::collections::{HashMap, HashSet};

        let files: HashSet<_> = issues.iter().map(|i| &i.file).collect();

        // Count by severity
        let mut by_severity = HashMap::new();
        for issue in issues {
            *by_severity.entry(issue.severity.clone()).or_insert(0) += 1;
        }

        // Count by rule
        let mut by_rule = HashMap::new();
        for issue in issues {
            *by_rule.entry(issue.rule.clone()).or_insert(0) += 1;
        }

        Self {
            total_issues: issues.len(),
            by_severity,
            by_rule,
            affected_files: files.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(content: &str) -> Vec<ReactLintIssue> {
        analyze_react_file(content, Path::new("test.tsx"), "test.tsx".to_string())
    }

    #[test]
    fn test_async_effect_no_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [data, setData] = useState(null);

                useEffect(() => {
                    async function fetchData() {
                        const result = await fetch('/api/data');
                        setData(result);
                    }
                    fetchData();
                }, []);

                return <div>{data}</div>;
            }
        "#;

        let issues = analyze(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "react/async-effect-no-cleanup");
        assert_eq!(issues[0].severity, "high");
    }

    #[test]
    fn test_async_effect_with_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [data, setData] = useState(null);

                useEffect(() => {
                    let cancelled = false;
                    async function fetchData() {
                        const result = await fetch('/api/data');
                        if (!cancelled) setData(result);
                    }
                    fetchData();
                    return () => { cancelled = true; };
                }, []);

                return <div>{data}</div>;
            }
        "#;

        let issues = analyze(content);
        assert!(
            issues.is_empty(),
            "Should not flag effect with cleanup: {:?}",
            issues
        );
    }

    #[test]
    fn test_async_effect_with_abort_controller() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [data, setData] = useState(null);

                useEffect(() => {
                    const controller = new AbortController();
                    fetch('/api/data', { signal: controller.signal })
                        .then(setData);
                    return () => controller.abort();
                }, []);

                return <div>{data}</div>;
            }
        "#;

        let issues = analyze(content);
        assert!(
            issues.is_empty(),
            "Should not flag effect with AbortController: {:?}",
            issues
        );
    }

    #[test]
    fn test_settimeout_no_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [visible, setVisible] = useState(true);

                useEffect(() => {
                    setTimeout(() => {
                        setVisible(false);
                    }, 5000);
                }, []);

                return <div>{visible ? 'Hello' : 'Bye'}</div>;
            }
        "#;

        let issues = analyze(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "react/settimeout-no-cleanup");
    }

    #[test]
    fn test_settimeout_with_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [visible, setVisible] = useState(true);

                useEffect(() => {
                    const timer = setTimeout(() => {
                        setVisible(false);
                    }, 5000);
                    return () => clearTimeout(timer);
                }, []);

                return <div>{visible ? 'Hello' : 'Bye'}</div>;
            }
        "#;

        let issues = analyze(content);
        assert!(
            issues.is_empty(),
            "Should not flag setTimeout with cleanup: {:?}",
            issues
        );
    }

    #[test]
    fn test_setinterval_no_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [count, setCount] = useState(0);

                useEffect(() => {
                    setInterval(() => {
                        setCount(c => c + 1);
                    }, 1000);
                }, []);

                return <div>{count}</div>;
            }
        "#;

        let issues = analyze(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "react/setinterval-no-cleanup");
        assert_eq!(issues[0].severity, "high");
    }

    #[test]
    fn test_event_listener_no_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [size, setSize] = useState(window.innerWidth);

                useEffect(() => {
                    window.addEventListener('resize', () => {
                        setSize(window.innerWidth);
                    });
                }, []);

                return <div>{size}</div>;
            }
        "#;

        let issues = analyze(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "react/eventlistener-no-cleanup");
    }

    #[test]
    fn test_event_listener_with_cleanup() {
        let content = r#"
            import { useEffect, useState } from 'react';

            function Component() {
                const [size, setSize] = useState(window.innerWidth);

                useEffect(() => {
                    const handler = () => setSize(window.innerWidth);
                    window.addEventListener('resize', handler);
                    return () => window.removeEventListener('resize', handler);
                }, []);

                return <div>{size}</div>;
            }
        "#;

        let issues = analyze(content);
        assert!(
            issues.is_empty(),
            "Should not flag addEventListener with cleanup: {:?}",
            issues
        );
    }

    #[test]
    fn test_no_react_hooks_file() {
        let content = r#"
            export function utils() {
                return 42;
            }
        "#;

        let issues = analyze(content);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_use_layout_effect() {
        let content = r#"
            import { useLayoutEffect, useState } from 'react';

            function Component() {
                const [height, setHeight] = useState(0);

                useLayoutEffect(() => {
                    async function measure() {
                        const h = await someAsyncMeasure();
                        setHeight(h);
                    }
                    measure();
                }, []);

                return <div style={{height}}></div>;
            }
        "#;

        let issues = analyze(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "react/async-effect-no-cleanup");
    }

    #[test]
    fn test_summary() {
        let issues = vec![
            ReactLintIssue {
                file: "a.tsx".into(),
                line: 1,
                column: 1,
                rule: "react/async-effect-no-cleanup".into(),
                severity: "high".into(),
                message: "test".into(),
                suggestion: None,
            },
            ReactLintIssue {
                file: "a.tsx".into(),
                line: 10,
                column: 1,
                rule: "react/settimeout-no-cleanup".into(),
                severity: "medium".into(),
                message: "test".into(),
                suggestion: None,
            },
            ReactLintIssue {
                file: "b.tsx".into(),
                line: 5,
                column: 1,
                rule: "react/setinterval-no-cleanup".into(),
                severity: "high".into(),
                message: "test".into(),
                suggestion: None,
            },
        ];

        let summary = ReactLintSummary::from_issues(&issues);
        assert_eq!(summary.total_issues, 3);
        assert_eq!(summary.by_severity.get("high"), Some(&2));
        assert_eq!(summary.by_severity.get("medium"), Some(&1));
        assert_eq!(summary.affected_files, 2);
    }
}
