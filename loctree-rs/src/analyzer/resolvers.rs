use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use regex::Regex;
use serde_json;
use serde_json::Value;

/// SvelteKit virtual modules that are provided by the framework at runtime.
/// These don't have actual file sources and should be recognized as valid imports.
/// See: https://kit.svelte.dev/docs/modules
const SVELTEKIT_VIRTUAL_MODULES: &[&str] = &[
    "$app/environment",
    "$app/forms",
    "$app/navigation",
    "$app/paths",
    "$app/server",
    "$app/stores",
    "$app/state",
    "$env/static/public",
    "$env/static/private",
    "$env/dynamic/public",
    "$env/dynamic/private",
    "$service-worker",
];

/// Check if an import specifier is a SvelteKit virtual module
pub(crate) fn is_sveltekit_virtual_module(spec: &str) -> bool {
    // Exact match for known virtual modules
    if SVELTEKIT_VIRTUAL_MODULES.contains(&spec) {
        return true;
    }
    // Check prefix patterns for $app/* and $env/*
    spec.starts_with("$app/") || spec.starts_with("$env/") || spec == "$service-worker"
}

/// Simple TS/JS path resolver backed by tsconfig.json `baseUrl` and `paths`.
/// Supports alias patterns with wildcards and falls back to `baseUrl`.
/// Also checks package.json exports field as fallback.
/// Now also parses vite.config.js/ts for resolve.alias configuration.
#[derive(Debug)]
pub(crate) struct TsPathResolver {
    base_dir: PathBuf,
    root: PathBuf,
    mappings: Vec<AliasMapping>,
    cache: Mutex<HashMap<String, Option<String>>>,
    package_exports: HashMap<String, String>,
    /// Vite resolve.alias mappings (prefix -> target path)
    vite_aliases: HashMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
struct AliasMapping {
    pattern: String,
    targets: Vec<String>,
    wildcard_count: usize,
}

/// Extracted resolver configuration for caching in snapshots
#[derive(Debug, Clone, Default)]
pub struct ExtractedResolverConfig {
    /// TypeScript path aliases
    pub ts_paths: HashMap<String, Vec<String>>,
    /// Base URL for resolution
    pub ts_base_url: Option<String>,
}

impl TsPathResolver {
    pub(crate) fn from_tsconfig(root: &Path) -> Option<Self> {
        let ts_path = find_tsconfig(root)?;
        let json = load_tsconfig_recursive(&ts_path)?;
        let compiler = json
            .get("compilerOptions")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let base_url = compiler
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let base_dir = ts_path.parent().unwrap_or(root).join(base_url);

        let mut mappings = Vec::new();
        if let Some(paths) = compiler.get("paths").and_then(|p| p.as_object()) {
            for (alias, targets) in paths {
                let targets_vec: Vec<String> = targets
                    .as_array()
                    .into_iter()
                    .flat_map(|arr| arr.iter())
                    .filter_map(|v| v.as_str())
                    .map(|s| s.replace('\\', "/"))
                    .collect();
                if targets_vec.is_empty() {
                    continue;
                }

                let alias_norm = alias.replace('\\', "/");
                let wildcard_count = alias_norm.matches('*').count();

                mappings.push(AliasMapping {
                    pattern: alias_norm,
                    targets: targets_vec,
                    wildcard_count,
                });
            }
        }

        // Load package.json exports if available
        let package_exports = load_package_exports(root).unwrap_or_default();

        // Load SvelteKit aliases from svelte.config.js
        // These take precedence over tsconfig paths for $-prefixed aliases
        let sveltekit_aliases = load_sveltekit_aliases(root);
        for (alias, target_path) in sveltekit_aliases {
            // Convert to tsconfig-style mapping: $components/* -> src/components/*
            let pattern = format!("{}/*", alias);
            let target = format!("{}/*", target_path);
            mappings.push(AliasMapping {
                pattern,
                targets: vec![target],
                wildcard_count: 1,
            });
            // Also add exact match without wildcard for direct imports
            mappings.push(AliasMapping {
                pattern: alias,
                targets: vec![target_path],
                wildcard_count: 0,
            });
        }

        // Load Vite aliases from vite.config.js/ts
        let vite_aliases = load_vite_aliases(root);

        Some(Self {
            base_dir: base_dir.canonicalize().unwrap_or(base_dir),
            root: root.to_path_buf(),
            mappings,
            cache: Mutex::new(HashMap::new()),
            package_exports,
            vite_aliases,
        })
    }

    /// Extract the resolver configuration for caching in snapshots
    pub(crate) fn extract_config(&self) -> ExtractedResolverConfig {
        let ts_paths: HashMap<String, Vec<String>> = self
            .mappings
            .iter()
            .map(|m| (m.pattern.clone(), m.targets.clone()))
            .collect();

        let ts_base_url = self
            .base_dir
            .strip_prefix(&self.root)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| Some(self.base_dir.to_string_lossy().to_string()));

        ExtractedResolverConfig {
            ts_paths,
            ts_base_url,
        }
    }

    pub(crate) fn resolve(&self, spec: &str, exts: Option<&HashSet<String>>) -> Option<String> {
        if spec.starts_with('.') {
            return None;
        }

        // Check cache first
        let cache_key = format!("{:?}:{}", exts, spec);
        if let Ok(cache) = self.cache.lock()
            && let Some(cached) = cache.get(&cache_key)
        {
            return cached.clone();
        }

        let normalized = spec.replace('\\', "/");
        let result = self.resolve_internal(&normalized, exts);

        // Store in cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(cache_key, result.clone());
        }

