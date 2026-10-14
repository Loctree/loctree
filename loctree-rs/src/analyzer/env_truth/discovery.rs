//! File discovery for `loct env-truth`.
//!
//! Walks scan roots collecting every file that any sensor needs to inspect.
//! Uses a hard-coded directory blocklist (`SKIP_DIRS`) to skip irrelevant
//! directories such as `node_modules`, `target`, and `.git`. `.env*` files
//! are always visited even when they are gitignored — auditing declarations
//! regardless of commit status is the point of this tool.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

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
///
/// H5: this `WalkDir` is independent of the main scanner and gitignore-blind
/// — it only knows the hardcoded `SKIP_DIRS` list, so its file membership can
/// silently diverge from the rest of loctree's (a repo-specific ignored
/// directory not on that list gets walked anyway; a directory that merely
/// *shares a name* with a `SKIP_DIRS` entry but isn't actually ignored here
/// gets skipped anyway). `snapshot_files` (relative paths from a prior,
/// properly gitignore-aware scan, when reachable) supplements the
/// dockerfile/compose/k8s/helm/workflow/npm/tauri buckets with the real
/// scanner's own file list so this module can't quietly disagree with it.
///
/// `.env`/`.envrc`/SOPS files are deliberately EXCLUDED from that
/// supplemental pass and only ever come from the raw filesystem walk below:
/// per this module's own contract (see module doc), they must be visited
/// even when gitignored — auditing declarations regardless of commit status
/// is the whole point of `env-truth` — and the main snapshot typically never
/// indexes dotfiles as analyzable source in the first place, so there is
/// nothing useful to reuse for them anyway.
///
/// No `ignore`-crate dependency exists in this crate yet (checked
/// `Cargo.toml` before choosing this approach over adding one, which is
/// outside this executor's edit surface) — the snapshot-reuse path is the
/// documented fallback for closing this gap without one.
pub fn discover_candidates(
    roots: &[PathBuf],
    restricted_paths: &[PathBuf],
    snapshot_files: Option<&[String]>,
) -> CandidateFiles {
    let mut out = CandidateFiles::default();
    for root in roots {
        let root = root.as_path();
        if !root.exists() {
            continue;
        }
        let gitignore = gitignore_patterns(root);
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
            // N08 (audit): the walk honors the repo's own .gitignore
            // patterns (segment-matched, additive over SKIP_DIRS) for every
            // source bucket — EXCEPT the dotenv/envrc/SOPS family, whose
            // whole point is to be audited even when gitignored. The
            // snapshot cannot serve as the canonical list here because it
            // never indexes yml/Dockerfile/dotenv shapes at all.
            if ignored_by_gitignore(&path, root, &gitignore) {
                classify_into_fs_only(&mut out, &path, root);
            } else {
                classify_into(&mut out, &path, root);
            }
        }
    }

    if let Some(files) = snapshot_files
        && let Some(root) = roots.first()
    {
        for rel in files {
            let path = root.join(rel);
            if !restricted_paths.is_empty()
                && !path_matches_restriction(&path, root, restricted_paths)
            {
                continue;
            }
            classify_into_supplemental(&mut out, &path, root);
        }
    }

    out
}

/// Same bucket rules as [`classify_into`], applied through a scratch
/// [`CandidateFiles`] so dotenv/envrc/sops entries never come from the
/// snapshot-reuse pass (see [`discover_candidates`] doc comment), and
/// de-duplicated against whatever the raw filesystem walk already found.
fn classify_into_supplemental(out: &mut CandidateFiles, path: &Path, root: &Path) {
    let mut probe = CandidateFiles::default();
    classify_into(&mut probe, path, root);

    macro_rules! merge {
        ($field:ident) => {
            for p in probe.$field {
                if !out.$field.contains(&p) {
                    out.$field.push(p);
                }
            }
        };
    }
    merge!(dockerfile);
    merge!(docker_compose);
    merge!(k8s_yaml);
    merge!(helm_values);
    merge!(github_workflows);
    merge!(npm_packages);
    merge!(tauri_conf);
    // dotenv / envrc / sops_files intentionally NOT merged - FS-walk-only.
}

/// Walk-side counterpart of [`classify_into_supplemental`]: keep only the
/// buckets that must come from the raw filesystem (dotenv/envrc/SOPS — the
/// module's deliberate, named gitignore exception).
fn classify_into_fs_only(out: &mut CandidateFiles, path: &Path, root: &Path) {
    let mut probe = CandidateFiles::default();
    classify_into(&mut probe, path, root);
    macro_rules! merge {
        ($field:ident) => {
            for p in probe.$field {
                if !out.$field.contains(&p) {
                    out.$field.push(p);
                }
            }
        };
    }
    merge!(dotenv);
    merge!(envrc);
    merge!(sops_files);
}

