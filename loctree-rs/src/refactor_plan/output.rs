//! Output formatters for refactor plans (markdown, JSON, shell script).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use std::fs;
use std::io;
use std::path::Path;

use super::{RefactorPlan, RiskLevel, execution_blockers};

// ============================================================================
// Markdown Output
// ============================================================================

/// Output refactor plan as markdown.
pub fn output_as_markdown(plan: &RefactorPlan, path: &Path) -> io::Result<()> {
    let content = format_as_markdown(plan);
    fs::write(path, content)
}

/// Format refactor plan as markdown string.
pub fn format_as_markdown(plan: &RefactorPlan) -> String {
    let mut md = String::new();

    // Header
    md.push_str(&format!("# Refactor Plan: {}\n\n", plan.target));
    md.push_str("Path layers are candidate evidence, not permission to reorganize a repository. Kept/unresolved files require a represented relocation contract.\n\n");
    let blockers = execution_blockers(std::slice::from_ref(plan));
    if !blockers.is_empty() {
        md.push_str("## Execution withheld\n\n");
        for blocker in &blockers {
            md.push_str(&format!("- {blocker}\n"));
        }
    }
    md.push_str("## Kept / unresolved candidates\n\n");
    for candidate in &plan.unresolved {
        md.push_str(&format!(
            "- `{}` ({}): {}\n",
            candidate.source,
            candidate.layer.display_name(),
            candidate.reason
        ));
    }
    md.push_str("\nSupported execution: explicit same-package literal-export ESM leaves only, preserving relative topology and old-path shims. TypeScript requires a simple explicit ESM emitting tsconfig whose rootDir/include/exclude cover both source and destination; native/noEmit, JSONC/extended/project-reference configurations are withheld. Rust/Python relocation, dependency/asset rewriting, runtime modules and other syntax are withheld. The script requires Python 3, Git and the recorded repository root; use --dry first.\n\n");

    // Summary
    md.push_str("## Summary\n\n");
    md.push_str(&format!(
        "- **Files analyzed:** {}\n",
        plan.stats.total_files
    ));
    md.push_str(&format!(
        "- **Files to move:** {}\n",
        plan.stats.files_to_move
    ));
    md.push_str(&format!(
        "- **Shims needed:** {}\n",
        plan.stats.shims_needed
    ));

    // Risk breakdown
    md.push_str("- **Risk breakdown:** ");
    let risk_parts: Vec<String> = plan
        .stats
        .by_risk
        .iter()
        .map(|(k, v)| format!("{} {}", v, k))
        .collect();
    md.push_str(&risk_parts.join(", "));
    md.push_str("\n\n");

    // Layer Distribution
    md.push_str("## Layer Distribution\n\n");
    md.push_str("### Before\n");
    for (layer, count) in &plan.stats.layer_before {
        let bar = "█".repeat((*count).min(20));
        md.push_str(&format!("- {}: {} {}\n", layer, bar, count));
    }
    md.push_str("\n### After\n");
    for (layer, count) in &plan.stats.layer_after {
        let bar = "█".repeat((*count).min(20));
        md.push_str(&format!("- {}: {} {}\n", layer, bar, count));
    }
    md.push('\n');

    // Cyclic Dependencies Warning
    if !plan.cyclic_groups.is_empty() {
        md.push_str("## ⚠️ Cyclic Dependencies\n\n");
        md.push_str("The following groups have circular imports. Cycle membership alone does not authorize relocation; resolve their module contracts before proposing moves:\n\n");
        for (i, group) in plan.cyclic_groups.iter().enumerate() {
            md.push_str(&format!("**Cycle {}:**\n", i + 1));
            for file in group {
                md.push_str(&format!("- `{}`\n", file));
            }
            md.push('\n');
        }
    }

    // Phases
    for phase in &plan.phases {
        let risk_emoji = match phase.risk {
            RiskLevel::Low => "🟢",
            RiskLevel::Medium => "🟡",
            RiskLevel::High => "🔴",
        };

        md.push_str(&format!("## {} {}\n\n", risk_emoji, phase.name));
        md.push_str(&format!("{} file(s)\n\n", phase.moves.len()));

        // Moves table
        md.push_str("| Source | Destination | Layer | LOC | Consumers | Reason |\n");
        md.push_str("|------|------|----|----|-----------|--------|\n");

        for mv in &phase.moves {
            md.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                mv.source,
                mv.target,
                mv.target_layer.display_name(),
                mv.loc,
                mv.direct_consumers,
                mv.reason
            ));
        }
        md.push('\n');

        // Git commands
        md.push_str("**Commands:**\n\n```bash\n");
        if blockers.is_empty() {
            md.push_str(&super::build_git_script(&phase.moves));
        } else {
            md.push_str("# Withheld: execution contract rejected this plan.");
        }
        md.push_str("\n```\n\n");
    }

    // Shimming Strategy
    if !plan.shims.is_empty() {
        md.push_str("## Shimming Strategy\n\n");
        md.push_str(
            "Preserve every old module path, including consumers absent from the import graph:\n\n",
        );

        for shim in &plan.shims {
            md.push_str(&format!(
                "### `{}` ({} importers)\n\n",
                shim.old_path, shim.importer_count
            ));
            md.push_str("```\n");
            md.push_str(&shim.code);
            md.push_str("\n```\n\n");
        }
    }

    // Footer
    md.push_str("---\n\n");
    md.push_str("*Generated by loctree • 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI*\n");

    md
}

