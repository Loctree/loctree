//! File discovery for `loct env-truth`.
//!
//! Walks scan roots collecting every file that any sensor needs to inspect.
//! Uses a hard-coded directory blocklist (`SKIP_DIRS`) to skip irrelevant
//! directories such as `node_modules`, `target`, and `.git`. `.env*` files
//! are always visited even when they are gitignored — auditing declarations
//! regardless of commit status is the point of this tool.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Group of candidate files for the env-truth orchestrator. Each vector is
/// independent — a sensor reads only its own group.
#[derive(Debug, Default)]
pub struct CandidateFiles {
    pub dotenv: Vec<PathBuf>,
    pub envrc: Vec<PathBuf>,
    pub dockerfile: Vec<PathBuf>,
    pub docker_compose: Vec<PathBuf>,
    pub k8s_yaml: Vec<PathBuf>,
    pub helm_values: Vec<PathBuf>,
    pub github_workflows: Vec<PathBuf>,
    pub sops_files: Vec<PathBuf>,
    pub npm_packages: Vec<PathBuf>,
    pub tauri_conf: Vec<PathBuf>,
}

/// Hard-coded directory blocklist. `node_modules`, `target`, `.venv` etc are
/// never relevant for env audit and would balloon scan time.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    ".git",
    "dist",
    "build",
    ".next",
    ".turbo",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".cargo",
    ".idea",
    ".vscode",
    ".loctree",
];

/// Heuristic: directory names that strongly suggest k8s manifests live here.
const K8S_DIR_HINTS: &[&str] = &[
    "k8s",
    "kubernetes",
    "deploy",
    "manifests",
    "chart",
    "helm",
    "deployment",
    "deployments",
];

/// Walk every root and bin candidate files into the right bucket.
///
/// Restrict scan to `restricted_paths` (relative to root) when non-empty.
/// Restrictions match by `starts_with` semantics so passing `k8s/` will
/// pull every YAML below.
pub fn discover_candidates(roots: &[PathBuf], restricted_paths: &[PathBuf]) -> CandidateFiles {
    let mut out = CandidateFiles::default();
    for root in roots {
        let root = root.as_path();
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                if e.depth() == 0 {
                    return true;
                }
                if e.file_type().is_dir() && SKIP_DIRS.iter().any(|d| name == *d) {
                    return false;
                }
                true
            })
        {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().to_path_buf();
            if !restricted_paths.is_empty()
                && !path_matches_restriction(&path, root, restricted_paths)
            {
                continue;
            }
            classify_into(&mut out, &path, root);
        }
    }
    out
}

fn path_matches_restriction(path: &Path, root: &Path, restricted: &[PathBuf]) -> bool {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => path,
    };
    let rel_str = rel.to_string_lossy();
    restricted.iter().any(|r| {
        let r_str = r.to_string_lossy();
        rel_str.starts_with(r_str.as_ref())
    })
}

fn classify_into(out: &mut CandidateFiles, path: &Path, root: &Path) {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let rel = path
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let in_k8s_dir = rel.split('/').any(|seg| K8S_DIR_HINTS.contains(&seg));
    let in_workflows_dir = rel.contains(".github/workflows/");

    // Dotenv-family. Match `.env`, `.env.foo`, `.env.foo.bar` but not
    // `.env.swp`/`.env.bak` or random `*.env` rust crate examples.
    if (name == ".env" || name.starts_with(".env."))
        && !name.ends_with(".swp")
        && !name.ends_with(".bak")
        && !name.ends_with("~")
    {
        out.dotenv.push(path.to_path_buf());
        return;
    }
    if name == ".envrc" {
        out.envrc.push(path.to_path_buf());
        return;
    }

    if name == "dockerfile" || name.starts_with("dockerfile.") || name.ends_with(".dockerfile") {
        out.dockerfile.push(path.to_path_buf());
        return;
    }

    if name.starts_with("docker-compose") && (name.ends_with(".yml") || name.ends_with(".yaml")) {
        out.docker_compose.push(path.to_path_buf());
        return;
    }
    if name == "compose.yml" || name == "compose.yaml" {
        out.docker_compose.push(path.to_path_buf());
        return;
    }

    if in_workflows_dir && (name.ends_with(".yml") || name.ends_with(".yaml")) {
        out.github_workflows.push(path.to_path_buf());
        return;
    }

    if name == "tauri.conf.json" {
        out.tauri_conf.push(path.to_path_buf());
        return;
    }

    if name == "package.json" {
        out.npm_packages.push(path.to_path_buf());
        return;
    }

    // YAML files in k8s-ish locations OR named values*.yaml anywhere.
    if (name.ends_with(".yml") || name.ends_with(".yaml"))
        && (in_k8s_dir
            || name.starts_with("values")
            || name.starts_with("kustomization")
            || name.starts_with("deployment")
            || name.starts_with("statefulset")
            || name.starts_with("daemonset")
            || name.starts_with("configmap")
            || name.starts_with("secret"))
    {
        // Helm values vs k8s manifests: rough split. We keep a copy in helm
        // bucket only when the filename starts with `values` — orchestrator
        // re-classifies via apiVersion/kind if needed.
        if name.starts_with("values") {
            out.helm_values.push(path.to_path_buf());
        }
        out.k8s_yaml.push(path.to_path_buf());
        return;
    }

    // SOPS-encrypted files have varied extensions; rely on naming hints
    // (definitive detection happens in sops_marker.rs by reading the head).
    if name.ends_with(".sops.yaml")
        || name.ends_with(".sops.yml")
        || name.ends_with(".sops.json")
        || name.ends_with(".enc.yaml")
        || name.ends_with(".enc.yml")
        || name.ends_with(".sops.env")
    {
        out.sops_files.push(path.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn finds_dotenv_and_compose() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".env"), "X=1\n").unwrap();
        fs::write(root.join(".env.production"), "X=2\n").unwrap();
        fs::write(root.join("docker-compose.yml"), "services: {}\n").unwrap();
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/ci.yml"), "name: ci\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[]);
        assert_eq!(candidates.dotenv.len(), 2, "two .env files");
        assert_eq!(candidates.docker_compose.len(), 1);
        assert_eq!(candidates.github_workflows.len(), 1);
    }

    #[test]
    fn classifies_k8s_yaml_under_hint_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("k8s")).unwrap();
        fs::write(root.join("k8s/deployment.yaml"), "kind: Deployment\n").unwrap();
        fs::write(root.join("k8s/sealed.yaml"), "kind: SealedSecret\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[]);
        assert_eq!(candidates.k8s_yaml.len(), 2);
    }

    #[test]
    fn skips_node_modules_and_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        fs::write(root.join("node_modules/foo/.env"), "JUNK=1\n").unwrap();
        fs::write(root.join(".env"), "X=1\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[]);
        assert_eq!(candidates.dotenv.len(), 1, "node_modules ignored");
    }

    #[test]
    fn restricted_paths_filter_out_other_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("k8s")).unwrap();
        fs::write(root.join(".env"), "X=1\n").unwrap();
        fs::write(root.join("k8s/deployment.yaml"), "kind: Deployment\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[PathBuf::from("k8s")]);
        assert_eq!(candidates.dotenv.len(), 0);
        assert_eq!(candidates.k8s_yaml.len(), 1);
    }
}
