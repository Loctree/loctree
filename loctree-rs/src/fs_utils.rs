use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::types::Options;

/// Newtype proving a path has been:
/// 1. Canonicalized (symlinks resolved)
/// 2. Verified to live underneath an allowed root
/// 3. Free of `..` parent-dir traversal components
/// 4. Free of NUL bytes
///
/// All `std::fs::*` consumers in loctree's path-traversal-sensitive sites
/// receive a `SanitizedPath` (via `as_path()`) rather than a raw `&Path`.
/// This single-source-of-truth pattern lets Semgrep's `tainted-path`
/// data-flow analysis see one explicit sanitization sink
/// (`SanitizedPath::within`) instead of trying to follow canonicalize +
/// `starts_with` patterns scattered across the codebase.
pub struct SanitizedPath {
    canonical: PathBuf,
    #[allow(dead_code)] // kept for diagnostic display in errors
    root: PathBuf,
}

impl SanitizedPath {
    /// Single sanitization gate. Returns `Err(InvalidInput)` for empty
    /// paths, NUL bytes, or `..` components; `Err(PermissionDenied)` for
    /// paths that resolve outside `root`; propagates I/O errors from
    /// `canonicalize`.
    pub fn within(root: &Path, candidate: &Path) -> io::Result<Self> {
        if candidate.as_os_str().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "path is empty"));
        }
        if candidate.to_string_lossy().contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL byte",
            ));
        }
        for component in candidate.components() {
            if matches!(component, Component::ParentDir) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path contains '..' component",
                ));
            }
        }
        let canonical = candidate.canonicalize()?;
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !canonical.starts_with(&root_canon) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "path escapes allowed root: {} (root: {})",
                    canonical.display(),
                    root_canon.display()
                ),
            ));
        }
        Ok(SanitizedPath {
            canonical,
            root: root_canon,
        })
    }

    /// Allow membership under any of multiple trusted roots. Useful when a
    /// caller maintains a cache-root + local-root pair (e.g. snapshot
    /// resolution under both `~/.cache/loctree/...` and `<repo>/.loctree/`).
    pub fn within_any(roots: &[&Path], candidate: &Path) -> io::Result<Self> {
        if candidate.as_os_str().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "path is empty"));
        }
        if candidate.to_string_lossy().contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL byte",
            ));
        }
        for component in candidate.components() {
            if matches!(component, Component::ParentDir) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path contains '..' component",
                ));
            }
        }
        let canonical = candidate.canonicalize()?;
        let canon_roots: Vec<PathBuf> = roots
            .iter()
            .map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()))
            .collect();
        if let Some(matched) = canon_roots.iter().find(|root| canonical.starts_with(root)) {
            Ok(SanitizedPath {
                canonical,
                root: matched.clone(),
            })
        } else {
            let allowed = canon_roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(" | ");
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "path escapes allowed roots: {} (allowed: {})",
                    canonical.display(),
                    allowed
                ),
            ))
        }
    }

    /// Borrow the canonicalized, root-verified path for use with
    /// `std::fs::*` APIs. The newtype is the witness that all the
    /// sanitization gates ran; callers MUST funnel fs calls through
    /// `.as_path()` rather than reconstructing a raw `PathBuf`.
    pub fn as_path(&self) -> &Path {
        &self.canonical
    }
}

/// Compile-time-literal asset name. Construct only via `const fn new` so the
/// `&'static str` lifetime constraint guarantees the value cannot be derived
/// from runtime untrusted input.
#[derive(Clone, Copy, Debug)]
pub struct StaticAssetName(&'static str);

impl StaticAssetName {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Sanitize then read bytes. Use instead of `fs::read` on a
/// hand-canonicalized path so the boundary check and the I/O sink share
/// one call site that Semgrep's `tainted-path` analysis can see.
pub fn read_within(root: &Path, candidate: &Path) -> io::Result<Vec<u8>> {
    let sanitized = SanitizedPath::within(root, candidate)?;
    fs::read(sanitized.as_path())
}

/// Sanitize then read UTF-8 string.
pub fn read_to_string_within(root: &Path, candidate: &Path) -> io::Result<String> {
    let sanitized = SanitizedPath::within(root, candidate)?;
    fs::read_to_string(sanitized.as_path())
}

/// Sanitize then read UTF-8 string, allowing membership under any of
/// several trusted roots.
pub fn read_to_string_within_any(roots: &[&Path], candidate: &Path) -> io::Result<String> {
    let sanitized = SanitizedPath::within_any(roots, candidate)?;
    fs::read_to_string(sanitized.as_path())
}

/// Sanitize then `read_dir`.
pub fn read_dir_within(root: &Path, dir: &Path) -> io::Result<fs::ReadDir> {
    let sanitized = SanitizedPath::within(root, dir)?;
    fs::read_dir(sanitized.as_path())
}

/// Copy a static-named asset from a sanitized src directory to a dst
/// directory. Returns `Ok(())` when src does not exist (no-op). Both
/// `src_dir` and `dst_dir` are caller-trusted; we still funnel the source
/// read through `SanitizedPath` so the type system witnesses the gate
/// adjacent to the `fs::copy` sink.
pub fn copy_static_asset_within(
    src_dir: &Path,
    dst_dir: &Path,
    name: StaticAssetName,
) -> io::Result<()> {
    let src = src_dir.join(name.as_str());
    if !src.exists() {
        return Ok(());
    }
    let src_sanitized = SanitizedPath::within(src_dir, &src)?;
    let dst = dst_dir.join(name.as_str());
    fs::copy(src_sanitized.as_path(), &dst)?;
    Ok(())
}

/// Sanitize the source against `src_root`, then copy it to `dst`. This is
/// the one copy sink for runtime-named artifacts (names that are validated
/// at the call site but are not compile-time literals): the boundary check
/// and the `fs::copy` share this call site, so the taint analysis sees the
/// gate regardless of which file the caller lives in.
pub fn copy_within(src_root: &Path, src: &Path, dst: &Path) -> io::Result<u64> {
    let sanitized = SanitizedPath::within(src_root, src)?;
    fs::copy(sanitized.as_path(), dst)
}

/// Read a static-named artifact from a sanitized src directory under
/// `allowed_root`. The static-string name guarantees the filename
/// component cannot be derived from untrusted input.
pub fn read_static_artifact_within(
    allowed_root: &Path,
    src_dir: &Path,
    name: StaticAssetName,
) -> io::Result<Vec<u8>> {
    let src = src_dir.join(name.as_str());
    let sanitized = SanitizedPath::within(allowed_root, &src)?;
    fs::read(sanitized.as_path())
}

pub struct GitIgnoreChecker {
    repo_root: PathBuf,
    /// In-process libgit2 handle. `git2::Repository` is `Send` but not
    /// `Sync`; the `Mutex` restores `Sync` so shared references stay legal
    /// wherever callers hold `&GitIgnoreChecker` across threads. The hot
    /// path (`is_ignored`) must never spawn a subprocess: the previous
    /// `git check-ignore -q` implementation forked once per walked path,
    /// which alone cost ~13 s per warm `loct context` on an ~900-file repo.
    repo: std::sync::Mutex<git2::Repository>,
}

impl GitIgnoreChecker {
    /// Create a new GitIgnoreChecker for the given path.
    ///
    /// Uses libgit2's repository discovery which properly searches upward
    /// from the given path to find the git repository root. This handles:
    /// - Nested directories (e.g., running from src/deep/nested/)
    /// - Git worktrees (where .git is a file pointing to the main repo)
    /// - Submodules
    ///
    /// Returns `None` if the path is not inside a git repository.
    pub fn new(root: &Path) -> Option<Self> {
        // Use libgit2 to find git root (searches upward properly)
        let repo_root = crate::git::find_git_root(root)?;
        let repo = git2::Repository::discover(root).ok()?;
        Some(Self {
            repo_root,
            repo: std::sync::Mutex::new(repo),
        })
    }

    /// Make `full_path` relative to the repo root so libgit2 can resolve
    /// the ignore-rule source directories (nested .gitignore files).
    ///
    /// `find_git_root` returns libgit2's realpath'd workdir (`/private/var`
    /// on macOS) while walkers hand us paths spelled through symlinks
    /// (`/var/...`), so a raw `strip_prefix` alone is not enough — fall
    /// back to canonicalizing the candidate before giving up. `None` means
    /// "outside this worktree", which is never subject to its ignore rules.
    fn workdir_relative(&self, full_path: &Path) -> Option<PathBuf> {
        if let Ok(relative) = full_path.strip_prefix(&self.repo_root) {
            return Some(relative.to_path_buf());
        }
        let canonical = full_path.canonicalize().ok()?;
        canonical
            .strip_prefix(&self.repo_root)
            .ok()
            .map(Path::to_path_buf)
    }

    pub fn is_ignored(&self, full_path: &Path) -> bool {
        if full_path.as_os_str().is_empty() {
            return false;
        }
        let Some(relative) = self.workdir_relative(full_path) else {
            return false;
        };
        // libgit2 honors nested .gitignore files, .git/info/exclude and the
        // global core.excludesFile — same rule sources as `git check-ignore`.
        // Any error (path outside the worktree, poisoned lock) degrades to
        // "not ignored", matching the old subprocess behavior.
        self.repo
            .lock()
            .ok()
            .and_then(|repo| repo.is_path_ignored(&relative).ok())
            .unwrap_or(false)
    }

    pub fn explain_ignored(&self, full_path: &Path) -> Option<String> {
        if full_path.as_os_str().is_empty() {
            return None;
        }
        let relative = self.workdir_relative(full_path)?;
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .arg("check-ignore")
            .arg("-v")
            .arg("--")
            .arg(relative)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first = stdout.lines().next()?.trim();
        let (rule, _) = first.split_once('\t').unwrap_or((first, ""));
        let mut parts = rule.rsplitn(3, ':');
        let pattern = parts.next().unwrap_or("").trim();
        let line = parts.next().unwrap_or("").trim();
        let source = parts.next().unwrap_or("").trim();
        if source.is_empty() || pattern.is_empty() {
            return Some(format!("ignored by gitignore rule `{rule}`"));
        }
        Some(format!(
            "ignored by {}:{} pattern `{}`",
            source, line, pattern
        ))
    }
}

#[derive(Debug, Default, Clone)]
pub struct LoctignoreRules {
    /// Ignore patterns for file scanning.
    pub ignore_patterns: Vec<String>,
    /// Glob patterns for suppressing dead-export findings.
    ///
    /// Lines in `.loctignore`:
    /// - `@loctignore:dead-ok <glob>`
    pub dead_ok_globs: Vec<String>,
}

fn parse_loctignore_directive(line: &str) -> Option<(&str, &str)> {
    // Syntax: "@loctignore:<directive> <arg...>"
    let rest = line.strip_prefix("@loctignore:")?.trim_start();
    if rest.is_empty() {
        return None;
    }
    // Split once on whitespace (directive + remainder)
    let mut split_at: Option<usize> = None;
    for (idx, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            split_at = Some(idx);
            break;
        }
    }
    match split_at {
        Some(idx) => Some((&rest[..idx], rest[idx..].trim())),
        None => Some((rest, "")),
    }
}

pub fn load_loctignore_rules(root: &Path) -> LoctignoreRules {
    let Some(ignore_file) = active_loctignore_file(root) else {
        return LoctignoreRules::default();
    };

    let file = match File::open(&ignore_file) {
        Ok(f) => f,
        Err(_) => return LoctignoreRules::default(),
    };

    let reader = BufReader::new(file);
    let mut rules = LoctignoreRules::default();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with("@loctignore:") {
            if let Some((directive, arg)) = parse_loctignore_directive(trimmed)
                && directive == "dead-ok"
                && !arg.is_empty()
            {
                rules.dead_ok_globs.push(arg.to_string());
            }
            continue;
        }

        // Treat each non-directive line as an ignore pattern
        rules.ignore_patterns.push(trimmed.to_string());
    }

    rules
}

