//! Runtime API detection for Node.js and other environments.
//!
//! Handles exports that are never imported statically because they're invoked
//! by the runtime environment (Node.js, Deno, Bun, etc.).
//!
//! Examples:
//! - Node.js ES Module loader hooks: resolve(), load()
//! - Node.js test runner hooks: beforeEach(), afterEach()
//! - Web Workers: onmessage, postMessage
//! - Service Workers: install, activate, fetch
//!
//! Without this detection, these exports would be flagged as dead code.

use serde::Deserialize;

/// Pattern for matching runtime API exports
#[derive(Debug, Clone)]
pub struct RuntimeApiPattern {
    /// Glob pattern for file paths (e.g., "**/loader.js")
    pub file_pattern: String,
    /// Export names that are runtime-invoked
    pub export_names: Vec<String>,
    /// Description for debugging/documentation
    pub description: String,
}

/// User-configurable runtime API definition (from .loctree.toml).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CustomRuntimeApi {
    /// Framework/runtime identifier
    pub framework: String,
    /// Export names that are runtime-invoked
    pub exports: Vec<String>,
    /// File path patterns (glob patterns)
    pub file_patterns: Vec<String>,
}

/// Known runtime APIs that should not be flagged as dead code
pub struct RuntimeApiRegistry {
    patterns: Vec<RuntimeApiPattern>,
}

impl RuntimeApiRegistry {
    /// Create registry with default known runtime APIs
    pub fn new() -> Self {
        let mut patterns = Vec::new();

        // Node.js ES Module loader hooks
        // https://nodejs.org/api/esm.html#loaders
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/loader.{js,mjs,cjs}".to_string(),
            export_names: vec![
                "resolve".to_string(),
                "load".to_string(),
                "globalPreload".to_string(),
                "initialize".to_string(),
            ],
            description: "Node.js ES Module loader hooks".to_string(),
        });

        // Node.js ES Module loader hooks (nested directories)
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/loaders/*.{js,mjs,cjs}".to_string(),
            export_names: vec![
                "resolve".to_string(),
                "load".to_string(),
                "globalPreload".to_string(),
                "initialize".to_string(),
            ],
            description: "Node.js ES Module loader hooks (loaders directory)".to_string(),
        });

        // Node.js ES Module loader hooks (lib/internal/modules/esm/)
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/lib/internal/modules/esm/*.js".to_string(),
            export_names: vec![
                "resolve".to_string(),
                "load".to_string(),
                "getFormat".to_string(),
                "getSource".to_string(),
                "transformSource".to_string(),
            ],
            description: "Node.js internal ES Module hooks".to_string(),
        });

        // Node.js test runner hooks
        // https://nodejs.org/api/test.html
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/*.test.{js,mjs,cjs,ts,mts,cts}".to_string(),
            export_names: vec![
                "before".to_string(),
                "after".to_string(),
                "beforeEach".to_string(),
                "afterEach".to_string(),
            ],
            description: "Node.js test runner hooks".to_string(),
        });

        // Web Workers API
        // https://developer.mozilla.org/en-US/docs/Web/API/Worker
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/*.worker.{js,ts}".to_string(),
            export_names: vec![
                "onmessage".to_string(),
                "onmessageerror".to_string(),
                "onerror".to_string(),
            ],
            description: "Web Workers event handlers".to_string(),
        });

        // Service Workers API
        // https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/service-worker.{js,ts}".to_string(),
            export_names: vec![
                "install".to_string(),
                "activate".to_string(),
                "fetch".to_string(),
                "message".to_string(),
                "sync".to_string(),
                "push".to_string(),
            ],
            description: "Service Worker lifecycle hooks".to_string(),
        });

        patterns.push(RuntimeApiPattern {
            file_pattern: "**/sw.{js,ts}".to_string(),
            export_names: vec![
                "install".to_string(),
                "activate".to_string(),
                "fetch".to_string(),
                "message".to_string(),
                "sync".to_string(),
                "push".to_string(),
            ],
            description: "Service Worker lifecycle hooks (sw.js)".to_string(),
        });

        // Vite plugins
        // https://vitejs.dev/guide/api-plugin.html
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/vite.config.{js,ts}".to_string(),
            export_names: vec![
                "config".to_string(),
                "configResolved".to_string(),
                "buildStart".to_string(),
                "buildEnd".to_string(),
            ],
            description: "Vite plugin hooks".to_string(),
        });

        // Webpack plugins
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/webpack.config.{js,ts}".to_string(),
            export_names: vec![
                "apply".to_string(),
            ],
            description: "Webpack plugin hooks".to_string(),
        });

        // Next.js middleware
        // https://nextjs.org/docs/app/building-your-application/routing/middleware
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/middleware.{js,ts}".to_string(),
            export_names: vec![
                "middleware".to_string(),
                "config".to_string(),
            ],
            description: "Next.js middleware".to_string(),
        });

        // Astro components
        patterns.push(RuntimeApiPattern {
            file_pattern: "**/*.astro".to_string(),
            export_names: vec![
                "getStaticPaths".to_string(),
                "prerender".to_string(),
            ],
            description: "Astro component static generation".to_string(),
        });

        Self { patterns }
    }

    /// Add custom runtime API patterns from config
    pub fn add_custom_pattern(&mut self, pattern: RuntimeApiPattern) {
        self.patterns.push(pattern);
    }

    /// Check if an export is a runtime-invoked API
    pub fn is_runtime_api(&self, file_path: &str, export_name: &str) -> Option<String> {
        // Normalize path separators for cross-platform matching
        let normalized_path = file_path.replace('\\', "/");

        for pattern in &self.patterns {
            // Build glob matcher for this pattern
            if let Ok(glob) = globset::Glob::new(&pattern.file_pattern) {
                let matcher = glob.compile_matcher();

                // Check if file matches pattern and export name is in the list
                if matcher.is_match(&normalized_path)
                    && pattern.export_names.iter().any(|name| name == export_name)
                {
                    return Some(pattern.description.clone());
                }
            }
        }

        None
    }

    /// Check if an export is a runtime-invoked API, including custom patterns
    pub fn is_runtime_api_with_custom(
        &self,
        file_path: &str,
        export_name: &str,
        custom_apis: &[CustomRuntimeApi],
    ) -> Option<String> {
        // Check built-in patterns first
        if let Some(desc) = self.is_runtime_api(file_path, export_name) {
            return Some(desc);
        }

        // Check custom patterns from config
        let normalized_path = file_path.replace('\\', "/");
        for custom in custom_apis {
            // Check if export name matches
            if !custom.exports.iter().any(|name| name == export_name) {
                continue;
            }

            // Check if file matches any of the patterns
            for pattern_str in &custom.file_patterns {
                if let Ok(glob) = globset::Glob::new(pattern_str) {
                    let matcher = glob.compile_matcher();
                    if matcher.is_match(&normalized_path) {
                        return Some(format!("{} runtime API", custom.framework));
                    }
                }
            }
        }

        None
    }

    /// Get all runtime API patterns (for debugging/documentation)
    pub fn patterns(&self) -> &[RuntimeApiPattern] {
        &self.patterns
    }
}

