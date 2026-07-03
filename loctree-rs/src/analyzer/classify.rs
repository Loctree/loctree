use serde::{Deserialize, Serialize};
use std::path::Path;

/// Artifact fence — one shared classification of "this is not product code",
/// consumed by every detector (coverage, cycles, diff, findings, twins, dead).
///
/// Default-on: detectors exclude non-`Product` files from their primary
/// (actionable) sections and report the cut via [`ArtifactFenceStats`] so the
/// fence never silently drops anything. Opt-out is `--include-artifacts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactClass {
    /// Real product code — full signal.
    Product,
    /// Third-party code carried in-tree: vendor/, node_modules/, *.min.js,
    /// files with a minification signature (single >5k-char line).
    Vendored,
    /// Test fixtures: tests/fixtures/**, __fixtures__/, testdata/ — inputs
    /// for tests, not product surface.
    Fixture,
    /// Build products and machine-written files: dist/, public_dist/,
    /// lockfiles, source maps, codegen output.
    Generated,
    /// Documentation-by-example files: *.example (.env.example), *.sample,
    /// *.template — shapes, never live sources.
    Template,
}

impl ArtifactClass {
    /// Anything that is not product code.
    pub fn is_artifact(self) -> bool {
        self != ArtifactClass::Product
    }
}

/// Per-class counters for what an artifact fence cut from a report surface.
///
/// The unit is surface-specific (files, findings, bridges, cycles, exports)
/// but the summary line shape is shared: `excluded: vendored(3), generated(40)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactFenceStats {
    #[serde(default)]
    pub vendored: usize,
    #[serde(default)]
    pub fixtures: usize,
    #[serde(default)]
    pub generated: usize,
    #[serde(default)]
    pub templates: usize,
}

