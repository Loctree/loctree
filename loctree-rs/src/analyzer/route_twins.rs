//! Route-level twins detection.
//!
//! Distinct from symbol-level twins (`analyzer/twins.rs`), this module flags
//! HTTP route registrations that share the same `(framework, method, path)`
//! triple across multiple files. The runtime-contract drift surface is a real
//! parallel-implementation-rot signal: if two handlers register `POST /api/stt`
//! and one is patched while the other isn't, agents only fix half the surface
//! and bug remains live behind whichever registration loses the race.
//!
//! Source hak: 2026-05-18 Screenscribe HAK 6 — `loct twins` reported `0 twin
//! groups` while two FastAPI handlers registered the same `POST /api/stt` in
//! `analyze_server.py:2099` and `review_server.py:276`. Filter the noise out
//! of the twin signal so route collisions are visible at the `loct twins`
//! / `loct findings` surface, not buried in a separate `loct routes` flag.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders (c)2024-2026 LibraxisAI

use crate::types::{FileAnalysis, RouteInfo};
use serde::Serialize;
use std::collections::BTreeMap;

/// Severity classification for a route twin group.
///
/// `High` means both registrations live in the same source file and so will
/// trip a last-registration-wins collision in the same process. `Medium` means
/// distinct files (more often a parallel-app or sub-router pattern) but still
/// worth surfacing because patches won't propagate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteTwinSeverity {
    High,
    Medium,
}

/// One registration that participates in a route-twin group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RouteTwinLocation {
    pub file: String,
    pub line: usize,
    /// Handler name if extracted (`def transcribe_voice():` → `"transcribe_voice"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
}

/// A group of route registrations sharing `(framework, method, path)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RouteTwin {
    pub framework: String,
    pub method: String,
    pub path: String,
    pub locations: Vec<RouteTwinLocation>,
    pub severity: RouteTwinSeverity,
}

impl RouteTwin {
    pub fn count(&self) -> usize {
        self.locations.len()
    }
}

/// Detect route twins across all files in a snapshot view.
///
/// Routes with no `path` (decorator without literal — e.g. `@app.route` used as
/// a method decorator without an argument) are skipped because the collision
/// key is undefined.
pub fn detect_route_twins(files: &[FileAnalysis]) -> Vec<RouteTwin> {
    let mut groups: BTreeMap<(String, String, String), Vec<RouteTwinLocation>> = BTreeMap::new();

    for file in files {
        for route in &file.routes {
            let Some(path) = route_path(route) else {
                continue;
            };
            let key = (
                route.framework.clone(),
                normalized_method(&route.method),
                path,
            );
            groups.entry(key).or_default().push(RouteTwinLocation {
                file: file.path.clone(),
                line: route.line,
                handler: route.name.clone(),
            });
        }
    }

    let mut twins: Vec<RouteTwin> = groups
        .into_iter()
        .filter(|(_, locations)| locations.len() > 1)
        .map(|((framework, method, path), locations)| {
            let severity = route_severity(&locations);
            RouteTwin {
                framework,
                method,
                path,
                locations,
                severity,
            }
        })
        .collect();

    // Stable sort: severity (High first), then path lex order.
    twins.sort_by(|a, b| {
        severity_rank(b.severity)
            .cmp(&severity_rank(a.severity))
            .then_with(|| a.framework.cmp(&b.framework))
            .then_with(|| a.method.cmp(&b.method))
            .then_with(|| a.path.cmp(&b.path))
    });

    twins
}

