//! Stack detection module for auto-configuring loctree based on project files.
//!
//! Detects project type by checking for common configuration files:
//! - Cargo.toml → Rust project
//! - tsconfig.json / package.json → TypeScript/JavaScript
//! - pyproject.toml / setup.py → Python
//! - src-tauri/ → Tauri preset
//! - vite.config.* → Vite project

use std::collections::HashSet;
use std::path::Path;

/// Result of stack detection
#[derive(Clone, Debug, Default)]
pub struct DetectedStack {
    /// File extensions to scan
    pub extensions: HashSet<String>,
    /// Patterns to ignore
    pub ignores: Vec<String>,
    /// Detected preset name (e.g., "tauri")
    pub preset_name: Option<String>,
    /// Human-readable description of detected stack
    pub description: String,
    /// Whether this appears to be a library/framework project (not an app)
    pub is_library: bool,
    /// Additional Python import roots (e.g., "Lib" for CPython)
    pub py_roots: Vec<std::path::PathBuf>,
}

impl DetectedStack {
    /// Check if anything was detected
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty() && self.preset_name.is_none()
    }
}

/// Detect additional Python package roots beyond the standard locations.
///
/// Heuristics:
/// 1. CPython/PyPy layout: `Lib/` directory alongside `Python/`, `Modules/`
/// 2. Directories with `__init__.py` that aren't standard names (src, tests, etc.)
/// 3. Hints from pyproject.toml `[tool.setuptools.packages]` or similar
fn detect_python_roots(root: &Path) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();

    // CPython/PyPy detection: Lib/ alongside Python/ or Modules/
    let lib_dir = root.join("Lib");
    if lib_dir.is_dir() {
        let has_python_dir = root.join("Python").is_dir();
        let has_modules_dir = root.join("Modules").is_dir();
        let has_include_dir = root.join("Include").is_dir();

        // CPython has Lib + (Python or Modules or Include)
        if has_python_dir || has_modules_dir || has_include_dir {
            roots.push(std::path::PathBuf::from("Lib"));
        }
    }

    // Check pyproject.toml for explicit package locations
    let pyproject_path = root.join("pyproject.toml");
    if pyproject_path.exists()
        && let Ok(content) = std::fs::read_to_string(&pyproject_path)
    {
        // Look for [tool.setuptools.packages] or package-dir patterns
        // Simple heuristic: find lines like `packages = ["something"]`
        for line in content.lines() {
            let trimmed = line.trim();
            // Match patterns like: packages = ["Lib"] or package-dir = {src = "Lib"}
            if (trimmed.starts_with("packages") || trimmed.starts_with("package-dir"))
                && trimmed.contains('=')
            {
                // Extract directory names from the value
                if let Some(value_part) = trimmed.split('=').nth(1) {
                    for segment in value_part.split(['"', '\'', ',', '[', ']', '{', '}']) {
                        let dir_name = segment.trim();
                        if !dir_name.is_empty()
                            && !dir_name.contains('=')
                            && !dir_name.contains(':')
                            && dir_name != "src"
                            && dir_name != "."
                        {
                            let dir_path = root.join(dir_name);
                            if dir_path.is_dir() && !roots.contains(&dir_name.into()) {
                                roots.push(std::path::PathBuf::from(dir_name));
                            }
                        }
                    }
                }
            }
        }
    }

    roots
}

