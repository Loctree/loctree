//! Help text generation for CLI commands.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use super::help_texts::*;
use super::types::Command;

struct CommandTopic {
    name: &'static str,
    aliases: &'static [&'static str],
    text: &'static str,
}

// Shared vocabulary for syntax recognition, suggestions, topic help and the full reference.
const COMMAND_TOPICS: &[CommandTopic] = &[
    CommandTopic {
        name: "auto",
        aliases: &[],
        text: AUTO_HELP,
    },
    CommandTopic {
        name: "agent",
        aliases: &[],
        text: AGENT_HELP,
    },
    CommandTopic {
        name: "scan",
        aliases: &[],
        text: SCAN_HELP,
    },
    CommandTopic {
        name: "watch",
        aliases: &[],
        text: WATCH_HELP,
    },
    CommandTopic {
        name: "tree",
        aliases: &[],
        text: TREE_HELP,
    },
    CommandTopic {
        name: "slice",
        aliases: &["s"],
        text: SLICE_HELP,
    },
    CommandTopic {
        name: "context",
        aliases: &[],
        text: CONTEXT_HELP,
    },
    CommandTopic {
        name: "repo-view",
        aliases: &[],
        text: REPO_VIEW_HELP,
    },
    CommandTopic {
        name: "find",
        aliases: &["f"],
        text: FIND_HELP,
    },
    CommandTopic {
        name: "occurrences",
        aliases: &[],
        text: OCCURRENCES_HELP,
    },
    CommandTopic {
        name: "findings",
        aliases: &[],
        text: FINDINGS_HELP,
    },
    CommandTopic {
        name: "dead",
        aliases: &["d", "unused"],
        text: DEAD_HELP,
    },
    CommandTopic {
        name: "cycles",
        aliases: &["c"],
        text: CYCLES_HELP,
    },
    CommandTopic {
        name: "trace",
        aliases: &[],
        text: TRACE_HELP,
    },
    CommandTopic {
        name: "commands",
        aliases: &[],
        text: COMMANDS_HELP,
    },
    CommandTopic {
        name: "events",
        aliases: &[],
        text: EVENTS_HELP,
    },
    CommandTopic {
        name: "pipelines",
        aliases: &[],
        text: PIPELINES_HELP,
    },
    CommandTopic {
        name: "insights",
        aliases: &[],
        text: INSIGHTS_HELP,
    },
    CommandTopic {
        name: "manifests",
        aliases: &[],
        text: MANIFESTS_HELP,
    },
    CommandTopic {
        name: "info",
        aliases: &[],
        text: INFO_HELP,
    },
    CommandTopic {
        name: "anchors",
        aliases: &[],
        text: ANCHORS_HELP,
    },
    CommandTopic {
        name: "lint",
        aliases: &[],
        text: LINT_HELP,
    },
    CommandTopic {
        name: "report",
        aliases: &[],
        text: REPORT_HELP,
    },
    CommandTopic {
        name: "prism",
        aliases: &[],
        text: PRISM_HELP,
    },
    CommandTopic {
        name: "help",
        aliases: &[],
        text: HELP_HELP,
    },
    CommandTopic {
        name: "query",
        aliases: &["q"],
        text: QUERY_HELP,
    },
    CommandTopic {
        name: "body",
        aliases: &[],
        text: BODY_HELP,
    },
    CommandTopic {
        name: "diff",
        aliases: &[],
        text: DIFF_HELP,
    },
    CommandTopic {
        name: "history",
        aliases: &[],
        text: HISTORY_HELP,
    },
    CommandTopic {
        name: "crowd",
        aliases: &[],
        text: CROWD_HELP,
    },
    CommandTopic {
        name: "tagmap",
        aliases: &[],
        text: TAGMAP_HELP,
    },
    CommandTopic {
        name: "twins",
        aliases: &["t"],
        text: TWINS_HELP,
    },
    CommandTopic {
        name: "suppress",
        aliases: &[],
        text: SUPPRESS_HELP,
    },
    CommandTopic {
        name: "suppressions",
        aliases: &[],
        text: SUPPRESSIONS_HELP,
    },
    CommandTopic {
        name: "routes",
        aliases: &[],
        text: ROUTES_HELP,
    },
    CommandTopic {
        name: "dist",
        aliases: &[],
        text: DIST_HELP,
    },
    CommandTopic {
        name: "coverage",
        aliases: &[],
        text: COVERAGE_HELP,
    },
    CommandTopic {
        name: "impact",
        aliases: &["i"],
        text: IMPACT_HELP,
    },
    CommandTopic {
        name: "focus",
        aliases: &[],
        text: FOCUS_HELP,
    },
    CommandTopic {
        name: "hotspots",
        aliases: &[],
        text: HOTSPOTS_HELP,
    },
    CommandTopic {
        name: "follow",
        aliases: &[],
        text: FOLLOW_HELP,
    },
    CommandTopic {
        name: "layoutmap",
        aliases: &[],
        text: LAYOUTMAP_HELP,
    },
    CommandTopic {
        name: "health",
        aliases: &["h"],
        text: HEALTH_HELP,
    },
    CommandTopic {
        name: "audit",
        aliases: &[],
        text: AUDIT_HELP,
    },
    CommandTopic {
        name: "doctor",
        aliases: &[],
        text: DOCTOR_HELP,
    },
    CommandTopic {
        name: "env-truth",
        aliases: &["envtruth"],
        text: ENV_TRUTH_HELP,
    },
    CommandTopic {
        name: "plan",
        aliases: &["p"],
        text: PLAN_HELP,
    },
    CommandTopic {
        name: "cache",
        aliases: &[],
        text: CACHE_HELP,
    },
    CommandTopic {
        name: "prune-old-artifacts",
        aliases: &[],
        text: PRUNE_OLD_ARTIFACTS_HELP,
    },
    CommandTopic {
        name: "snapshot-path",
        aliases: &[],
        text: SNAPSHOT_PATH_HELP,
    },
    CommandTopic {
        name: "inventory",
        aliases: &[],
        text: INVENTORY_HELP,
    },
    CommandTopic {
        name: "atlas",
        aliases: &[],
        text: ATLAS_HELP,
    },
];