impl ArtifactFenceStats {
    /// Record one excluded item of the given class. `Product` is a no-op.
    pub fn record(&mut self, class: ArtifactClass) {
        match class {
            ArtifactClass::Product => {}
            ArtifactClass::Vendored => self.vendored += 1,
            ArtifactClass::Fixture => self.fixtures += 1,
            ArtifactClass::Generated => self.generated += 1,
            ArtifactClass::Template => self.templates += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.vendored + self.fixtures + self.generated + self.templates
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Shared summary-line shape: `excluded: vendored(3), fixtures(12), generated(40)`.
    /// Empty string when nothing was cut (callers skip the line then).
    pub fn summary_line(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        if self.vendored > 0 {
            parts.push(format!("vendored({})", self.vendored));
        }
        if self.fixtures > 0 {
            parts.push(format!("fixtures({})", self.fixtures));
        }
        if self.generated > 0 {
            parts.push(format!("generated({})", self.generated));
        }
        if self.templates > 0 {
            parts.push(format!("templates({})", self.templates));
        }
        format!("excluded: {}", parts.join(", "))
    }
}

/// A minified file packs everything on one enormous line.
/// Threshold: any of the first lines longer than 5k chars.
const MINIFIED_LINE_LEN: usize = 5000;

fn content_head_looks_minified(content_head: &str) -> bool {
    content_head
        .lines()
        .take(10)
        .any(|line| line.len() > MINIFIED_LINE_LEN)
}

fn path_segment(lower: &str, segment: &str) -> bool {
    lower.starts_with(&format!("{}/", segment)) || lower.contains(&format!("/{}/", segment))
}

/// Classify a file against the artifact fence.
///
/// `content_head` (first bytes of the file, when available) enables the
/// minification signature check; pass `None` for path-only classification.
/// Precedence: Fixture > Vendored > Generated > Template > content signature.
pub fn artifact_class(path: &str, content_head: Option<&str>) -> ArtifactClass {
    let lower = path
        .trim_start_matches("./")
        .replace('\\', "/")
        .to_ascii_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(lower.as_str());

    // FIXTURE — test inputs (tests/fixtures/**, tools/fixtures/**, testdata/)
    if path_segment(&lower, "fixtures")
        || path_segment(&lower, "fixture")
        || lower.contains("__fixtures__")
        || path_segment(&lower, "testdata")
    {
        return ArtifactClass::Fixture;
    }

    // VENDORED — third-party code carried in-tree
    if path_segment(&lower, "vendor")
        || path_segment(&lower, "vendored")
        || path_segment(&lower, "node_modules")
        || path_segment(&lower, "third_party")
        || path_segment(&lower, "thirdparty")
        || filename.ends_with(".min.js")
        || filename.ends_with(".min.mjs")
        || filename.ends_with(".min.cjs")
        || filename.ends_with(".min.css")
        || filename.ends_with(".bundle.js")
    {
        return ArtifactClass::Vendored;
    }

    // GENERATED — build products, lockfiles, source maps, codegen
    if path_segment(&lower, "dist")
        || path_segment(&lower, "public_dist")
        || path_segment(&lower, "__generated__")
        || filename.ends_with(".lock")
        || filename.contains("-lock.")
        || filename.ends_with(".map")
        || is_generated_path(&lower)
    {
        return ArtifactClass::Generated;
    }

    // TEMPLATE — example/sample/template shapes (.env.example & friends)
    if filename.contains(".example")
        || filename.contains(".sample")
        || filename.contains(".template")
        || filename.starts_with("example.")
        || filename.starts_with("sample.")
        || filename.starts_with("template.")
    {
        return ArtifactClass::Template;
    }

    // Content signature: minified single-line bundles without a .min marker
    if let Some(head) = content_head
        && content_head_looks_minified(head)
    {
        return ArtifactClass::Vendored;
    }

    ArtifactClass::Product
}

/// Canonical test-file predicate (single definition; both
/// `analyzer::is_test_file` and `cli::dispatch::is_test_file` delegate here).
pub fn is_test_file(path: &str) -> bool {
    let p = path.replace('\\', "/").to_lowercase();
    let filename = p.rsplit('/').next().unwrap_or(p.as_str());

    // Directory patterns: tests/, __tests__/, test/, spec/, fixtures/, mocks/
    if p.contains("/tests/")
        || p.starts_with("tests/")
        || p.contains("/test/")
        || p.starts_with("test/")
        || p.contains("__tests__")
        || p.contains("/spec/")
        || p.starts_with("spec/")
        || p.contains("/fixtures/")
        || p.starts_with("fixtures/")
        || p.contains("/mocks/")
        || p.contains("__mocks__")
        || p.contains("test-utils")
        || p.contains("/testing/")
    {
        return true;
    }

    // File patterns: *_test.*, *.test.*, *_spec.*, *.spec.*, test_*, tests.*
    filename.contains("_test.")
        || filename.contains(".test.")
        || filename.contains("_spec.")
        || filename.contains(".spec.")
        || filename.contains("_tests.")
        || filename.starts_with("test_")
        || filename.starts_with("spec_")
        || filename.starts_with("tests.")
        || filename == "conftest.py"
        || p.ends_with("setup.ts")
        || p.ends_with("setup.tsx")
}

/// Classification of a file's test status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestClassification {
    /// Production code (not a test)
    Production,
    /// Unit test file
    UnitTest,
    /// Integration test file
    IntegrationTest,
    /// Test fixture/mock data
    TestFixture,
    /// Test utility/helper
    TestHelper,
}

pub fn is_dev_file(path: &str) -> bool {
    path.contains("__tests__")
        || path.contains("stories")
        || path.contains(".stories.")
        || path.contains("story.")
        || path.contains("fixture")
        || path.contains("fixtures")
}

pub fn detect_language(ext: &str) -> String {
    match ext {
        "ts" | "tsx" => "ts".to_string(),
        "js" | "jsx" | "mjs" | "cjs" => "js".to_string(),
        "astro" => "astro".to_string(),
        "rs" => "rs".to_string(),
        "py" => "py".to_string(),
        "rb" => "ruby".to_string(),
        "dart" => "dart".to_string(),
        "kt" | "kts" => "kotlin".to_string(),
        "css" => "css".to_string(),
        "sh" | "bash" | "zsh" | "fish" => "shell".to_string(),
        "mk" | "make" => "make".to_string(),
        "zig" | "zon" => "zig".to_string(),
        "swift" => "swift".to_string(),
        "m" | "mm" => "objc".to_string(),
        "c" | "h" => "c".to_string(),
        "cc" | "cpp" | "cxx" | "hpp" => "cpp".to_string(),
        other => other.to_string(),
    }
}

fn path_extension(lower: &str) -> &str {
    lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
}

/// Classify non-code resources that should remain first-class inspectable files.
///
/// This is intentionally path/extension membership, not semantic parsing.
pub fn resource_kind(path: &str) -> Option<&'static str> {
    let lower = path
        .trim_start_matches("./")
        .replace('\\', "/")
        .to_ascii_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(lower.as_str());
    let ext = path_extension(&lower);

    if lower.starts_with(".github/workflows/") && matches!(ext, "yml" | "yaml") {
        return Some("workflow");
    }

    if lower.contains("/locales/")
        || lower.starts_with("locales/")
        || lower.contains("/locale/")
        || lower.starts_with("locale/")
        || lower.contains("/i18n/")
        || lower.starts_with("i18n/")
        || lower.contains("/lang/")
        || lower.starts_with("lang/")
    {
        return Some("locale");
    }

    if matches!(ext, "md" | "markdown" | "mdx" | "rst") {
        return Some("doc");
    }

    let config_filename = matches!(
        filename,
        ".loctignore"
            | ".loctreeignore"
            | "cargo.toml"
            | "package.json"
            | "pyproject.toml"
            | "tsconfig.json"
            | "deno.json"
            | "deno.jsonc"
            | "pnpm-workspace.yaml"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | "config.toml"
            | "suppressions.toml"
    );
    if config_filename
        || lower.contains("/config/")
        || lower.starts_with("config/")
        || lower.contains("/.loctree/")
        || filename.ends_with(".config.js")
        || filename.ends_with(".config.ts")
        || filename.ends_with(".config.json")
        || filename.ends_with(".config.toml")
        || filename.ends_with(".config.yaml")
        || filename.ends_with(".config.yml")
    {
        return Some("config");
    }