        result
    }

    fn resolve_internal(&self, normalized: &str, exts: Option<&HashSet<String>>) -> Option<String> {
        // SvelteKit virtual module resolution:
        // 1. $lib/* - user code in src/lib/
        // 2. $app/*, $env/* - framework runtime code or build-time generated

        // SvelteKit convention: $lib/ maps to src/lib/ (user code)
        if let Some(rest) = normalized.strip_prefix("$lib/") {
            let candidate = self.root.join("src/lib").join(rest);
            if let Some(res) = resolve_with_extensions(candidate, &self.root, exts) {
                return Some(res);
            }
        }

        // Try to resolve SvelteKit runtime virtual modules to actual files
        // $app/* and $env/* map to @sveltejs/kit runtime in node_modules or packages/kit
        if normalized.starts_with("$app/") || normalized.starts_with("$env/") {
            // Try monorepo layout first (e.g., packages/kit/src/runtime/...)
            if let Some(resolved) = self.resolve_sveltekit_runtime(normalized, exts) {
                return Some(resolved);
            }

            // Try node_modules/@sveltejs/kit/src/runtime/...
            if let Some(resolved) = self.resolve_sveltekit_node_modules(normalized, exts) {
                return Some(resolved);
            }

            // Fallback: return synthetic path for truly virtual modules (build-time generated)
            // This indicates they're valid imports but not file-based
            if is_sveltekit_virtual_module(normalized) {
                return Some(format!("__virtual__/{}", normalized));
            }
        }

        // Other SvelteKit virtual modules ($service-worker, etc.)
        if is_sveltekit_virtual_module(normalized) {
            return Some(format!("__virtual__/{}", normalized));
        }

        // Try Vite aliases (resolve.alias from vite.config.js/ts)
        // Sort by prefix length (longest first) to ensure @core matches before @
        let mut sorted_aliases: Vec<_> = self.vite_aliases.iter().collect();
        sorted_aliases.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        for (prefix, target_path) in sorted_aliases {
            if let Some(rest) = normalized.strip_prefix(prefix.as_str()) {
                let candidate = if rest.is_empty() || rest == "/" {
                    target_path.clone()
                } else {
                    let rest_clean = rest.strip_prefix('/').unwrap_or(rest);
                    target_path.join(rest_clean)
                };
                if let Some(res) = resolve_with_extensions(candidate, &self.root, exts) {
                    return Some(res);
                }
            }
        }

        // Try tsconfig path mappings
        for mapping in &self.mappings {
            if mapping.wildcard_count > 0 {
                if let Some(res) = self.match_wildcard_pattern(
                    &mapping.pattern,
                    normalized,
                    &mapping.targets,
                    exts,
                ) {
                    return Some(res);
                }
            } else if normalized == mapping.pattern {
                for target in &mapping.targets {
                    let candidate = self.base_dir.join(target);
                    if let Some(res) = resolve_with_extensions(candidate, &self.root, exts) {
                        return Some(res);
                    }
                }
            }
        }

        // Try package.json exports
        if let Some(export_path) = self.package_exports.get(normalized) {
            let candidate = self.root.join(export_path.trim_start_matches("./"));
            if let Some(res) = resolve_with_extensions(candidate, &self.root, exts) {
                return Some(res);
            }
        }

        // Fallback to baseUrl resolution
        if normalized.starts_with('/') {
            let candidate = self.root.join(normalized.trim_start_matches('/'));
            return resolve_with_extensions(candidate, &self.root, exts);
        }

        let candidate = self.base_dir.join(normalized);
        resolve_with_extensions(candidate, &self.root, exts)
    }

    /// Resolve SvelteKit runtime virtual modules in monorepo layout
    /// E.g., $app/paths/internal/server -> packages/kit/src/runtime/app/paths/internal/server.js
    fn resolve_sveltekit_runtime(
        &self,
        normalized: &str,
        exts: Option<&HashSet<String>>,
    ) -> Option<String> {
        // Strip $app/ or $env/ prefix
        let rest = if let Some(r) = normalized.strip_prefix("$app/") {
            format!("app/{}", r)
        } else {
            format!("env/{}", normalized.strip_prefix("$env/")?)
        };

        // Try monorepo layout: packages/kit/src/runtime/{app|env}/...
        let monorepo_candidates = [
            self.root.join("packages/kit/src/runtime").join(&rest),
            // Also try from current directory in case we're inside packages/kit
            self.root.join("src/runtime").join(&rest),
        ];

        for candidate in monorepo_candidates {
            if let Some(res) = resolve_with_extensions(candidate, &self.root, exts) {
                return Some(res);
            }
        }

        None
    }

    /// Resolve SvelteKit runtime virtual modules from node_modules
    /// E.g., $app/paths/internal/server -> node_modules/@sveltejs/kit/src/runtime/app/paths/internal/server.js
    fn resolve_sveltekit_node_modules(
        &self,
        normalized: &str,
        exts: Option<&HashSet<String>>,
    ) -> Option<String> {
        // Strip $app/ or $env/ prefix
        let rest = if let Some(r) = normalized.strip_prefix("$app/") {
            format!("app/{}", r)
        } else {
            format!("env/{}", normalized.strip_prefix("$env/")?)
        };

        // Try node_modules layout
        let candidate = self
            .root
            .join("node_modules/@sveltejs/kit/src/runtime")
            .join(&rest);

        resolve_with_extensions(candidate, &self.root, exts)
    }

    fn match_wildcard_pattern(
        &self,
        pattern: &str,
        spec: &str,
        targets: &[String],
        exts: Option<&HashSet<String>>,
    ) -> Option<String> {
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() < 2 {
            return None;
        }

        let mut spec_rest = spec;
        let mut captures = Vec::new();

        for (i, part) in parts.iter().enumerate() {
            if i == 0 {
                spec_rest = spec_rest.strip_prefix(part)?;
            } else if i == parts.len() - 1 {
                if !spec_rest.ends_with(part) {
                    return None;
                }
                let captured = spec_rest.strip_suffix(part).unwrap_or(spec_rest);
                captures.push(captured);
            } else {
                let idx = spec_rest.find(part)?;
                captures.push(&spec_rest[..idx]);
                spec_rest = &spec_rest[idx + part.len()..];
            }
        }

        for target in targets {
            let replaced = if captures.len() == 1 {
                target.replace('*', captures[0])
            } else {
                let mut result = target.to_string();
                for capture in &captures {
                    if let Some(idx) = result.find('*') {
                        result.replace_range(idx..=idx, capture);
                    }
                }
                result
            };

            let candidate = self.base_dir.join(replaced);
            if let Some(res) = resolve_with_extensions(candidate, &self.root, exts) {
                return Some(res);
            }
        }

        None
    }
}

pub(crate) fn resolve_reexport_target(
    file_path: &Path,
    root: &Path,
    spec: &str,
    exts: Option<&HashSet<String>>,
) -> Option<String> {
    if !spec.starts_with('.') {
        return None;
    }
    let parent = file_path.parent()?;
    let candidate = parent.join(spec);
    resolve_python_candidate(candidate, root, exts)
}

pub(crate) fn resolve_python_relative(
    module: &str,
    file_path: &Path,
    root: &Path,
    exts: Option<&HashSet<String>>,
) -> Option<String> {
    if !module.starts_with('.') {
        return None;
    }

    let mut leading = 0usize;
    for ch in module.chars() {
        if ch == '.' {
            leading += 1;
        } else {
            break;
        }
    }

    let mut base = file_path.parent()?;
    for _ in 1..leading {
        base = base.parent()?;
    }

    let remainder = module.trim_start_matches('.').replace('.', "/");
    let joined = if remainder.is_empty() {
        base.to_path_buf()
    } else {
        base.join(remainder)
    };

    resolve_python_candidate(joined, root, exts)
}

