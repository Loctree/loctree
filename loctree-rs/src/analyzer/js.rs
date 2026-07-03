use std::collections::HashSet;
use std::path::Path;

use crate::types::FileAnalysis;

use super::ast_js;
use super::resolvers::TsPathResolver;

/// Analyze JS/TS file using AST parser.
/// Delegated to `ast_js` module which uses OXC parser.
pub(crate) fn analyze_js_file(
    content: &str,
    path: &Path,
    root: &Path,
    extensions: Option<&HashSet<String>>,
    ts_resolver: Option<&TsPathResolver>,
    relative: String,
    command_cfg: &super::ast_js::CommandDetectionConfig,
) -> FileAnalysis {
    ast_js::analyze_js_file_ast(
        content,
        path,
        root,
        extensions,
        ts_resolver,
        relative,
        command_cfg,
    )
}

#[cfg(test)]
mod tests {
    use super::{analyze_js_file, ast_js::CommandDetectionConfig};
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn detects_commands_reexports_and_exports() {
        let content = r#"
import defaultThing from "./dep";
import type { Foo } from "./types";
import "./side.css";
export { bar } from "./reexports";
export * from "./star";
export const localValue = 1;
export default function MyComp() {}
export { namedA, namedB as aliasB };
const dyn = import("./lazy");
safeInvoke("cmd_safe");
invokeSnake("cmd_snake");
invoke("cmd_invoke");
safeInvoke<Foo.Bar>("cmd_generic_safe");
invokeSnake<MyType>("cmd_generic_snake");
invoke<Inline<Ok>>("cmd_generic_invoke");
invokeAudioCamel<Baz>("cmd_audio_generic");
// Wrapper function patterns (Bug #2 fix)
invokePinCommand('get_pin_status', () => ({}));
myInvokeHelper<Response>('some_command', payload);
customCommandWrapper("another_cmd", options);
        "#;

        let analysis = analyze_js_file(
            content,
            Path::new("src/app.tsx"),
            Path::new("src"),
            Some(&HashSet::from(["ts".to_string(), "tsx".to_string()])),
            None,
            "app.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis
                .imports
                .iter()
                .any(|i| i.source == "./dep" && matches!(i.kind, crate::types::ImportKind::Static))
        );
        assert!(
            analysis.imports.iter().any(|i| i.source == "./side.css"
                && matches!(i.kind, crate::types::ImportKind::SideEffect))
        );
        assert!(analysis.reexports.iter().any(|r| r.source == "./reexports"));
        assert!(analysis.reexports.iter().any(|r| r.source == "./star"));
        assert!(analysis.dynamic_imports.iter().any(|s| s == "./lazy"));

        let commands: Vec<_> = analysis
            .command_calls
            .iter()
            .map(|c| c.name.clone())
            .collect();
        assert!(commands.contains(&"cmd_safe".to_string()));
        assert!(commands.contains(&"cmd_snake".to_string()));
        assert!(commands.contains(&"cmd_invoke".to_string()));
        assert!(commands.contains(&"cmd_generic_safe".to_string()));
        assert!(commands.contains(&"cmd_generic_snake".to_string()));
        assert!(commands.contains(&"cmd_generic_invoke".to_string()));
        assert!(commands.contains(&"cmd_audio_generic".to_string()));
        // Wrapper function patterns (Bug #2 fix)
        assert!(
            commands.contains(&"get_pin_status".to_string()),
            "Should detect invokePinCommand wrapper"
        );
        assert!(
            commands.contains(&"some_command".to_string()),
            "Should detect myInvokeHelper wrapper"
        );
        assert!(
            commands.contains(&"another_cmd".to_string()),
            "Should detect customCommandWrapper"
        );

        let generics: Vec<_> = analysis
            .command_calls
            .iter()
            .filter_map(|c| c.generic_type.clone())
            .collect();
        assert!(generics.iter().any(|g| g.contains("Foo.Bar")));

        // exports should include defaults (named "default") and named exports
        let export_names: Vec<_> = analysis.exports.iter().map(|e| e.name.clone()).collect();
        assert!(export_names.contains(&"localValue".to_string()));
        // Default exports are now named "default", original name in export_type
        assert!(export_names.contains(&"default".to_string()));
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "default" && e.export_type == "MyComp")
        );
        assert!(export_names.contains(&"namedA".to_string()));
    }

    #[test]
    fn default_import_tracking() {
        let content = r#"
import DefaultFoo from "./foo";
import DefaultBar, { named1, named2 } from "./bar";
import { named3 } from "./baz";
import * as Everything from "./everything";
        "#;

        let analysis = analyze_js_file(
            content,
            Path::new("src/app.tsx"),
            Path::new("src"),
            Some(&HashSet::from(["ts".to_string(), "tsx".to_string()])),
            None,
            "app.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Check default import from "./foo"
        let foo_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./foo")
            .expect("Should find ./foo import");
        assert_eq!(foo_import.symbols.len(), 1);
        assert_eq!(foo_import.symbols[0].name, "DefaultFoo");
        assert!(
            foo_import.symbols[0].is_default,
            "DefaultFoo should be marked as default import"
        );

        // Check mixed default + named imports from "./bar"
        let bar_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./bar")
            .expect("Should find ./bar import");
        assert_eq!(bar_import.symbols.len(), 3);

        let default_sym = bar_import
            .symbols
            .iter()
            .find(|s| s.name == "DefaultBar")
            .expect("Should find DefaultBar symbol");
        assert!(
            default_sym.is_default,
            "DefaultBar should be marked as default import"
        );

        let named1_sym = bar_import
            .symbols
            .iter()
            .find(|s| s.name == "named1")
            .expect("Should find named1 symbol");
        assert!(
            !named1_sym.is_default,
            "named1 should NOT be marked as default import"
        );

        // Check named-only import from "./baz"
        let baz_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./baz")
            .expect("Should find ./baz import");
        assert_eq!(baz_import.symbols.len(), 1);
        assert_eq!(baz_import.symbols[0].name, "named3");
        assert!(
            !baz_import.symbols[0].is_default,
            "named3 should NOT be marked as default import"
        );

        // Check namespace import
        let ns_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./everything")
            .expect("Should find ./everything import");
        assert_eq!(ns_import.symbols.len(), 1);
        assert_eq!(ns_import.symbols[0].name, "*");
        assert!(
            !ns_import.symbols[0].is_default,
            "Namespace import should NOT be marked as default import"
        );
    }

    #[test]
    fn default_export_matching_scenario() {
        // This test demonstrates the problem this fix solves:
        // Default exports should be matchable with default imports,
        // regardless of the aliased import name.

        let exporter_content = r#"
export default function MyComponent() {
    return <div>Hello</div>;
}
        "#;

        let importer_content = r#"
import Foo from "./component";
import Bar from "./component";
import Baz from "./component";
        "#;

        let exporter = analyze_js_file(
            exporter_content,
            Path::new("src/component.tsx"),
            Path::new("src"),
            None,
            None,
            "component.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        let importer = analyze_js_file(
            importer_content,
            Path::new("src/app.tsx"),
            Path::new("src"),
            None,
            None,
            "app.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        // The export should be stored with name "default" for matching with `import X from`
        // The original function name is preserved in export_type
        assert_eq!(exporter.exports.len(), 1);
        let export = &exporter.exports[0];
        assert_eq!(export.name, "default");
        assert_eq!(export.kind, "default");
        assert_eq!(export.export_type, "MyComponent");

        // All three imports should be marked as default imports
        // even though they have different names (Foo, Bar, Baz)
        assert_eq!(importer.imports.len(), 3);
        for imp in &importer.imports {
            assert_eq!(imp.symbols.len(), 1);
            let sym = &imp.symbols[0];
            assert!(
                sym.is_default,
                "{} should be marked as default import",
                sym.name
            );
        }

        // With is_default flag, all three imports (Foo, Bar, Baz) should match
        // the single default export "MyComponent" because they all have is_default=true
        // This prevents false positives where MyComponent appears "unused"
    }
}