/// Plain (non-glob, non-negated) `.gitignore` lines of `root`, for
/// segment-boundary matching. Additive over `SKIP_DIRS`, never subtractive —
/// same contract as `layoutmap::gitignore_patterns` (audit N08).
fn gitignore_patterns(root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(root.join(".gitignore")) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with('!')
                && !line.contains('*')
        })
        .map(|line| {
            line.trim_start_matches('/')
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

fn ignored_by_gitignore(path: &Path, root: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|pattern| {
        let bytes = rel_str.as_bytes();
        let mut from = 0usize;
        while let Some(pos) = rel_str[from..].find(pattern.as_str()) {
            let begin = from + pos;
            let end = begin + pattern.len();
            let before_ok = begin == 0 || bytes[begin - 1] == b'/';
            let after_ok = end == bytes.len() || bytes[end] == b'/';
            if before_ok && after_ok {
                return true;
            }
            from = begin + 1;
        }
        false
    })
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
    let is_leading_dotenv = (name == ".env" || name.starts_with(".env."))
        && !name.ends_with(".swp")
        && !name.ends_with(".bak")
        && !name.ends_with('~');
    // H5: monorepo naming conventions like `s4.env.example` are dotenv
    // TEMPLATES even though they don't start with a leading `.env` - the
    // `.env.example`/`.env.template`/`.env.sample` SUFFIX is the actual
    // signal, not the leading dot. Without this, such a file never even
    // becomes a candidate, so `is_template_path`'s substring match on
    // "example"/"template"/"sample" (see `precedence.rs`) never gets a
    // chance to run — the file is invisible to env-truth entirely.
    let is_prefixed_dotenv_template = name.ends_with(".env.example")
        || name.ends_with(".env.template")
        || name.ends_with(".env.sample");
    if is_leading_dotenv || is_prefixed_dotenv_template {
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

        let candidates = discover_candidates(&[root.to_path_buf()], &[], None);
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

        let candidates = discover_candidates(&[root.to_path_buf()], &[], None);
        assert_eq!(candidates.k8s_yaml.len(), 2);
    }

    #[test]
    fn skips_node_modules_and_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        fs::write(root.join("node_modules/foo/.env"), "JUNK=1\n").unwrap();
        fs::write(root.join(".env"), "X=1\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[], None);
        assert_eq!(candidates.dotenv.len(), 1, "node_modules ignored");
    }

    #[test]
    fn restricted_paths_filter_out_other_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("k8s")).unwrap();
        fs::write(root.join(".env"), "X=1\n").unwrap();
        fs::write(root.join("k8s/deployment.yaml"), "kind: Deployment\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[PathBuf::from("k8s")], None);
        assert_eq!(candidates.dotenv.len(), 0);
        assert_eq!(candidates.k8s_yaml.len(), 1);
    }

    /// H5 RED: `s4.env.example` is a dotenv TEMPLATE by monorepo convention
    /// (`<service>.env.example`) even though it has no leading `.env` — the
    /// old leading-dot-only match made it invisible to env-truth entirely.
    #[test]
    fn prefixed_env_example_is_classified_as_dotenv_template() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("s4.env.example"), "S4_API_KEY=fill-me\n").unwrap();

        let candidates = discover_candidates(&[root.to_path_buf()], &[], None);
        assert_eq!(
            candidates.dotenv.len(),
            1,
            "s4.env.example must be discovered as a dotenv-family file"
        );
        assert!(
            super::super::precedence::is_template_path("s4.env.example"),
            "once discovered, s4.env.example must still classify as a template"
        );
    }

    /// H5 RED: the independent WalkDir's hardcoded `SKIP_DIRS` list can
    /// disagree with the real (gitignore-aware) scanner — a repo-specific
    /// directory literally named "dist" that ISN'T actually ignored here
    /// still gets skipped by this module's own walk. Supplementing with the
    /// snapshot's own file list recovers files the raw walk missed instead
    /// of silently disagreeing with the rest of loctree.
    #[test]
    fn snapshot_file_list_recovers_files_skipped_by_hardcoded_skip_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("dist/tauri.conf.json"), "{}\n").unwrap();

        let without_snapshot = discover_candidates(&[root.to_path_buf()], &[], None);
        assert_eq!(
            without_snapshot.tauri_conf.len(),
            0,
            "the raw FS walk alone misses it (SKIP_DIRS hardcodes \"dist\")"
        );

        let snapshot_files = vec!["dist/tauri.conf.json".to_string()];
        let with_snapshot = discover_candidates(&[root.to_path_buf()], &[], Some(&snapshot_files));
        assert_eq!(
            with_snapshot.tauri_conf.len(),
            1,
            "the snapshot's own file list must recover the file the raw walk skipped"
        );
    }
    /// N08 (audit): the walk honors the repo's own .gitignore for source
    /// buckets — only the dotenv/SOPS family keeps its deliberate
    /// audited-even-when-ignored exception.
    #[test]
    fn snapshot_restricts_walked_source_buckets_but_never_dotenv() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::write(root.join("vendor/Dockerfile"), "FROM scratch\n").unwrap();
        std::fs::write(root.join("Dockerfile"), "FROM scratch\n").unwrap();
        std::fs::write(root.join(".env"), "KEY=1\n").unwrap();
        std::fs::write(root.join(".gitignore"), "vendor/\n.env\n").unwrap();

        let out = discover_candidates(&[root.to_path_buf()], &[], None);

        assert!(
            out.dockerfile
                .iter()
                .any(|p| p.ends_with("Dockerfile") && !p.to_string_lossy().contains("vendor")),
            "the snapshot-listed Dockerfile is present: {:?}",
            out.dockerfile
        );
        assert!(
            !out.dockerfile
                .iter()
                .any(|p| p.to_string_lossy().contains("vendor")),
            "a walked file missing from the snapshot must not join a source bucket: {:?}",
            out.dockerfile
        );
        assert!(
            out.dotenv.iter().any(|p| p.ends_with(".env")),
            ".env stays discovered by contract even when gitignored: {:?}",
            out.dotenv
        );
    }
}
