//! Framework detection and convention patterns
//!
//! Detects frameworks like SvelteKit, Next.js, Nuxt, Remix, Astro and knows
//! their file-based routing conventions to avoid flagging intentional patterns
//! as duplicates.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Supported frameworks with file-based routing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    SvelteKit,
    NextJs,
    Nuxt,
    Remix,
    Astro,
}

impl std::fmt::Display for Framework {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Framework {
    /// Detect framework from config files in project root
    pub fn detect_from_root(root: &Path) -> Option<Self> {
        // SvelteKit: svelte.config.js or svelte.config.ts
        if root.join("svelte.config.js").exists() || root.join("svelte.config.ts").exists() {
            return Some(Framework::SvelteKit);
        }

        // Next.js: next.config.js or next.config.ts or next.config.mjs
        if root.join("next.config.js").exists()
            || root.join("next.config.ts").exists()
            || root.join("next.config.mjs").exists()
        {
            return Some(Framework::NextJs);
        }

        // Nuxt: nuxt.config.js or nuxt.config.ts
        if root.join("nuxt.config.js").exists()
            || root.join("nuxt.config.ts").exists()
            || root.join("nuxt.config.mjs").exists()
        {
            return Some(Framework::Nuxt);
        }

        // Remix: remix.config.js or remix.config.ts
        if root.join("remix.config.js").exists() || root.join("remix.config.ts").exists() {
            return Some(Framework::Remix);
        }

        // Astro: astro.config.js or astro.config.ts or astro.config.mjs
        if root.join("astro.config.js").exists()
            || root.join("astro.config.ts").exists()
            || root.join("astro.config.mjs").exists()
        {
            return Some(Framework::Astro);
        }

        None
    }

    /// Get the human-readable name for this framework
    pub fn name(&self) -> &'static str {
        match self {
            Framework::SvelteKit => "SvelteKit",
            Framework::NextJs => "Next.js",
            Framework::Nuxt => "Nuxt",
            Framework::Remix => "Remix",
            Framework::Astro => "Astro",
        }
    }

    /// Get route handler export names that are conventional for this framework
    fn route_handler_exports(&self) -> HashSet<&'static str> {
        match self {
            Framework::SvelteKit => {
                // SvelteKit +server.js/ts files export request handlers
                vec![
                    "GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD",
                    "load", // +page.ts load function
                ]
                .into_iter()
                .collect()
            }
            Framework::NextJs => {
                // Next.js App Router route handlers (route.js/ts)
                vec!["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"]
                    .into_iter()
                    .collect()
            }
            Framework::Nuxt => {
                // Nuxt server routes
                vec!["default", "get", "post", "put", "patch", "delete"]
                    .into_iter()
                    .collect()
            }
            Framework::Remix => {
                // Remix route exports
                vec!["loader", "action", "default", "headers", "meta", "links"]
                    .into_iter()
                    .collect()
            }
            Framework::Astro => {
                // Astro endpoints
                vec!["get", "post", "put", "patch", "del", "all"]
                    .into_iter()
                    .collect()
            }
        }
    }

    /// Check if a file path matches this framework's routing conventions
    fn is_route_file(&self, path: &str) -> bool {
        let path_lower = path.to_lowercase();

        // Helper: check if path contains segment (handles both "/segment/" and "segment/" at start)
        let has_segment = |segment: &str| {
            let with_slashes = format!("/{}/", segment);
            let at_start = format!("{}/", segment);
            path_lower.contains(&with_slashes) || path_lower.starts_with(&at_start)
        };

        // Helper: check if filename matches (e.g., "route.ts" at end of path)
        let ends_with_file = |prefix: &str| {
            // Check for /prefix. or path starting with prefix.
            let pattern = format!("/{prefix}.");
            path_lower.contains(&pattern) || path_lower.starts_with(&format!("{prefix}."))
        };

        match self {
            Framework::SvelteKit => {
                // SvelteKit: src/routes/**/{+page,+server,+layout}.{js,ts,svelte}
                has_segment("routes")
                    && (path_lower.contains("+server.")
                        || path_lower.contains("+page.")
                        || path_lower.contains("+layout."))
            }
            Framework::NextJs => {
                // Next.js App Router: app/**/route.{js,ts,jsx,tsx}
                // Next.js Pages Router: pages/**/*.{js,ts,jsx,tsx}
                (has_segment("app") && ends_with_file("route"))
                    || (has_segment("pages")
                        && !path_lower.contains("_app.")
                        && !path_lower.contains("_document."))
            }
            Framework::Nuxt => {
                // Nuxt: server/api/**/*.{js,ts} or pages/**/*.vue
                (has_segment("server") && has_segment("api")) || has_segment("pages")
            }
            Framework::Remix => {
                // Remix: app/routes/**/*.{js,ts,jsx,tsx}
                has_segment("routes")
            }
            Framework::Astro => {
                // Astro: src/pages/**/*.astro or src/pages/api/**/*.{js,ts}
                has_segment("pages")
            }
        }
    }

    /// Check if a symbol export is a conventional pattern for this framework
    /// that should not be flagged as a duplicate
    fn is_conventional_export(&self, symbol_name: &str, file_path: &str) -> bool {
        // First check if it's a route file
        if !self.is_route_file(file_path) {
            return false;
        }

        // Then check if the symbol is a conventional export
        self.route_handler_exports().contains(symbol_name)
    }
}