/// Output multiple refactor plans as a single markdown file.
pub fn output_bundle_as_markdown(plans: &[RefactorPlan], path: &Path) -> io::Result<()> {
    let content = format_bundle_as_markdown(plans);
    fs::write(path, content)
}

/// Format multiple refactor plans as a single markdown string.
pub fn format_bundle_as_markdown(plans: &[RefactorPlan]) -> String {
    let mut md = String::new();
    for blocker in execution_blockers(plans) {
        md.push_str(&format!("- Execution withheld: {blocker}\n"));
    }
    for (idx, plan) in plans.iter().enumerate() {
        if idx > 0 {
            md.push_str("\n\n---\n\n");
        }
        md.push_str(&format_as_markdown(plan));
    }
    md
}

// ============================================================================
// JSON Output
// ============================================================================

/// Output refactor plan as JSON.
pub fn output_as_json(plan: &RefactorPlan, path: &Path) -> io::Result<()> {
    fs::write(path, format_as_json(plan))
}

/// Format refactor plan as JSON string.
pub fn format_as_json(plan: &RefactorPlan) -> String {
    serde_json::to_string_pretty(&json_plan(
        plan,
        &execution_blockers(std::slice::from_ref(plan)),
    ))
    .expect("refactor plan is serializable")
}

/// Output multiple refactor plans as JSON.
pub fn output_bundle_as_json(plans: &[RefactorPlan], path: &Path) -> io::Result<()> {
    fs::write(path, format_bundle_as_json(plans))
}

/// Format multiple refactor plans as JSON string.
pub fn format_bundle_as_json(plans: &[RefactorPlan]) -> String {
    let blockers = execution_blockers(plans);
    let values: Vec<_> = plans
        .iter()
        .map(|plan| json_plan(plan, &blockers))
        .collect();
    serde_json::to_string_pretty(&values).expect("refactor plans are serializable")
}

fn json_plan(plan: &RefactorPlan, blockers: &[String]) -> serde_json::Value {
    let mut value = serde_json::to_value(plan).expect("refactor plan is serializable");
    value["execution"] = serde_json::json!({"blockers": blockers, "supported": blockers.is_empty(),
        "mode": if !blockers.is_empty() { "withheld" } else if plan.moves.is_empty() { "analysis_only" } else { "guarded_moves" },
        "contract": "explicit same-package literal-export ESM leaves; topology and old-path shims preserved"});
    value
}