impl Default for RuntimeApiRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nodejs_loader_hooks() {
        let registry = RuntimeApiRegistry::new();

        // Node.js loader hooks should be detected
        assert!(registry
            .is_runtime_api("lib/internal/modules/esm/hooks.js", "resolve")
            .is_some());
        assert!(registry
            .is_runtime_api("lib/internal/modules/esm/hooks.js", "load")
            .is_some());

        // Custom loader files
        assert!(registry
            .is_runtime_api("custom-loader.js", "resolve")
            .is_some());
        assert!(registry
            .is_runtime_api("loaders/typescript-loader.mjs", "load")
            .is_some());

        // Non-loader files should not match
        assert!(registry
            .is_runtime_api("src/utils.js", "resolve")
            .is_none());
    }

    #[test]
    fn test_test_runner_hooks() {
        let registry = RuntimeApiRegistry::new();

        assert!(registry
            .is_runtime_api("src/app.test.js", "beforeEach")
            .is_some());
        assert!(registry
            .is_runtime_api("tests/integration.test.ts", "afterEach")
            .is_some());

        // Non-test files should not match
        assert!(registry
            .is_runtime_api("src/app.js", "beforeEach")
            .is_none());
    }

    #[test]
    fn test_web_workers() {
        let registry = RuntimeApiRegistry::new();

        assert!(registry
            .is_runtime_api("workers/data-processor.worker.js", "onmessage")
            .is_some());
        assert!(registry
            .is_runtime_api("src/background.worker.ts", "onerror")
            .is_some());

        // Non-worker files should not match
        assert!(registry
            .is_runtime_api("src/app.js", "onmessage")
            .is_none());
    }

    #[test]
    fn test_service_workers() {
        let registry = RuntimeApiRegistry::new();

        assert!(registry
            .is_runtime_api("public/service-worker.js", "install")
            .is_some());
        assert!(registry
            .is_runtime_api("src/sw.js", "activate")
            .is_some());
        assert!(registry
            .is_runtime_api("service-worker.ts", "fetch")
            .is_some());
    }

    #[test]
    fn test_vite_plugins() {
        let registry = RuntimeApiRegistry::new();

        assert!(registry
            .is_runtime_api("vite.config.js", "config")
            .is_some());
        assert!(registry
            .is_runtime_api("config/vite.config.ts", "buildStart")
            .is_some());
    }

    #[test]
    fn test_nextjs_middleware() {
        let registry = RuntimeApiRegistry::new();

        assert!(registry
            .is_runtime_api("middleware.ts", "middleware")
            .is_some());
        assert!(registry
            .is_runtime_api("src/middleware.js", "config")
            .is_some());
    }

    #[test]
    fn test_custom_patterns() {
        let registry = RuntimeApiRegistry::new();
        let custom_apis = vec![CustomRuntimeApi {
            framework: "Remix".to_string(),
            exports: vec!["loader".to_string(), "action".to_string()],
            file_patterns: vec!["**/routes/*.{jsx,tsx}".to_string()],
        }];

        assert!(registry
            .is_runtime_api_with_custom("app/routes/index.tsx", "loader", &custom_apis)
            .is_some());
        assert!(registry
            .is_runtime_api_with_custom("app/routes/about.jsx", "action", &custom_apis)
            .is_some());
    }

    #[test]
    fn test_cross_platform_paths() {
        let registry = RuntimeApiRegistry::new();

        // Test Windows-style paths
        assert!(registry
            .is_runtime_api("lib\\internal\\modules\\esm\\hooks.js", "resolve")
            .is_some());

        // Test Unix-style paths
        assert!(registry
            .is_runtime_api("lib/internal/modules/esm/hooks.js", "load")
            .is_some());
    }
}