fn active_loctignore_file(root: &Path) -> Option<PathBuf> {
    let ignore_file = root.join(".loctignore");
    if ignore_file.exists() {
        return Some(ignore_file);
    }
    let legacy = root.join(".loctreeignore");
    legacy.exists().then_some(legacy)
}

/// Load ignore patterns from `.loctignore` (preferred) or `.loctreeignore` (legacy).
///
/// Notes:
/// - Supports `#` comments and empty lines.
/// - Skips `@loctignore:*` directives (handled separately by `load_loctignore_rules`).
/// - Returns empty vec if file doesn't exist.
pub fn load_loctreeignore(root: &Path) -> Vec<String> {
    load_loctignore_rules(root).ignore_patterns
}

pub fn load_loctignore_dead_ok_globs(root: &Path) -> Vec<String> {
    load_loctignore_rules(root).dead_ok_globs
}

fn is_glob_pattern(pattern: &str) -> bool {
    // Minimal, pragmatic detection: if it looks like a glob, treat it as one.
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

#[derive(Debug, Default, Clone)]
pub struct IgnoreMatchers {
    pub ignore_paths: Vec<PathBuf>,
    pub ignore_globs: Option<Arc<globset::GlobSet>>,
}

pub fn build_ignore_matchers(patterns: &[String], root: &Path) -> IgnoreMatchers {
    let mut ignore_paths: Vec<PathBuf> = Vec::new();
    let mut builder = globset::GlobSetBuilder::new();
    let mut any_globs = false;

    for pattern in patterns {
        let trimmed = pattern.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("@loctignore:") {
            continue;
        }

        if is_glob_pattern(trimmed) {
            // For relative patterns, anchor at scan root (absolute match against `full_path`).
            let mut add_glob = |glob_pat: &str| {
                let candidate = if Path::new(glob_pat).is_absolute() {
                    PathBuf::from(glob_pat)
                } else {
                    root.join(glob_pat)
                };
                let Some(mut glob_str) = candidate.to_str().map(|s| s.replace('\\', "/")) else {
                    return;
                };
                // Normalize accidental "./" segments for nicer patterns.
                if glob_str.contains("/./") {
                    glob_str = glob_str.replace("/./", "/");
                }
                match globset::Glob::new(&glob_str) {
                    Ok(glob) => {
                        builder.add(glob);
                        any_globs = true;
                    }
                    Err(e) => {
                        eprintln!("[loctree][warn] invalid ignore glob '{}': {}", glob_pat, e);
                    }
                }
            };

            // A trailing slash means "directory" in gitignore-ish conventions.
            // We add both the directory itself and its contents.
            if let Some(base) = trimmed.strip_suffix('/') {
                if !base.is_empty() {
                    add_glob(base);
                    add_glob(&format!("{}/**", base));
                }
            } else {
                add_glob(trimmed);
            }
            continue;
        }

        // Literal path prefix ignore (fast)
        let candidate = PathBuf::from(trimmed);
        let full = if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        };
        ignore_paths.push(full.canonicalize().unwrap_or(full));
    }

    let ignore_globs = if any_globs {
        match builder.build() {
            Ok(set) => Some(Arc::new(set)),
            Err(e) => {
                eprintln!("[loctree][warn] failed to build ignore globset: {}", e);
                None
            }
        }
    } else {
        None
    };

    IgnoreMatchers {
        ignore_paths,
        ignore_globs,
    }
}

/// Whether `matchers` exclude `abs_path`, using the same two-pronged logic the
/// scanner applies: literal-prefix `ignore_paths` and glob `ignore_globs`.
fn matchers_exclude(matchers: &IgnoreMatchers, abs_path: &Path) -> bool {
    if matchers
        .ignore_paths
        .iter()
        .any(|prefix| abs_path.starts_with(prefix))
    {
        return true;
    }
    if let Some(set) = matchers.ignore_globs.as_ref() {
        let normalized = abs_path.to_string_lossy().replace('\\', "/");
        if set.is_match(&normalized) {
            return true;
        }
    }
    false
}

/// If `target` (relative to `root`) names a path that EXISTS on disk but is
/// excluded from the snapshot by `.loctignore`, return a hint naming the
/// responsible pattern.
///
/// Returns `None` when the target is absent on disk (a genuine wrong path) or is
/// not loctignore-excluded. This lets callers (focus / slice) distinguish
/// "wrong path — check it" from "right path, but parked outside the snapshot by
/// .loctignore", instead of misleadingly telling the user to check a path that
/// is in fact correct (loctree-feedback.md: example-app `docs/` excluded by .loctignore).
pub fn loctignore_exclusion_hint(root: &Path, target: &str) -> Option<String> {
    let abs = root.join(target);
    if !abs.exists() {
        return None;
    }
    let patterns = load_loctreeignore(root);
    if patterns.is_empty() {
        return None;
    }
    let canon = abs.canonicalize().unwrap_or(abs);

    // Best-effort: isolate the single responsible pattern for a precise hint.
    for pattern in &patterns {
        let single = build_ignore_matchers(std::slice::from_ref(pattern), root);
        if matchers_exclude(&single, &canon) {
            return Some(format!(
                "`{}` exists on disk but is excluded from the snapshot by .loctignore (pattern `{}`) — loctree only indexes scanned files, so this is not a wrong path",
                target,
                pattern.trim()
            ));
        }
    }

    // Matched in aggregate but not isolatable to one pattern.
    let all = build_ignore_matchers(&patterns, root);
    if matchers_exclude(&all, &canon) {
        return Some(format!(
            "`{}` exists on disk but is excluded from the snapshot by .loctignore — loctree only indexes scanned files, so this is not a wrong path",
            target
        ));
    }
    None
}