/// Legacy-path usage banner. Version is compile-time so it cannot drift from Cargo.toml.
pub const LOCT_USAGE: &str = concat!(
    "loct - Static Analysis for AI Agents (v",
    env!("CARGO_PKG_VERSION"),
    ")\n\n\
PHILOSOPHY: Scan the WHOLE repo once with `loct auto`, then query with subcommands.\n\
            Artifacts live in your cache dir by default (override with LOCT_CACHE_DIR).\n\n\
Quick Start:\n  \
  loct auto                      Full scan → cached artifacts\n  \
  loct slice src/foo.ts          Extract file context for AI agent\n  \
  loct report --html out.html    Generate visual HTML report\n\n\
Core Commands:\n  \
  auto              Full scan + findings (writes artifacts)\n  \
  doctor            Interactive diagnostics and quick-wins\n  \
  findings          Emit canonical findings JSON\n  \
  slice <file>      Extract file + dependencies + consumers\n  \
  find <name>       Find symbol definitions\n  \
  trace <handler>   Debug Tauri handler pipeline\n  \
  --for-ai          AI-optimized project summary (JSON)\n\n\
Analysis:\n  \
  dead              Find unused exports (dead code)\n  \
  cycles            Find circular imports\n  \
  twins             Find duplicate symbol names\n  \
  health            Quick structural health check\n  \
  crowds            Find hub files (high import/export counts)\n\n\
Output:\n  \
  findings          Full findings JSON / summary JSON\n  \
  report            Generate HTML/JSON/SARIF reports\n  \
  --json            Machine-readable output\n  \
  --sarif           SARIF for GitHub Code Scanning\n\n\
Common:\n  \
  -g, --gitignore   Respect .gitignore\n  \
  --verbose         Detailed progress\n  \
  --help-full       Complete command reference\n\n\
Examples:\n  \
  loct auto                                  # Full analysis\n  \
  loct slice src/main.rs --consumers         # Context for AI\n  \
  loct findings --summary | jq '.health_score' # CI summary JSON\n  \
  loct dead --confidence high                # Find dead code\n  \
  loct report --html out.html --serve        # Interactive report\n  \
  loct doctor                                # Interactive fixes\n\n\
Tip: Run `loct auto` from repo root first, then query!\n\n\
More: loct --help-full\n"
);

