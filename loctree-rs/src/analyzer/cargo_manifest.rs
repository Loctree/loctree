use crate::types::{CargoTarget, TargetKind};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct WorkspaceManifest {
    pub members: Vec<PathBuf>,
    pub resolved_members: Vec<CrateManifest>,
}

#[derive(Clone, Debug)]
pub struct CrateManifest {
    pub package_name: String,
    pub lib_name: Option<String>,
    pub targets: Vec<CargoTarget>,
    pub intra_workspace_deps: Vec<String>,
}

pub fn parse_workspace_root(path: &Path) -> Result<WorkspaceManifest, String> {
    let manifest_path = manifest_path(path);
    let root = manifest_path
        .parent()
        .ok_or_else(|| format!("manifest has no parent: {}", manifest_path.display()))?;
    let value = read_manifest(&manifest_path)?;
    let workspace = value
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("not a workspace manifest: {}", manifest_path.display()))?;

    let mut members = Vec::new();
    if let Some(items) = workspace.get("members").and_then(toml::Value::as_array) {
        for item in items.iter().filter_map(toml::Value::as_str) {
            members.extend(resolve_member_pattern(root, item));
        }
    }

    let mut resolved_pairs = Vec::new();
    for member in &members {
        let member_manifest = member.join("Cargo.toml");
        if member_manifest.exists()
            && let Ok(parsed) = parse_crate_manifest(&member_manifest)
        {
            resolved_pairs.push((member_manifest, parsed));
        }
    }

    let names: BTreeSet<String> = resolved_pairs
        .iter()
        .map(|(_, member)| member.package_name.clone())
        .collect();
    for (member_manifest, member) in &mut resolved_pairs {
        member.intra_workspace_deps =
            workspace_deps_for(member_manifest, &names).unwrap_or_default();
    }
    let resolved_members = resolved_pairs
        .into_iter()
        .map(|(_, member)| member)
        .collect();

    Ok(WorkspaceManifest {
        members,
        resolved_members,
    })
}

pub fn parse_crate_manifest(path: &Path) -> Result<CrateManifest, String> {
    let manifest_path = manifest_path(path);
    let root = manifest_path
        .parent()
        .ok_or_else(|| format!("manifest has no parent: {}", manifest_path.display()))?;
    let value = read_manifest(&manifest_path)?;
    let package_name = value
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("crate")
        })
        .to_string();

    let mut targets = Vec::new();
    let lib_name = value.get("lib").and_then(toml::Value::as_table).map(|lib| {
        let name = lib
            .get("name")
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| package_name.replace('-', "_"));
        let path = lib
            .get("path")
            .and_then(toml::Value::as_str)
            .map(|p| root.join(p))
            .unwrap_or_else(|| root.join("src/lib.rs"));
        targets.push(CargoTarget {
            name: name.clone(),
            kind: TargetKind::Lib,
            path,
            crate_root: root.to_path_buf(),
        });
        name
    });

    if lib_name.is_none() && root.join("src/lib.rs").exists() {
        targets.push(CargoTarget {
            name: package_name.replace('-', "_"),
            kind: TargetKind::Lib,
            path: root.join("src/lib.rs"),
            crate_root: root.to_path_buf(),
        });
    }

    push_targets(
        &mut targets,
        &value,
        root,
        "bin",
        TargetKind::Bin,
        "src/main.rs",
        &package_name,
    );
    push_targets(
        &mut targets,
        &value,
        root,
        "example",
        TargetKind::Example,
        "examples",
        &package_name,
    );
    push_targets(
        &mut targets,
        &value,
        root,
        "bench",
        TargetKind::Bench,
        "benches",
        &package_name,
    );
    push_targets(
        &mut targets,
        &value,
        root,
        "test",
        TargetKind::Test,
        "tests",
        &package_name,
    );

    if !targets.iter().any(|target| target.kind == TargetKind::Bin)
        && root.join("src/main.rs").exists()
    {
        targets.push(CargoTarget {
            name: package_name.clone(),
            kind: TargetKind::Bin,
            path: root.join("src/main.rs"),
            crate_root: root.to_path_buf(),
        });
    }

    Ok(CrateManifest {
        package_name,
        lib_name,
        targets,
        intra_workspace_deps: Vec::new(),
    })
}