    if matches!(
        ext,
        "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "storyboard"
            | "xib"
            | "properties"
            | "xml"
            | "svg"
    ) {
        return Some("resource");
    }

    if ext == "txt" {
        return Some("doc");
    }

    None
}

/// Classify an extensionless filename to a language (e.g. `Makefile` → `make`).
/// Returns empty string when unknown.
pub fn detect_language_from_filename(filename: &str) -> String {
    match filename {
        "Makefile" | "makefile" | "GNUmakefile" | "BSDmakefile" => "make".to_string(),
        ".loctignore" | ".loctreeignore" => "config".to_string(),
        _ => String::new(),
    }
}

/// Languages that have **semantic** analyzers in loctree — i.e. produce
/// imports/exports/symbols/dispatch edges rather than just LOC counts.
///
/// Used by the for-ai health gate to detect "blind scan" repos: when an
/// analysis pass contains zero files in any of these languages, the
/// health score is structurally meaningless (we couldn't see the code).
/// Examples: Objective-C, Kotlin, Java, C++, Scala, Erlang — all
/// languages without a loctree analyzer today.
///
/// Keep this list in sync with the `match ext.as_str()` dispatch in
/// `analyzer/scan.rs::analyze_file` and the `analyze_*_file` helpers.
///
/// **Why this list, not [`detect_language`]'s superset:** `css` / `html` /
/// `json` / `yaml` / `toml` / `md` get parsed into [`FileAnalysis`]
/// entries but yield no code-graph signal — a repo whose only analyzed
/// files are README + CSS is structurally invisible to loctree even
/// though `files_analyzed > 0`. Only languages that produce real
/// import/export edges count here.
pub fn is_semantic_code_language(lang: &str) -> bool {
    matches!(
        lang,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "go"
            | "dart"
            | "swift"
            | "m"
            | "mm"
            | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "zig"
            | "shell"
            | "make"
            | "astro"
            | "vue"
            | "svelte"
    )
}

pub fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("__tests__")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.dart")
        || lower.starts_with("test_")
        || lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test_")
}

/// Check if a path should be excluded from production analysis reports.
/// This includes test files, test fixtures, mocks, and test-related paths.
///
/// # Examples
/// ```
/// use loctree::analyzer::classify::should_exclude_from_reports;
/// assert!(should_exclude_from_reports("tests/fixtures/foo.rs"));
/// assert!(should_exclude_from_reports("src/__tests__/bar.spec.ts"));
/// assert!(should_exclude_from_reports("src/__mocks__/api.ts"));
/// assert!(!should_exclude_from_reports("src/api/handler.rs"));
/// ```
pub fn should_exclude_from_reports(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();

    // Test directories
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("__tests__")
        || lower.contains("__mocks__")

    // Fixture/mock directories
        || lower.contains("/fixtures/")
        || lower.contains("/fixture/")
        || lower.contains("__fixtures__")
        || lower.contains("/mocks/")
        || lower.contains("/mock/")

    // Test file patterns
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.ts")
        || lower.ends_with("_test.tsx")
        || lower.ends_with("_test.js")
        || lower.ends_with("_test.jsx")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.dart")
        || lower.ends_with("_test.py")

    // Test helpers/utilities
        || lower.contains("/test_utils/")
        || lower.contains("/test_helpers/")
        || lower.contains("/testing/")
}

fn is_story_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("stories") || lower.contains(".story.") || lower.contains(".stories.")
}

fn is_generated_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("generated")
        || lower.contains("codegen")
        || lower.contains("/gen/")
        || lower.ends_with(".gen.ts")
        || lower.ends_with(".gen.tsx")
        || lower.ends_with(".gen.rs")
        || lower.ends_with(".g.rs")
        || lower.ends_with(".g.dart")
        || lower.ends_with(".freezed.dart")
        || lower.ends_with(".gr.dart")
        || lower.ends_with(".pb.dart")
        || lower.ends_with(".pbjson.dart")
        || lower.ends_with(".pbenum.dart")
        || lower.ends_with(".pbserver.dart")
        || lower.ends_with(".config.dart")
}

