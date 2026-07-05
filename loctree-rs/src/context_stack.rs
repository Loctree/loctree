use std::collections::HashSet;
use std::path::Path;

use serde_json::Value as JsonValue;
use toml::Table as TomlTable;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectStack {
    Rust {
        workspace_members: Vec<String>,
    },
    Python {
        pyproject_present: bool,
        has_pytest: bool,
        has_ruff: bool,
        has_mypy: bool,
    },
    NodeJs {
        package_manager: PackageManager,
        has_lint: bool,
        has_test: bool,
        has_check: bool,
        has_build: bool,
    },
    Mixed(Vec<ProjectStack>),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Pnpm,
    Npm,
    Yarn,
}

impl PackageManager {
    pub fn command(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm",
            PackageManager::Npm => "npm run",
            PackageManager::Yarn => "yarn",
        }
    }
}

pub fn detect_project_stack(project_root: &Path) -> ProjectStack {
    let mut stacks = Vec::new();

    if let Some(stack) = detect_rust_stack(project_root) {
        stacks.push(stack);
    }
    if let Some(stack) = detect_python_stack(project_root) {
        stacks.push(stack);
    }
    if let Some(stack) = detect_node_stack(project_root) {
        stacks.push(stack);
    }

    match stacks.len() {
        0 => ProjectStack::Unknown,
        1 => stacks.remove(0),
        _ => ProjectStack::Mixed(stacks),
    }
}

pub fn read_makefile_test_targets(project_root: &Path) -> Vec<String> {
    let path = project_root.join("Makefile");
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut targets = HashSet::new();
    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with('\t')
            || trimmed.starts_with(".PHONY")
        {
            continue;
        }
        let Some((target_part, rest)) = trimmed.split_once(':') else {
            continue;
        };
        if target_part.contains('=')
            || target_part.contains('%')
            || target_part.contains('/')
            || rest.starts_with('=')
        {
            continue;
        }
        for target in target_part.split_whitespace() {
            if is_verification_make_target(target) {
                targets.insert(target.to_string());
            }
        }
    }

    let priority = [
        "precheck",
        "test",
        "check",
        "lint",
        "clippy",
        "fmt",
        "typecheck",
    ];
    priority
        .iter()
        .filter(|target| targets.contains(**target))
        .map(|target| (*target).to_string())
        .collect()
}

pub fn extract_ci_test_commands(project_root: &Path) -> Vec<String> {
    let workflows = project_root.join(".github/workflows");
    let Ok(entries) = std::fs::read_dir(workflows) else {
        return Vec::new();
    };

    let mut commands = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        commands.extend(extract_run_commands_from_workflow(&content));
    }

    dedup_top_n(commands, 5)
}

pub fn dedup_top_n(items: Vec<String>, limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            deduped.push(item);
        }
        if deduped.len() >= limit {
            break;
        }
    }
    deduped
}

fn detect_rust_stack(project_root: &Path) -> Option<ProjectStack> {
    let cargo_toml = parse_toml_file(&project_root.join("Cargo.toml"))?;
    let has_workspace = cargo_toml.get("workspace").is_some();
    let root_package = package_name_from_toml(&cargo_toml);
    if !has_workspace && root_package.is_none() {
        return None;
    }

    let mut crate_names = Vec::new();
    if let Some(name) = root_package {
        crate_names.push(name);
    }

    if let Some(workspace) = cargo_toml.get("workspace").and_then(|v| v.as_table())
        && let Some(members) = workspace.get("members").and_then(|v| v.as_array())
    {
        for member in members.iter().filter_map(|v| v.as_str()) {
            if member.contains('*') {
                continue;
            }
            if let Some(name) = parse_member_crate_name(project_root, member) {
                crate_names.push(name);
            }
        }
    }

    crate_names.sort();
    crate_names.dedup();
    Some(ProjectStack::Rust {
        workspace_members: crate_names,
    })
}