pub fn normalise_ignore_patterns(patterns: &[String], root: &Path) -> Vec<PathBuf> {
    patterns
        .iter()
        .filter(|pattern| {
            let trimmed = pattern.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("@loctignore:")
                && !is_glob_pattern(trimmed)
        })
        .map(|pattern| {
            let candidate = PathBuf::from(pattern);
            let full = if candidate.is_absolute() {
                candidate
            } else {
                root.join(candidate)
            };
            full.canonicalize().unwrap_or(full)
        })
        .collect()
}

/// Count complete UTF-8, NUL-free text; unavailable is distinct from empty.
/// Binary extensions use the scanner's existing policy, not a tree-only list.
pub fn count_lines(path: &Path) -> Option<usize> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            crate::analyzer::scan::BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str())
        })
    {
        return None;
    }
    count_text_lines(File::open(path).ok()?)
}

fn count_text_lines(mut reader: impl Read) -> Option<usize> {
    // At most three bytes of an incomplete UTF-8 codepoint cross a read boundary.
    // Memory stays bounded even for a file consisting of one enormous line.
    let mut buffer = [0u8; 8192 + 3];
    let mut pending = 0;
    let mut count = 0;
    let mut has_bytes = false;
    let mut ends_with_newline = false;
    loop {
        let read = match reader.read(&mut buffer[pending..]) {
            Ok(0) => {
                return (pending == 0)
                    .then_some(count + usize::from(has_bytes && !ends_with_newline));
            }
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let len = pending + read;
        if buffer[..len].contains(&0) {
            return None;
        }
        let valid = match std::str::from_utf8(&buffer[..len]) {
            Ok(_) => len,
            Err(err) if err.error_len().is_none() => err.valid_up_to(),
            Err(_) => return None,
        };
        count += buffer[..valid]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        has_bytes = true;
        ends_with_newline = buffer[len - 1] == b'\n';
        pending = len - valid;
        buffer.copy_within(valid..len, 0);
    }
}

pub fn matches_extension(
    path: &Path,
    extensions: Option<&std::collections::HashSet<String>>,
) -> bool {
    match extensions {
        None => true,
        Some(set) => {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_hidden_truth_config_filename)
            {
                return true;
            }
            if let Some(ext) = path.extension().and_then(|ext| ext.to_str())
                && set.contains(&ext.to_lowercase())
            {
                return true;
            }
            // Filename-based match for extensionless files (Makefile family)
            // — only when the allowed set actually opted into make parsing.
            if (set.contains("mk") || set.contains("make"))
                && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                && matches!(
                    filename,
                    "Makefile" | "makefile" | "GNUmakefile" | "BSDmakefile"
                )
            {
                return true;
            }
            false
        }
    }
}

pub fn is_loctree_config_filename(filename: &str) -> bool {
    matches!(filename, ".loctignore" | ".loctreeignore")
}

pub fn is_hidden_truth_config_filename(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    matches!(
        lower.as_str(),
        ".loctignore"
            | ".loctreeignore"
            | ".editorconfig"
            | ".envrc"
            | ".gitignore"
            | ".gitattributes"
            | ".npmrc"
            | ".nvmrc"
            | ".node-version"
            | ".python-version"
            | ".ruby-version"
            | ".semgrep.yaml"
            | ".semgrep.yml"
            | ".semgrepignore"
            | ".shellcheckrc"
            | ".tool-versions"
    ) || lower.starts_with(".loctree.")
        || lower.starts_with(".eslintrc")
        || lower.starts_with(".prettierrc")
}

/// Shebang-based fallback for extensionless shell scripts (e.g. `./install`,
/// `./bootstrap`, `./configure`). Only fires when:
///   (a) the caller opted into shell parsing via the extensions allow-list, and
///   (b) the file has no extension (so we don't double-classify `.sh` files).
///
/// Reads only the first line — the shebang is always line 1 or nothing.
/// Returns `false` on any I/O error (fail-closed: don't surface unreadable
/// files as shell scripts).
pub fn shebang_source_extension(first_line: &str) -> Option<&'static str> {
    let line = first_line.trim();
    let shebang = line.strip_prefix("#!")?.trim();
    let mut parts = shebang.split_whitespace();
    let first = parts.next()?;
    let mut interpreter = first.rsplit('/').next().unwrap_or(first);

    if interpreter == "env" {
        interpreter = parts
            .find(|part| !part.starts_with('-') && !part.contains('='))
            .and_then(|part| part.rsplit('/').next())
            .unwrap_or_default();
    }

    match interpreter {
        "python" | "python2" | "python3" => Some("py"),
        "node" | "nodejs" | "deno" => Some("js"),
        "ruby" => Some("rb"),
        "bash" | "zsh" | "fish" | "sh" => Some("sh"),
        _ => None,
    }
}

fn extension_set_accepts_shebang_source(
    ext: &str,
    extensions: Option<&std::collections::HashSet<String>>,
) -> bool {
    let Some(set) = extensions else {
        return true;
    };
    match ext {
        "js" => ["js", "jsx", "mjs", "cjs"]
            .iter()
            .any(|candidate| set.contains(*candidate)),
        "sh" => ["sh", "bash", "zsh", "fish"]
            .iter()
            .any(|candidate| set.contains(*candidate)),
        other => set.contains(other),
    }
}

/// Shebang-based fallback for extensionless source entrypoints (e.g. `./tool`,
/// `./bootstrap`, `./git-agent-blackbox`). Only fires when:
///   (a) the file has no extension, and
///   (b) the first line names a known source interpreter accepted by the
///       current extensions allow-list.
pub fn matches_extensionless_source_shebang(
    path: &Path,
    extensions: Option<&std::collections::HashSet<String>>,
) -> bool {
    if path.extension().is_some() {
        return false;
    }
    // Skip the Makefile-family names we already classify as `make` above.
    if let Some(filename) = path.file_name().and_then(|n| n.to_str())
        && matches!(
            filename,
            "Makefile" | "makefile" | "GNUmakefile" | "BSDmakefile"
        )
    {
        return false;
    }
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    // `read_line` returns 0 on EOF; treat both EOF and error as "no shebang".
    if reader.read_line(&mut first_line).unwrap_or(0) == 0 {
        return false;
    }
    shebang_source_extension(&first_line)
        .is_some_and(|ext| extension_set_accepts_shebang_source(ext, extensions))
}

const EXTENSIONLESS_TEXT_SNIFF_BYTES: usize = 512;

/// Generic fallback for extensionless text that should participate in literal
/// truth surfaces (LICENSE, Dockerfile, CODEOWNERS, extensionless scripts).
/// Reads only a small prefix and accepts files whose prefix contains no NUL.
pub fn matches_extensionless_text(path: &Path) -> bool {
    if path.extension().is_some() {
        return false;
    }
    if let Some(filename) = path.file_name().and_then(|n| n.to_str())
        && filename.starts_with('.')
        && !is_hidden_truth_config_filename(filename)
    {
        return false;
    }

    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut buffer = [0u8; EXTENSIONLESS_TEXT_SNIFF_BYTES];
    let Ok(read) = file.read(&mut buffer) else {
        return false;
    };
    !buffer[..read].contains(&0)
}

pub fn matches_extensionless_shell(
    path: &Path,
    extensions: Option<&std::collections::HashSet<String>>,
) -> bool {
    if extensions.is_none() {
        return false;
    }
    if path.extension().is_some() {
        return false;
    }
    if let Some(filename) = path.file_name().and_then(|n| n.to_str())
        && matches!(
            filename,
            "Makefile" | "makefile" | "GNUmakefile" | "BSDmakefile"
        )
    {
        return false;
    }
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).unwrap_or(0) == 0 {
        return false;
    }
    shebang_source_extension(&first_line) == Some("sh")
        && extension_set_accepts_shebang_source("sh", extensions)
}

pub fn is_allowed_hidden(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower == ".env" || lower.starts_with(".env.") {
        return true;
    }
    matches!(
        lower.as_str(),
        ".cargo" | ".config" | ".github" | ".example" | ".devcontainer"
    ) || is_hidden_truth_config_filename(&lower)
}

pub fn explain_ignore_for_path(root: &Path, full_path: &Path) -> Option<String> {
    let comparable_path = full_path
        .canonicalize()
        .unwrap_or_else(|_| full_path.to_path_buf());
    if let Some(note) = explain_loctignore_match(root, &comparable_path) {
        return Some(note);
    }
    if let Some(checker) = GitIgnoreChecker::new(root)
        && let Some(note) = checker.explain_ignored(&comparable_path)
    {
        return Some(note);
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel_path = comparable_path
        .strip_prefix(&root)
        .unwrap_or(&comparable_path);
    for component in rel_path.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let name = name.to_string_lossy();
        if name.starts_with('.') && !is_allowed_hidden(&name) {
            return Some(format!(
                "skipped by default hidden-file filter for `{}`",
                name
            ));
        }
    }
    let name = comparable_path.file_name()?.to_string_lossy();
    if name.starts_with('.') && !is_allowed_hidden(&name) {
        return Some(format!(
            "skipped by default hidden-file filter for `{}`",
            name
        ));
    }
    None
}