// ============================================================================
// Shell Script Output
// ============================================================================

/// Output refactor plan as executable shell script.
pub fn output_as_script(plan: &RefactorPlan, path: &Path) -> io::Result<()> {
    let content = format_as_script(plan);
    fs::write(path, content)?;

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }

    Ok(())
}

/// Format refactor plan as shell script string.
pub fn format_as_script(plan: &RefactorPlan) -> String {
    format_bundle_as_script(std::slice::from_ref(plan))
}

/// Output multiple refactor plans as a single executable shell script.
pub fn output_bundle_as_script(plans: &[RefactorPlan], path: &Path) -> io::Result<()> {
    let content = format_bundle_as_script(plans);
    fs::write(path, content)?;

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }

    Ok(())
}

/// Format multiple refactor plans as a single shell script string.
pub fn format_bundle_as_script(plans: &[RefactorPlan]) -> String {
    let blockers = execution_blockers(plans);
    if !blockers.is_empty() {
        return format!(
            "#!/bin/bash\n# Execution withheld: invalid relocation contract.\nprintf '%s\\n' '{}' >&2\nexit 2\n",
            blockers.join("; ").replace('\'', "'\\''")
        );
    }
    let payload = serde_json::to_string(plans).expect("plans are serializable");
    // JSON is data, not Python source or shell interpolation. Its lines cannot
    // terminate the quoted heredoc, even when paths or shim strings are hostile.
    let mut script = String::from(
        "#!/bin/bash\n# Guarded refactor plan: --dry first; optional numeric phase filter.\n# Requires Python 3 and Git. No rollback is promised on external failures.\nset -euo pipefail\npython3 - \"$@\" <<'LOCTREE_EXECUTION_PY'\nimport hashlib, json, os, pathlib, subprocess, sys\nplans = json.loads(",
    );
    script.push_str(&serde_json::to_string(&payload).expect("JSON payload is serializable"));
    script.push_str(")\n");
    script.push_str(EXECUTION_ENGINE);
    script.push_str("\nLOCTREE_EXECUTION_PY\n");
    script
}

const EXECUTION_ENGINE: &str = r#"
def fail(message):
    sys.exit('Refactor withheld: ' + message)

args = sys.argv[1:]
dry = '--dry' in args
filters = [arg for arg in args if arg != '--dry']
if len(filters) > 1 or any(not arg.isdecimal() for arg in filters):
    fail('usage: refactor.sh [--dry] [PHASE]')
selected = int(filters[0]) if filters else None
steps = []
phase_number = 0
for plan in plans:
    for phase in plan['phases']:
        phase_number += 1
        for move in phase['moves']:
            shim = next(s for s in plan['shims'] if s['old_path'] == move['source'])
            steps.append((phase_number, move, shim, plan['source_hashes'][move['source']]))
if selected is not None and not 1 <= selected <= phase_number:
    fail('unknown phase')
if not steps:
    print('Analysis only: no justified executable moves; inspect kept/unresolved candidates.')
    for plan in plans:
        for candidate in plan['unresolved']:
            print('Kept ' + candidate['source'] + ': ' + candidate['reason'])
    sys.exit(0)
root = pathlib.Path(plans[0]['repository_root'])
if pathlib.Path.cwd().resolve() != root:
    fail('run from the recorded repository root: ' + str(root))
git_root = subprocess.check_output(['git', 'rev-parse', '--show-toplevel'], text=True).strip()
if pathlib.Path(git_root).resolve() != root:
    fail('Git root differs from the recorded repository root')

def inspect(path, destination):
    current = root
    parts = pathlib.PurePosixPath(path).parts
    for index, part in enumerate(parts):
        if current.is_dir():
            for entry in current.iterdir():
                if entry.name.casefold() == part.casefold():
                    if entry.name != part or destination and index == len(parts) - 1:
                        fail('existing or case-colliding destination: ' + str(entry))
        current = current / part
        if current.is_symlink() or index < len(parts) - 1 and current.exists() and not current.is_dir():
            fail('symlink or non-directory ancestor: ' + str(current))