pub(crate) fn resolve_js_relative(
    file_path: &Path,
    root: &Path,
    spec: &str,
    exts: Option<&HashSet<String>>,
) -> Option<String> {
    if !spec.starts_with('.') {
        return None;
    }
    let parent = file_path.parent()?;
    let candidate = parent.join(spec);
    resolve_with_extensions(candidate, root, exts)
}

pub(crate) fn resolve_python_candidate(
    candidate: PathBuf,
    root: &Path,
    exts: Option<&HashSet<String>>,
) -> Option<String> {
    if candidate.is_dir() {
        let init_candidates = [
            candidate.join("__init__.py"),
            candidate.join("__init__.pyi"),
            candidate.join("mod.py"),
        ];
        for init in init_candidates {
            if init.exists() {
                return canonical_rel(&init, root).or_else(|| canonical_abs(&init));
            }
        }

        if is_namespace_package(&candidate) {
            return canonical_rel(&candidate, root).or_else(|| canonical_abs(&candidate));
        }
    }

    resolve_with_extensions(candidate, root, exts)
}

fn is_namespace_package(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension()
                && (ext == "py" || ext == "pyi")
            {
                return true;
            }
        } else if path.is_dir() {
            let subdir_init = path.join("__init__.py");
            if subdir_init.exists() || is_namespace_package(&path) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn has_py_typed_marker(package_dir: &Path) -> bool {
    package_dir.join("py.typed").exists()
}

pub(crate) fn resolve_python_absolute(
    module: &str,
    roots: &[PathBuf],
    root_for_rel: &Path,
    exts: Option<&HashSet<String>>,
) -> Option<String> {
    let normalized = module.replace('.', "/");
    for base in roots {
        let candidate = base.join(&normalized);
        if let Some(resolved) = resolve_python_candidate(candidate.clone(), root_for_rel, exts) {
            return Some(resolved);
        }
    }
    None
}

const KNOWN_JS_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "svelte", "vue", "astro",
];

fn has_known_js_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| KNOWN_JS_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub(crate) fn resolve_with_extensions(
    candidate: PathBuf,
    root: &Path,
    exts: Option<&HashSet<String>>,
) -> Option<String> {
    if !has_known_js_extension(&candidate)
        && let Some(set) = exts
    {
        for ext in set {
            let mut new_name = candidate.as_os_str().to_os_string();
            new_name.push(".");
            new_name.push(ext);
            let with_ext = PathBuf::from(new_name);
            if with_ext.exists() {
                return canonical_rel(&with_ext, root).or_else(|| canonical_abs(&with_ext));
            }
        }
    }

    if candidate.exists() {
        // If the candidate is a directory, try to resolve to an index file
        if candidate.is_dir() && !has_known_js_extension(&candidate) {
            for index_name in [
                "index.ts",
                "index.tsx",
                "index.js",
                "index.jsx",
                "index.svelte",
                "index.vue",
                "index.astro",
            ] {
                let index_candidate = candidate.join(index_name);
                if index_candidate.exists() {
                    return canonical_rel(&index_candidate, root)
                        .or_else(|| canonical_abs(&index_candidate));
                }
            }
        }
        canonical_rel(&candidate, root).or_else(|| canonical_abs(&candidate))
    } else {
        // Candidate doesn't exist, try to find it with index files
        if !has_known_js_extension(&candidate) {
            let dir_path = candidate.clone();
            for index_name in [
                "index.ts",
                "index.tsx",
                "index.js",
                "index.jsx",
                "index.svelte",
                "index.vue",
                "index.astro",
            ] {
                let index_candidate = dir_path.join(index_name);
                if index_candidate.exists() {
                    return canonical_rel(&index_candidate, root)
                        .or_else(|| canonical_abs(&index_candidate));
                }
            }
        }
        None
    }
}

fn canonical_rel(path: &Path, root: &Path) -> Option<String> {
    path.canonicalize().ok().and_then(|p| {
        p.strip_prefix(root)
            .ok()
            .map(|q| q.to_string_lossy().to_string())
    })
}

