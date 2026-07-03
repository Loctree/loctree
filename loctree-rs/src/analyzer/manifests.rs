use std::path::Path;

use serde_json::Value as JsonValue;

use crate::snapshot::{
    CargoBinSummary, CargoTomlSummary, ManifestEntry, ManifestSummary, PackageJsonSummary,
    PyProjectSummary,
};

pub fn summarize_manifests(root: &Path) -> ManifestSummary {
    ManifestSummary {
        root: root.display().to_string(),
        package_json: summarize_package_json(root),
        cargo_toml: summarize_cargo_toml(root),
        pyproject_toml: summarize_pyproject_toml(root),
    }
}

fn summarize_package_json(root: &Path) -> Option<PackageJsonSummary> {
    let path = root.join("package.json");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let json: JsonValue = serde_json::from_str(&content).ok()?;

    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let package_type = json
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let main = json
        .get("main")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let module = json
        .get("module")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let types = json
        .get("types")
        .or_else(|| json.get("typings"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let package_manager = json
        .get("packageManager")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let scripts = if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
        let mut items: Vec<String> = scripts.keys().cloned().collect();
        items.sort();
        items
    } else {
        Vec::new()
    };

    let workspaces = if let Some(workspaces) = json.get("workspaces") {
        let mut items = Vec::new();
        match workspaces {
            JsonValue::Array(arr) => {
                for item in arr.iter().filter_map(|v| v.as_str()) {
                    items.push(item.to_string());
                }
            }
            JsonValue::Object(obj) => {
                if let Some(packages) = obj.get("packages").and_then(|v| v.as_array()) {
                    for item in packages.iter().filter_map(|v| v.as_str()) {
                        items.push(item.to_string());
                    }
                }
            }
            _ => {}
        }
        items
    } else {
        Vec::new()
    };

    let exports = json
        .get("exports")
        .map(parse_package_exports)
        .unwrap_or_default();

    let bin = if let Some(bin) = json.get("bin") {
        parse_package_bin(bin, name.as_deref())
    } else {
        Vec::new()
    };

    Some(PackageJsonSummary {
        name,
        package_type,
        main,
        module,
        types,
        exports,
        bin,
        workspaces,
        scripts,
        package_manager,
    })
}

fn parse_package_exports(exports: &JsonValue) -> Vec<ManifestEntry> {
    let mut entries = Vec::new();
    match exports {
        JsonValue::String(path) => entries.push(ManifestEntry {
            key: ".".to_string(),
            path: path.to_string(),
        }),
        JsonValue::Object(map) => {
            for (key, value) in map {
                let path = match value {
                    JsonValue::String(s) => Some(s.to_string()),
                    JsonValue::Object(conds) => conds
                        .get("import")
                        .or_else(|| conds.get("require"))
                        .or_else(|| conds.get("default"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    _ => None,
                };
                if let Some(path) = path {
                    entries.push(ManifestEntry {
                        key: key.clone(),
                        path,
                    });
                }
            }
        }
        _ => {}
    }
    entries
}

fn parse_package_bin(bin: &JsonValue, package_name: Option<&str>) -> Vec<ManifestEntry> {
    let mut entries = Vec::new();
    match bin {
        JsonValue::String(path) => entries.push(ManifestEntry {
            key: package_name.unwrap_or("bin").to_string(),
            path: path.to_string(),
        }),
        JsonValue::Object(map) => {
            for (key, value) in map {
                if let Some(path) = value.as_str() {
                    entries.push(ManifestEntry {
                        key: key.clone(),
                        path: path.to_string(),
                    });
                }
            }
        }
        _ => {}
    }
    entries
}

fn summarize_cargo_toml(root: &Path) -> Option<CargoTomlSummary> {
    let path = root.join("Cargo.toml");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let table: toml::Table = content.parse().ok()?;

    let mut summary = CargoTomlSummary::default();

    if let Some(package) = table.get("package").and_then(|v| v.as_table()) {
        summary.package_name = package
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    if let Some(workspace) = table.get("workspace").and_then(|v| v.as_table()) {
        summary.workspace_members = workspace
            .get("members")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        summary.workspace_default_members = workspace
            .get("default-members")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
    }

    if let Some(lib) = table.get("lib").and_then(|v| v.as_table()) {
        summary.lib_path = lib
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    if let Some(bins) = table.get("bin").and_then(|v| v.as_array()) {
        for bin in bins {
            if let Some(bin_table) = bin.as_table() {
                let name = bin_table
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let path = bin_table
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                summary.bins.push(CargoBinSummary { name, path });
            }
        }
    }

    if let Some(features) = table.get("features").and_then(|v| v.as_table()) {
        summary.features = features.keys().cloned().collect();
        summary.features.sort();
    }

    summary.crate_roots = collect_cargo_crate_roots(root, &summary.workspace_members);

    Some(summary)
}

fn collect_cargo_crate_roots(root: &Path, members: &[String]) -> Vec<String> {
    let mut roots = Vec::new();
    let base = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let root_src = base.join("src");
    if root_src.is_dir() {
        roots.push(root_src.display().to_string());
    } else {
        roots.push(base.display().to_string());
    }

    for member in members {
        let member_path = base.join(member);
        if member_path.join("Cargo.toml").exists() {
            let member_src = member_path.join("src");
            if member_src.is_dir() {
                roots.push(member_src.display().to_string());
            } else {
                roots.push(member_path.display().to_string());
            }
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

fn summarize_pyproject_toml(root: &Path) -> Option<PyProjectSummary> {
    let path = root.join("pyproject.toml");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let table: toml::Table = content.parse().ok()?;

    let mut scripts = Vec::new();
    let mut entry_points = Vec::new();
    let mut project_name = None;

    if let Some(project) = table.get("project").and_then(|v| v.as_table()) {
        project_name = project
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(project_scripts) = project.get("scripts").and_then(|v| v.as_table()) {
            scripts = project_scripts.keys().cloned().collect();
            scripts.sort();
        }

        if let Some(project_entries) = project.get("entry-points").and_then(|v| v.as_table()) {
            entry_points = project_entries.keys().cloned().collect();
            entry_points.sort();
        }
    }

    let mut poetry_name = None;
    if let Some(tool) = table.get("tool").and_then(|v| v.as_table())
        && let Some(poetry) = tool.get("poetry").and_then(|v| v.as_table())
    {
        poetry_name = poetry
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(poetry_scripts) = poetry.get("scripts").and_then(|v| v.as_table()) {
            for key in poetry_scripts.keys() {
                if !scripts.contains(key) {
                    scripts.push(key.clone());
                }
            }
            scripts.sort();
        }
    }

    Some(PyProjectSummary {
        project_name,
        poetry_name,
        scripts,
        entry_points,
    })
}