fn explain_loctignore_match(root: &Path, full_path: &Path) -> Option<String> {
    let ignore_file = active_loctignore_file(root)?;
    let file = File::open(&ignore_file).ok()?;
    let reader = BufReader::new(file);
    for (idx, line) in reader.lines().enumerate() {
        let line = line.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("@loctignore:") {
            continue;
        }
        let matchers = build_ignore_matchers(&[trimmed.to_string()], root);
        let options = Options {
            ignore_paths: matchers.ignore_paths,
            ignore_globs: matchers.ignore_globs,
            use_gitignore: false,
            ..Default::default()
        };
        if should_ignore(full_path, &options, None) {
            let source = ignore_file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(".loctignore");
            return Some(format!(
                "ignored by {}:{} pattern `{}`",
                source,
                idx + 1,
                trimmed
            ));
        }
    }
    None
}

pub fn should_ignore(
    full_path: &Path,
    options: &Options,
    git_checker: Option<&GitIgnoreChecker>,
) -> bool {
    if options
        .ignore_paths
        .iter()
        .any(|ignored| full_path.starts_with(ignored))
    {
        return true;
    }
    if let Some(globs) = &options.ignore_globs
        && globs.is_match(full_path)
    {
        return true;
    }
    if options.use_gitignore
        && let Some(checker) = git_checker
        && checker.is_ignored(full_path)
    {
        return true;
    }
    false
}

/// N4 (round-2): why the scan walk dropped a path. Recorded per reason into
/// [`GatherDiagnostics`] so universe accounting can name what fell outside
/// the index instead of a bare "outside_snapshot" boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatherExclusionReason {
    /// Matched by the git ignore checker (`.gitignore` and friends).
    Gitignore,
    /// Matched a `.loctignore`/`--ignore` path-prefix or glob rule (both
    /// share one matcher channel at walk time and cannot be told apart here).
    IgnoreRule,
    /// Dotfile/dot-dir not whitelisted by `show_hidden`/allowed-hidden rules.
    Hidden,
    /// `node_modules`/`.git`/`target`/`.venv`/`venv`/`__pycache__` with
    /// `scan_all` off.
    HeavyDir,
    /// File extension not in the analyzer extension set.
    UnsupportedExtension,
    /// Directory carrying its own `.git` - another project's checkout.
    NestedRepo,
    /// Symlink whose target escapes the scan root.
    SymlinkEscape,
    /// Directory beyond `max_depth`.
    Depth,
    /// Broken symlink or metadata/stat failure.
    Unreadable,
}

impl GatherExclusionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Gitignore => "gitignore",
            Self::IgnoreRule => "ignore_rule",
            Self::Hidden => "hidden",
            Self::HeavyDir => "heavy_dir",
            Self::UnsupportedExtension => "unsupported_extension",
            Self::NestedRepo => "nested_repo",
            Self::SymlinkEscape => "symlink_escape",
            Self::Depth => "depth",
            Self::Unreadable => "unreadable",
        }
    }

    /// Human phrasing used in universe exclusion `reason` strings.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Gitignore => "excluded by gitignore rules",
            Self::IgnoreRule => "excluded by .loctignore/--ignore rules",
            Self::Hidden => "hidden dotfile/dot-dir not shown",
            Self::HeavyDir => "heavy directory skipped (node_modules/.git/target/...)",
            Self::UnsupportedExtension => "extension outside the analyzer set",
            Self::NestedRepo => "nested git checkout boundary",
            Self::SymlinkEscape => "symlink target escapes the scan root",
            Self::Depth => "beyond max_depth",
            Self::Unreadable => "broken symlink or unreadable metadata",
        }
    }
}

/// Max sample paths kept per exclusion reason. Beyond the cap only the exact
/// count grows - full excluded lists on huge repos would be a payload-budget
/// DoS (N4 boundary 4).
pub const GATHER_SAMPLE_CAP: usize = 50;

/// One reason bucket: exact count plus a capped path sample.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatherExclusionBucket {
    pub reason: GatherExclusionReason,
    pub count: usize,
    /// First [`GATHER_SAMPLE_CAP`] excluded paths (scan-root-relative when
    /// possible). Truncated by construction; compare against `count`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample: Vec<String>,
}

/// Scan-walk exclusion diagnostics: which paths the gather dropped and why.
/// Attached to the `Snapshot` so universe accounting can name excluded
/// surfaces instead of a bare `outside_snapshot` with `files: None`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatherDiagnostics {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<GatherExclusionBucket>,
}

impl GatherDiagnostics {
    fn record(&mut self, reason: GatherExclusionReason, path: &Path, scan_root: &Path) {
        let idx = match self.buckets.iter().position(|b| b.reason == reason) {
            Some(idx) => idx,
            None => {
                self.buckets.push(GatherExclusionBucket {
                    reason,
                    count: 0,
                    sample: Vec::new(),
                });
                self.buckets.len() - 1
            }
        };
        let bucket = &mut self.buckets[idx];
        bucket.count += 1;
        if bucket.sample.len() < GATHER_SAMPLE_CAP {
            let display = path
                .strip_prefix(scan_root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            bucket.sample.push(display);
        }
    }

    pub fn total(&self) -> usize {
        self.buckets.iter().map(|bucket| bucket.count).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// Fold another root's diagnostics into this one (multi-root scans),
    /// keeping the per-reason sample cap.
    pub fn merge(&mut self, other: GatherDiagnostics) {
        for bucket in other.buckets {
            match self.buckets.iter_mut().find(|b| b.reason == bucket.reason) {
                Some(existing) => {
                    existing.count += bucket.count;
                    for path in bucket.sample {
                        if existing.sample.len() >= GATHER_SAMPLE_CAP {
                            break;
                        }
                        existing.sample.push(path);
                    }
                }
                None => self.buckets.push(bucket),
            }
        }
    }
}

/// Process-local handoff of gather diagnostics from the scan walk to
/// snapshot assembly, keyed by canonical scan root. This avoids changing the
/// `gather_files` signature for callers outside this cut's write scope
/// (`root_scan.rs`); the snapshot builder drains entries via
/// [`take_gather_diagnostics`]. At most one entry per distinct scanned root
/// lives here, and only until the snapshot build drains it.
static GATHER_DIAGNOSTICS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, GatherDiagnostics>>,
> = std::sync::OnceLock::new();

fn gather_diagnostics_registry()
-> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, GatherDiagnostics>> {
    GATHER_DIAGNOSTICS.get_or_init(std::sync::Mutex::default)
}

/// Drain gather-walk diagnostics recorded for `root` (canonicalized inside).
/// Returns `None` when nothing was excluded or the entry was already taken.
pub fn take_gather_diagnostics(root: &Path) -> Option<GatherDiagnostics> {
    let canon = root.canonicalize().ok()?;
    let registry = GATHER_DIAGNOSTICS.get()?;
    let mut map = registry.lock().ok()?;
    map.remove(&canon).filter(|diag| !diag.is_empty())
}

/// Like [`should_ignore`], but reports WHICH channel matched so the walk can
/// record an honest exclusion reason.
fn ignore_reason(
    full_path: &Path,
    options: &Options,
    git_checker: Option<&GitIgnoreChecker>,
) -> Option<GatherExclusionReason> {
    if options
        .ignore_paths
        .iter()
        .any(|ignored| full_path.starts_with(ignored))
    {
        return Some(GatherExclusionReason::IgnoreRule);
    }
    if let Some(globs) = &options.ignore_globs
        && globs.is_match(full_path)
    {
        return Some(GatherExclusionReason::IgnoreRule);
    }
    if options.use_gitignore
        && let Some(checker) = git_checker
        && checker.is_ignored(full_path)
    {
        return Some(GatherExclusionReason::Gitignore);
    }
    None
}

pub fn gather_files(
    dir: &Path,
    options: &Options,
    depth: usize,
    git_checker: Option<&GitIgnoreChecker>,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let dir_canon = dir.canonicalize()?;
    let mut diagnostics = GatherDiagnostics::default();
    // First call: scan_root = dir_canon. Recursive calls pass it through.
    gather_files_inner(
        dir,
        &dir_canon,
        options,
        depth,
        git_checker,
        visited,
        files,
        &mut diagnostics,
    )?;
    if !diagnostics.is_empty()
        && let Ok(mut map) = gather_diagnostics_registry().lock()
    {
        map.insert(dir_canon, diagnostics);
    }
    Ok(())
}

/// A subdirectory that carries its own `.git` (directory, or file for
/// submodules/worktrees) is another project's checkout, not content of the
/// scanned repo. Field incident 2026-08-14: an untracked Xcode `build-local/`
/// held `SourcePackages/checkouts/*` — dozens of full dependency repos — and
/// walking into them ballooned a scan of a mid-size app to 2.9 GB RSS and
/// minutes of allocator churn, twice over because `context` and `scan` ran
/// concurrently. The walk must stop at nested repo boundaries.
fn is_nested_git_repo(dir: &Path) -> bool {
    // `.git` is a dir in normal checkouts and a file in submodules/worktrees;
    // either form marks a repo boundary.
    dir.join(".git").exists()
}

// The walker's state is threaded explicitly through the recursion; folding it
// into a context struct would touch every call site for no behavioral gain.
#[allow(clippy::too_many_arguments)]
fn gather_files_inner(
    dir: &Path,
    scan_root: &Path,
    options: &Options,
    depth: usize,
    git_checker: Option<&GitIgnoreChecker>,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
    diagnostics: &mut GatherDiagnostics,
) -> io::Result<()> {
    let dir_canon = dir.canonicalize()?;
    if !visited.insert(dir_canon.clone()) {
        return Ok(());
    }

    let mut dir_entries: Vec<_> = fs::read_dir(&dir_canon)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Skip common heavy directories unless --scan-all is set
            if !options.scan_all
                && (name_str == "node_modules"
                    || name_str == ".git"
                    || name_str == "target"
                    || name_str == ".venv"
                    || name_str == "venv"
                    || name_str == "__pycache__")
            {
                diagnostics.record(GatherExclusionReason::HeavyDir, &entry.path(), scan_root);
                return false;
            }

            let is_hidden = name_str.starts_with('.');
            let shown = options.show_hidden || !is_hidden || is_allowed_hidden(&name_str);
            if !shown {
                diagnostics.record(GatherExclusionReason::Hidden, &entry.path(), scan_root);
            }
            shown
        })
        .collect();