/// Detect project stack from root directory
pub fn detect_stack(root: &Path) -> DetectedStack {
    let mut result = DetectedStack::default();
    let mut detected_parts: Vec<&str> = Vec::new();

    // Check for Cargo.toml -> Rust project
    // Also check direct subdirectories for monorepo-style layouts (e.g., codex-rs/Cargo.toml)
    let has_cargo_toml = root.join("Cargo.toml").exists() || has_cargo_in_subdir(root);
    if has_cargo_toml {
        result.extensions.insert("rs".to_string());
        result.extensions.insert("toml".to_string());
        result.ignores.push("target".to_string());
        detected_parts.push("Rust");
    }

    // Check for Dart/Flutter (pubspec.yaml)
    if root.join("pubspec.yaml").exists() {
        result.extensions.insert("dart".to_string());
        result.ignores.push(".dart_tool".to_string());
        result.ignores.push("build".to_string());
        result.ignores.push(".packages".to_string());
        detected_parts.push("Dart/Flutter");
    }

    // Check for Go projects (go.mod or .go files)
    if root.join("go.mod").exists()
        || root
            .read_dir()
            .ok()
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("go"))
                })
            })
            .unwrap_or(false)
    {
        result.extensions.insert("go".to_string());
        result.ignores.push("vendor".to_string());
        detected_parts.push("Go");
    }

    // Check for src-tauri/ -> Tauri preset (must check before generic TS)
    if root.join("src-tauri").exists() {
        result.preset_name = Some("tauri".to_string());
        result.extensions.insert("rs".to_string());
        result.extensions.insert("toml".to_string());
        result.extensions.insert("ts".to_string());
        result.extensions.insert("tsx".to_string());
        result.extensions.insert("js".to_string());
        result.extensions.insert("jsx".to_string());
        result.extensions.insert("css".to_string());
        result.ignores.push("target".to_string());
        result.ignores.push("node_modules".to_string());
        result.ignores.push("dist".to_string());
        detected_parts.push("Tauri");
    }

    // Check for package.json + tsconfig.json -> TypeScript
    let has_tsconfig = root.join("tsconfig.json").exists();
    let has_package_json = root.join("package.json").exists();

    if has_tsconfig || has_package_json {
        result.extensions.insert("ts".to_string());
        result.extensions.insert("tsx".to_string());
        result.extensions.insert("js".to_string());
        result.extensions.insert("jsx".to_string());
        result.extensions.insert("mjs".to_string());
        result.extensions.insert("cjs".to_string());

        if !result.ignores.contains(&"node_modules".to_string()) {
            result.ignores.push("node_modules".to_string());
        }
        if !result.ignores.contains(&"dist".to_string()) {
            result.ignores.push("dist".to_string());
        }

        if has_tsconfig && !detected_parts.contains(&"Tauri") {
            detected_parts.push("TypeScript");
        } else if has_package_json && !detected_parts.contains(&"Tauri") {
            detected_parts.push("JavaScript");
        }

        // Check if this is a library/framework project
        if is_npm_library(root) {
            result.is_library = true;
            if !detected_parts.contains(&"Library") {
                detected_parts.push("Library");
            }
        }
    }

    // Check for vite.config.* -> Vite project (add build to ignores)
    let vite_extensions = ["js", "ts", "mjs"];
    for ext in vite_extensions {
        if root.join(format!("vite.config.{}", ext)).exists() {
            if !result.ignores.contains(&"dist".to_string()) {
                result.ignores.push("dist".to_string());
            }
            result.ignores.push("build".to_string());
            if !detected_parts.contains(&"Vite") {
                detected_parts.push("Vite");
            }
            break;
        }
    }

    // Check for svelte.config.* -> SvelteKit project
    let svelte_exists =
        root.join("svelte.config.js").exists() || root.join("svelte.config.ts").exists();
    // Also check apps/* and packages/* for monorepos
    let mut svelte_in_subdir = false;
    for subdir in ["apps", "packages"] {
        let dir = root.join(subdir);
        if dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir()
                    && (path.join("svelte.config.js").exists()
                        || path.join("svelte.config.ts").exists())
                {
                    svelte_in_subdir = true;
                    break;
                }
            }
        }
        if svelte_in_subdir {
            break;
        }
    }
    if svelte_exists || svelte_in_subdir {
        result.extensions.insert("svelte".to_string());
        result.ignores.push(".svelte-kit".to_string());
        if !detected_parts.contains(&"SvelteKit") {
            detected_parts.push("SvelteKit");
        }
    }

    // Check for astro.config.* -> Astro project
    let astro_exists = has_astro_config(root);
    let mut astro_in_subdir = false;
    for subdir in ["apps", "packages"] {
        let dir = root.join(subdir);
        if dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() && has_astro_config(&path) {
                    astro_in_subdir = true;
                    break;
                }
            }
        }
        if astro_in_subdir {
            break;
        }
    }
    if astro_exists || astro_in_subdir {
        result.extensions.insert("astro".to_string());
        result.extensions.insert("ts".to_string());
        result.extensions.insert("js".to_string());
        if !detected_parts.contains(&"Astro") {
            detected_parts.push("Astro");
        }
    }

    // Check for Vue projects (vue.config.*, vite.config.* with Vue, or .vue files in src/)
    let vue_config_exists =
        root.join("vue.config.js").exists() || root.join("vue.config.ts").exists();
    let has_vue_files = root.join("src").exists()
        && std::fs::read_dir(root.join("src"))
            .map(|entries| {
                entries.flatten().any(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("vue"))
                })
            })
            .unwrap_or(false);
    // Also check packages/* for monorepos (common in Vue ecosystem)
    let mut vue_in_subdir = false;
    for subdir in ["packages", "packages-private", "apps"] {
        let dir = root.join(subdir);
        if dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    // Check for .vue files in this package's src/
                    let pkg_src = path.join("src");
                    if pkg_src.is_dir()
                        && std::fs::read_dir(&pkg_src)
                            .map(|entries| {
                                entries.flatten().any(|e| {
                                    e.path()
                                        .extension()
                                        .is_some_and(|ext| ext.eq_ignore_ascii_case("vue"))
                                })
                            })
                            .unwrap_or(false)
                    {
                        vue_in_subdir = true;
                        break;
                    }
                }
            }
        }
        if vue_in_subdir {
            break;
        }
    }
    if vue_config_exists || has_vue_files || vue_in_subdir {
        result.extensions.insert("vue".to_string());
        if !detected_parts.contains(&"Vue") {
            detected_parts.push("Vue");
        }
    }

    // Check for pyproject.toml / setup.py -> Python
    if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        result.extensions.insert("py".to_string());
        result.ignores.push(".venv".to_string());
        result.ignores.push("venv".to_string());
        result.ignores.push("__pycache__".to_string());
        result.ignores.push(".pytest_cache".to_string());
        result.ignores.push(".mypy_cache".to_string());
        result.ignores.push(".ruff_cache".to_string());
        result.ignores.push("*.egg-info".to_string());
        result.ignores.push(".eggs".to_string());
        result.ignores.push("dist".to_string());
        result.ignores.push("build".to_string());
        result.ignores.push(".tox".to_string());
        // Common ML/data caches that often contain symlinks
        result.ignores.push(".fastembed_cache".to_string());
        result.ignores.push(".cache".to_string());
        result.ignores.push("logs".to_string());
        result.ignores.push("packaging".to_string());
        // uv specific
        result.ignores.push(".uv".to_string());
        detected_parts.push("Python");

        // Auto-detect additional Python package roots
        result.py_roots = detect_python_roots(root);
    }

    // Check for style assets in common locations. Rust-first UI stacks such
    // as Leptos can carry their design system in `styles/*.css` without any
    // JS/TS config, so style assets must not be gated on npm detection.
    if has_style_assets(root) {
        for ext in ["css", "scss", "less"] {
            result.extensions.insert(ext.to_string());
        }
        if !detected_parts.contains(&"Styles") {
            detected_parts.push("Styles");
        }
    }

    // NOTE (W1-a snapshot authority): dev/test noise directories (e2e,
    // scripts, mobile, __mocks__, __fixtures__) are intentionally NOT added
    // as scan-level ignores anymore. Hiding them removed real files (e.g.
    // `scripts/*.sh`) from the snapshot universe, so slice/find could not see
    // them and internal rescans disagreed with the initial scan. Noise
    // reduction for dead-export reports happens at report level
    // (`classify.rs`, `dead_parrots/filters.rs`), not by shrinking the
    // universe. Detection ignores stay limited to build artifacts.

    // Build description
    if !detected_parts.is_empty() {
        result.description = format!("Detected: {}", detected_parts.join(" + "));
    }

    result
}