impl Command {
    pub(crate) fn subcommand_names() -> impl Iterator<Item = &'static str> {
        COMMAND_TOPICS
            .iter()
            .flat_map(|topic| std::iter::once(topic.name).chain(topic.aliases.iter().copied()))
    }

    pub(crate) fn canonical_subcommand(name: &str) -> Option<&'static str> {
        COMMAND_TOPICS
            .iter()
            .find(|topic| topic.name == name || topic.aliases.contains(&name))
            .map(|topic| topic.name)
    }

    /// Hard retirement messages for command shells kept only for migration.
    pub fn retired_command_message(command: &str) -> Option<&'static str> {
        match command {
            "sniff" => Some(
                "loct sniff has been retired.\nUse `loct findings` for the canonical findings pipeline.\n",
            ),
            "zombie" => Some(
                "loct zombie has been retired.\nUse `loct findings` for the canonical findings pipeline.\n",
            ),
            _ => None,
        }
    }

    /// Get the command name as a string.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Auto(_) => "auto",
            Command::Scan(_) => "scan",
            Command::Watch(_) => "watch",
            Command::Tree(_) => "tree",
            Command::Slice(_) => "slice",
            Command::Context(_) => "context",
            Command::RepoView(_) => "repo-view",
            Command::Find(_) => "find",
            Command::Occurrences(_) => "occurrences",
            Command::Dead(_) => "dead",
            Command::Cycles(_) => "cycles",
            Command::Trace(_) => "trace",
            Command::Commands(_) => "commands",
            Command::Routes(_) => "routes",
            Command::Events(_) => "events",
            Command::Pipelines(_) => "pipelines",
            Command::Insights(_) => "insights",
            Command::Manifests(_) => "manifests",
            Command::Info(_) => "info",
            Command::Anchors(_) => "anchors",
            Command::Lint(_) => "lint",
            Command::Report(_) => "report",
            Command::Prism(_) => "prism",
            Command::Findings(_) => "findings",
            Command::Help(_) => "help",
            Command::Version => "version",
            Command::Query(_) => "query",
            Command::Body(_) => "body",
            Command::Diff(_) => "diff",
            Command::History(_) => "history",
            Command::Crowd(_) => "crowd",
            Command::Tagmap(_) => "tagmap",
            Command::Twins(_) => "twins",
            Command::Suppress(_) => "suppress",
            Command::Suppressions(_) => "suppressions",
            Command::Dist(_) => "dist",
            Command::Coverage(_) => "coverage",
            Command::Sniff(_) => "sniff",
            Command::JqQuery(_) => "jq",
            Command::Impact(_) => "impact",
            Command::Focus(_) => "focus",
            Command::Hotspots(_) => "hotspots",
            Command::Follow(_) => "follow",
            Command::Layoutmap(_) => "layoutmap",
            Command::Zombie(_) => "zombie",
            Command::Health(_) => "health",
            Command::Audit(_) => "audit",
            Command::Doctor(_) => "doctor",
            Command::Plan(_) => "plan",
            Command::Cache(_) => "cache",
            Command::EnvTruth(_) => "env-truth",
            Command::PruneOldArtifacts(_) => "prune-old-artifacts",
            Command::SnapshotPath(_) => "snapshot-path",
            Command::Inventory(_) => "inventory",
            Command::Atlas(_) => "atlas",
        }
    }

    /// Get a short description of the command.
    pub fn description(&self) -> &'static str {
        match self {
            Command::Auto(_) => "Full auto-scan with stack detection (default)",
            Command::Scan(_) => "Build/update snapshot for current HEAD (supports --watch)",
            Command::Watch(_) => {
                "Single-instance watch loop (foreground/background/LSP co-process)"
            }
            Command::Tree(_) => "Display LOC tree / structural overview",
            Command::Slice(_) => "Extract holographic context for a file",
            Command::Context(_) => "Emit an agent-ready ContextPack",
            Command::RepoView(_) => "Repository overview for AI agents",
            Command::Find(_) => "Exact literal search; broad discovery with --discover",
            Command::Occurrences(_) => "Literal exact-identifier scan (truth layer, no fuzz)",
            Command::Dead(_) => "Detect unused exports / dead code",
            Command::Cycles(_) => "Detect circular imports",
            Command::Trace(_) => "Trace a Tauri/IPC handler end-to-end",
            Command::Commands(_) => "Show Tauri command bridges (FE <-> BE)",
            Command::Events(_) => "Show event flow and issues",
            Command::Pipelines(_) => "Show pipeline summary (events/commands/risks)",
            Command::Insights(_) => "Show AI insights summary",
            Command::Manifests(_) => "Show manifest summaries (package.json/Cargo.toml)",
            Command::Info(_) => "Show snapshot metadata and project info",
            Command::Anchors(_) => "Emit the deterministic loctree.anchors.v1 catalog",
            Command::Lint(_) => "Structural lint/policy checks",
            Command::Report(_) => "Generate HTML report + cached artifacts",
            Command::Prism(_) => {
                "Compare context packs across task framings and score conceptual smear"
            }
            Command::Findings(_) => "Emit the canonical findings artifact for agents and CI",
            Command::Help(_) => "Show help for commands",
            Command::Version => "Show version information",
            Command::Query(_) => "Query snapshot data (who-imports, where-symbol, component-of)",
            Command::Body(_) => "Show bounded source body/range of a symbol (no grep)",
            Command::Diff(_) => "Compare snapshots and show semantic delta",
            Command::History(_) => "Browse Git history and structural comparison entry points",
            Command::Crowd(_) => "Detect functional crowds (similar files clustering)",
            Command::Tagmap(_) => "Unified search: files + crowd + dead around a keyword",
            Command::Twins(_) => "Show symbol registry and dead parrots (0 imports)",
            Command::Suppress(_) => "Manage false positive suppressions",
            Command::Suppressions(_) => {
                "Source-side silencer inventory (allow/nosemgrep/ts-ignore/noqa/unsafe...) — literal-only"
            }
            Command::Routes(_) => "List Python FastAPI/Flask and narrow Rust Axum routes",
            Command::Dist(_) => "Verify tree-shaking from production source maps",
            Command::Coverage(_) => "Analyze test coverage gaps (structural coverage)",
            Command::Sniff(_) => "Sniff for code smells (twins + dead parrots + crowds)",
            Command::JqQuery(_) => "Query snapshot with jq-style filters (loct '.filter')",
            Command::Impact(_) => "Analyze impact of modifying/removing a file",
            Command::Focus(_) => "Extract holographic context for a directory",
            Command::Hotspots(_) => "Show static dependency degree by file",
            Command::Follow(_) => "Pursue structural signals from one unified surface",
            Command::Layoutmap(_) => "Analyze CSS layout (z-index, position, grid/flex)",
            Command::Zombie(_) => "Find zombie code (dead exports + orphan files + shadows)",
            Command::Health(_) => {
                "Quick health check; aggregates dead/twins/cycles — no additional detectors"
            }
            Command::Audit(_) => {
                "Audit report; aggregates dead/twins/cycles — no additional detectors"
            }
            Command::Doctor(_) => "Cache identity and snapshot scope diagnostics",
            Command::Plan(_) => {
                "Experimental path-heuristic refactoring preview (not proof of safety)"
            }
            Command::Cache(_) => "Manage snapshot cache (list, clean)",
            Command::EnvTruth(_) => {
                "Audit env declaration sources for resolution-order drift (Cut 8 / Lane 4)"
            }
            Command::PruneOldArtifacts(_) => {
                "Prune old per-branch snapshot artifacts from local `.loctree/` dirs"
            }
            Command::SnapshotPath(_) => {
                "Resolve snapshot.json path + sibling organ artifacts (no body dump)"
            }
            Command::Inventory(_) => "Stream compact file inventory as JSONL with coverage receipt",
            Command::Atlas(_) => {
                "Materialize loctree.repo-atlas.v1 pointer pack (sense/inventory/signals)"
            }
        }
    }

    /// Generate the main help text listing the core commands.
    pub fn format_help() -> String {
        let mut help = String::new();
        help.push_str(&format!(
            "loctree {} - codebase map for agents and humans\n\n",
            crate::BUILD_VERSION
        ));

        help.push_str("POWER PATH:\n");
        help.push_str("  Map:        loct context --task \"fix auth\" --file src/auth.ts\n");
        help.push_str("              loct focus src/cli/     loct hotspots     loct tree --files --match 'help|cli'\n");
        help.push_str("  Search:     loct find Auth          loct find --discover Auth\n");
        help.push_str("              loct occurrences Auth   loct tagmap auth   loct query where-symbol Auth\n");
        help.push_str("  Understand: loct body Auth          loct slice src/auth.ts\n");
        help.push_str("              loct impact src/auth.ts loct follow all\n");
        help.push_str("  Trust:      loct doctor             loct env-truth\n");
        help.push_str(
            "              loct findings --summary loct suppressions  loct diff --since HEAD~1\n",
        );
        help.push_str("  Compare:    loct prism --task \"auth\" --task \"auth api\"\n");
        help.push_str("  Decide:     use prism/follow/findings output to choose the next cut\n\n");

        help.push_str("CAPABILITIES:\n");
        help.push_str("  JS/TS AST:   ast_js powered by oxc parser\n");
        help.push_str("  C-family:   tree-sitter C-family support for Swift/ObjC/C/C++\n");
        help.push_str("  MCP HTTP:   loct watch --http starts streamable-http MCP on /mcp\n");
        help.push_str("  Watch lock: per-root watch lock prevents duplicate scanners\n\n");

        help.push_str("CORE COMMANDS:\n");
        help.push_str("  loct                  Scan repo and cache the map\n");
        help.push_str("  loct context          Build the agent-ready context pack\n");
        help.push_str("  loct focus <dir>      Map a directory before editing\n");
        help.push_str("  loct slice <file>     Show file deps + consumers by default\n");
        help.push_str("  loct impact <file>    Show what changes if this file moves\n");
        help.push_str("  loct find <identifier> Exact literal truth (default)\n");
        help.push_str("  loct find --discover X Broad AST/parameter/fuzzy discovery\n");
        help.push_str("  loct tagmap <keyword> Unified keyword map: files + crowd + dead\n");
        help.push_str("  loct occurrences ID   Literal exact-identifier truth scan\n");
        help.push_str("  loct body <symbol>    Show bounded source body/range\n");
        help.push_str("  loct follow all       Pursue dead/cycles/twins/hotspots\n");
        help.push_str(
            "  loct health           Aggregates dead/twins/cycles — no additional detectors\n",
        );
        help.push_str("  loct report           Write the full report\n\n");
        help.push_str("  loct anchors --format json Emit stable file and symbol anchors\n\n");

        help.push_str("EXAMPLES:\n");
        help.push_str("  loct context --task \"fix auth\"      Build context for an agent\n");
        help.push_str("  loct query where-symbol handle_auth Find precise definition sites\n");
        help.push_str("  loct tree --files --match 'test|api' List matching files from the map\n");
        help.push_str("  loct doctor --cache --scope          Verify cache and snapshot scope\n\n");

        help.push_str("OUTPUT MODES:\n");
        help.push_str("  default              Human-readable summary with scope and counts\n");
        help.push_str("  --json               Machine-readable stdout\n");
        help.push_str("  --verbose            More progress and supporting detail\n");
        help.push_str("  --quiet              Only essential output\n\n");

        help.push_str("MORE:\n");
        help.push_str("  loct <cmd> --help    Help for one command\n");
        help.push_str("  loct --help-full     Full power command reference\n");
        help.push_str("  loct --help-legacy   Deprecated flag migration\n");

        help
    }

    /// Generate help text for a specific subcommand (new CLI).
    pub fn format_command_help(command: &str) -> Option<&'static str> {
        if command == "jq" {
            return Some(JQ_HELP);
        }
        let canonical = Self::canonical_subcommand(command)?;
        COMMAND_TOPICS
            .iter()
            .find(|topic| topic.name == canonical)
            .map(|topic| topic.text)
    }

    /// Generate the full help text with ALL commands (auto-generated).
    /// This replaces the hardcoded format_usage_full() in loct.rs.
    pub fn format_help_full() -> String {
        let mut help = String::new();
        help.push_str(&format!(
            "loctree {} - AI-oriented codebase analyzer (Full Reference)\n\n",
            crate::BUILD_VERSION
        ));

        help.push_str("PHILOSOPHY: Scan once, query everything.\n");
        help.push_str(
            "            Run `loct` to create artifacts, then query with subcommands.\n\n",
        );

        help.push_str("=== START WITH YOUR QUESTION ===\n\n");
        help.push_str("  Where is this name used?       loct find GitContext\n");
        help.push_str("  What does its file connect to? loct slice loctree-rs/src/git.rs\n");
        help.push_str("  What would this change affect? loct impact loctree-rs/src/git.rs\n");
        help.push_str("  How does this module fit?      loct focus loctree-rs/src/cli/\n");
        help.push_str("  Where does this domain live?   loct tagmap snapshot --limit 10\n");
        help.push_str("  Which structural risks exist?  loct follow all --limit 10\n");
        help.push_str(
            "  Where are the HTTP endpoints?  loct routes  # FastAPI/Flask; narrow Axum shapes\n",
        );
        help.push_str("  What changed since a commit?   loct diff --since HEAD~1\n\n");
        help.push_str("  Start with the name you know. Read its definitions, enclosing symbols,\n");
        help.push_str("  and consumers; use slice/focus/impact to follow the relationships.\n");
        help.push_str("  Inspect implementation with body. Read <command> --help for scope.\n");
        help.push_str("  follow trace is a Tauri command bridge, not a general call graph.\n\n");

        help.push_str("=== LIVE COMMANDS ===\n\n");
        for topic in COMMAND_TOPICS {
            let description = topic
                .text
                .lines()
                .next()
                .unwrap_or_default()
                .split_once(" - ")
                .map(|(_, description)| description)
                .unwrap_or_default();
            help.push_str(&format!("  loct {:<20} {}\n", topic.name, description));
            for alias in topic.aliases {
                help.push_str(&format!(
                    "  loct {:<20} Alias for loct {}\n",
                    alias, topic.name
                ));
            }
        }
        help.push('\n');
        help.push_str("  loct find --discover X    Broad AST/parameter/fuzzy discovery\n\n");
        help.push_str("=== LIMITS AND MIGRATION ===\n\n");
        help.push_str("  hotspots: Static indexed snapshot degree; all edge kinds, distinct known-file (from,to) pairs.\n");
        help.push_str("            In-degree = source neighbors; out-degree = target neighbors.\n");
        help.push_str(
            "            CORE/SHARED/PERIPHERAL/LEAF are degree bands, not architectural roles.\n",
        );
        help.push_str("            High degree suggests impact/follow inspection; zero/low-degree external or runtime entry points can matter.\n");
        help.push_str("            No runtime call counts, hot paths, performance scores, dead-code proof or refactoring safety certificate.\n");
        help.push_str("            --min default: 0; --limit default: 50; --leaves selects zero and overrides --min.\n");
        help.push_str("  occurrences: Literal inclusion does not depend on symbol extraction.\n");
        help.push_str("               Role metadata may use source AST and snapshot anchors.\n");
        help.push_str("               No fuzzy suggestions are promoted as primary results.\n");
        help.push_str(
            "  routes: Python FastAPI/Flask decorators; Rust Axum .route(path, method(handler)).\n",
        );
        help.push_str(
            "          Axum method-chain registrations are partial (first method only).\n",
        );
        help.push_str("          TypeScript routing frameworks are unsupported; zero is not proof of absence.\n");
        help.push_str(
            "  plan: experimental path-heuristic preview, not proof of safe refactoring.\n",
        );
        help.push_str(
            "        Generated destinations can collide. Do not execute generated scripts as-is.\n",
        );
        help.push_str("  sniff, zombie: retired; use `loct findings`. Not live analyzers; help exits nonzero.\n");
        help.push_str(
            "  jq: help-only topic (`loct help jq`); execute quoted filters, not `loct jq`.\n",
        );
        help.push_str("  version: use `loct --version`; not a positional command.\n\n");

        // === CAPABILITIES ===
        help.push_str("=== CAPABILITIES ===\n\n");
        help.push_str("  ast_js / oxc             JS/TS AST extraction powered by oxc\n");
        help.push_str(
            "  tree-sitter C-family     Swift, Objective-C, C, and C++ structure support\n",
        );
        help.push_str("  streamable-http MCP      loct watch --http starts loctree-mcp at /mcp\n");
        help.push_str("  per-root watch lock      One watcher per canonical snapshot root\n\n");

        // === JQ QUERIES ===
        help.push_str("=== JQ QUERIES ===\n\n");
        help.push_str("  loct '. | keys'               Snapshot surface — what you can query\n");
        help.push_str("  loct '.metadata'              Extract metadata from snapshot\n");
        help.push_str("  loct '.files | length'        Count analyzed files\n");
        help.push_str("  loct '.edges | length'        Count import edges\n\n");
        help.push_str(
            "  Dead exports, cycles and health are findings, not snapshot keys. Point the\n",
        );
        help.push_str("  same filters at the sibling artifacts with --artifact:\n\n");
        help.push_str(
            "  loct '.summary.health_score' --artifact agent       Health score (0-100)\n",
        );
        help.push_str("  loct '.dead_parrots | length' --artifact findings   Dead export count\n");
        help.push_str(
            "  loct '.cycles | length' --artifact findings         Import cycle count\n\n",
        );
        help.push_str("  Artifacts: snapshot (default), agent, findings, analysis, manifest,\n");
        help.push_str(
            "             handlers, dead, circular — materialized by a full `loct` run.\n",
        );
        help.push_str(
            "  A missing top-level key exits non-zero and names the keys that ARE there;\n",
        );
        help.push_str("  `.foo?` and `.foo // x` still answer quietly, as jq intends.\n\n");

        // === GLOBAL OPTIONS ===
        help.push_str("=== GLOBAL OPTIONS ===\n\n");
        help.push_str("  --json             Output as JSON\n");
        help.push_str("  --fresh            Force rescan (ignore cache)\n");
        help.push_str(
            "  --include-ignored  Also surface .loctignore-excluded files (tests/scripts/docs),\n",
        );
        help.push_str(
            "                     marked `ignored`; for find/slice/impact. Ephemeral scan,\n",
        );
        help.push_str("                     does not touch the cache or override .gitignore.\n");
        help.push_str("  --force-non-git-repository-snapshot\n");
        help.push_str(
            "  --force-non-git    Opt-in scan of a directory that is not a git checkout.\n",
        );
        help.push_str(
            "                     Default is refuse. Filesystem root `/` is still refused.\n",
        );
        help.push_str("  --verbose          Detailed progress\n");
        help.push_str("  --fail             Exit non-zero on issues (CI mode)\n");
        help.push_str("  --sarif            SARIF 2.1.0 output for CI\n\n");

        // === ARTIFACTS ===
        help.push_str("=== ARTIFACTS ===\n\n");
        help.push_str("  (default: user cache dir; override via LOCT_CACHE_DIR)\n\n");
        help.push_str("  snapshot.json      Full dependency graph (jq-queryable)\n");
        help.push_str("  Findings artifact  All issues (dead, cycles, twins...)\n");
        help.push_str("  agent.json         AI-optimized bundle with health_score\n");
        help.push_str("  manifest.json      Index for tooling integration\n\n");

        help.push_str("PROJECT CONFIG (repo-local, optional):\n");
        help.push_str("  .loctree/config.toml       Custom rules/macros\n");
        help.push_str("  .loctree/suppressions.toml Suppress issue records\n\n");

        // === PER-COMMAND HELP ===
        help.push_str("=== PER-COMMAND HELP ===\n\n");
        help.push_str("  loct <command> --help    Detailed help for any command\n");
        help.push_str("  loct --help-legacy       Legacy flag migration guide\n\n");

        // === EXAMPLES ===
        help.push_str("=== EXAMPLES ===\n\n");
        help.push_str("  # Quick analysis\n");
        help.push_str("  loct                       # Scan repo, create artifacts\n");
        help.push_str("  loct health                # Quick health check\n");
        help.push_str("  loct health --json         # Summary JSON for CI\n");
        help.push_str("  loct hotspots              # Rank static incoming degree\n\n");

        help.push_str("  # Deep analysis\n");
        help.push_str("  loct focus src/features/   # Directory context\n");
        help.push_str("  loct coverage              # Test gaps\n");
        help.push_str("  loct audit                 # Full audit\n\n");

        help.push_str("  # AI integration\n");
        help.push_str("  loct slice src/main.rs --json | claude\n");
        help.push_str("  loct context --full --json > context.json\n");
        help.push_str("  loct occurrences handle_auth --json\n");
        help.push_str("  loct body handle_auth --json\n\n");

        help.push_str("  # CI integration\n");
        help.push_str("  loct lint --fail --sarif > loctree.sarif\n");
        help.push_str("  loct health --json | jq '.summary.health_score'\n");

        help
    }

    /// Generate legacy help text with migration hints.
    pub fn format_legacy_help() -> String {
        let mut help = String::new();
        help.push_str("loctree - Legacy Flag Reference\n\n");
        help.push_str("These flags are deprecated and will be removed in v1.0.\n");
        help.push_str("Please migrate to the new subcommand interface.\n\n");

        help.push_str("LEGACY FLAG              -> NEW COMMAND\n");
        help.push_str("-------------------------------------------\n");
        help.push_str("loct                     -> loct auto (unchanged)\n");
        help.push_str("loct --tree              -> loct tree\n");
        help.push_str("loct -A                  -> loct report\n");
        help.push_str("loct -A --dead           -> loct dead\n");
        help.push_str("loct -A --circular       -> loct cycles\n");
        help.push_str("loct -A --entrypoints    -> loct lint --entrypoints\n");
        help.push_str("loct -A --symbol NAME    -> loct find --symbol NAME\n");
        help.push_str("loct -A --impact FILE    -> loct find --impact FILE\n");
        help.push_str("loct --findings          -> loct findings\n");
        help.push_str("loct --summary           -> loct findings --summary\n");
        help.push_str("loct --for-ai            -> loct context --full --json (or loct agent for legacy bundle JSON)\n");
        help.push_str("loct slice PATH          -> loct slice PATH (unchanged)\n");

        help.push_str("\nFor the new command reference, run: loct --help\n");

        help
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::command::options::{
        CyclesOptions, DeadOptions, ScanOptions, SliceOptions, TreeOptions,
    };

    #[test]
    fn test_command_default_is_auto() {
        let cmd = Command::default();
        assert_eq!(cmd.name(), "auto");
    }

    #[test]
    fn test_command_names() {
        assert_eq!(Command::Scan(ScanOptions::default()).name(), "scan");
        assert_eq!(Command::Tree(TreeOptions::default()).name(), "tree");
        assert_eq!(Command::Slice(SliceOptions::default()).name(), "slice");
        assert_eq!(Command::Dead(DeadOptions::default()).name(), "dead");
        assert_eq!(Command::Cycles(CyclesOptions::default()).name(), "cycles");
    }

    #[test]
    fn test_help_format_contains_commands() {
        let help = Command::format_help();
        assert!(help.contains("CORE COMMANDS"));
        assert!(help.contains("POWER PATH"));
        assert!(help.contains("Map:"));
        assert!(help.contains("Search:"));
        assert!(help.contains("Understand:"));
        assert!(help.contains("Trust:"));
        assert!(help.contains("Compare:"));
        assert!(help.contains("slice"));
        assert!(help.contains("find"));
        assert!(help.contains("impact"));
        assert!(help.contains("health"));
        assert!(help.contains("--help-full"));
    }

    #[test]
    fn test_help_format_promotes_power_surface() {
        let help = Command::format_help();
        assert!(!help.contains("ALIASES"));
        assert!(!help.contains("ARTIFACTS"));
        assert!(help.contains("loct focus"));
        assert!(help.contains("loct hotspots"));
        assert!(help.contains("loct follow"));
        assert!(help.contains("loct body"));
        assert!(help.contains("loct occurrences"));
        assert!(help.contains("loct query where-symbol"));
        assert!(help.contains("loct tree --files --match"));
        assert!(help.contains("loct doctor"));
        assert!(help.contains("loct diff"));
        assert!(help.contains("loct env-truth"));
        assert!(help.contains("loct suppressions"));
        assert!(help.contains("loct prism"));
        assert!(!help.contains("internal command reference"));
        assert!(!help.contains("without grep"));
        assert!(!help.contains("bounded output"));
        assert!(help.contains("scope and counts"));
    }

    #[test]
    fn test_truth_surface_help_matches_shipped_contracts() {
        let find = Command::format_command_help("find").unwrap();
        assert!(find.contains("Exact literal search by default"));
        assert!(find.contains("--discover"));
        assert!(find.contains("identifier-boundary"));
        assert!(find.contains("--regex"));
        assert!(find.contains("--count-only"));
        assert!(find.contains("--offset"));
        // Advertised flags must stay parseable (loctree-feedback help↔parser drift).
        for flag in ["--root", "--compact", "--project", "--path"] {
            assert!(find.contains(flag), "find help missing {flag}");
        }

        let occurrences = Command::format_command_help("occurrences").unwrap();
        for flag in [
            "--root",
            "--whole-token",
            "--group-by-file",
            "--count-only",
            "--compact",
            "--limit",
            "--offset",
        ] {
            assert!(occurrences.contains(flag), "missing {flag}");
        }

        let context = Command::format_command_help("context").unwrap();
        assert!(!context.contains("filled by later cut"));
        assert!(context.contains("--aicx-project"));

        let tree = Command::format_command_help("tree").unwrap();
        assert!(tree.contains("does not filter the rendered tree"));

        let slice = Command::format_command_help("slice").unwrap();
        assert!(!slice.contains("--json"));
    }

    #[test]
    fn test_legacy_help_format_contains_mappings() {
        let help = Command::format_legacy_help();
        assert!(help.contains("--tree"));
        assert!(help.contains("-A --dead"));
        assert!(help.contains("loct dead"));
    }

    #[test]
    fn test_command_specific_help_exists() {
        let tree_help = Command::format_command_help("tree").unwrap();
        assert!(tree_help.contains("loct tree"));
        let findings_help = Command::format_command_help("findings").unwrap();
        assert!(findings_help.contains("loct findings"));
        assert!(Command::format_command_help("unknown").is_none());
    }

    #[test]
    fn w4_01_banner_uses_package_version() {
        let version = env!("CARGO_PKG_VERSION");
        let help = Command::format_help();
        assert!(
            help.contains(version),
            "format_help must show CARGO_PKG_VERSION {version}: {help}"
        );
        assert!(
            !help.contains("v0.8.x"),
            "format_help must not leak the stale v0.8.x banner"
        );
        assert!(
            LOCT_USAGE.contains(version),
            "LOCT_USAGE must embed CARGO_PKG_VERSION {version}: {LOCT_USAGE}"
        );
        assert!(
            LOCT_USAGE.contains(&format!("(v{version})")),
            "LOCT_USAGE banner must be '(v{{CARGO_PKG_VERSION}})', got: {LOCT_USAGE}"
        );
        assert!(
            !LOCT_USAGE.contains("v0.8.x"),
            "LOCT_USAGE must not leak the stale v0.8.x banner"
        );
    }
}