fn canonical_abs(path: &Path) -> Option<String> {
    path.canonicalize()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

pub(crate) fn find_tsconfig(start: &Path) -> Option<PathBuf> {
    let mut current = start
        .canonicalize()
        .ok()
        .unwrap_or_else(|| start.to_path_buf());
    loop {
        let candidate = current.join("tsconfig.json");
        if candidate.exists() {
            return Some(candidate);
        }
        if let Some(parent) = current.parent() {
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    None
}

fn load_tsconfig_recursive(ts_path: &Path) -> Option<Value> {
    let content = std::fs::read_to_string(ts_path).ok()?;
    let mut current: Value = parse_tsconfig_value(&content)?;

    if let Some(ext) = current.get("extends").and_then(|v| v.as_str()) {
        let base_path = if Path::new(ext).is_absolute() {
            PathBuf::from(ext)
        } else {
            ts_path
                .parent()
                .map(|p| p.join(ext))
                .unwrap_or_else(|| PathBuf::from(ext))
        };
        if base_path.exists()
            && let Some(parent) = load_tsconfig_recursive(&base_path)
        {
            if let (Some(child_co), Some(parent_co)) = (
                current
                    .get("compilerOptions")
                    .and_then(|v| v.as_object())
                    .cloned(),
                parent
                    .get("compilerOptions")
                    .and_then(|v| v.as_object())
                    .cloned(),
            ) {
                let merged = merge_compiler_options(&parent_co, &child_co);
                current["compilerOptions"] = Value::Object(merged);
            } else if let Some(parent_co) = parent
                .get("compilerOptions")
                .and_then(|v| v.as_object())
                .cloned()
            {
                current["compilerOptions"] = Value::Object(parent_co);
            }
        }
    }

    Some(current)
}

pub(crate) fn parse_tsconfig_value(content: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(content) {
        return Some(v);
    }
    if let Ok(v) = json_five::from_str::<serde_json::Value>(content) {
        return Some(v);
    }
    None
}

fn merge_compiler_options(
    parent: &serde_json::Map<String, Value>,
    child: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut merged = parent.clone();
    for (k, v) in child {
        if k == "paths" {
            let mut combined = parent
                .get("paths")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            if let Some(child_paths) = v.as_object() {
                for (alias, targets) in child_paths {
                    combined.insert(alias.clone(), targets.clone());
                }
            }
            merged.insert(k.clone(), Value::Object(combined));
        } else {
            merged.insert(k.clone(), v.clone());
        }
    }
    merged
}

fn load_package_exports(root: &Path) -> Option<HashMap<String, String>> {
    let package_json_path = root.join("package.json");
    if !package_json_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&package_json_path).ok()?;
    let json: Value = serde_json::from_str(&content).ok()?;

    let exports = json.get("exports")?;
    let mut result = HashMap::new();

    match exports {
        Value::String(path) => {
            result.insert(".".to_string(), path.clone());
        }
        Value::Object(map) => {
            for (key, value) in map {
                let export_path = match value {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(conditions) => conditions
                        .get("import")
                        .or_else(|| conditions.get("require"))
                        .or_else(|| conditions.get("default"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    _ => None,
                };

                if let Some(path) = export_path {
                    result.insert(key.clone(), path);
                }
            }
        }
        _ => {}
    }

    Some(result)
}

fn load_sveltekit_aliases(root: &Path) -> HashMap<String, String> {
    let mut result = HashMap::new();

    let config_candidates = [root.join("svelte.config.js"), root.join("svelte.config.ts")];

    let mut all_candidates: Vec<PathBuf> = config_candidates.to_vec();
    for entry in ["apps", "packages"].iter() {
        let dir = root.join(entry);
        if dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    all_candidates.push(path.join("svelte.config.js"));
                    all_candidates.push(path.join("svelte.config.ts"));
                }
            }
        }
    }

    let alias_regex = match Regex::new(r#"alias\s*:\s*\{([^}]+)\}"#) {
        Ok(re) => re,
        Err(_) => return result,
    };
    let entry_regex =
        match Regex::new(r#"['"]?(\$[a-zA-Z_][a-zA-Z0-9_]*)['"]?\s*:\s*['"]([^'"]+)['"]"#) {
            Ok(re) => re,
            Err(_) => return result,
        };

    for config_path in all_candidates {
        if !config_path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Some(caps) = alias_regex.captures(&content) {
            let alias_block = &caps[1];
            let config_dir = config_path.parent().unwrap_or(root);

            for entry_caps in entry_regex.captures_iter(alias_block) {
                let alias = entry_caps[1].to_string();
                let path_str = entry_caps[2].to_string();

                let resolved = if let Some(stripped) = path_str.strip_prefix("./") {
                    config_dir.join(stripped)
                } else {
                    config_dir.join(&path_str)
                };

                if let Ok(canonical) = resolved.canonicalize()
                    && let Ok(rel) = canonical.strip_prefix(root)
                {
                    result.insert(alias, rel.to_string_lossy().to_string());
                }
            }
        }
    }

    result
}

fn load_vite_aliases(root: &Path) -> HashMap<String, PathBuf> {
    let mut result = HashMap::new();

    let config_names = [
        "vite.config.js",
        "vite.config.ts",
        "vite.config.mjs",
        "vite.config.mts",
    ];
    let mut all_candidates: Vec<PathBuf> = config_names.iter().map(|n| root.join(n)).collect();

    for entry in ["apps", "packages"].iter() {
        let dir = root.join(entry);
        if dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    for name in &config_names {
                        all_candidates.push(path.join(name));
                    }
                }
            }
        }
    }

    let alias_block_regex = match Regex::new(r#"resolve\s*:\s*\{[^}]*alias\s*:\s*\{([^}]+)\}"#) {
        Ok(re) => re,
        Err(_) => return result,
    };

    // Regex for array format: alias: [ { find: '@', replacement: '/src' }, ... ]
    let alias_array_regex = match Regex::new(r#"resolve\s*:\s*\{[^}]*alias\s*:\s*\[([^\]]+)\]"#) {
        Ok(re) => re,
        Err(_) => return result,
    };

    // Regex for array entries: { find: '@', replacement: '/src' }
    let array_entry_regex = match Regex::new(
        r#"\{\s*find\s*:\s*['"](@[a-zA-Z0-9_/-]*|\$[a-zA-Z_][a-zA-Z0-9_]*)['"].*?replacement\s*:\s*(?:['"]([^'"]+)['"]|(?:path\.)?resolve\s*\([^)]*['"]([^'"]+)['"]\s*\))"#,
    ) {
        Ok(re) => re,
        Err(_) => return result,
    };

    let entry_regex = match Regex::new(
        r#"['"]?(@[a-zA-Z0-9_/-]*|\$[a-zA-Z_][a-zA-Z0-9_]*)['"]?\s*:\s*(?:['"]([^'"]+)['"]|(?:path\.)?resolve\s*\([^)]*['"]([^'"]+)['"]\s*\))"#,
    ) {
        Ok(re) => re,
        Err(_) => return result,
    };

    let simple_alias_regex =
        match Regex::new(r#"['"](@[a-zA-Z0-9_/-]*)['"]?\s*:\s*['"]([^'"]+)['"]"#) {
            Ok(re) => re,
            Err(_) => return result,
        };

    for config_path in all_candidates {
        if !config_path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let config_dir = config_path.parent().unwrap_or(root);

        // Try object format: alias: { '@': './src' }
        if let Some(caps) = alias_block_regex.captures(&content) {
            let alias_block = &caps[1];

            for entry_caps in entry_regex.captures_iter(alias_block) {
                let alias = entry_caps[1].to_string();
                let path_str = entry_caps
                    .get(2)
                    .or_else(|| entry_caps.get(3))
                    .map(|m| m.as_str().to_string());

                if let Some(path_str) = path_str {
                    let resolved = if let Some(stripped) = path_str.strip_prefix("./") {
                        config_dir.join(stripped)
                    } else if path_str.starts_with('/') {
                        PathBuf::from(&path_str)
                    } else {
                        config_dir.join(&path_str)
                    };

                    let final_path = resolved.canonicalize().unwrap_or(resolved);
                    result.insert(alias, final_path);
                }
            }
        }

        // Try array format: alias: [{ find: '@', replacement: './src' }]
        if let Some(caps) = alias_array_regex.captures(&content) {
            let alias_array = &caps[1];

            for entry_caps in array_entry_regex.captures_iter(alias_array) {
                let alias = entry_caps[1].to_string();
                let path_str = entry_caps
                    .get(2)
                    .or_else(|| entry_caps.get(3))
                    .map(|m| m.as_str().to_string());

                if let Some(path_str) = path_str {
                    // Skip if already found via object format
                    if result.contains_key(&alias) {
                        continue;
                    }

                    let resolved = if let Some(stripped) = path_str.strip_prefix("./") {
                        config_dir.join(stripped)
                    } else if path_str.starts_with('/') {
                        PathBuf::from(&path_str)
                    } else {
                        config_dir.join(&path_str)
                    };

                    let final_path = resolved.canonicalize().unwrap_or(resolved);
                    result.insert(alias, final_path);
                }
            }
        }

        for entry_caps in simple_alias_regex.captures_iter(&content) {
            let alias = entry_caps[1].to_string();
            let path_str = entry_caps[2].to_string();

            if result.contains_key(&alias) {
                continue;
            }

            let resolved = if let Some(stripped) = path_str.strip_prefix("./") {
                config_dir.join(stripped)
            } else if path_str.starts_with('/') {
                PathBuf::from(&path_str)
            } else {
                config_dir.join(&path_str)
            };

            let final_path = resolved.canonicalize().unwrap_or(resolved);
            result.insert(alias, final_path);
        }
    }

    result
}