/// Check if any direct subdirectory contains a Cargo.toml (monorepo detection)
fn has_cargo_in_subdir(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip common non-Rust directories
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.')
                || name_str == "node_modules"
                || name_str == "dist"
                || name_str == "build"
            {
                continue;
            }

            if path.join("Cargo.toml").exists() {
                return true;
            }
        }
    }
    false
}

fn has_astro_config(root: &Path) -> bool {
    ["js", "ts", "mjs"]
        .iter()
        .any(|ext| root.join(format!("astro.config.{ext}")).exists())
}

fn has_style_assets(root: &Path) -> bool {
    const STYLE_EXTS: &[&str] = &["css", "scss", "less"];
    const STYLE_DIRS: &[&str] = &[
        "styles", "style", "css", "assets", "public", "static", "src",
    ];

    if STYLE_EXTS
        .iter()
        .any(|ext| root.join(format!("main.{ext}")).exists())
    {
        return true;
    }

    STYLE_DIRS
        .iter()
        .any(|dir| dir_has_style_assets(&root.join(dir)))
}

fn dir_has_style_assets(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let path = entry.path();
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(ext.to_ascii_lowercase().as_str(), "css" | "scss" | "less")
                    })
        })
}

/// Check if a package.json indicates this is a library/framework (not an app)
///
/// Library indicators:
/// - Has "exports" field (npm package exports map)
/// - Has "main", "module", or "types" field (package entry points)
/// - Has "packages/" directory (monorepo with publishable packages)
/// - Lacks typical app indicators (index.html, vite.config.*, etc.)
fn is_npm_library(root: &Path) -> bool {
    let package_json_path = root.join("package.json");
    if !package_json_path.exists() {
        return false;
    }

    // Read and parse package.json
    let Ok(content) = std::fs::read_to_string(&package_json_path) else {
        return false;
    };

    let Ok(parsed): Result<serde_json::Value, _> = serde_json::from_str(&content) else {
        return false;
    };

    // Strong library indicators
    if parsed.get("exports").is_some() {
        // Modern npm package with exports field
        return true;
    }

    // Has package entry points (main, module, types)
    let has_main = parsed.get("main").is_some();
    let has_module = parsed.get("module").is_some();
    let has_types = parsed.get("types").is_some() || parsed.get("typings").is_some();

    if has_main || has_module || has_types {
        // Check if it's NOT an app by looking for app-specific files
        let has_index_html = root.join("index.html").exists();
        let has_public_html = root.join("public/index.html").exists();

        if !has_index_html && !has_public_html {
            // Likely a library - has entry points but no HTML
            return true;
        }
    }

    // Check for monorepo packages/ directory
    let packages_dir = root.join("packages");
    if packages_dir.is_dir()
        && std::fs::read_dir(&packages_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .any(|e| e.path().join("package.json").exists())
            })
            .unwrap_or(false)
    {
        return true;
    }

    false
}