fn route_path(route: &RouteInfo) -> Option<String> {
    route
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

fn normalized_method(method: &str) -> String {
    method.trim().to_ascii_uppercase()
}

fn route_severity(locations: &[RouteTwinLocation]) -> RouteTwinSeverity {
    let mut seen = std::collections::HashSet::new();
    for loc in locations {
        if !seen.insert(loc.file.as_str()) {
            return RouteTwinSeverity::High; // same file repeats path
        }
    }
    if locations.len() >= 2 && seen.len() == 1 {
        RouteTwinSeverity::High
    } else {
        RouteTwinSeverity::Medium
    }
}

fn severity_rank(s: RouteTwinSeverity) -> u8 {
    match s {
        RouteTwinSeverity::High => 2,
        RouteTwinSeverity::Medium => 1,
    }
}

/// Print route twins in human-readable format. Skips silently if empty so
/// `loct twins` does not spam "No route twins" on every clean repo.
pub fn print_route_twins_human(twins: &[RouteTwin]) {
    if twins.is_empty() {
        return;
    }

    println!("ROUTE TWINS ({} groups)", twins.len());
    println!();
    for twin in twins {
        println!(
            "  [{}] {} {} ({} registrations)",
            severity_label(twin.severity),
            twin.method,
            twin.path,
            twin.count()
        );
        println!("    framework: {}", twin.framework);
        for loc in &twin.locations {
            match &loc.handler {
                Some(h) => println!("    ├─ {}:{} ({})", loc.file, loc.line, h),
                None => println!("    ├─ {}:{}", loc.file, loc.line),
            }
        }
        println!(
            "    └─ runtime contract drift risk — patches on one registration won't propagate"
        );
        println!();
    }
}

fn severity_label(s: RouteTwinSeverity) -> &'static str {
    match s {
        RouteTwinSeverity::High => "HIGH",
        RouteTwinSeverity::Medium => "MEDIUM",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(
        framework: &str,
        method: &str,
        path: Option<&str>,
        name: Option<&str>,
        line: usize,
    ) -> RouteInfo {
        RouteInfo {
            framework: framework.to_string(),
            method: method.to_string(),
            path: path.map(str::to_string),
            name: name.map(str::to_string),
            line,
        }
    }

    fn file_with_routes(path: &str, routes: Vec<RouteInfo>) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            routes,
            ..FileAnalysis::default()
        }
    }

    /// Source hak: 2026-05-18 Screenscribe HAK 6 — two FastAPI handlers
    /// registered the same `POST /api/stt`; existing twin detector reported
    /// `0 twin groups`. Group must surface with severity Medium (distinct
    /// files) and both registration locations.
    #[test]
    fn detect_route_twins_groups_duplicate_paths() {
        let files = vec![
            file_with_routes(
                "screenscribe/analyze_server.py",
                vec![route(
                    "fastapi",
                    "POST",
                    Some("/api/stt"),
                    Some("transcribe_voice"),
                    2099,
                )],
            ),
            file_with_routes(
                "screenscribe/review_server.py",
                vec![route(
                    "fastapi",
                    "POST",
                    Some("/api/stt"),
                    Some("transcribe_voice"),
                    276,
                )],
            ),
        ];

        let twins = detect_route_twins(&files);

        assert_eq!(twins.len(), 1, "exactly one route twin group expected");
        let twin = &twins[0];
        assert_eq!(twin.framework, "fastapi");
        assert_eq!(twin.method, "POST");
        assert_eq!(twin.path, "/api/stt");
        assert_eq!(twin.count(), 2);
        assert_eq!(twin.severity, RouteTwinSeverity::Medium);

        let files_in_twin: Vec<_> = twin.locations.iter().map(|l| l.file.as_str()).collect();
        assert!(files_in_twin.contains(&"screenscribe/analyze_server.py"));
        assert!(files_in_twin.contains(&"screenscribe/review_server.py"));
    }

    /// Unique registrations must NOT trigger a twin group — that would
    /// re-introduce the noise we removed from coverage gaps.
    #[test]
    fn detect_route_twins_skips_unique_paths() {
        let files = vec![
            file_with_routes(
                "screenscribe/analyze_server.py",
                vec![
                    route(
                        "fastapi",
                        "POST",
                        Some("/api/stt"),
                        Some("transcribe_voice"),
                        2099,
                    ),
                    route("fastapi", "GET", Some("/api/health"), Some("health"), 1),
                ],
            ),
            file_with_routes(
                "screenscribe/cli.py",
                vec![route(
                    "fastapi",
                    "POST",
                    Some("/api/report"),
                    Some("generate_report"),
                    50,
                )],
            ),
        ];

        let twins = detect_route_twins(&files);
        assert!(twins.is_empty(), "no twins expected, got: {:?}", twins);
    }

    /// Two registrations in the same file = last-registration-wins runtime
    /// collision. Severity must escalate to High.
    #[test]
    fn detect_route_twins_high_severity_same_file() {
        let files = vec![file_with_routes(
            "screenscribe/analyze_server.py",
            vec![
                route(
                    "fastapi",
                    "POST",
                    Some("/api/stt"),
                    Some("transcribe_voice"),
                    100,
                ),
                route(
                    "fastapi",
                    "POST",
                    Some("/api/stt"),
                    Some("transcribe_voice_v2"),
                    250,
                ),
            ],
        )];

        let twins = detect_route_twins(&files);
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].severity, RouteTwinSeverity::High);
    }

    /// Method case differences (`POST` vs `post`) must collapse to one group;
    /// HTTP methods are case-insensitive.
    #[test]
    fn detect_route_twins_normalizes_method_case() {
        let files = vec![
            file_with_routes(
                "a.py",
                vec![route("fastapi", "POST", Some("/x"), Some("a"), 1)],
            ),
            file_with_routes(
                "b.py",
                vec![route("fastapi", "post", Some("/x"), Some("b"), 1)],
            ),
        ];

        let twins = detect_route_twins(&files);
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].method, "POST");
    }

    /// Routes with `path: None` (e.g. decorator detected but path argument
    /// not extracted) cannot participate in collision keying. Skip silently
    /// — they're not lying, just under-extracted.
    #[test]
    fn detect_route_twins_skips_routes_without_path() {
        let files = vec![
            file_with_routes("a.py", vec![route("fastapi", "POST", None, None, 1)]),
            file_with_routes("b.py", vec![route("fastapi", "POST", None, None, 1)]),
        ];

        let twins = detect_route_twins(&files);
        assert!(twins.is_empty());
    }
}