/// Detect if a path is a test file and classify it
pub fn classify_test_path(path: &Path) -> TestClassification {
    let path_str = path.to_str().unwrap_or("");
    let lower = path_str.to_ascii_lowercase();
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let filename_lower = filename.to_ascii_lowercase();

    // Fixture/mock detection (not including "test_" prefix files)
    if (lower.contains("fixture") || lower.contains("fixtures") || lower.contains("mock"))
        && !filename_lower.starts_with("test_")
    {
        return TestClassification::TestFixture;
    }

    // Test helper/utility detection - BEFORE integration/unit test detection
    // Only specific helper patterns (not Python test_ files)
    let is_python = filename_lower.ends_with(".py");
    let is_python_test_file = is_python && filename_lower.starts_with("test_");

    if !is_python_test_file
        && (filename_lower.contains("test_helper")
            || filename_lower.contains("test_utils")
            || filename_lower == "setup.py"
            || lower.contains("testing/"))
    {
        return TestClassification::TestHelper;
    }

    // Integration test detection (in tests/ directory but not __tests__)
    // Check if path starts with "tests/" or contains "/tests/"
    if (lower.starts_with("tests/") || lower.contains("/tests/")) && !lower.contains("__tests__") {
        return TestClassification::IntegrationTest;
    }

    // Unit test detection
    if lower.contains("__tests__")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.dart")
        || filename_lower.starts_with("test_") // Python test_*.py files
        || lower.contains("/test_")
    {
        return TestClassification::UnitTest;
    }

    TestClassification::Production
}

/// Check if file content contains test code (for Rust #[cfg(test)] detection)
pub fn has_test_code(content: &str, lang: &str) -> bool {
    match lang {
        "rs" | "rust" => content.contains("#[cfg(test)]") || content.contains("#[test]"),
        "ts" | "js" | "tsx" | "jsx" => {
            content.contains("describe(") || content.contains("it(") || content.contains("test(")
        }
        "py" | "python" => {
            content.contains("def test_")
                || content.contains("import unittest")
                || content.contains("import pytest")
        }
        "go" => content.contains("func Test") || content.contains("testing.T"),
        _ => false,
    }
}

/// Get all test-related file patterns for a language
pub fn test_patterns(lang: &str) -> Vec<&'static str> {
    match lang {
        "rs" | "rust" => vec!["*_test.rs", "*_tests.rs", "tests/**/*.rs"],
        "ts" | "tsx" | "js" | "jsx" => vec![
            "*.test.ts",
            "*.test.tsx",
            "*.test.js",
            "*.test.jsx",
            "*.spec.ts",
            "*.spec.tsx",
            "*.spec.js",
            "*.spec.jsx",
            "__tests__/**/*",
        ],
        "py" | "python" => vec!["test_*.py", "*_test.py", "tests/**/*.py"],
        "go" => vec!["*_test.go"],
        "dart" => vec!["*_test.dart", "test/**/*.dart"],
        _ => vec![],
    }
}

pub fn file_kind(path: &str) -> (String, bool, bool) {
    let generated = is_generated_path(path);
    let test = is_test_path(path);
    let story = is_story_path(path);
    let lower = path.to_ascii_lowercase();
    let config = lower.contains("config/")
        || lower.contains("/config/")
        || lower == ".loctignore"
        || lower == ".loctreeignore"
        || lower.ends_with("config.ts")
        || lower.ends_with("config.tsx")
        || lower.ends_with("config.js")
        || lower.ends_with("config.rs")
        || lower.ends_with(".config.ts")
        || lower.ends_with(".config.js")
        || lower.ends_with(".config.json");

    let kind = if generated {
        "generated"
    } else if test {
        "test"
    } else if story {
        "story"
    } else if let Some(resource) = resource_kind(path) {
        resource
    } else if config {
        "config"
    } else {
        "code"
    };

    (kind.to_string(), test, generated)
}