def boundary(path):
    for parent in (root / path).parents:
        if any((parent / name).exists() for name in ('package.json', 'Cargo.toml', 'pyproject.toml', '__init__.py')):
            return parent
        if parent == root:
            break

sources = {move['source'] for _, move, _, _ in steps}
contract_inputs = {}
for plan in plans:
    contract_inputs.update(plan['source_hashes'])
    for path, digest in plan['source_hashes'].items():
        if path not in sources:
            inspect(path, False)
            if hashlib.sha256((root / path).read_bytes()).hexdigest() != digest:
                fail('manifest/compiler contract changed since planning: ' + path)

def inspect_compiler_scope(move):
    if not move['source'].endswith('.ts'):
        return
    scopes = []
    for path in (move['source'], move['target']):
        for parent in (root / path).parents:
            config = parent / 'tsconfig.json'
            if config.exists():
                relative = config.relative_to(root).as_posix()
                if relative not in contract_inputs:
                    fail('unrepresented compiler scope: ' + relative)
                scopes.append(relative)
                break
            if parent == root:
                break
    if len(scopes) != 2 or scopes[0] != scopes[1]:
        fail('compiler scope differs between source and destination')

def completed(move, shim, digest):
    source, target = root / move['source'], root / move['target']
    return (source.is_file() and target.is_file() and not source.is_symlink() and not target.is_symlink()
            and source.read_bytes() == shim['code'].encode('utf-8')
            and hashlib.sha256(target.read_bytes()).hexdigest() == digest)

# Preflight the complete bundle before any mutation, even with a phase filter.
# Completed phases are verified, never executed again or overwritten.
pending = []
for phase, move, shim, digest in steps:
    source, target = move['source'], move['target']
    inspect_compiler_scope(move)
    if completed(move, shim, digest):
        inspect(source, False)
        inspect(target, False)
        if not boundary(source) or boundary(source) != boundary(target):
            fail('completed move package boundary changed: ' + source)
        continue
    inspect(source, False)
    inspect(target, True)
    if not boundary(source) or boundary(source) != boundary(target):
        fail('package boundary changed: ' + source)
    if hashlib.sha256((root / source).read_bytes()).hexdigest() != digest:
        fail('source changed since planning: ' + source)
    subprocess.run(['git', 'ls-files', '--error-unmatch', '--', source], check=True, stdout=subprocess.DEVNULL)
    pending.append((phase, move, shim, digest))

for phase, move, shim, digest in pending:
    if selected is not None and selected != phase:
        continue
    source, target = move['source'], move['target']
    print(('[DRY] ' if dry else '[RUN] ') + 'phase_' + str(phase) + ': git mv ' + repr(source) + ' ' + repr(target), flush=True)
    if dry:
        print('[DRY] create exclusive compatibility shim: ' + source)
        continue
    inspect(source, False)
    inspect(target, True)
    if hashlib.sha256((root / source).read_bytes()).hexdigest() != digest:
        fail('source changed during execution: ' + source)
    (root / target).parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['git', 'mv', '--', source, target], check=True)
    if os.path.lexists(root / source):
        fail('move did not complete; refusing to overwrite source with shim: ' + source)
    with (root / source).open('x', encoding='utf-8') as output:
        output.write(shim['code'])
print('Dry run complete; no files changed.' if dry else 'Moves and shims written; review Git status and run language checks. Shims are not staged.')
"#;

// ============================================================================
// Print to stdout
// ============================================================================