    dir_entries.sort_by(|a, b| {
        a.file_name()
            .to_string_lossy()
            .to_lowercase()
            .cmp(&b.file_name().to_string_lossy().to_lowercase())
    });

    for entry in dir_entries {
        let path = entry.path();
        if let Some(reason) = ignore_reason(&path, options, git_checker) {
            diagnostics.record(reason, &path, scan_root);
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => {
                diagnostics.record(GatherExclusionReason::Unreadable, &path, scan_root);
                continue;
            }
        };
        if file_type.is_symlink() {
            let target = match fs::canonicalize(&path) {
                Ok(p) => p,
                Err(_) => {
                    diagnostics.record(GatherExclusionReason::Unreadable, &path, scan_root);
                    continue; // broken symlink
                }
            };
            if visited.contains(&target) {
                continue;
            }
            // Don't follow symlinks that escape the scan root (e.g. DMG staging
            // dirs with /Applications symlink). Compares against the top-level
            // scan root, not the current recursion dir, so intra-repo symlinks
            // like src/data -> ../shared/data still work.
            if !target.starts_with(scan_root) {
                diagnostics.record(GatherExclusionReason::SymlinkEscape, &path, scan_root);
                continue;
            }
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => {
                    diagnostics.record(GatherExclusionReason::Unreadable, &path, scan_root);
                    continue;
                }
            };
            if meta.is_dir() && options.max_depth.is_none_or(|max| depth < max) {
                if !options.scan_all && is_nested_git_repo(&target) {
                    diagnostics.record(GatherExclusionReason::NestedRepo, &target, scan_root);
                    continue;
                }
                gather_files_inner(
                    &target,
                    scan_root,
                    options,
                    depth + 1,
                    git_checker,
                    visited,
                    files,
                    diagnostics,
                )?;
            } else if meta.is_dir() {
                diagnostics.record(GatherExclusionReason::Depth, &target, scan_root);
            } else if meta.is_file()
                && (matches_extension(&target, options.extensions.as_ref())
                    || matches_extensionless_source_shebang(&target, options.extensions.as_ref())
                    || matches_extensionless_text(&target))
            {
                files.push(target);
            } else if meta.is_file() {
                diagnostics.record(
                    GatherExclusionReason::UnsupportedExtension,
                    &target,
                    scan_root,
                );
            }
            continue;
        }

        if path.is_file() {
            let canonical = path.canonicalize().unwrap_or(path.clone());
            if matches_extension(&canonical, options.extensions.as_ref())
                || matches_extensionless_source_shebang(&canonical, options.extensions.as_ref())
                || matches_extensionless_text(&canonical)
            {
                files.push(canonical);
            } else {
                diagnostics.record(
                    GatherExclusionReason::UnsupportedExtension,
                    &path,
                    scan_root,
                );
            }
            continue;
        }
        if path.is_dir() && options.max_depth.is_none_or(|max| depth < max) {
            if !options.scan_all && is_nested_git_repo(&path) {
                diagnostics.record(GatherExclusionReason::NestedRepo, &path, scan_root);
                continue;
            }
            gather_files_inner(
                &path,
                scan_root,
                options,
                depth + 1,
                git_checker,
                visited,
                files,
                diagnostics,
            )?;
        } else if path.is_dir() {
            diagnostics.record(GatherExclusionReason::Depth, &path, scan_root);
        }
    }

    Ok(())
}

