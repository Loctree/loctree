//! Memory leak detection for JavaScript/TypeScript
//!
//! Detects patterns that commonly cause memory leaks OUTSIDE of React hooks.
//! For React-specific leaks inside useEffect, see `react_lint.rs`.
//!
//! # Rules
//!
//! | Rule ID | Pattern | Severity |
//! |---------|---------|----------|
//! | `mem/module-cache-unbounded` | Module-level Map/Set without size limit | MEDIUM |
//! | `mem/subscription-leak` | .subscribe() without .unsubscribe() | HIGH |
//! | `mem/global-interval` | setInterval in non-React files | HIGH |
//! | `mem/global-event-listener` | addEventListener outside React components | MEDIUM |
//!
//! # Example
//!
//! ```ignore
//! // BAD: Unbounded cache at module level
//! const cache = new Map();  // mem/module-cache-unbounded
//! export function getData(key) {
//!     if (!cache.has(key)) {
//!         cache.set(key, fetchData(key));  // Grows forever!
//!     }
//!     return cache.get(key);
//! }
//!
//! // GOOD: LRU cache with size limit
//! const cache = new LRUCache({ max: 100 });
//! ```
//!
//! Developed by The Loctree Team ⓒ 2025-2026

use super::is_test_file;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Heuristic constants for context-based detection
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum indentation (spaces) to consider a declaration at module level.
/// Lines indented more than this are assumed to be inside functions/classes.
const MAX_MODULE_LEVEL_INDENT: usize = 4;

/// Number of lines to scan after a .subscribe() call for cleanup patterns.
const SUBSCRIPTION_CONTEXT_WINDOW: usize = 50;

/// Number of lines to scan after setInterval for clearInterval.
const INTERVAL_CONTEXT_WINDOW: usize = 30;

/// Number of lines to scan after addEventListener for removeEventListener.
const EVENT_LISTENER_CONTEXT_WINDOW: usize = 30;

/// A memory leak lint issue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLintIssue {
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

/// Memory leak rule identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryLintRule {
    /// Module-level Map/Set that can grow unbounded
    ModuleCacheUnbounded,
    /// .subscribe() without corresponding .unsubscribe()
    SubscriptionLeak,
    /// setInterval in non-React file
    GlobalInterval,
    /// addEventListener outside React component
    GlobalEventListener,
}

impl MemoryLintRule {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ModuleCacheUnbounded => "mem/module-cache-unbounded",
            Self::SubscriptionLeak => "mem/subscription-leak",
            Self::GlobalInterval => "mem/global-interval",
            Self::GlobalEventListener => "mem/global-event-listener",
        }
    }

    pub fn severity(&self) -> &'static str {
        match self {
            Self::ModuleCacheUnbounded => "medium",
            Self::SubscriptionLeak => "high",
            Self::GlobalInterval => "high",
            Self::GlobalEventListener => "medium",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::ModuleCacheUnbounded => {
                "Module-level Map/Set without size limit can grow unbounded"
            }
            Self::SubscriptionLeak => {
                "Subscription created without corresponding unsubscribe - potential memory leak"
            }
            Self::GlobalInterval => "setInterval in non-React file without cleanup mechanism",
            Self::GlobalEventListener => {
                "addEventListener outside React lifecycle - ensure cleanup exists"
            }
        }
    }

    pub fn suggestion(&self) -> &'static str {
        match self {
            Self::ModuleCacheUnbounded => {
                "Consider using LRU cache with max size, or implement eviction logic"
            }
            Self::SubscriptionLeak => {
                "Store subscription and call .unsubscribe() when done, or use takeUntil pattern"
            }
            Self::GlobalInterval => "Store interval ID and call clearInterval() in cleanup logic",
            Self::GlobalEventListener => {
                "Ensure removeEventListener is called when listener is no longer needed"
            }
        }
    }
}

/// Summary statistics for memory lint
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryLintSummary {
    pub total_issues: usize,
    pub by_severity: HashMap<String, usize>,
    pub by_rule: HashMap<String, usize>,
    pub affected_files: usize,
}

// Regex patterns for detection
static MODULE_CACHE_REGEX: Lazy<Regex> = Lazy::new(|| {
    // Match: const/let/var name = new Map() or new Set() at module level
    // We detect these at any level and filter by context
    Regex::new(r"(?:const|let|var)\s+\w+\s*=\s*new\s+(?:Map|Set)\s*\(\s*\)").unwrap()
});