fn manifest_path(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some("Cargo.toml") {
        path.to_path_buf()
    } else {
        path.join("Cargo.toml")
    }
}

fn read_manifest(path: &Path) -> Result<toml::Value, String> {
    let raw = fs::read_to_string(path).map_err(|err| format!("{}: {}", path.display(), err))?;
    toml::from_str::<toml::Value>(&raw).map_err(|err| format!("{}: {}", path.display(), err))
}

fn resolve_member_pattern(root: &Path, pattern: &str) -> Vec<PathBuf> {
    if !pattern.contains('*') {
        return vec![root.join(pattern)];
    }
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return Vec::new();
    };
    let base = root.join(prefix.trim_end_matches('/'));
    let Ok(entries) = fs::read_dir(base) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path.join("Cargo.toml").exists()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(suffix.trim_start_matches('/')))
        })
        .collect()
}

fn push_targets(
    targets: &mut Vec<CargoTarget>,
    value: &toml::Value,
    root: &Path,
    key: &str,
    kind: TargetKind,
    default_dir_or_file: &str,
    package_name: &str,
) {
    let Some(items) = value.get(key).and_then(toml::Value::as_array) else {
        return;
    };
    for item in items.iter().filter_map(toml::Value::as_table) {
        let name = item
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or(package_name)
            .to_string();
        let path = item
            .get("path")
            .and_then(toml::Value::as_str)
            .map(|p| root.join(p))
            .unwrap_or_else(|| {
                if default_dir_or_file.ends_with(".rs") {
                    root.join(default_dir_or_file)
                } else {
                    root.join(default_dir_or_file).join(format!("{name}.rs"))
                }
            });
        targets.push(CargoTarget {
            name,
            kind: kind.clone(),
            path,
            crate_root: root.to_path_buf(),
        });
    }
}

fn workspace_deps_for(
    path: &Path,
    workspace_names: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let value = read_manifest(path)?;
    let mut deps = BTreeSet::new();
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = value.get(table_name).and_then(toml::Value::as_table) {
            for name in table.keys() {
                if workspace_names.contains(name) {
                    deps.insert(name.clone());
                }
            }
        }
    }
    Ok(deps.into_iter().collect())
}

pub fn find_nearest_crate_manifest(path: &Path, root: &Path) -> Option<PathBuf> {
    let mut current = if path.is_file() {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    loop {
        let candidate = current.join("Cargo.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if current == root {
            return None;
        }
        current = current.parent()?.to_path_buf();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cargo_manifest_extracts_targets_and_renamed_lib() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).expect("src");
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[package]
name = "rust-mux"

[lib]
name = "rust_mux"
path = "src/lib.rs"

[[bin]]
name = "rust-mux-proxy"
path = "src/proxy.rs"
"#,
        )
        .expect("manifest");

        let parsed = parse_crate_manifest(dir.path()).expect("parse crate");
        assert_eq!(parsed.package_name, "rust-mux");
        assert_eq!(parsed.lib_name.as_deref(), Some("rust_mux"));
        assert!(
            parsed
                .targets
                .iter()
                .any(|target| target.name == "rust_mux")
        );
        assert!(
            parsed
                .targets
                .iter()
                .any(|target| target.name == "rust-mux-proxy")
        );
    }

    #[test]
    fn cargo_manifest_resolves_workspace_glob_members() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("crates/a/src")).expect("a");
        fs::create_dir_all(dir.path().join("crates/b/src")).expect("b");
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["crates/*"]
"#,
        )
        .expect("workspace");
        fs::write(
            dir.path().join("crates/a/Cargo.toml"),
            r#"[package]
name = "a"

[dependencies]
b = { path = "../b" }
"#,
        )
        .expect("a manifest");
        fs::write(
            dir.path().join("crates/b/Cargo.toml"),
            r#"[package]
name = "b"
"#,
        )
        .expect("b manifest");

        let parsed = parse_workspace_root(dir.path()).expect("parse workspace");
        assert_eq!(parsed.members.len(), 2);
        let crate_a = parsed
            .resolved_members
            .iter()
            .find(|member| member.package_name == "a")
            .expect("crate a");
        assert_eq!(crate_a.intra_workspace_deps, vec!["b"]);
    }
}