/// Apply detected stack to parsed args if no explicit config provided
pub fn apply_detected_stack(
    root: &Path,
    extensions: &mut Option<HashSet<String>>,
    ignore_patterns: &mut Vec<String>,
    tauri_preset: &mut bool,
    library_mode: &mut bool,
    py_roots: &mut Vec<std::path::PathBuf>,
    verbose: bool,
) {
    // Skip if user already specified extensions
    if extensions.is_some() {
        return;
    }

    // Skip if tauri preset is already set
    if *tauri_preset {
        return;
    }

    let detected = detect_stack(root);

    if detected.is_empty() {
        return;
    }

    if verbose && !detected.description.is_empty() {
        eprintln!("[loctree][detect] {}", detected.description);
    }

    // NOTE (W1-a snapshot authority): detected extensions are intentionally
    // NOT applied anymore. Narrowing the scan universe to the detected stack
    // (e.g. rs+toml on a Cargo root) made the initial scan disagree with
    // every internal drift-rescan (default extensions), so the snapshot
    // fingerprints never converged — a self-sustaining [DRIFT] loop — and
    // agents could not slice/find ts/py/sh files in mixed repos. The file
    // universe is the default analyzer extension set everywhere; detection
    // still contributes ignores, presets, library mode and Python roots.
    // Users who want a narrower universe pass explicit extensions.
    let _ = &detected.extensions;

    // Apply ignores only if user didn't specify any
    if ignore_patterns.is_empty() {
        *ignore_patterns = detected.ignores;
    }

    // Apply preset
    if let Some(preset) = detected.preset_name
        && preset == "tauri"
    {
        *tauri_preset = true;
    }

    // Apply library mode if detected and not already set by user
    if detected.is_library && !*library_mode {
        *library_mode = true;
        if verbose {
            eprintln!(
                "[loctree][detect] Detected library/framework project - enabling library mode"
            );
        }
    }

    // Apply detected Python roots if user didn't specify any
    if py_roots.is_empty() && !detected.py_roots.is_empty() {
        *py_roots = detected.py_roots;
        if verbose {
            let roots_str: Vec<_> = py_roots.iter().map(|p| p.display().to_string()).collect();
            eprintln!(
                "[loctree][detect] Auto-detected Python roots: {}",
                roots_str.join(", ")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_rust_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"")
            .expect("write Cargo.toml");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("rs"));
        assert!(detected.extensions.contains("toml"));
        assert!(detected.ignores.contains(&"target".to_string()));
    }

    #[test]
    fn detects_rust_project_style_assets_without_javascript() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"")
            .expect("write Cargo.toml");
        std::fs::create_dir(tmp.path().join("styles")).expect("create styles dir");
        std::fs::write(
            tmp.path().join("styles").join("tokens.css"),
            ":root { --accent: red; }",
        )
        .expect("write css tokens");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("rs"));
        assert!(detected.extensions.contains("css"));
        assert!(detected.description.contains("Styles"));
    }

    #[test]
    fn test_detect_typescript_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("tsconfig.json"), "{}").expect("write tsconfig.json");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("ts"));
        assert!(detected.extensions.contains("tsx"));
        assert!(detected.ignores.contains(&"node_modules".to_string()));
    }

    #[test]
    fn test_detect_tauri_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::create_dir(tmp.path().join("src-tauri")).expect("create src-tauri dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"")
            .expect("write Cargo.toml");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write package.json");

        let detected = detect_stack(tmp.path());

        assert_eq!(detected.preset_name, Some("tauri".to_string()));
        assert!(detected.extensions.contains("rs"));
        assert!(detected.extensions.contains("toml"));
        assert!(detected.extensions.contains("ts"));
        assert!(detected.extensions.contains("tsx"));
    }

    #[test]
    fn test_detect_python_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"test\"",
        )
        .expect("write pyproject.toml");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("py"));
        assert!(detected.ignores.contains(&".venv".to_string()));
        assert!(detected.ignores.contains(&"__pycache__".to_string()));
    }

    #[test]
    fn test_detect_empty_project() {
        let tmp = TempDir::new().expect("create temp dir");

        let detected = detect_stack(tmp.path());

        assert!(detected.is_empty());
    }

    #[test]
    fn test_detect_mixed_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "").expect("write Cargo.toml");
        std::fs::write(tmp.path().join("pyproject.toml"), "").expect("write pyproject.toml");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("rs"));
        assert!(detected.extensions.contains("py"));
    }

    #[test]
    fn test_detect_vite_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("vite.config.ts"), "export default {}").expect("write");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write");

        let detected = detect_stack(tmp.path());

        assert!(detected.ignores.contains(&"build".to_string()));
        assert!(detected.description.contains("Vite"));
    }

    #[test]
    fn test_detect_astro_project() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("astro.config.mjs"), "export default {}").expect("write");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("astro"));
        assert!(detected.description.contains("Astro"));
    }

    #[test]
    fn test_detect_astro_monorepo_app() {
        let tmp = TempDir::new().expect("create temp dir");
        let app = tmp.path().join("apps/site");
        std::fs::create_dir_all(&app).expect("create astro app");
        std::fs::write(app.join("astro.config.ts"), "export default {}").expect("write");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("astro"));
        assert!(detected.description.contains("Astro"));
    }

    #[test]
    fn test_detect_javascript_only() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write package.json");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("js"));
        assert!(detected.description.contains("JavaScript"));
    }

    #[test]
    fn test_detect_with_src_adds_css() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write package.json");
        std::fs::create_dir(tmp.path().join("src")).expect("create src");
        std::fs::write(tmp.path().join("src").join("main.css"), "body {}").expect("write css");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("css"));
    }

    #[test]
    fn detected_stack_does_not_hide_fixtures_from_snapshot() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"")
            .expect("write Cargo.toml");

        let detected = detect_stack(tmp.path());

        // W1-a snapshot authority: only build artifacts are ignored at scan
        // level. Dev/test noise dirs stay in the snapshot universe so
        // slice/find can see them; reports filter them downstream.
        for noise in [
            "fixtures",
            "__fixtures__",
            "scripts",
            "e2e",
            "mobile",
            "__mocks__",
        ] {
            assert!(
                !detected.ignores.contains(&noise.to_string()),
                "scan-level ignores must not hide {noise} from the snapshot"
            );
        }
        assert!(detected.ignores.contains(&"target".to_string()));
    }

    #[test]
    fn test_apply_detected_stack_skips_if_extensions_set() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "").expect("write");

        let mut extensions = Some(HashSet::from(["py".to_string()]));
        let mut ignores = Vec::new();
        let mut tauri = false;
        let mut library_mode = false;

        apply_detected_stack(
            tmp.path(),
            &mut extensions,
            &mut ignores,
            &mut tauri,
            &mut library_mode,
            &mut Vec::new(),
            false,
        );

        // Should not have changed - user specified extensions
        assert!(extensions.as_ref().unwrap().contains("py"));
        assert!(!extensions.as_ref().unwrap().contains("rs"));
    }

    #[test]
    fn test_apply_detected_stack_skips_if_tauri_preset() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "").expect("write");

        let mut extensions: Option<HashSet<String>> = None;
        let mut ignores = Vec::new();
        let mut tauri = true; // Already set
        let mut library_mode = false;

        apply_detected_stack(
            tmp.path(),
            &mut extensions,
            &mut ignores,
            &mut tauri,
            &mut library_mode,
            &mut Vec::new(),
            false,
        );

        // Should not have changed - tauri already set
        assert!(extensions.is_none());
    }

    #[test]
    fn test_apply_detected_stack_applies_tauri() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::create_dir(tmp.path().join("src-tauri")).expect("mkdir");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write");

        let mut extensions: Option<HashSet<String>> = None;
        let mut ignores = Vec::new();
        let mut tauri = false;
        let mut library_mode = false;

        apply_detected_stack(
            tmp.path(),
            &mut extensions,
            &mut ignores,
            &mut tauri,
            &mut library_mode,
            &mut Vec::new(),
            false,
        );

        assert!(tauri);
        // W1-a snapshot authority: detection no longer narrows the scan
        // universe — extensions stay None so the default analyzer set is
        // used by initial scan and rescan alike.
        assert!(extensions.is_none());
    }

    #[test]
    fn test_apply_detected_stack_preserves_user_ignores() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "").expect("write");

        let mut extensions: Option<HashSet<String>> = None;
        let mut ignores = vec!["custom_ignore".to_string()];
        let mut tauri = false;
        let mut library_mode = false;

        apply_detected_stack(
            tmp.path(),
            &mut extensions,
            &mut ignores,
            &mut tauri,
            &mut library_mode,
            &mut Vec::new(),
            false,
        );

        // Should NOT have applied detected ignores since user specified their own
        assert_eq!(ignores, vec!["custom_ignore".to_string()]);
    }

    #[test]
    fn test_apply_detected_stack_verbose() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("Cargo.toml"), "").expect("write");

        let mut extensions: Option<HashSet<String>> = None;
        let mut ignores = Vec::new();
        let mut tauri = false;
        let mut library_mode = false;

        // Should not panic with verbose=true
        apply_detected_stack(
            tmp.path(),
            &mut extensions,
            &mut ignores,
            &mut tauri,
            &mut library_mode,
            &mut Vec::new(),
            true,
        );
    }

    #[test]
    fn test_detected_stack_is_empty() {
        let empty = DetectedStack::default();
        assert!(empty.is_empty());

        let with_ext = DetectedStack {
            extensions: HashSet::from(["rs".to_string()]),
            ..Default::default()
        };
        assert!(!with_ext.is_empty());

        let with_preset = DetectedStack {
            preset_name: Some("tauri".to_string()),
            ..Default::default()
        };
        assert!(!with_preset.is_empty());
    }

    #[test]
    fn test_detect_rust_in_subdirectory() {
        // Monorepo layout: package.json at root, Cargo.toml in subdirectory
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write package.json");
        std::fs::create_dir(tmp.path().join("codex-rs")).expect("mkdir codex-rs");
        std::fs::write(
            tmp.path().join("codex-rs").join("Cargo.toml"),
            "[package]\nname = \"test\"",
        )
        .expect("write Cargo.toml");

        let detected = detect_stack(tmp.path());

        // Should detect both JavaScript and Rust
        assert!(detected.extensions.contains("rs"));
        assert!(detected.extensions.contains("js"));
        assert!(detected.description.contains("JavaScript"));
        assert!(detected.description.contains("Rust"));
    }

    #[test]
    fn test_has_cargo_in_subdir() {
        let tmp = TempDir::new().expect("create temp dir");

        // No subdirs yet
        assert!(!has_cargo_in_subdir(tmp.path()));

        // Add a subdir without Cargo.toml
        std::fs::create_dir(tmp.path().join("src")).expect("mkdir");
        assert!(!has_cargo_in_subdir(tmp.path()));

        // Add a subdir with Cargo.toml
        std::fs::create_dir(tmp.path().join("backend")).expect("mkdir");
        std::fs::write(tmp.path().join("backend").join("Cargo.toml"), "").expect("write");
        assert!(has_cargo_in_subdir(tmp.path()));
    }

    #[test]
    fn test_has_cargo_in_subdir_skips_hidden() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::create_dir(tmp.path().join(".hidden")).expect("mkdir");
        std::fs::write(tmp.path().join(".hidden").join("Cargo.toml"), "").expect("write");

        // Should not find Cargo.toml in hidden directories
        assert!(!has_cargo_in_subdir(tmp.path()));
    }

    #[test]
    fn test_detect_library_with_exports_field() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "solid-js", "exports": {"./jsx-runtime": "./jsx-runtime/index.js"}}"#,
        )
        .expect("write package.json");

        let detected = detect_stack(tmp.path());

        assert!(
            detected.is_library,
            "Should detect library project with exports field"
        );
        assert!(detected.description.contains("Library"));
    }

    #[test]
    fn test_detect_library_with_main_field_no_html() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "some-lib", "main": "dist/index.js", "types": "dist/index.d.ts"}"#,
        )
        .expect("write package.json");

        let detected = detect_stack(tmp.path());

        assert!(
            detected.is_library,
            "Should detect library with main/types but no HTML"
        );
    }

    #[test]
    fn test_detect_app_with_index_html() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "some-app", "main": "src/main.js"}"#,
        )
        .expect("write package.json");
        std::fs::write(tmp.path().join("index.html"), "<!DOCTYPE html>").expect("write index.html");

        let detected = detect_stack(tmp.path());

        assert!(
            !detected.is_library,
            "Should NOT detect library when index.html exists"
        );
    }

    #[test]
    fn test_detect_monorepo_with_packages() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("package.json"), "{}").expect("write package.json");
        std::fs::create_dir(tmp.path().join("packages")).expect("mkdir packages");
        std::fs::create_dir(tmp.path().join("packages/foo")).expect("mkdir foo");
        std::fs::write(tmp.path().join("packages/foo/package.json"), "{}")
            .expect("write foo package.json");

        let detected = detect_stack(tmp.path());

        assert!(
            detected.is_library,
            "Should detect monorepo with packages/ as library"
        );
    }

    #[test]
    fn test_library_mode_applied_automatically() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "test-lib", "exports": {"./index": "./index.js"}}"#,
        )
        .expect("write package.json");

        let mut extensions: Option<HashSet<String>> = None;
        let mut ignores = Vec::new();
        let mut tauri = false;
        let mut library_mode = false;

        apply_detected_stack(
            tmp.path(),
            &mut extensions,
            &mut ignores,
            &mut tauri,
            &mut library_mode,
            &mut Vec::new(),
            false,
        );

        assert!(
            library_mode,
            "Library mode should be auto-enabled for library projects"
        );
    }

    #[test]
    fn test_detect_cpython_py_roots() {
        // CPython layout: Lib/ alongside Python/ and Modules/
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"cpython\"",
        )
        .expect("write pyproject.toml");
        std::fs::create_dir(tmp.path().join("Lib")).expect("mkdir Lib");
        std::fs::create_dir(tmp.path().join("Python")).expect("mkdir Python");
        std::fs::create_dir(tmp.path().join("Modules")).expect("mkdir Modules");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("py"));
        assert_eq!(detected.py_roots.len(), 1);
        assert_eq!(detected.py_roots[0], std::path::PathBuf::from("Lib"));
    }

    #[test]
    fn test_detect_no_py_roots_for_standard_layout() {
        // Standard Python project without special py_roots
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"myapp\"",
        )
        .expect("write pyproject.toml");
        std::fs::create_dir(tmp.path().join("src")).expect("mkdir src");
        std::fs::create_dir(tmp.path().join("tests")).expect("mkdir tests");

        let detected = detect_stack(tmp.path());

        assert!(detected.extensions.contains("py"));
        assert!(
            detected.py_roots.is_empty(),
            "Standard layout should not add py_roots"
        );
    }
}