static SUBSCRIBE_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.subscribe\s*\(").unwrap());

static UNSUBSCRIBE_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.unsubscribe\s*\(").unwrap());

static SET_INTERVAL_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bsetInterval\s*\(").unwrap());

static CLEAR_INTERVAL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bclearInterval\s*\(").unwrap());

static ADD_EVENT_LISTENER_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\.addEventListener\s*\(").unwrap());

static REMOVE_EVENT_LISTENER_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\.removeEventListener\s*\(").unwrap());

static USE_EFFECT_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\buseEffect\s*\(").unwrap());

/// Check if file is a React component file
fn is_react_file(path: &str) -> bool {
    path.ends_with(".tsx") || path.ends_with(".jsx")
}

/// Check if file is a service worker (intentionally long-lived)
fn is_service_worker(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with("sw.js")
        || p.ends_with("service-worker.js")
        || p.ends_with("serviceworker.js")
        || p.contains("/sw/")
        || p.contains("workbox")
}

/// Check if the code likely uses useEffect (React hook pattern)
fn has_use_effect(content: &str) -> bool {
    USE_EFFECT_REGEX.is_match(content)
}

/// Check for LRU or bounded cache patterns
fn has_cache_limit_pattern(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("lru")
        || lower.contains("maxsize")
        || lower.contains("max_size")
        || lower.contains("maxentries")
        || lower.contains("max_entries")
        || lower.contains(".delete(")  // Manual eviction
        || lower.contains(".clear(") // Manual clearing
}

/// Lint a TypeScript/JavaScript file for memory leak patterns
pub fn lint_memory_file(path: &Path, content: &str) -> Vec<MemoryLintIssue> {
    let mut issues = Vec::new();
    let relative_path = path.to_string_lossy().to_string();

    // Skip test files - they have different lifecycle
    if is_test_file(&relative_path) {
        return issues;
    }

    // Skip service workers - intentionally long-lived with permanent listeners
    if is_service_worker(&relative_path) {
        return issues;
    }

    let is_react = is_react_file(&relative_path);
    let uses_effect = has_use_effect(content);

    // Check for module-level caches
    check_module_cache(content, &relative_path, &mut issues);

    // Check for subscription leaks
    check_subscription_leaks(content, &relative_path, &mut issues);

    // Check for global intervals (only in non-React or React without useEffect context)
    if !is_react || !uses_effect {
        check_global_intervals(content, &relative_path, &mut issues);
    }

    // Check for global event listeners (only in non-React files)
    if !is_react {
        check_global_event_listeners(content, &relative_path, &mut issues);
    }

    issues
}

fn check_module_cache(content: &str, file: &str, issues: &mut Vec<MemoryLintIssue>) {
    // Skip if file has cache limit patterns
    if has_cache_limit_pattern(content) {
        return;
    }

    for (line_num, line) in content.lines().enumerate() {
        // Skip lines inside functions (rough heuristic: indented lines)
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        // Module-level declarations typically have 0 or minimal indent
        // This is a rough heuristic - AST would be more accurate
        if indent > MAX_MODULE_LEVEL_INDENT {
            continue;
        }

        if MODULE_CACHE_REGEX.is_match(line) {
            // Check if this line or nearby lines have size limits
            let context_start = line_num.saturating_sub(2);
            let context_end = (line_num + 3).min(content.lines().count());
            let context: String = content
                .lines()
                .skip(context_start)
                .take(context_end - context_start)
                .collect::<Vec<_>>()
                .join("\n");

            if has_cache_limit_pattern(&context) {
                continue;
            }

            let col = line.find("new").unwrap_or(0) + 1;
            issues.push(MemoryLintIssue {
                file: file.to_string(),
                line: line_num + 1,
                column: col,
                rule: MemoryLintRule::ModuleCacheUnbounded.as_str().to_string(),
                severity: MemoryLintRule::ModuleCacheUnbounded.severity().to_string(),
                message: MemoryLintRule::ModuleCacheUnbounded.message().to_string(),
                suggestion: Some(
                    MemoryLintRule::ModuleCacheUnbounded
                        .suggestion()
                        .to_string(),
                ),
            });
        }
    }
}

/// Check for Zustand-style unsubscribe pattern (function stored and called later)
fn has_zustand_unsubscribe_pattern(content: &str) -> bool {
    let lower = content.to_lowercase();
    // Pattern: unsubscribe variable is stored and called
    // e.g., "let unsubscribe = store.subscribe(...)" + "unsubscribe()"
    // e.g., "let unsub = store.subscribe(...)" + "unsub()"
    (lower.contains("unsubscribe") || lower.contains("unsub"))
        && (lower.contains("unsubscribe()")
            || lower.contains("unsub()")
            || lower.contains("unsubscribe?.()"))
}

/// Check for React useSyncExternalStore pattern (cleanup managed by React)
fn has_use_sync_external_store_pattern(content: &str) -> bool {
    content.contains("useSyncExternalStore")
}

fn check_subscription_leaks(content: &str, file: &str, issues: &mut Vec<MemoryLintIssue>) {
    // Check for Zustand-style cleanup first (stored function pattern)
    if has_zustand_unsubscribe_pattern(content) {
        return; // File has proper cleanup
    }

    // Check for useSyncExternalStore (React handles cleanup internally)
    if has_use_sync_external_store_pattern(content) {
        return; // React manages subscription lifecycle
    }

    let subscribe_count = SUBSCRIBE_REGEX.find_iter(content).count();
    let unsubscribe_count = UNSUBSCRIBE_REGEX.find_iter(content).count();

    // If there are more subscribes than unsubscribes, flag each unmatched subscribe
    if subscribe_count > unsubscribe_count {
        let unmatched = subscribe_count - unsubscribe_count;
        let mut found = 0;

        for (line_num, line) in content.lines().enumerate() {
            if found >= unmatched {
                break;
            }

            if SUBSCRIBE_REGEX.is_match(line) {
                // Check if subscription result is stored for later cleanup
                // Pattern: "const/let unsubscribe = " or "unsubscribe = "
                if line.contains("unsubscribe") || line.contains("unsub") {
                    continue; // Stored for cleanup
                }

                // Check if there's a corresponding unsubscribe nearby
                let context_start = line_num.saturating_sub(5);
                let context_end =
                    (line_num + SUBSCRIPTION_CONTEXT_WINDOW).min(content.lines().count());
                let context: String = content
                    .lines()
                    .skip(context_start)
                    .take(context_end - context_start)
                    .collect::<Vec<_>>()
                    .join("\n");

                // Skip if there's unsubscribe in context, or takeUntil pattern
                if UNSUBSCRIBE_REGEX.is_match(&context)
                    || context.contains("takeUntil")
                    || context.contains("take(1)")
                    || context.contains("first()")
                {
                    continue;
                }

                let col = line.find(".subscribe").unwrap_or(0) + 1;
                issues.push(MemoryLintIssue {
                    file: file.to_string(),
                    line: line_num + 1,
                    column: col,
                    rule: MemoryLintRule::SubscriptionLeak.as_str().to_string(),
                    severity: MemoryLintRule::SubscriptionLeak.severity().to_string(),
                    message: MemoryLintRule::SubscriptionLeak.message().to_string(),
                    suggestion: Some(MemoryLintRule::SubscriptionLeak.suggestion().to_string()),
                });
                found += 1;
            }
        }
    }
}

fn check_global_intervals(content: &str, file: &str, issues: &mut Vec<MemoryLintIssue>) {
    let interval_count = SET_INTERVAL_REGEX.find_iter(content).count();
    let clear_count = CLEAR_INTERVAL_REGEX.find_iter(content).count();

    if interval_count > clear_count {
        for (line_num, line) in content.lines().enumerate() {
            if SET_INTERVAL_REGEX.is_match(line) {
                // Check context for clearInterval
                let context_start = line_num.saturating_sub(5);
                let context_end = (line_num + INTERVAL_CONTEXT_WINDOW).min(content.lines().count());
                let context: String = content
                    .lines()
                    .skip(context_start)
                    .take(context_end - context_start)
                    .collect::<Vec<_>>()
                    .join("\n");

                if CLEAR_INTERVAL_REGEX.is_match(&context) {
                    continue;
                }

                let col = line.find("setInterval").unwrap_or(0) + 1;
                issues.push(MemoryLintIssue {
                    file: file.to_string(),
                    line: line_num + 1,
                    column: col,
                    rule: MemoryLintRule::GlobalInterval.as_str().to_string(),
                    severity: MemoryLintRule::GlobalInterval.severity().to_string(),
                    message: MemoryLintRule::GlobalInterval.message().to_string(),
                    suggestion: Some(MemoryLintRule::GlobalInterval.suggestion().to_string()),
                });
            }
        }
    }
}

fn check_global_event_listeners(content: &str, file: &str, issues: &mut Vec<MemoryLintIssue>) {
    let add_count = ADD_EVENT_LISTENER_REGEX.find_iter(content).count();
    let remove_count = REMOVE_EVENT_LISTENER_REGEX.find_iter(content).count();

    if add_count > remove_count {
        for (line_num, line) in content.lines().enumerate() {
            if ADD_EVENT_LISTENER_REGEX.is_match(line) {
                // Check context for removeEventListener
                let context_start = line_num.saturating_sub(5);
                let context_end =
                    (line_num + EVENT_LISTENER_CONTEXT_WINDOW).min(content.lines().count());
                let context: String = content
                    .lines()
                    .skip(context_start)
                    .take(context_end - context_start)
                    .collect::<Vec<_>>()
                    .join("\n");

                if REMOVE_EVENT_LISTENER_REGEX.is_match(&context) {
                    continue;
                }

                let col = line.find(".addEventListener").unwrap_or(0) + 1;
                issues.push(MemoryLintIssue {
                    file: file.to_string(),
                    line: line_num + 1,
                    column: col,
                    rule: MemoryLintRule::GlobalEventListener.as_str().to_string(),
                    severity: MemoryLintRule::GlobalEventListener.severity().to_string(),
                    message: MemoryLintRule::GlobalEventListener.message().to_string(),
                    suggestion: Some(MemoryLintRule::GlobalEventListener.suggestion().to_string()),
                });
            }
        }
    }
}

/// Calculate summary statistics from issues
pub fn calculate_summary(issues: &[MemoryLintIssue]) -> MemoryLintSummary {
    let mut by_severity: HashMap<String, usize> = HashMap::new();
    let mut by_rule: HashMap<String, usize> = HashMap::new();
    let mut files: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for issue in issues {
        *by_severity.entry(issue.severity.clone()).or_insert(0) += 1;
        *by_rule.entry(issue.rule.clone()).or_insert(0) += 1;
        files.insert(&issue.file);
    }

    MemoryLintSummary {
        total_issues: issues.len(),
        by_severity,
        by_rule,
        affected_files: files.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lint(code: &str) -> Vec<MemoryLintIssue> {
        lint_memory_file(&PathBuf::from("test.ts"), code)
    }

    fn lint_tsx(code: &str) -> Vec<MemoryLintIssue> {
        lint_memory_file(&PathBuf::from("test.tsx"), code)
    }

    #[test]
    fn test_module_cache_unbounded() {
        let code = r#"
const cache = new Map();

export function getData(key: string) {
    if (!cache.has(key)) {
        cache.set(key, fetchData(key));
    }
    return cache.get(key);
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "mem/module-cache-unbounded");
    }

    #[test]
    fn test_module_cache_with_lru_ok() {
        let code = r#"
import { LRUCache } from 'lru-cache';
const cache = new Map();  // Has LRU in file

export function getData(key: string) {
    return cache.get(key);
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_module_cache_with_delete_ok() {
        let code = r#"
const cache = new Map();

export function getData(key: string) {
    if (cache.size > 100) {
        cache.delete(cache.keys().next().value);
    }
    return cache.get(key);
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_subscription_leak() {
        let code = r#"
import { fromEvent } from 'rxjs';

export function setupListener() {
    fromEvent(document, 'click').subscribe(event => {
        console.log(event);
    });
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "mem/subscription-leak");
    }

    #[test]
    fn test_subscription_with_unsubscribe_ok() {
        let code = r#"
import { fromEvent, Subscription } from 'rxjs';

let sub: Subscription;

export function setupListener() {
    sub = fromEvent(document, 'click').subscribe(event => {
        console.log(event);
    });
}

export function cleanup() {
    sub.unsubscribe();
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_subscription_with_take_until_ok() {
        let code = r#"
import { fromEvent, Subject } from 'rxjs';
import { takeUntil } from 'rxjs/operators';

const destroy$ = new Subject();

export function setupListener() {
    fromEvent(document, 'click')
        .pipe(takeUntil(destroy$))
        .subscribe(event => console.log(event));
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_global_interval_in_ts() {
        let code = r#"
export function startPolling() {
    setInterval(() => {
        fetchData();
    }, 5000);
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "mem/global-interval");
    }

    #[test]
    fn test_global_interval_with_clear_ok() {
        let code = r#"
let intervalId: number;

export function startPolling() {
    intervalId = setInterval(() => {
        fetchData();
    }, 5000);
}

export function stopPolling() {
    clearInterval(intervalId);
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_global_event_listener_in_ts() {
        let code = r#"
export function init() {
    window.addEventListener('resize', handleResize);
}

function handleResize() {
    console.log('resized');
}
"#;
        let issues = lint(code);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, "mem/global-event-listener");
    }

    #[test]
    fn test_skip_test_files() {
        let code = r#"
const cache = new Map();
setInterval(() => {}, 1000);
"#;
        let issues = lint_memory_file(&PathBuf::from("test.test.ts"), code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_tsx_with_use_effect_skips_interval() {
        // In React files with useEffect, intervals are handled by react_lint
        let code = r#"
import { useEffect } from 'react';

export function Component() {
    useEffect(() => {
        setInterval(() => tick(), 1000);
    }, []);

    return <div>Hello</div>;
}
"#;
        let issues = lint_tsx(code);
        // Should not flag - react_lint handles this
        assert!(issues.iter().all(|i| i.rule != "mem/global-interval"));
    }

    #[test]
    fn test_summary_calculation() {
        let issues = vec![
            MemoryLintIssue {
                file: "a.ts".to_string(),
                line: 1,
                column: 1,
                rule: "mem/subscription-leak".to_string(),
                severity: "high".to_string(),
                message: "test".to_string(),
                suggestion: None,
            },
            MemoryLintIssue {
                file: "b.ts".to_string(),
                line: 1,
                column: 1,
                rule: "mem/module-cache-unbounded".to_string(),
                severity: "medium".to_string(),
                message: "test".to_string(),
                suggestion: None,
            },
        ];

        let summary = calculate_summary(&issues);
        assert_eq!(summary.total_issues, 2);
        assert_eq!(summary.affected_files, 2);
        assert_eq!(summary.by_severity.get("high"), Some(&1));
        assert_eq!(summary.by_severity.get("medium"), Some(&1));
    }

    #[test]
    fn test_skip_service_worker() {
        let code = r#"
self.addEventListener('install', handleInstall);
self.addEventListener('fetch', handleFetch);
self.addEventListener('activate', handleActivate);
"#;
        // Service workers are intentionally long-lived
        let issues = lint_memory_file(&PathBuf::from("public/sw.js"), code);
        assert_eq!(issues.len(), 0);
    }

    #[test]
    fn test_zustand_style_unsubscribe_ok() {
        // Zustand returns a function, not an object with .unsubscribe()
        let code = r#"
let unsubscribeStore: (() => void) | null = null;

export function setupSync() {
    if (unsubscribeStore) {
        unsubscribeStore();
        unsubscribeStore = null;
    }
    unsubscribeStore = voiceStore.subscribe(() => {
        syncState();
    });
}

export function cleanup() {
    if (unsubscribeStore) {
        unsubscribeStore();
    }
}
"#;
        let issues = lint(code);
        assert!(issues.iter().all(|i| i.rule != "mem/subscription-leak"));
    }

    #[test]
    fn test_use_sync_external_store_ok() {
        // useSyncExternalStore handles cleanup internally
        let code = r#"
import { useSyncExternalStore, useCallback } from 'react';

export const useProfileSnapshot = () => {
    const store = useProfileStore();
    const subscribe = useCallback(
        (listener: () => void) => store.subscribe(listener),
        [store]
    );
    return useSyncExternalStore(subscribe, store.getSnapshot, store.getSnapshot);
};
"#;
        let issues = lint_tsx(code);
        assert!(issues.iter().all(|i| i.rule != "mem/subscription-leak"));
    }
}