pub(crate) fn resolve_rust_import(
    source: &str,
    file_path: &Path,
    crate_root: &Path,
    root: &Path,
) -> Option<String> {
    // Handle mod declarations: mod::foo or mod::path:foo.rs
    if source.starts_with("mod::") {
        let remainder = source.strip_prefix("mod::")?;

        // Determine the directory to search for the module
        // For mod.rs, lib.rs, main.rs: search in the same directory
        // For other files like foo.rs: search in foo/ directory
        let file_name = file_path.file_name()?.to_str()?;
        let parent = file_path.parent()?;

        let search_dir = if file_name == "mod.rs" || file_name == "lib.rs" || file_name == "main.rs"
        {
            parent.to_path_buf()
        } else {
            // For foo.rs, submodules are in foo/ directory
            let stem = file_path.file_stem()?.to_str()?;
            parent.join(stem)
        };

        let module_path = if let Some(explicit_path) = remainder.strip_prefix("path:") {
            // #[path = "foo.rs"] mod bar; -> resolve the explicit path
            let resolved = parent.join(explicit_path);
            if resolved.exists() {
                Some(resolved)
            } else {
                None
            }
        } else {
            // Regular mod foo; -> check for foo.rs or foo/mod.rs
            let mod_name = remainder;

            // Try foo.rs first
            let as_file = search_dir.join(format!("{}.rs", mod_name));
            if as_file.exists() {
                Some(as_file)
            } else {
                // Try foo/mod.rs
                let as_mod = search_dir.join(mod_name).join("mod.rs");
                if as_mod.exists() {
                    Some(as_mod)
                } else {
                    // Also check in parent dir for foo.rs (for lib.rs/main.rs importing sibling modules)
                    let sibling_file = parent.join(format!("{}.rs", mod_name));
                    if sibling_file.exists() {
                        Some(sibling_file)
                    } else {
                        let sibling_mod = parent.join(mod_name).join("mod.rs");
                        if sibling_mod.exists() {
                            Some(sibling_mod)
                        } else {
                            None
                        }
                    }
                }
            }
        };

        return module_path.and_then(|p| canonical_rel(&p, root).or_else(|| canonical_abs(&p)));
    }

    if source.starts_with("std::")
        || source.starts_with("core::")
        || source.starts_with("alloc::")
        || !source.contains("::")
    {
        return None;
    }

    let module_path = if source.starts_with("crate::") {
        let remainder = source.strip_prefix("crate::")?;
        resolve_rust_module_path(remainder, crate_root)
    } else if source.starts_with("super::") {
        let remainder = source.strip_prefix("super::")?;
        let parent = file_path.parent()?.parent()?;
        resolve_rust_module_path(remainder, parent)
    } else if source.starts_with("self::") {
        let remainder = source.strip_prefix("self::")?;
        let current_dir = if file_path.file_name()?.to_str()? == "mod.rs" {
            file_path.parent()?
        } else {
            let stem = file_path.file_stem()?.to_str()?;
            &file_path.parent()?.join(stem)
        };
        resolve_rust_module_path(remainder, current_dir)
    } else {
        resolve_rust_module_path(source, crate_root)
    };

    module_path.and_then(|p| canonical_rel(&p, root).or_else(|| canonical_abs(&p)))
}

fn resolve_rust_module_path(module: &str, base: &Path) -> Option<PathBuf> {
    // Resolve a crate-relative (or relative) module path (e.g. "foo::bar" or "foo::bar::Item"
    // or "foo::{A,B}") to the *file that owns the module* (the .rs or mod.rs surface).
    // Trailing ::Item or ::{...} (the imported symbol(s)) must not prevent resolution
    // to the defining module file. This powers consumer edges for impact/slice/who-imports
    // on leaf modules and on module-directory facades (mod.rs).
    // See loctree-feedback.md:2997 (facade), 3144 (cross-module use with item), 2900 (fn-body use).
    let segments: Vec<&str> = module.split("::").collect();
    if segments.is_empty() {
        return None;
    }

    let mut last_resolved: Option<PathBuf> = None;
    let mut search_base = base.to_path_buf();

    for &seg in segments.iter() {
        // Try seg.rs (single-file module or leaf under current search dir)
        let as_file = search_base.join(format!("{}.rs", seg));
        if as_file.exists() {
            last_resolved = Some(as_file.clone());
            // Parent dir becomes search base for any submodules (though items inside this file
            // would terminate the module path here).
            search_base = as_file.parent().unwrap_or(&search_base).to_path_buf();
            // If this was the final segment in the provided path, it is the module surface.
            // If more segments follow, they are items inside; we still return this file as owner
            // (handled by the non-match of next segment below).
            continue;
        }

        // Try seg/mod.rs (directory module)
        let mod_dir = search_base.join(seg);
        let as_mod = mod_dir.join("mod.rs");
        if as_mod.exists() {
            last_resolved = Some(as_mod.clone());
            search_base = mod_dir;
            continue;
        }

        // This segment did not name a module file/dir.
        // If we already resolved a prior module, this segment is an item name (or braced list)
        // inside that module — return the last resolved module file (the true target for
        // consumer wiring).
        if last_resolved.is_some() {
            return last_resolved;
        }

        // Cannot resolve even the first segment here.
        break;
    }

    // If we walked everything and have a resolution, return it (covers pure module paths
    // that end on a file or mod.rs).
    if let Some(p) = last_resolved {
        return Some(p);
    }

    // Fallback: first segment as mod dir (len==1 case that had no early .rs)
    let first = segments[0];
    let m = base.join(first).join("mod.rs");
    if m.exists() {
        return Some(m);
    }

    None
}