pub fn sort_dir_entries(entries: &mut [std::fs::DirEntry]) {
    entries.sort_by(|a, b| {
        let a_path = a.path();
        let b_path = b.path();
        let a_is_dir = a_path.is_dir();
        let b_is_dir = b_path.is_dir();
        match (a_is_dir, b_is_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .cmp(&b.file_name().to_string_lossy().to_lowercase()),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ColorMode, Options, OutputMode};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn opts_with_ext(ext: &str) -> Options {
        Options {
            extensions: Some(HashSet::from([ext.to_string()])),
            ignore_paths: Vec::new(),
            ignore_globs: None,
            use_gitignore: false,
            max_depth: Some(3),
            color: ColorMode::Never,
            output: OutputMode::Human,
            summary: false,
            summary_limit: 5,
            summary_only: false,
            show_hidden: false,
            show_ignored: false,
            loc_threshold: crate::types::DEFAULT_LOC_THRESHOLD,
            analyze_limit: 8,
            report_path: None,
            serve: false,
            editor_cmd: None,
            max_graph_nodes: None,
            max_graph_edges: None,
            verbose: false,
            scan_all: false,
            symbol: None,
            impact: None,
            find_artifacts: false,
        }
    }

    fn default_opts() -> Options {
        Options {
            extensions: None,
            ignore_paths: Vec::new(),
            ignore_globs: None,
            use_gitignore: false,
            max_depth: None,
            color: ColorMode::Never,
            output: OutputMode::Human,
            summary: false,
            summary_limit: 5,
            summary_only: false,
            show_hidden: false,
            show_ignored: false,
            loc_threshold: crate::types::DEFAULT_LOC_THRESHOLD,
            analyze_limit: 8,
            report_path: None,
            serve: false,
            editor_cmd: None,
            max_graph_nodes: None,
            max_graph_edges: None,
            verbose: false,
            scan_all: false,
            symbol: None,
            impact: None,
            find_artifacts: false,
        }
    }

    #[test]
    fn gather_files_filters_by_extension_and_depth() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("nested")).expect("tmp nested dir");
        std::fs::write(root.join("keep.rs"), "// ok").expect("write keep.rs");
        std::fs::write(root.join("skip.txt"), "// skip").expect("write skip.txt");
        std::fs::write(root.join(".hidden.rs"), "// hidden").expect("write hidden");
        std::fs::write(root.join("nested").join("deep.rs"), "// deep").expect("write deep.rs");

        let mut files = Vec::new();
        let opts = opts_with_ext("rs");
        let mut visited = HashSet::new();
        gather_files(root, &opts, 0, None, &mut visited, &mut files).expect("gather files");

        let as_strings: Vec<String> = files
            .iter()
            .map(|p| {
                p.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert!(as_strings.contains(&"keep.rs".to_string()));
        assert!(!as_strings.contains(&"skip.txt".to_string()));
        assert!(as_strings.contains(&"deep.rs".to_string()));
        assert!(!as_strings.contains(&".hidden.rs".to_string()));
    }

    /// N4 (round-2): the walk records excluded paths with reasons so universe
    /// accounting can name what fell outside the index.
    #[test]
    fn gather_records_exclusion_diagnostics() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();
        std::fs::write(root.join("keep.rs"), "// ok").expect("write keep.rs");
        std::fs::write(root.join("skip.txt"), "nope").expect("write skip.txt");
        std::fs::create_dir_all(root.join("node_modules")).expect("node_modules");
        std::fs::write(root.join("node_modules").join("pkg.rs"), "// vendored")
            .expect("vendored file");
        std::fs::create_dir_all(root.join(".hidden")).expect("hidden dir");
        std::fs::write(root.join(".hidden").join("data.rs"), "// hidden").expect("hidden file");

        let mut files = Vec::new();
        let opts = opts_with_ext("rs");
        let mut visited = HashSet::new();
        gather_files(root, &opts, 0, None, &mut visited, &mut files).expect("gather files");

        let diag = take_gather_diagnostics(root).expect("diagnostics must be recorded");
        let bucket =
            |reason: GatherExclusionReason| diag.buckets.iter().find(|b| b.reason == reason);

        let unsupported = bucket(GatherExclusionReason::UnsupportedExtension)
            .expect("skip.txt must be counted as unsupported_extension");
        assert_eq!(unsupported.count, 1);
        assert!(
            unsupported.sample.iter().any(|p| p.ends_with("skip.txt")),
            "sample must name the excluded path: {:?}",
            unsupported.sample
        );

        let heavy =
            bucket(GatherExclusionReason::HeavyDir).expect("node_modules must be heavy_dir");
        assert!(heavy.sample.iter().any(|p| p.ends_with("node_modules")));

        let hidden = bucket(GatherExclusionReason::Hidden).expect(".hidden must be hidden");
        assert!(hidden.sample.iter().any(|p| p.ends_with(".hidden")));

        // The take drains the entry - a second take observes nothing.
        assert!(take_gather_diagnostics(root).is_none());
    }

    #[test]
    fn allows_whitelisted_hidden_files() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();
        std::fs::write(root.join(".env.local"), "KEY=1").expect("env local");
        std::fs::create_dir_all(root.join(".cargo")).expect("cargo dir");
        std::fs::write(root.join(".cargo").join("config.toml"), "[target]\n")
            .expect("cargo config");
        std::fs::create_dir_all(root.join(".config")).expect("config dir");
        std::fs::write(
            root.join(".config").join("loctree.toml"),
            "mode = 'local'\n",
        )
        .expect("config file");
        std::fs::create_dir_all(root.join(".github").join("workflows")).expect("github workflows");
        std::fs::write(
            root.join(".github").join("workflows").join("release.yml"),
            "name: release\n",
        )
        .expect("github workflow");
        std::fs::write(root.join(".loctree.json"), "{}").expect("loctree json");
        std::fs::write(root.join(".example"), "// example").expect("example");
        std::fs::write(root.join(".ignored"), "// ignore").expect("ignored");

        let mut files = Vec::new();
        let opts = Options {
            extensions: None,
            ignore_paths: Vec::new(),
            ignore_globs: None,
            use_gitignore: false,
            max_depth: None,
            color: ColorMode::Never,
            output: OutputMode::Human,
            summary: false,
            summary_limit: 5,
            summary_only: false,
            show_hidden: false,
            show_ignored: false,
            loc_threshold: crate::types::DEFAULT_LOC_THRESHOLD,
            analyze_limit: 8,
            report_path: None,
            serve: false,
            editor_cmd: None,
            max_graph_nodes: None,
            max_graph_edges: None,
            verbose: false,
            scan_all: false,
            symbol: None,
            impact: None,
            find_artifacts: false,
        };
        let mut visited = HashSet::new();
        gather_files(root, &opts, 0, None, &mut visited, &mut files).expect("gather files");
        let names: HashSet<PathBuf> = files
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.into()))
            .collect();
        assert!(names.contains(&PathBuf::from(".env.local")));
        assert!(names.contains(&PathBuf::from("config.toml")));
        assert!(names.contains(&PathBuf::from("loctree.toml")));
        assert!(names.contains(&PathBuf::from("release.yml")));
        assert!(names.contains(&PathBuf::from(".loctree.json")));
        assert!(names.contains(&PathBuf::from(".example")));
        assert!(!names.contains(&PathBuf::from(".ignored")));
    }

    #[test]
    #[cfg(unix)]
    fn avoids_symlink_loops() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).expect("mkdir a");
        std::fs::create_dir_all(&b).expect("mkdir b");
        std::fs::write(a.join("keep.rs"), "// ok").expect("write keep");
        symlink(&b, a.join("loop_to_b")).expect("symlink b");
        symlink(&a, b.join("loop_to_a")).expect("symlink a");

        let mut files = Vec::new();
        let opts = opts_with_ext("rs");
        let mut visited = HashSet::new();
        gather_files(root, &opts, 0, None, &mut visited, &mut files).expect("gather files");
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert_eq!(names, vec!["keep.rs".to_string()]);
    }

    #[test]
    fn test_count_lines() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "line1\nline2\nline3\n").expect("write file");

        let count = count_lines(&file_path);
        assert_eq!(count, Some(3));
    }

    #[test]
    fn test_count_lines_empty_file() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let file_path = tmp.path().join("empty.txt");
        std::fs::write(&file_path, "").expect("write file");

        let count = count_lines(&file_path);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_lines_missing_file() {
        let count = count_lines(Path::new("/nonexistent/file.txt"));
        assert!(count.is_none());
    }

    #[test]
    fn test_count_lines_text_contract_and_utf8_boundaries() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let path = tmp.path().join("text.rs");
        for (text, expected) in [
            ("", 0),
            ("\n", 1),
            ("one", 1),
            ("one\n", 1),
            ("one\ntwo", 2),
            ("one\r\ntwo\r\n", 2),
            ("Zażółć gęślą 🦀\n東京", 2),
            ("\r", 1),
        ] {
            std::fs::write(&path, text).expect("write text");
            assert_eq!(count_lines(&path), Some(expected), "{text:?}");
        }
        for padding in 8189..8197 {
            let text = format!("{}é東🦀\nlast", "a".repeat(padding));
            std::fs::write(&path, &text).expect("write boundary text");
            assert_eq!(count_lines(&path), Some(2), "padding={padding}");
        }
        std::fs::write(&path, "a".repeat(1_000_000)).expect("write long line");
        assert_eq!(count_lines(&path), Some(1));
    }

    #[test]
    fn test_count_lines_rejects_binary_and_late_invalid_text() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let path = tmp.path().join("text.rs");
        for tail in [b"\xff\n".as_slice(), b"\xe2\x82", b"\0tail\n"] {
            let mut bytes = b"valid\n".repeat(20_000);
            bytes.extend_from_slice(tail);
            std::fs::write(&path, bytes).expect("write invalid text");
            assert_eq!(count_lines(&path), None, "tail={tail:?}");
        }
        for bytes in [
            b"\x89PNG\r\n\x1a\nreadable tail".as_slice(),
            b"a\0b\0\n\0",
            b"\xff\xfea\0",
        ] {
            std::fs::write(&path, bytes).expect("write binary bytes");
            assert_eq!(count_lines(&path), None);
        }
        // Shared binary policy applies even when a binary suffix has ASCII bytes.
        let binary = tmp.path().join("asset.PNG");
        std::fs::write(&binary, b"readable tail\n").expect("write binary extension");
        assert_eq!(count_lines(&binary), None);
        let svg = tmp.path().join("asset.svg");
        std::fs::write(&svg, "<svg>é</svg>\n").expect("write text resource");
        assert_eq!(count_lines(&svg), Some(1));
        assert_eq!(
            count_lines(tmp.path()),
            None,
            "directory is not readable text"
        );
    }

    #[test]
    fn test_count_lines_read_failure_never_returns_partial_count() {
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        // Inject permission denial after real successful reads: deterministic
        // under root too, unlike chmod-based tests that can falsely pass.
        let prefix = io::Cursor::new(b"valid\n".repeat(20_000));
        assert_eq!(count_text_lines(prefix.chain(FailingReader)), None);
    }

    #[test]
    fn test_matches_extension_with_set() {
        let extensions: HashSet<String> = ["rs", "ts", "js"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        assert!(matches_extension(Path::new("file.rs"), Some(&extensions)));
        assert!(matches_extension(Path::new("file.ts"), Some(&extensions)));
        assert!(matches_extension(Path::new("file.RS"), Some(&extensions))); // case insensitive
        assert!(matches_extension(
            Path::new(".loctignore"),
            Some(&extensions)
        ));
        assert!(matches_extension(
            Path::new(".loctreeignore"),
            Some(&extensions)
        ));
        assert!(matches_extension(
            Path::new(".gitignore"),
            Some(&extensions)
        ));
        assert!(matches_extension(
            Path::new(".semgrep.yaml"),
            Some(&extensions)
        ));
        assert!(matches_extension(
            Path::new(".prettierrc.json"),
            Some(&extensions)
        ));
        assert!(!matches_extension(
            Path::new(".env.local"),
            Some(&extensions)
        ));
        assert!(!matches_extension(Path::new("file.py"), Some(&extensions)));
        assert!(!matches_extension(Path::new("noext"), Some(&extensions)));
    }

    #[test]
    fn test_matches_extension_none() {
        // None means no filter - all files match
        assert!(matches_extension(Path::new("file.rs"), None));
        assert!(matches_extension(Path::new("file.txt"), None));
        assert!(matches_extension(Path::new("noext"), None));
    }

    #[test]
    fn test_matches_extension_makefile_filename_fallback() {
        // When `mk` or `make` is in the allowed set, Makefile-family names
        // (which have no extension) should match via filename fallback.
        let exts: HashSet<String> = ["mk", "make"].into_iter().map(|s| s.to_string()).collect();
        assert!(matches_extension(Path::new("Makefile"), Some(&exts)));
        assert!(matches_extension(Path::new("src/GNUmakefile"), Some(&exts)));
        assert!(matches_extension(Path::new("common.mk"), Some(&exts)));
        assert!(!matches_extension(Path::new("Dockerfile"), Some(&exts)));

        // Without `mk`/`make` in the set, Makefile should be excluded.
        let exts_no_make: HashSet<String> = ["rs"].into_iter().map(|s| s.to_string()).collect();
        assert!(!matches_extension(
            Path::new("Makefile"),
            Some(&exts_no_make)
        ));
    }

    #[test]
    fn test_matches_extensionless_shell_shebang() {
        let tmp = tempfile::tempdir().expect("tmp dir");

        // Extensionless shell script with bash shebang
        let install = tmp.path().join("install");
        std::fs::write(&install, "#!/usr/bin/env bash\nset -e\necho hi\n").expect("write install");

        // Extensionless script with non-shell shebang (python) — must NOT match
        let pyscript = tmp.path().join("run-tool");
        std::fs::write(&pyscript, "#!/usr/bin/env python3\nprint('hi')\n").expect("write py");

        // Extensionless file with no shebang — must NOT match
        let random = tmp.path().join("README");
        std::fs::write(&random, "plain text\n").expect("write random");

        // File *with* .sh extension — must NOT be re-classified by this fallback
        // (the primary extension check already catches it).
        let real_sh = tmp.path().join("deploy.sh");
        std::fs::write(&real_sh, "#!/bin/bash\necho ok\n").expect("write sh");

        // Makefile-family names must NOT be re-classified as shell.
        let makefile = tmp.path().join("Makefile");
        std::fs::write(&makefile, "#!/bin/bash\nall:\n").expect("write makefile");

        let exts: HashSet<String> = ["sh", "bash", "zsh", "fish"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        assert!(
            matches_extensionless_shell(&install, Some(&exts)),
            "extensionless bash script must match"
        );
        assert!(
            !matches_extensionless_shell(&pyscript, Some(&exts)),
            "python shebang must NOT match"
        );
        assert!(
            !matches_extensionless_shell(&random, Some(&exts)),
            "no shebang must NOT match"
        );
        assert!(
            !matches_extensionless_shell(&real_sh, Some(&exts)),
            "file with .sh extension must be handled by primary check, not fallback"
        );
        assert!(
            !matches_extensionless_shell(&makefile, Some(&exts)),
            "Makefile name must NOT be re-classified as shell"
        );

        // Without shell extensions in the allow-list the fallback must stay quiet.
        let no_shell: HashSet<String> = ["rs", "ts"].into_iter().map(|s| s.to_string()).collect();
        assert!(!matches_extensionless_shell(&install, Some(&no_shell)));
        assert!(!matches_extensionless_shell(&install, None));
    }

    #[test]
    fn test_matches_extensionless_source_shebang() {
        let tmp = tempfile::tempdir().expect("tmp dir");

        let py = tmp.path().join("py-tool");
        std::fs::write(&py, "#!/usr/bin/env python3\nprint('hi')\n").expect("write py");

        let node = tmp.path().join("node-tool");
        std::fs::write(&node, "#!/usr/bin/env node\nconsole.log('hi')\n").expect("write node");

        let deno = tmp.path().join("deno-tool");
        std::fs::write(&deno, "#!/usr/bin/env deno\nconsole.log('hi')\n").expect("write deno");

        let ruby = tmp.path().join("ruby-tool");
        std::fs::write(&ruby, "#!/usr/bin/env ruby\nputs 'hi'\n").expect("write ruby");

        let random = tmp.path().join("README");
        std::fs::write(&random, "plain text\n").expect("write random");

        let exts: HashSet<String> = ["py", "js", "rb"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        assert!(matches_extensionless_source_shebang(&py, Some(&exts)));
        assert!(matches_extensionless_source_shebang(&node, Some(&exts)));
        assert!(matches_extensionless_source_shebang(&deno, Some(&exts)));
        assert!(matches_extensionless_source_shebang(&ruby, Some(&exts)));
        assert!(!matches_extensionless_source_shebang(&random, Some(&exts)));

        let no_python: HashSet<String> = ["rs", "ts"].into_iter().map(|s| s.to_string()).collect();
        assert!(!matches_extensionless_source_shebang(&py, Some(&no_python)));
    }

    #[test]
    fn test_gather_files_collects_extensionless_shell() {
        // E2E of the gate: put an extensionless bash script into a temp dir and
        // confirm gather_files picks it up when shell extensions are opted in.
        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();

        std::fs::write(root.join("install"), "#!/usr/bin/env bash\necho ok\n").unwrap();
        std::fs::write(root.join("run-tool"), "#!/usr/bin/env python3\n").unwrap();
        std::fs::write(root.join("deploy.sh"), "#!/bin/bash\n").unwrap();

        let exts: HashSet<String> = ["sh", "bash", "zsh", "fish", "py"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let options = crate::types::Options {
            extensions: Some(exts),
            ..Default::default()
        };
        let mut visited = HashSet::new();
        let mut files: Vec<PathBuf> = Vec::new();
        gather_files(root, &options, 0, None, &mut visited, &mut files).expect("gather");

        let names: Vec<String> = files
            .iter()
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .collect();

        assert!(
            names.iter().any(|n| n == "install"),
            "extensionless bash script missing from collection: {:?}",
            names
        );
        assert!(
            names.iter().any(|n| n == "deploy.sh"),
            "regular .sh file missing from collection: {:?}",
            names
        );
        assert!(
            names.iter().any(|n| n == "run-tool"),
            "extensionless python script missing from collection: {:?}",
            names
        );
    }

    #[test]
    fn test_gather_files_collects_extensionless_text_but_not_binary() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();

        std::fs::write(root.join("LICENSE"), "extensionless license text\n")
            .expect("write LICENSE");
        std::fs::write(root.join("Dockerfile"), "FROM scratch\n").expect("write Dockerfile");
        std::fs::write(root.join("CODEOWNERS"), "* @loctree/team\n").expect("write CODEOWNERS");
        std::fs::write(root.join("plain-script"), "echo no shebang but text\n")
            .expect("write plain-script");
        std::fs::write(root.join("blob"), b"\x7fELF\0not text").expect("write blob");
        std::fs::write(root.join(".env"), "SECRET_SHOULD_NOT_ENTER=1\n").expect("write .env");

        let exts: HashSet<String> = ["rs", "make", "mk"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let options = crate::types::Options {
            extensions: Some(exts),
            ..Default::default()
        };
        let mut visited = HashSet::new();
        let mut files: Vec<PathBuf> = Vec::new();
        gather_files(root, &options, 0, None, &mut visited, &mut files).expect("gather");

        let names: Vec<String> = files
            .iter()
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .collect();

        for expected in ["LICENSE", "Dockerfile", "CODEOWNERS", "plain-script"] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} missing from extensionless text collection: {:?}",
                names
            );
        }
        assert!(
            !names.iter().any(|n| n == "blob"),
            "NUL-bearing extensionless binary must stay out of collection: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n == ".env"),
            "generic hidden extensionless files must not enter via text sniff: {:?}",
            names
        );
    }

    #[test]
    fn test_is_allowed_hidden() {
        // Allowed hidden files
        assert!(is_allowed_hidden(".env"));
        assert!(is_allowed_hidden(".ENV")); // case insensitive
        assert!(is_allowed_hidden(".env.local"));
        assert!(is_allowed_hidden(".env.production"));
        assert!(is_allowed_hidden(".loctignore"));
        assert!(is_allowed_hidden(".loctreeignore"));
        assert!(is_allowed_hidden(".loctree.json"));
        assert!(is_allowed_hidden(".loctree.yml"));
        assert!(is_allowed_hidden(".example"));
        assert!(is_allowed_hidden(".cargo"));
        assert!(is_allowed_hidden(".config"));
        assert!(is_allowed_hidden(".github"));
        assert!(is_allowed_hidden(".editorconfig"));
        assert!(is_allowed_hidden(".gitignore"));
        assert!(is_allowed_hidden(".npmrc"));
        assert!(is_allowed_hidden(".semgrep.yaml"));
        assert!(is_allowed_hidden(".semgrepignore"));
        assert!(is_allowed_hidden(".tool-versions"));
        assert!(is_allowed_hidden(".prettierrc.json"));
        assert!(is_allowed_hidden(".eslintrc.cjs"));

        // Not allowed
        assert!(!is_allowed_hidden(".hidden"));
        assert!(!is_allowed_hidden(".ssh"));
    }

    #[test]
    fn test_is_allowed_hidden_accepts_devcontainer() {
        // H9: `.devcontainer/` is a tracked, agent-relevant dot-dir (devcontainer.json,
        // Dockerfile) that was silently dropped by the hidden-dir filter like any
        // other dotfile. It belongs in the allowlist alongside `.github`/`.cargo`.
        assert!(is_allowed_hidden(".devcontainer"));
        assert!(is_allowed_hidden(".DEVCONTAINER")); // case insensitive, matches other entries
    }

    #[test]
    fn explain_ignore_for_path_reports_hidden_parent_filter() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        std::fs::create_dir_all(tmp.path().join(".secret")).expect("mkdir hidden dir");
        let file = tmp.path().join(".secret").join("config.toml");
        std::fs::write(&file, "[tool]\n").expect("write hidden config");

        let note = explain_ignore_for_path(tmp.path(), &file).expect("hidden parent note");
        assert!(
            note.contains("skipped by default hidden-file filter for `.secret`"),
            "hidden ancestor should be named, got: {note}"
        );
    }

    #[test]
    fn test_should_ignore_with_ignore_paths() {
        let opts = Options {
            ignore_paths: vec![PathBuf::from("/ignored/path")],
            ..default_opts()
        };

        assert!(should_ignore(
            Path::new("/ignored/path/file.rs"),
            &opts,
            None
        ));
        assert!(!should_ignore(
            Path::new("/other/path/file.rs"),
            &opts,
            None
        ));
    }

    #[test]
    fn test_load_loctreeignore_nonexistent() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let patterns = load_loctreeignore(tmp.path());
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_load_loctreeignore_with_patterns() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let ignore_file = tmp.path().join(".loctreeignore");
        std::fs::write(
            &ignore_file,
            "# Comment\nnode_modules\n\n*.log\n# Another comment\nbuild/\n",
        )
        .expect("write loctreeignore");

        let patterns = load_loctreeignore(tmp.path());
        assert_eq!(patterns.len(), 3);
        assert!(patterns.contains(&"node_modules".to_string()));
        assert!(patterns.contains(&"*.log".to_string()));
        assert!(patterns.contains(&"build/".to_string()));
    }

    #[test]
    fn test_load_loctignore_directives() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let ignore_file = tmp.path().join(".loctignore");
        std::fs::write(
            &ignore_file,
            "# Comment\nfixtures/\n@loctignore:dead-ok src/generated/**\n",
        )
        .expect("write loctignore");

        let patterns = load_loctreeignore(tmp.path());
        assert_eq!(patterns, vec!["fixtures/".to_string()]);

        let dead_ok = load_loctignore_dead_ok_globs(tmp.path());
        assert_eq!(dead_ok, vec!["src/generated/**".to_string()]);
    }

    #[test]
    fn test_explain_loctignore_match_reports_source_line_and_pattern() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("fixtures")).expect("mkdir fixtures");
        let ignored = tmp.path().join("fixtures/local.rs");
        std::fs::write(tmp.path().join(".loctignore"), "# Comment\nfixtures/\n")
            .expect("write loctignore");
        std::fs::write(&ignored, "pub fn fixture_only() {}\n").expect("write ignored");

        let note = explain_ignore_for_path(tmp.path(), &ignored).expect("ignore explanation");
        assert_eq!(note, "ignored by .loctignore:2 pattern `fixtures/`");
    }

    #[test]
    fn test_should_ignore_with_ignore_globs() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let patterns = vec!["**/*.log".to_string()];
        let matchers = build_ignore_matchers(&patterns, tmp.path());
        let opts = Options {
            ignore_paths: matchers.ignore_paths,
            ignore_globs: matchers.ignore_globs,
            ..default_opts()
        };

        assert!(should_ignore(&tmp.path().join("app.log"), &opts, None));
        assert!(!should_ignore(&tmp.path().join("app.txt"), &opts, None));
    }

    #[test]
    fn test_normalise_ignore_patterns_relative() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let patterns = vec!["src".to_string(), "lib".to_string()];

        let normalized = normalise_ignore_patterns(&patterns, tmp.path());
        assert_eq!(normalized.len(), 2);
        // Normalized paths should be based on root
        assert!(normalized[0].ends_with("src") || normalized[0].to_string_lossy().contains("src"));
    }

    #[test]
    fn test_sort_dir_entries() {
        let tmp = tempfile::tempdir().expect("tmp dir");

        // Create some files and directories
        std::fs::create_dir(tmp.path().join("z_dir")).expect("mkdir");
        std::fs::create_dir(tmp.path().join("a_dir")).expect("mkdir");
        std::fs::write(tmp.path().join("z_file.txt"), "").expect("write");
        std::fs::write(tmp.path().join("a_file.txt"), "").expect("write");

        let mut entries: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .collect();

        sort_dir_entries(&mut entries);

        // After sorting: directories first (a_dir, z_dir), then files (a_file, z_file)
        let names: Vec<_> = entries
            .iter()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        // First two should be directories
        assert!(entries[0].path().is_dir());
        assert!(entries[1].path().is_dir());
        // Directories alphabetically
        assert_eq!(names[0], "a_dir");
        assert_eq!(names[1], "z_dir");
        // Files alphabetically
        assert_eq!(names[2], "a_file.txt");
        assert_eq!(names[3], "z_file.txt");
    }

    #[test]
    fn loctignore_exclusion_hint_distinguishes_ignored_from_wrong_path() {
        // loctree-feedback.md (2026-06-25, example-app): focus(docs)/slice fell back with
        // "No files found. Check the path." when docs/ EXISTS on disk but is
        // excluded by .loctignore. The hint must name .loctignore so the agent
        // does not chase a wrong-path that is actually correct.
        let tmp = tempfile::tempdir().expect("tmp dir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/operations")).expect("mkdir docs");
        std::fs::write(root.join("docs/operations/lexicon.md"), "# x").expect("write doc");
        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        std::fs::write(root.join("src/lib.rs"), "pub fn f() {}").expect("write src");
        std::fs::write(root.join(".loctignore"), "docs/\n").expect("write loctignore");

        // On-disk but ignored → precise hint naming .loctignore.
        let hint = loctignore_exclusion_hint(root, "docs").expect("docs is on-disk but ignored");
        assert!(
            hint.contains(".loctignore"),
            "hint must name .loctignore: {hint}"
        );
        assert!(hint.contains("docs"), "hint must name the target: {hint}");
        // A sub-path under the ignored dir is flagged too.
        assert!(loctignore_exclusion_hint(root, "docs/operations").is_some());

        // Genuinely absent path → None (a real wrong path; keep "check it").
        assert!(loctignore_exclusion_hint(root, "nonexistent").is_none());
        // Existing, non-ignored dir → None.
        assert!(loctignore_exclusion_hint(root, "src").is_none());
    }

    #[test]
    fn gather_files_stops_at_nested_git_repos() {
        // Field incident 2026-08-14: untracked build-local/ carrying
        // SourcePackages/checkouts/* (full dependency repos) blew a scan up to
        // 2.9 GB. A dir with its own .git is another project — prune it.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");

        let dep = root.join("build-local/SourcePackages/checkouts/dep");
        std::fs::create_dir_all(dep.join("src")).expect("dep src");
        // .git as a dir (normal checkout)…
        std::fs::create_dir_all(dep.join(".git")).expect("dep .git");
        std::fs::write(dep.join("src/lib.rs"), "pub fn dep() {}\n").expect("dep lib");
        // …and .git as a file (submodule/worktree form).
        let wt = root.join("build-local/worktree-form");
        std::fs::create_dir_all(&wt).expect("wt");
        std::fs::write(wt.join(".git"), "gitdir: /elsewhere\n").expect("wt .git file");
        std::fs::write(wt.join("inner.rs"), "pub fn wt() {}\n").expect("wt file");

        let options = Options {
            use_gitignore: false,
            ..Options::default()
        };
        let mut visited = HashSet::new();
        let mut files = Vec::new();
        gather_files(root, &options, 0, None, &mut visited, &mut files).expect("gather");

        let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert!(
            names.iter().any(|p| p.ends_with("src/main.rs")),
            "root project content must be scanned: {names:?}"
        );
        assert!(
            !names.iter().any(|p| p.contains("checkouts")),
            "nested git checkout must be pruned: {names:?}"
        );
        assert!(
            !names.iter().any(|p| p.contains("worktree-form")),
            ".git-file (worktree/submodule) form must be pruned too: {names:?}"
        );

        // --scan-all keeps the escape hatch: nested repos are walked again.
        let options_all = Options {
            use_gitignore: false,
            scan_all: true,
            ..Options::default()
        };
        let mut visited = HashSet::new();
        let mut files = Vec::new();
        gather_files(root, &options_all, 0, None, &mut visited, &mut files).expect("gather all");
        assert!(
            files
                .iter()
                .any(|p| p.display().to_string().contains("checkouts")),
            "--scan-all must still reach nested repos"
        );
    }
}