/// Detect all frameworks in a project root directory
///
/// This checks for framework config files and also scans monorepo subdirectories
/// (apps/, packages/) for additional frameworks.
pub fn detect_frameworks(root: &Path) -> Vec<Framework> {
    let mut frameworks = Vec::new();

    // Check root directory
    if let Some(fw) = Framework::detect_from_root(root) {
        frameworks.push(fw);
    }

    // Check monorepo subdirectories
    for subdir_name in &["apps", "packages"] {
        let subdir = root.join(subdir_name);
        if subdir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&subdir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && let Some(fw) = Framework::detect_from_root(&path)
                    && !frameworks.contains(&fw)
                {
                    frameworks.push(fw);
                }
            }
        }
    }

    frameworks
}

/// Check if a symbol should be excluded from twin detection due to framework conventions
///
/// This helper function checks all frameworks in the list to see if any of them
/// consider this export a convention that should not be flagged.
pub fn is_framework_convention(
    symbol_name: &str,
    file_path: &str,
    frameworks: &[Framework],
) -> bool {
    frameworks
        .iter()
        .any(|fw| fw.is_conventional_export(symbol_name, file_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_sveltekit_detection() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("svelte.config.js"), "export default {}").unwrap();

        let detected = Framework::detect_from_root(temp.path());
        assert_eq!(detected, Some(Framework::SvelteKit));
    }

    #[test]
    fn test_nextjs_detection() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("next.config.js"), "module.exports = {}").unwrap();

        let detected = Framework::detect_from_root(temp.path());
        assert_eq!(detected, Some(Framework::NextJs));
    }

    #[test]
    fn test_nuxt_detection() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("nuxt.config.ts"), "export default {}").unwrap();

        let detected = Framework::detect_from_root(temp.path());
        assert_eq!(detected, Some(Framework::Nuxt));
    }

    #[test]
    fn test_no_framework_detection() {
        let temp = TempDir::new().unwrap();
        let detected = Framework::detect_from_root(temp.path());
        assert_eq!(detected, None);
    }

    #[test]
    fn test_sveltekit_route_handlers() {
        let framework = Framework::SvelteKit;
        let handlers = framework.route_handler_exports();

        assert!(handlers.contains("GET"));
        assert!(handlers.contains("POST"));
        assert!(handlers.contains("PUT"));
        assert!(handlers.contains("DELETE"));
        assert!(handlers.contains("load"));
    }

    #[test]
    fn test_sveltekit_is_route_file() {
        let framework = Framework::SvelteKit;

        assert!(framework.is_route_file("src/routes/api/users/+server.ts"));
        assert!(framework.is_route_file("src/routes/blog/+page.ts"));
        assert!(framework.is_route_file("src/routes/+layout.svelte"));

        assert!(!framework.is_route_file("src/lib/utils.ts"));
        assert!(!framework.is_route_file("src/components/Button.svelte"));
    }

    #[test]
    fn test_nextjs_is_route_file() {
        let framework = Framework::NextJs;

        assert!(framework.is_route_file("app/api/users/route.ts"));
        assert!(framework.is_route_file("app/dashboard/route.js"));
        assert!(framework.is_route_file("pages/index.tsx"));

        assert!(!framework.is_route_file("components/Button.tsx"));
        assert!(!framework.is_route_file("lib/utils.ts"));
    }

    #[test]
    fn test_sveltekit_conventional_export() {
        let framework = Framework::SvelteKit;

        // Route file with conventional export
        assert!(framework.is_conventional_export("GET", "src/routes/api/users/+server.ts"));
        assert!(framework.is_conventional_export("POST", "src/routes/api/posts/+server.js"));
        assert!(framework.is_conventional_export("load", "src/routes/blog/+page.ts"));

        // Non-route file should return false
        assert!(!framework.is_conventional_export("GET", "src/lib/api.ts"));

        // Route file with non-conventional export
        assert!(!framework.is_conventional_export("myCustomFunction", "src/routes/api/+server.ts"));
    }

    #[test]
    fn test_is_framework_convention_helper() {
        let frameworks = vec![Framework::SvelteKit, Framework::NextJs];

        // SvelteKit convention
        assert!(is_framework_convention(
            "GET",
            "src/routes/api/users/+server.ts",
            &frameworks
        ));

        // Next.js convention
        assert!(is_framework_convention(
            "GET",
            "app/api/users/route.ts",
            &frameworks
        ));

        // Not a convention
        assert!(!is_framework_convention(
            "GET",
            "src/lib/utils.ts",
            &frameworks
        ));

        // Empty frameworks list
        assert!(!is_framework_convention(
            "GET",
            "src/routes/api/users/+server.ts",
            &[]
        ));
    }

    #[test]
    fn test_framework_names() {
        assert_eq!(Framework::SvelteKit.name(), "SvelteKit");
        assert_eq!(Framework::NextJs.name(), "Next.js");
        assert_eq!(Framework::Nuxt.name(), "Nuxt");
        assert_eq!(Framework::Remix.name(), "Remix");
        assert_eq!(Framework::Astro.name(), "Astro");
    }
}