pub fn language_from_path(path: &str) -> String {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if !ext.is_empty() {
        return detect_language(&ext);
    }
    // Fall back to filename-based detection (Makefile, GNUmakefile, ...)
    let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    detect_language_from_filename(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dev_and_non_dev_files() {
        assert!(is_dev_file("features/__tests__/thing.ts"));
        assert!(is_dev_file("components/Button.stories.tsx"));
        assert!(is_dev_file("fixtures/foo.rs"));
        assert!(!is_dev_file("src/app.tsx"));
    }

    #[test]
    fn classifies_file_kinds_and_flags() {
        let (kind, test, is_generated) = file_kind("src/generated/foo.gen.ts");
        assert_eq!(kind, "generated");
        assert!(!test);
        assert!(is_generated);

        let (kind, test, is_generated) = file_kind("src/components/Button.story.tsx");
        assert_eq!(kind, "story");
        assert!(!is_generated);
        assert!(!test);

        let (kind, test, _) = file_kind("src/__tests__/foo.test.ts");
        assert_eq!(kind, "test");
        assert!(test);

        let (kind, _, _) = file_kind("config/vite.config.ts");
        assert_eq!(kind, "config");

        let (kind, _, _) = file_kind("docs/README.md");
        assert_eq!(kind, "doc");

        let (kind, _, _) = file_kind(".github/workflows/ci.yml");
        assert_eq!(kind, "workflow");

        let (kind, _, _) = file_kind("locales/en.json");
        assert_eq!(kind, "locale");

        let (kind, _, _) = file_kind("src/features/app.tsx");
        assert_eq!(kind, "code");
    }

    #[test]
    fn classifies_resource_membership() {
        assert_eq!(resource_kind("docs/guide.md"), Some("doc"));
        assert_eq!(resource_kind("Cargo.toml"), Some("config"));
        assert_eq!(resource_kind(".github/workflows/ci.yaml"), Some("workflow"));
        assert_eq!(resource_kind("src/i18n/pl.json"), Some("locale"));
        assert_eq!(resource_kind("fixtures/data.json"), Some("resource"));
        assert_eq!(
            resource_kind("legacy/MarkdownEditor/Base.lproj/Main.storyboard"),
            Some("resource")
        );
        assert_eq!(resource_kind("Views/MainWindow.xib"), Some("resource"));
        // W2-02 scorecard: JVM bundles, XML descriptors, and plain text are
        // literal truth rg sees — they must classify as resources/docs so the
        // scan-only path admits them into the snapshot universe.
        assert_eq!(
            resource_kind("src/main/resources/messages/LoctreeBundle.properties"),
            Some("resource")
        );
        assert_eq!(
            resource_kind("src/main/resources/META-INF/loctree-lsp.xml"),
            Some("resource")
        );
        assert_eq!(resource_kind("notes/todo.txt"), Some("doc"));
        assert_eq!(resource_kind("src/main.rs"), None);
    }

    #[test]
    fn detects_language_from_path() {
        assert_eq!(language_from_path("foo/bar.tsx"), "ts");
        assert_eq!(language_from_path("foo/bar.rs"), "rs");
        assert_eq!(language_from_path("foo/bar.py"), "py");
        assert_eq!(language_from_path("foo/bar.css"), "css");
        assert_eq!(language_from_path("foo/bar.unknown"), "unknown");
    }

    #[test]
    fn detect_language_all_extensions() {
        assert_eq!(detect_language("ts"), "ts");
        assert_eq!(detect_language("tsx"), "ts");
        assert_eq!(detect_language("js"), "js");
        assert_eq!(detect_language("jsx"), "js");
        assert_eq!(detect_language("mjs"), "js");
        assert_eq!(detect_language("cjs"), "js");
        assert_eq!(detect_language("rs"), "rs");
        assert_eq!(detect_language("py"), "py");
        assert_eq!(detect_language("go"), "go");
        assert_eq!(detect_language("kt"), "kotlin");
        assert_eq!(detect_language("kts"), "kotlin");
        assert_eq!(detect_language("css"), "css");
        assert_eq!(detect_language("html"), "html");
        assert_eq!(detect_language("astro"), "astro");
        // New languages (shell/make/zig)
        assert_eq!(detect_language("sh"), "shell");
        assert_eq!(detect_language("bash"), "shell");
        assert_eq!(detect_language("zsh"), "shell");
        assert_eq!(detect_language("fish"), "shell");
        assert_eq!(detect_language("mk"), "make");
        assert_eq!(detect_language("zig"), "zig");
        assert_eq!(detect_language("zon"), "zig");
        assert_eq!(detect_language("storyboard"), "storyboard");
        assert_eq!(detect_language("xib"), "xib");
    }

    #[test]
    fn detects_language_from_filename_for_makefiles() {
        assert_eq!(detect_language_from_filename("Makefile"), "make");
        assert_eq!(detect_language_from_filename("makefile"), "make");
        assert_eq!(detect_language_from_filename("GNUmakefile"), "make");
        assert_eq!(detect_language_from_filename("BSDmakefile"), "make");
        assert_eq!(detect_language_from_filename(".loctignore"), "config");
        assert_eq!(detect_language_from_filename(".loctreeignore"), "config");
        assert_eq!(detect_language_from_filename("random"), "");
        assert_eq!(detect_language_from_filename("Dockerfile"), "");
    }

    #[test]
    fn is_dev_file_variations() {
        // __tests__ variations
        assert!(is_dev_file("src/__tests__/Button.test.ts"));
        assert!(is_dev_file("__tests__/unit/helper.ts"));

        // stories variations
        assert!(is_dev_file("components/stories/Button.tsx"));
        assert!(is_dev_file("Button.stories.tsx"));
        assert!(is_dev_file("Button.story.tsx"));

        // fixtures
        assert!(is_dev_file("test/fixtures/data.json"));
        assert!(is_dev_file("fixture/mock.ts"));

        // regular files should not match
        assert!(!is_dev_file("src/components/Button.tsx"));
        assert!(!is_dev_file("lib/utils.ts"));
        assert!(!is_dev_file("src/store/index.ts"));
    }

    #[test]
    fn is_test_path_variations() {
        assert!(is_test_path("src/__tests__/foo.ts"));
        assert!(is_test_path("src/Button.test.tsx"));
        assert!(is_test_path("utils.spec.ts"));
        assert!(is_test_path("lib_test.rs"));
        assert!(is_test_path("module_tests.rs"));
        assert!(is_test_path("SRC/__TESTS__/FOO.TS")); // case insensitive
        // New patterns
        assert!(is_test_path("test_parser.py"));
        assert!(is_test_path("src/test_utils.py"));
        assert!(is_test_path("tests/api/test.rs"));
        assert!(is_test_path("src/tests/integration.py"));

        assert!(!is_test_path("src/Button.tsx"));
        assert!(!is_test_path("testing.ts")); // 'testing' not a test marker
    }

    #[test]
    fn is_story_path_variations() {
        assert!(is_story_path("Button.stories.tsx"));
        assert!(is_story_path("Button.story.tsx"));
        assert!(is_story_path("components/stories/Button.tsx"));
        assert!(is_story_path("BUTTON.STORIES.TSX")); // case insensitive

        assert!(!is_story_path("src/Button.tsx"));
        assert!(!is_story_path("history.ts")); // 'history' doesn't match
    }

    #[test]
    fn is_generated_path_variations() {
        assert!(is_generated_path("src/generated/types.ts"));
        assert!(is_generated_path("lib/codegen/schema.ts"));
        assert!(is_generated_path("out/gen/api.ts"));
        assert!(is_generated_path("types.gen.ts"));
        assert!(is_generated_path("api.gen.tsx"));
        assert!(is_generated_path("schema.gen.rs"));
        assert!(is_generated_path("proto.g.rs"));
        assert!(is_generated_path("SRC/GENERATED/FOO.TS")); // case insensitive

        assert!(!is_generated_path("src/utils.ts"));
        assert!(!is_generated_path("generic.ts")); // 'generic' != 'generated'
    }

    #[test]
    fn file_kind_config_variations() {
        // Directory-based config (must have /config/ in middle)
        let (kind, _, _) = file_kind("src/config/app.ts");
        assert_eq!(kind, "config");

        // File-suffix based config (must end with "config.ts" or ".config.ts" etc)
        let (kind, _, _) = file_kind("vite.config.ts");
        assert_eq!(kind, "config");

        let (kind, _, _) = file_kind("tailwind.config.js");
        assert_eq!(kind, "config");

        // Note: tsconfig.json doesn't match pattern - would need config.json or .config.json
        let (kind, _, _) = file_kind("app.config.json");
        assert_eq!(kind, "config");
    }

    #[test]
    fn file_kind_priority_generated_over_test() {
        // Generated takes priority over test for kind, but test flag is set independently
        let (kind, test, generated) = file_kind("__tests__/generated/mock.gen.ts");
        assert_eq!(kind, "generated");
        assert!(test); // test flag is true because path contains __tests__
        assert!(generated);
    }

    #[test]
    fn file_kind_priority_test_over_story() {
        // Test takes priority over story
        let (kind, test, _) = file_kind("Button.stories.test.ts");
        assert_eq!(kind, "test");
        assert!(test);
    }

    #[test]
    fn language_from_path_edge_cases() {
        // Makefile-family filenames now resolve via filename-based fallback
        assert_eq!(language_from_path("Makefile"), "make");
        assert_eq!(language_from_path("src/GNUmakefile"), "make");

        // Truly unknown extensionless filenames still return empty
        assert_eq!(language_from_path("src/noext"), "");

        // Hidden files without extension - returns empty (filename fallback misses)
        assert_eq!(language_from_path(".gitignore"), "");
        assert_eq!(language_from_path(".env"), "");

        // Double extensions (only last matters) - note tsx -> ts mapping
        assert_eq!(language_from_path("file.test.ts"), "ts");
        assert_eq!(language_from_path("app.module.tsx"), "ts"); // tsx mapped to ts

        // New language extensions
        assert_eq!(language_from_path("deploy.sh"), "shell");
        assert_eq!(language_from_path("common.mk"), "make");
        assert_eq!(language_from_path("main.zig"), "zig");
        assert_eq!(language_from_path("build.zon"), "zig");
        assert_eq!(language_from_path("Main.kt"), "kotlin");
        assert_eq!(language_from_path("build.gradle.kts"), "kotlin");
    }

    #[test]
    fn classify_test_path_unit_tests() {
        // TypeScript/JavaScript unit tests
        assert_eq!(
            classify_test_path(Path::new("src/components/Button.test.tsx")),
            TestClassification::UnitTest
        );
        assert_eq!(
            classify_test_path(Path::new("utils.spec.ts")),
            TestClassification::UnitTest
        );
        assert_eq!(
            classify_test_path(Path::new("src/__tests__/helper.ts")),
            TestClassification::UnitTest
        );

        // Rust unit tests
        assert_eq!(
            classify_test_path(Path::new("src/parser_test.rs")),
            TestClassification::UnitTest
        );
        assert_eq!(
            classify_test_path(Path::new("lib/module_tests.rs")),
            TestClassification::UnitTest
        );

        // Python unit tests
        assert_eq!(
            classify_test_path(Path::new("test_utils.py")),
            TestClassification::UnitTest
        );

        // Go unit tests
        assert_eq!(
            classify_test_path(Path::new("handler_test.go")),
            TestClassification::UnitTest
        );
    }

    #[test]
    fn classify_test_path_integration_tests() {
        // Rust integration tests (in tests/ directory)
        assert_eq!(
            classify_test_path(Path::new("tests/api/endpoints.rs")),
            TestClassification::IntegrationTest
        );
        assert_eq!(
            classify_test_path(Path::new("tests/integration/database.rs")),
            TestClassification::IntegrationTest
        );

        // __tests__ should still be unit tests even with /tests/ in path
        assert_eq!(
            classify_test_path(Path::new("src/__tests__/integration.test.ts")),
            TestClassification::UnitTest
        );
    }

    #[test]
    fn classify_test_path_fixtures() {
        assert_eq!(
            classify_test_path(Path::new("tests/fixtures/data.json")),
            TestClassification::TestFixture
        );
        assert_eq!(
            classify_test_path(Path::new("__tests__/fixture/mock.ts")),
            TestClassification::TestFixture
        );
        assert_eq!(
            classify_test_path(Path::new("test/mock/server.rs")),
            TestClassification::TestFixture
        );
    }

    #[test]
    fn classify_test_path_helpers() {
        assert_eq!(
            classify_test_path(Path::new("tests/test_helper.rs")),
            TestClassification::TestHelper
        );
        assert_eq!(
            classify_test_path(Path::new("__tests__/test_utils.ts")),
            TestClassification::TestHelper
        );
        assert_eq!(
            classify_test_path(Path::new("testing/setup.py")),
            TestClassification::TestHelper
        );
    }

    #[test]
    fn classify_test_path_production() {
        assert_eq!(
            classify_test_path(Path::new("src/components/Button.tsx")),
            TestClassification::Production
        );
        assert_eq!(
            classify_test_path(Path::new("lib/parser.rs")),
            TestClassification::Production
        );
        assert_eq!(
            classify_test_path(Path::new("utils/helpers.py")),
            TestClassification::Production
        );
    }

    #[test]
    fn has_test_code_rust() {
        // Rust with #[cfg(test)]
        let code_with_test_module = r#"
            fn main() {}

            #[cfg(test)]
            mod tests {
                #[test]
                fn it_works() {
                    assert_eq!(2 + 2, 4);
                }
            }
        "#;
        assert!(has_test_code(code_with_test_module, "rs"));

        // Rust with standalone #[test]
        let code_with_test_fn = r#"
            #[test]
            fn test_something() {}
        "#;
        assert!(has_test_code(code_with_test_fn, "rust"));

        // Rust production code
        let production_code = r#"
            fn parse(input: &str) -> Result<(), Error> {
                Ok(())
            }
        "#;
        assert!(!has_test_code(production_code, "rs"));
    }

    #[test]
    fn has_test_code_typescript() {
        // Jest/Vitest style tests
        let jest_test = r#"
            describe('Button', () => {
                it('should render', () => {
                    expect(true).toBe(true);
                });
            });
        "#;
        assert!(has_test_code(jest_test, "ts"));

        let vitest_test = r#"
            test('adds 1 + 2 to equal 3', () => {
                expect(1 + 2).toBe(3);
            });
        "#;
        assert!(has_test_code(vitest_test, "tsx"));

        // Production code
        let production = r#"
            export function add(a: number, b: number): number {
                return a + b;
            }
        "#;
        assert!(!has_test_code(production, "ts"));
    }

    #[test]
    fn has_test_code_python() {
        // unittest style
        let unittest_code = r#"
            import unittest

            class TestMath(unittest.TestCase):
                def test_addition(self):
                    self.assertEqual(1 + 1, 2)
        "#;
        assert!(has_test_code(unittest_code, "py"));

        // pytest style
        let pytest_code = r#"
            import pytest

            def test_addition():
                assert 1 + 1 == 2
        "#;
        assert!(has_test_code(pytest_code, "python"));

        // Function name pattern
        let test_function = r#"
            def test_something():
                pass
        "#;
        assert!(has_test_code(test_function, "py"));

        // Production code
        let production = r#"
            def add(a, b):
                return a + b
        "#;
        assert!(!has_test_code(production, "py"));
    }

    #[test]
    fn has_test_code_go() {
        // Go test
        let go_test = r#"
            package main

            import "testing"

            func TestAdd(t *testing.T) {
                result := Add(1, 2)
                if result != 3 {
                    t.Errorf("Expected 3, got %d", result)
                }
            }
        "#;
        assert!(has_test_code(go_test, "go"));

        // Production code
        let production = r#"
            package main

            func Add(a, b int) int {
                return a + b
            }
        "#;
        assert!(!has_test_code(production, "go"));
    }

    #[test]
    fn artifact_class_vendored_minified() {
        assert_eq!(
            artifact_class("loctree-rs/src/analyzer/assets/cytoscape.min.js", None),
            ArtifactClass::Vendored
        );
        assert_eq!(
            artifact_class("vendor/lodash/index.js", None),
            ArtifactClass::Vendored
        );
        assert_eq!(
            artifact_class("web/node_modules/react/index.js", None),
            ArtifactClass::Vendored
        );
        // Minification signature without .min marker: one >5k-char line
        let minified = "x".repeat(6000);
        assert_eq!(
            artifact_class("assets/lib.js", Some(&minified)),
            ArtifactClass::Vendored
        );
        // Normal content stays product
        assert_eq!(
            artifact_class("src/app.js", Some("import x from './y';\n")),
            ArtifactClass::Product
        );
    }

    #[test]
    fn artifact_class_generated_lockfiles_and_dist() {
        assert_eq!(
            artifact_class("public_dist/index.html", None),
            ArtifactClass::Generated
        );
        assert_eq!(
            artifact_class("dist/bundle.js", None),
            ArtifactClass::Generated
        );
        assert_eq!(
            artifact_class("package-lock.json", None),
            ArtifactClass::Generated
        );
        assert_eq!(artifact_class("Cargo.lock", None), ArtifactClass::Generated);
        assert_eq!(
            artifact_class("pnpm-lock.yaml", None),
            ArtifactClass::Generated
        );
        assert_eq!(
            artifact_class("public_dist/app-1234.js.map", None),
            ArtifactClass::Generated
        );
        assert_eq!(
            artifact_class("src/generated/types.gen.ts", None),
            ArtifactClass::Generated
        );
    }

    #[test]
    fn artifact_class_fixture_and_template() {
        assert_eq!(
            artifact_class("tests/fixtures/diamond/a.ts", None),
            ArtifactClass::Fixture
        );
        assert_eq!(
            artifact_class("tools/fixtures/sample.py", None),
            ArtifactClass::Fixture
        );
        assert_eq!(
            artifact_class(".env.example", None),
            ArtifactClass::Template
        );
        assert_eq!(
            artifact_class("config/settings.sample.toml", None),
            ArtifactClass::Template
        );
        assert_eq!(
            artifact_class("deploy/nginx.conf.template", None),
            ArtifactClass::Template
        );
        // Fixture wins over generated when nested
        assert_eq!(
            artifact_class("tests/fixtures/dist/bundle.js", None),
            ArtifactClass::Fixture
        );
    }

    #[test]
    fn artifact_class_product_untouched() {
        assert_eq!(artifact_class("src/main.rs", None), ArtifactClass::Product);
        assert_eq!(
            artifact_class("loctree-rs/src/analyzer/classify.rs", None),
            ArtifactClass::Product
        );
        assert_eq!(
            artifact_class("examples/demo/app.ts", None),
            ArtifactClass::Product,
            "library examples are NOT artifacts (library_mode handles them)"
        );
    }

    #[test]
    fn artifact_fence_stats_summary_line() {
        let mut stats = ArtifactFenceStats::default();
        assert!(stats.is_empty());
        assert_eq!(stats.summary_line(), "");
        stats.record(ArtifactClass::Vendored);
        stats.record(ArtifactClass::Vendored);
        stats.record(ArtifactClass::Generated);
        stats.record(ArtifactClass::Product); // no-op
        assert_eq!(stats.total(), 3);
        assert_eq!(stats.summary_line(), "excluded: vendored(2), generated(1)");
    }

    #[test]
    fn canonical_is_test_file_union() {
        // analyzer/mod.rs legacy patterns
        assert!(is_test_file("src/Button.test.tsx"));
        assert!(is_test_file("src/__tests__/foo.ts"));
        assert!(is_test_file("vitest.setup.ts"));
        assert!(is_test_file("src/test-utils/render.tsx"));
        // cli/dispatch legacy patterns
        assert!(is_test_file("tests/e2e_cli.rs"));
        assert!(is_test_file("conftest.py"));
        assert!(is_test_file("src/fixtures/data.ts"));
        assert!(is_test_file("module_tests.rs"));
        assert!(is_test_file("test_parser.py"));
        // production stays production
        assert!(!is_test_file("src/main.rs"));
        assert!(!is_test_file("src/components/Button.tsx"));
        assert!(!is_test_file("attestation.rs"));
    }

    #[test]
    fn test_patterns_all_languages() {
        // Rust patterns
        let rust_patterns = test_patterns("rs");
        assert!(rust_patterns.contains(&"*_test.rs"));
        assert!(rust_patterns.contains(&"*_tests.rs"));
        assert!(rust_patterns.contains(&"tests/**/*.rs"));

        // TypeScript patterns
        let ts_patterns = test_patterns("ts");
        assert!(ts_patterns.contains(&"*.test.ts"));
        assert!(ts_patterns.contains(&"*.spec.tsx"));
        assert!(ts_patterns.contains(&"__tests__/**/*"));

        // Python patterns
        let py_patterns = test_patterns("py");
        assert!(py_patterns.contains(&"test_*.py"));
        assert!(py_patterns.contains(&"*_test.py"));
        assert!(py_patterns.contains(&"tests/**/*.py"));

        // Go patterns
        let go_patterns = test_patterns("go");
        assert!(go_patterns.contains(&"*_test.go"));

        // Unknown language
        let unknown_patterns = test_patterns("unknown");
        assert!(unknown_patterns.is_empty());
    }
}