pub(crate) fn find_rust_crate_root(file_path: &Path) -> Option<PathBuf> {
    let mut current = file_path.parent()?;
    loop {
        if current.join("Cargo.toml").exists() {
            let manifest_path = current.join("Cargo.toml");
            if let Ok(manifest) =
                crate::analyzer::cargo_manifest::parse_crate_manifest(&manifest_path)
            {
                let file_canon = file_path
                    .canonicalize()
                    .unwrap_or_else(|_| file_path.to_path_buf());
                let mut target_roots: Vec<PathBuf> = manifest
                    .targets
                    .iter()
                    .filter_map(|target| target.path.parent().map(Path::to_path_buf))
                    .collect();
                target_roots.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
                for target_root in target_roots {
                    let target_canon = target_root
                        .canonicalize()
                        .unwrap_or_else(|_| target_root.clone());
                    if file_canon.starts_with(&target_canon) {
                        return Some(target_root);
                    }
                }
            }

            let src_dir = current.join("src");
            if src_dir.exists() && src_dir.is_dir() {
                return Some(src_dir);
            }
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        let tsconfig = r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"],"@components/*":["src/components/*"],"utils":["src/utils/index.ts"]}}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        fs::create_dir_all(dir.path().join("src/components")).unwrap();
        fs::create_dir_all(dir.path().join("src/utils")).unwrap();
        fs::write(dir.path().join("src/index.ts"), "export {}").unwrap();
        fs::write(dir.path().join("src/components/Button.tsx"), "export {}").unwrap();
        fs::write(dir.path().join("src/utils/index.ts"), "export {}").unwrap();
        dir
    }

    fn create_python_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/mypackage")).unwrap();
        fs::write(dir.path().join("src/mypackage/__init__.py"), "").unwrap();
        fs::write(dir.path().join("src/mypackage/utils.py"), "").unwrap();
        fs::write(dir.path().join("src/mypackage/helpers.py"), "").unwrap();
        dir
    }

    fn create_rust_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "mod utils;").unwrap();
        fs::write(dir.path().join("src/utils.rs"), "pub fn helper() {}").unwrap();
        dir
    }

    #[test]
    fn test_sveltekit_virtual_module_detection() {
        assert!(is_sveltekit_virtual_module("$app/forms"));
        assert!(is_sveltekit_virtual_module("$app/navigation"));
        assert!(is_sveltekit_virtual_module("$env/static/public"));
        assert!(is_sveltekit_virtual_module("$service-worker"));
        assert!(!is_sveltekit_virtual_module("$lib/components"));
        assert!(!is_sveltekit_virtual_module("lodash"));
    }

    #[test]
    fn test_sveltekit_virtual_module_resolution() {
        let dir = TempDir::new().unwrap();
        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let result = resolver.resolve("$app/forms", None);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "__virtual__/$app/forms");
    }

    #[test]
    fn test_vite_alias_loading() {
        let dir = TempDir::new().unwrap();
        let vite_config = r#"export default { resolve: { alias: { '@': './src' } } };"#;
        fs::write(dir.path().join("vite.config.ts"), vite_config).unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/index.ts"), "export {}").unwrap();
        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["ts"].iter().map(|s| s.to_string()).collect();
        let result = resolver.resolve("@/index", Some(&exts));
        assert!(result.is_some());
    }

    #[test]
    fn test_vite_alias_array_format() {
        let dir = TempDir::new().unwrap();
        let vite_config = r#"export default {
            resolve: {
                alias: [
                    { find: '@', replacement: './src' },
                    { find: '@core', replacement: './core' }
                ]
            }
        };"#;
        fs::write(dir.path().join("vite.config.ts"), vite_config).unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("core")).unwrap();
        fs::write(dir.path().join("src/index.ts"), "export {}").unwrap();
        fs::write(dir.path().join("core/utils.ts"), "export {}").unwrap();
        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["ts"].iter().map(|s| s.to_string()).collect();

        // Test both aliases work
        let result1 = resolver.resolve("@/index", Some(&exts));
        assert!(result1.is_some());

        let result2 = resolver.resolve("@core/utils", Some(&exts));
        assert!(result2.is_some());
    }

    #[test]
    fn test_sveltekit_runtime_virtual_module_monorepo() {
        let dir = TempDir::new().unwrap();
        // Create SvelteKit monorepo structure
        fs::create_dir_all(
            dir.path()
                .join("packages/kit/src/runtime/app/paths/internal"),
        )
        .unwrap();
        fs::write(
            dir.path()
                .join("packages/kit/src/runtime/app/paths/internal/server.js"),
            "export function set_assets() {}",
        )
        .unwrap();

        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();

        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["js"].iter().map(|s| s.to_string()).collect();

        // Should resolve $app/paths/internal/server to the actual file
        let result = resolver.resolve("$app/paths/internal/server", Some(&exts));
        assert!(result.is_some(), "Should resolve SvelteKit virtual module");
        let resolved = result.unwrap();
        assert!(
            resolved.contains("packages/kit/src/runtime/app/paths/internal/server"),
            "Should resolve to actual file path, got: {}",
            resolved
        );
    }

    #[test]
    fn test_sveltekit_runtime_virtual_module_node_modules() {
        let dir = TempDir::new().unwrap();
        // Create node_modules structure
        fs::create_dir_all(
            dir.path()
                .join("node_modules/@sveltejs/kit/src/runtime/app/forms"),
        )
        .unwrap();
        fs::write(
            dir.path()
                .join("node_modules/@sveltejs/kit/src/runtime/app/forms.js"),
            "export function enhance() {}",
        )
        .unwrap();

        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();

        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["js"].iter().map(|s| s.to_string()).collect();

        // Should resolve $app/forms to the actual file
        let result = resolver.resolve("$app/forms", Some(&exts));
        assert!(result.is_some(), "Should resolve SvelteKit virtual module");
        let resolved = result.unwrap();
        assert!(
            resolved.contains("node_modules/@sveltejs/kit/src/runtime/app/forms"),
            "Should resolve to node_modules path, got: {}",
            resolved
        );
    }

    #[test]
    fn test_sveltekit_virtual_module_fallback() {
        let dir = TempDir::new().unwrap();
        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();

        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();

        // For virtual modules that don't have physical files (build-time generated),
        // should return synthetic path
        let result = resolver.resolve("$env/static/public", None);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "__virtual__/$env/static/public");
    }

    #[test]
    fn test_vite_alias_overlapping_prefixes() {
        let dir = TempDir::new().unwrap();
        let vite_config = r#"export default {
            resolve: {
                alias: {
                    '@': './src',
                    '@core': './core'
                }
            }
        };"#;
        fs::write(dir.path().join("vite.config.ts"), vite_config).unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("core")).unwrap();
        fs::write(dir.path().join("core/utils.ts"), "export {}").unwrap();
        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["ts"].iter().map(|s| s.to_string()).collect();

        // Test that @core matches @core, not @
        // This ensures longest prefix matching works correctly
        let result = resolver.resolve("@core/utils", Some(&exts));
        assert!(result.is_some());
        let resolved = result.unwrap();
        // Should resolve to core/utils.ts, not src/core/utils.ts
        assert!(resolved.contains("core") && resolved.contains("utils"));
        assert!(!resolved.contains("src/core"));
    }

    #[test]
    fn test_parse_tsconfig_value_valid_json() {
        let content = r#"{"compilerOptions": {"strict": true}}"#;
        assert!(parse_tsconfig_value(content).is_some());
    }

    #[test]
    fn test_parse_tsconfig_value_json5() {
        let content = r#"{// comment
            "compilerOptions": {"strict": true,}}"#;
        assert!(parse_tsconfig_value(content).is_some());
    }

    #[test]
    fn test_parse_tsconfig_value_invalid() {
        assert!(parse_tsconfig_value("not valid").is_none());
    }

    #[test]
    fn test_find_tsconfig() {
        let dir = create_test_project();
        let result = find_tsconfig(dir.path());
        assert!(result.is_some());
    }

    #[test]
    fn test_find_tsconfig_not_found() {
        let dir = TempDir::new().unwrap();
        assert!(find_tsconfig(dir.path()).is_none());
    }

    #[test]
    fn test_ts_path_resolver_from_tsconfig() {
        let dir = create_test_project();
        assert!(TsPathResolver::from_tsconfig(dir.path()).is_some());
    }

    #[test]
    fn test_ts_path_resolver_no_tsconfig() {
        let dir = TempDir::new().unwrap();
        assert!(TsPathResolver::from_tsconfig(dir.path()).is_none());
    }

    #[test]
    fn test_ts_path_resolver_resolve_relative_skipped() {
        let dir = create_test_project();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        assert!(resolver.resolve("./utils", None).is_none());
    }

    #[test]
    fn test_resolve_reexport_target_relative() {
        let dir = create_test_project();
        let file = dir.path().join("src/index.ts");
        let exts: HashSet<String> = ["ts", "tsx"].iter().map(|s| s.to_string()).collect();
        assert!(resolve_reexport_target(&file, dir.path(), "./utils/index", Some(&exts)).is_some());
    }

    #[test]
    fn test_resolve_reexport_target_non_relative() {
        let dir = create_test_project();
        let file = dir.path().join("src/index.ts");
        assert!(resolve_reexport_target(&file, dir.path(), "@/utils", None).is_none());
    }

    #[test]
    fn test_resolve_js_relative() {
        let dir = create_test_project();
        let file = dir.path().join("src/index.ts");
        let exts: HashSet<String> = ["ts", "tsx"].iter().map(|s| s.to_string()).collect();
        assert!(
            resolve_js_relative(&file, dir.path(), "./components/Button", Some(&exts)).is_some()
        );
    }

    #[test]
    fn test_resolve_js_relative_non_relative() {
        let dir = create_test_project();
        let file = dir.path().join("src/index.ts");
        assert!(resolve_js_relative(&file, dir.path(), "lodash", None).is_none());
    }

    #[test]
    fn test_resolve_python_relative() {
        let dir = create_python_project();
        let file = dir.path().join("src/mypackage/utils.py");
        let exts: HashSet<String> = ["py"].iter().map(|s| s.to_string()).collect();
        assert!(resolve_python_relative(".helpers", &file, dir.path(), Some(&exts)).is_some());
    }

    #[test]
    fn test_resolve_python_relative_double_dot() {
        let dir = create_python_project();
        fs::create_dir_all(dir.path().join("src/other")).unwrap();
        fs::write(dir.path().join("src/other/module.py"), "").unwrap();
        let file = dir.path().join("src/mypackage/utils.py");
        let exts: HashSet<String> = ["py"].iter().map(|s| s.to_string()).collect();
        assert!(
            resolve_python_relative("..other.module", &file, dir.path(), Some(&exts)).is_some()
        );
    }

    #[test]
    fn test_resolve_python_relative_non_relative() {
        let dir = create_python_project();
        let file = dir.path().join("src/mypackage/utils.py");
        assert!(resolve_python_relative("os", &file, dir.path(), None).is_none());
    }

    #[test]
    fn test_resolve_python_candidate_directory_with_init() {
        let dir = create_python_project();
        assert!(
            resolve_python_candidate(dir.path().join("src/mypackage"), dir.path(), None).is_some()
        );
    }

    #[test]
    fn test_resolve_python_candidate_file() {
        let dir = create_python_project();
        assert!(
            resolve_python_candidate(dir.path().join("src/mypackage/utils.py"), dir.path(), None)
                .is_some()
        );
    }

    #[test]
    fn test_resolve_python_absolute() {
        let dir = create_python_project();
        let roots = vec![dir.path().join("src")];
        let exts: HashSet<String> = ["py"].iter().map(|s| s.to_string()).collect();
        assert!(
            resolve_python_absolute("mypackage.utils", &roots, dir.path(), Some(&exts)).is_some()
        );
    }

    #[test]
    fn test_resolve_python_absolute_not_found() {
        let dir = create_python_project();
        let roots = vec![dir.path().join("src")];
        assert!(resolve_python_absolute("nonexistent.module", &roots, dir.path(), None).is_none());
    }

    #[test]
    fn test_resolve_with_extensions_adds_extension() {
        let dir = create_test_project();
        let exts: HashSet<String> = ["ts"].iter().map(|s| s.to_string()).collect();
        assert!(
            resolve_with_extensions(dir.path().join("src/index"), dir.path(), Some(&exts))
                .is_some()
        );
    }

    #[test]
    fn test_resolve_with_extensions_index_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/utils")).unwrap();
        fs::write(dir.path().join("src/utils/index.ts"), "").unwrap();
        assert!(resolve_with_extensions(dir.path().join("src/utils"), dir.path(), None).is_some());
    }

    #[test]
    fn test_resolve_with_extensions_not_found() {
        let dir = TempDir::new().unwrap();
        assert!(
            resolve_with_extensions(dir.path().join("nonexistent"), dir.path(), None).is_none()
        );
    }

    #[test]
    fn test_barrel_resolution_with_directory_alias() {
        // Test that @/components/auth resolves to @/components/auth/index.ts
        // This is the main use case to fix false positives in dead parrot detection
        let dir = TempDir::new().unwrap();
        let tsconfig = r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        fs::create_dir_all(dir.path().join("src/components/auth")).unwrap();
        fs::write(
            dir.path().join("src/components/auth/index.ts"),
            "export const login = () => {}",
        )
        .unwrap();

        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["ts", "tsx"].iter().map(|s| s.to_string()).collect();

        // Import: import { login } from '@/components/auth'
        // Should resolve to: src/components/auth/index.ts
        let result = resolver.resolve("@/components/auth", Some(&exts));
        assert!(
            result.is_some(),
            "Should resolve barrel directory to index file"
        );
        let resolved = result.unwrap();
        assert!(
            resolved.ends_with("src/components/auth/index.ts")
                || resolved.contains("components/auth/index.ts"),
            "Should resolve to index.ts file, got: {}",
            resolved
        );
    }

    #[test]
    fn test_find_rust_crate_root() {
        let dir = create_rust_project();
        let result = find_rust_crate_root(&dir.path().join("src/main.rs"));
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("src"));
    }

    #[test]
    fn test_find_rust_crate_root_honors_custom_lib_path() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("app/presentation")).unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n\n[lib]\npath = \"app/lib.rs\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("app/lib.rs"), "pub mod presentation;").unwrap();
        fs::write(
            dir.path().join("app/presentation/mod.rs"),
            "pub mod emitter;",
        )
        .unwrap();
        fs::write(
            dir.path().join("app/presentation/emitter.rs"),
            "pub struct PresentationEmitter;",
        )
        .unwrap();

        let result = find_rust_crate_root(&dir.path().join("app/presentation/emitter.rs"));
        assert_eq!(result.as_deref(), Some(dir.path().join("app").as_path()));
    }

    #[test]
    fn test_find_rust_crate_root_not_found() {
        let dir = TempDir::new().unwrap();
        assert!(find_rust_crate_root(&dir.path().join("some/random/file.rs")).is_none());
    }

    #[test]
    fn test_resolve_rust_import_crate() {
        let dir = create_rust_project();
        let file = dir.path().join("src/main.rs");
        let crate_root = dir.path().join("src");
        assert!(resolve_rust_import("crate::utils", &file, &crate_root, dir.path()).is_some());
    }

    #[test]
    fn test_resolve_rust_import_stdlib_skipped() {
        let dir = create_rust_project();
        let file = dir.path().join("src/main.rs");
        let crate_root = dir.path().join("src");
        assert!(
            resolve_rust_import("std::collections::HashMap", &file, &crate_root, dir.path())
                .is_none()
        );
    }

    #[test]
    fn test_resolve_rust_import_no_separator() {
        let dir = create_rust_project();
        let file = dir.path().join("src/main.rs");
        let crate_root = dir.path().join("src");
        assert!(resolve_rust_import("serde", &file, &crate_root, dir.path()).is_none());
    }

    #[test]
    fn test_resolve_rust_import_nested_module() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/analyzer")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "mod analyzer;").unwrap();
        fs::write(dir.path().join("src/analyzer/mod.rs"), "pub mod scan;").unwrap();
        fs::write(dir.path().join("src/analyzer/scan.rs"), "pub fn scan() {}").unwrap();
        let file = dir.path().join("src/lib.rs");
        let crate_root = dir.path().join("src");
        let result = resolve_rust_import("crate::analyzer::scan", &file, &crate_root, dir.path());
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("scan.rs"));
    }

    #[test]
    fn test_resolve_rust_import_nested_module_dir() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/foo/bar")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "mod foo;").unwrap();
        fs::write(dir.path().join("src/foo/mod.rs"), "pub mod bar;").unwrap();
        fs::write(dir.path().join("src/foo/bar/mod.rs"), "pub struct Baz;").unwrap();
        let file = dir.path().join("src/lib.rs");
        let crate_root = dir.path().join("src");
        let result = resolve_rust_import("crate::foo::bar", &file, &crate_root, dir.path());
        assert!(result.is_some());
    }

    #[test]
    fn test_ts_path_resolver_multiple_wildcards() {
        let dir = TempDir::new().unwrap();
        let tsconfig = r#"{"compilerOptions":{"baseUrl":".","paths":{"**/*":["src/**/*"]}}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        fs::create_dir_all(dir.path().join("src/components/ui")).unwrap();
        fs::write(dir.path().join("src/components/ui/Button.tsx"), "export {}").unwrap();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["tsx", "ts"].iter().map(|s| s.to_string()).collect();
        assert!(
            resolver
                .resolve("components/ui/Button", Some(&exts))
                .is_some()
        );
    }

    #[test]
    fn test_ts_path_resolver_caching() {
        let dir = create_test_project();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        let exts: HashSet<String> = ["ts"].iter().map(|s| s.to_string()).collect();
        let result1 = resolver.resolve("src/index", Some(&exts));
        let result2 = resolver.resolve("src/index", Some(&exts));
        assert!(result1.is_some());
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_load_package_exports() {
        let dir = TempDir::new().unwrap();
        let package_json = r#"{"name":"test","exports":{".":"./dist/index.js","./utils":"./dist/utils/index.js"}}"#;
        fs::write(dir.path().join("package.json"), package_json).unwrap();
        let exports = load_package_exports(dir.path()).unwrap();
        assert_eq!(exports.get("."), Some(&"./dist/index.js".to_string()));
    }

    #[test]
    fn test_ts_path_resolver_with_package_exports() {
        let dir = TempDir::new().unwrap();
        let tsconfig = r#"{"compilerOptions": {"baseUrl": "."}}"#;
        fs::write(dir.path().join("tsconfig.json"), tsconfig).unwrap();
        let package_json = r#"{"exports":{"./utils":"./dist/utils.js"}}"#;
        fs::write(dir.path().join("package.json"), package_json).unwrap();
        fs::create_dir_all(dir.path().join("dist")).unwrap();
        fs::write(dir.path().join("dist/utils.js"), "export {}").unwrap();
        let resolver = TsPathResolver::from_tsconfig(dir.path()).unwrap();
        assert!(resolver.resolve("./utils", None).is_none());
    }
}