/// Print plan summary to stdout.
pub fn print_plan_summary(plan: &RefactorPlan) {
    println!("Refactor Plan: {}/", plan.target);
    println!();
    println!(
        "  Files: {} total, {} to move",
        plan.stats.total_files, plan.stats.files_to_move
    );
    println!(
        "  Kept/unresolved: {} (path heuristics alone never authorize moves)",
        plan.unresolved.len()
    );

    if !plan.stats.by_risk.is_empty() {
        let risk_str: Vec<String> = plan
            .stats
            .by_risk
            .iter()
            .map(|(k, v)| format!("{} {}", v, k))
            .collect();
        println!("  Risk: {}", risk_str.join(", "));
    }

    if plan.stats.shims_needed > 0 {
        println!("  Shims: {} needed", plan.stats.shims_needed);
    }

    if !plan.cyclic_groups.is_empty() {
        println!(
            "  Cycles: {} groups (move together)",
            plan.cyclic_groups.len()
        );
    }

    println!();

    // Brief phase summary
    for phase in &plan.phases {
        let emoji = match phase.risk {
            RiskLevel::Low => "🟢",
            RiskLevel::Medium => "🟡",
            RiskLevel::High => "🔴",
        };
        println!("  {} {} ({} files)", emoji, phase.name, phase.moves.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refactor_plan::{Layer, Move, PlanStats, RefactorPhase, RiskLevel};
    use std::collections::HashMap;

    fn mock_plan() -> RefactorPlan {
        RefactorPlan {
            target: "src/features".to_string(),
            repository_root: String::new(),
            source_hashes: HashMap::new(),
            unresolved: vec![],
            moves: vec![Move {
                source: "src/features/utils.ts".to_string(),
                target: "src/features/infra/utils.ts".to_string(),
                current_layer: Layer::Unknown,
                target_layer: Layer::Infra,
                risk: RiskLevel::Low,
                direct_consumers: 3,
                transitive_consumers: 8,
                loc: 120,
                reason: "Unknown → Infra".to_string(),
                verify_cmd: "loct impact src/features/infra/utils.ts".to_string(),
                affected_files: vec!["src/features/main.ts".to_string()],
            }],
            shims: vec![],
            cyclic_groups: vec![],
            phases: vec![RefactorPhase {
                name: "Phase 1: LOW Risk".to_string(),
                risk: RiskLevel::Low,
                moves: vec![Move {
                    source: "src/features/utils.ts".to_string(),
                    target: "src/features/infra/utils.ts".to_string(),
                    current_layer: Layer::Unknown,
                    target_layer: Layer::Infra,
                    risk: RiskLevel::Low,
                    direct_consumers: 3,
                    transitive_consumers: 8,
                    loc: 120,
                    reason: "Unknown → Infra".to_string(),
                    verify_cmd: "loct impact src/features/infra/utils.ts".to_string(),
                    affected_files: vec!["src/features/main.ts".to_string()],
                }],
                git_script: "git mv src/features/utils.ts src/features/infra/utils.ts".to_string(),
            }],
            stats: PlanStats {
                total_files: 10,
                files_to_move: 1,
                shims_needed: 0,
                layer_before: HashMap::from([("Unknown".to_string(), 1)]),
                layer_after: HashMap::from([("Infra".to_string(), 1)]),
                by_risk: HashMap::from([("LOW".to_string(), 1)]),
            },
        }
    }

    #[test]
    fn test_format_as_markdown() {
        let plan = mock_plan();
        let md = format_as_markdown(&plan);

        assert!(md.contains("# Refactor Plan: src/features"));
        assert!(md.contains("Files analyzed:** 10"));
        assert!(md.contains("Files to move:** 1"));
        assert!(md.contains("Phase 1: LOW Risk"));
    }

    #[test]
    fn test_format_as_script() {
        let plan = mock_plan();
        let script = format_as_script(&plan);

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("Execution withheld"));
        assert!(!script.contains("git mv"));
    }

    #[test]
    fn test_format_bundle_as_json() {
        let mut plan2 = mock_plan();
        plan2.target = "src/other".to_string();

        let json = format_bundle_as_json(&[mock_plan(), plan2]);
        assert!(json.trim_start().starts_with('['));
        assert!(json.contains("\"target\": \"src/features\""));
        assert!(json.contains("\"target\": \"src/other\""));
    }
}