fn detect_python_stack(project_root: &Path) -> Option<ProjectStack> {
    let pyproject_path = project_root.join("pyproject.toml");
    let pyproject_present = pyproject_path.exists();
    let has_python_signal = pyproject_present
        || project_root.join("setup.py").exists()
        || project_root.join("requirements.txt").exists()
        || project_root.join("tox.ini").exists();
    if !has_python_signal {
        return None;
    }

    let pyproject = parse_toml_file(&pyproject_path);
    let tool = pyproject
        .as_ref()
        .and_then(|value| value.get("tool"))
        .and_then(|value| value.as_table());
    let requirements = std::fs::read_to_string(project_root.join("requirements.txt"))
        .unwrap_or_default()
        .to_lowercase();
    let tox = std::fs::read_to_string(project_root.join("tox.ini"))
        .unwrap_or_default()
        .to_lowercase();

    Some(ProjectStack::Python {
        pyproject_present,
        has_pytest: tool
            .map(|tool| tool.contains_key("pytest"))
            .unwrap_or(false)
            || requirements.contains("pytest")
            || tox.contains("pytest"),
        has_ruff: tool.map(|tool| tool.contains_key("ruff")).unwrap_or(false)
            || requirements.contains("ruff")
            || tox.contains("ruff"),
        has_mypy: tool.map(|tool| tool.contains_key("mypy")).unwrap_or(false)
            || requirements.contains("mypy")
            || tox.contains("mypy"),
    })
}

fn detect_node_stack(project_root: &Path) -> Option<ProjectStack> {
    let package_json = project_root.join("package.json");
    if !package_json.exists() {
        return None;
    }
    let content = std::fs::read_to_string(package_json).ok()?;
    let json: JsonValue = serde_json::from_str(&content).ok()?;
    let scripts = json.get("scripts").and_then(|value| value.as_object());

    Some(ProjectStack::NodeJs {
        package_manager: detect_package_manager(project_root),
        has_lint: script_present(scripts, "lint"),
        has_test: script_present(scripts, "test"),
        has_check: script_present(scripts, "check"),
        has_build: script_present(scripts, "build"),
    })
}

fn detect_package_manager(project_root: &Path) -> PackageManager {
    if project_root.join("pnpm-lock.yaml").exists() {
        PackageManager::Pnpm
    } else if project_root.join("yarn.lock").exists() {
        PackageManager::Yarn
    } else {
        PackageManager::Npm
    }
}

fn script_present(scripts: Option<&serde_json::Map<String, JsonValue>>, key: &str) -> bool {
    scripts
        .and_then(|scripts| scripts.get(key))
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn parse_member_crate_name(project_root: &Path, member: &str) -> Option<String> {
    let cargo_toml = parse_toml_file(&project_root.join(member).join("Cargo.toml"))?;
    package_name_from_toml(&cargo_toml)
}

fn package_name_from_toml(value: &TomlTable) -> Option<String> {
    value
        .get("package")
        .and_then(|package| package.as_table())
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .map(ToString::to_string)
}

fn parse_toml_file(path: &Path) -> Option<TomlTable> {
    let content = std::fs::read_to_string(path).ok()?;
    content.parse::<TomlTable>().ok()
}

fn is_verification_make_target(target: &str) -> bool {
    matches!(
        target,
        "precheck" | "test" | "check" | "lint" | "clippy" | "fmt" | "typecheck"
    )
}

fn extract_run_commands_from_workflow(content: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        let Some(run_body) = trimmed
            .strip_prefix("run:")
            .or_else(|| trimmed.strip_prefix("- run:"))
        else {
            continue;
        };
        let run_body = run_body.trim();
        if run_body == "|" || run_body == "|-" || run_body == ">" || run_body == ">-" {
            while let Some(next) = lines.peek().copied() {
                let block_line = next.trim();
                if block_line.starts_with('-') || block_line.contains(':') && !next.starts_with(' ')
                {
                    break;
                }
                lines.next();
                if let Some(command) = normalize_ci_command(block_line) {
                    commands.push(command);
                }
            }
        } else if let Some(command) = normalize_ci_command(run_body.trim_matches(['"', '\''])) {
            commands.push(command);
        }
    }
    commands
}

fn normalize_ci_command(raw: &str) -> Option<String> {
    let command = raw.trim().trim_start_matches("- ").trim();
    if command.is_empty() || command.starts_with('#') {
        return None;
    }
    let interesting = [
        "cargo check",
        "cargo clippy",
        "cargo test",
        "ruff check",
        "mypy",
        "pytest",
        "pnpm lint",
        "pnpm test",
        "npm run lint",
        "npm run test",
        "yarn lint",
        "yarn test",
    ];
    interesting
        .iter()
        .any(|prefix| command.starts_with(prefix))
        .then(|| command.to_string())
}
