use loctree_ast::{InputEdit, Parsers, Point};
use std::path::Path;

#[test]
fn parses_javascript_typescript_and_tsx() {
    let parsers = Parsers::new_default();
    let ids = parsers.language_ids();
    assert!(ids.contains(&"python"));
    assert_eq!(
        parsers
            .for_path(Path::new("types.pyi"))
            .expect("pyi parser")
            .lang_id(),
        "python"
    );
    let fixtures = [
        (
            "demo.js",
            "export function answer() { return 42; }\n",
            "javascript",
            "program",
        ),
        (
            "demo.py",
            "def answer():\n    return 42\n",
            "python",
            "module",
        ),
        (
            "demo.ts",
            "export const answer: number = 42;\n",
            "typescript",
            "program",
        ),
        (
            "demo.tsx",
            "export const View = () => <main>{42}</main>;\n",
            "tsx",
            "program",
        ),
    ];

    for (path, source, expected_lang, expected_root) in fixtures {
        let tree = parsers
            .parse_path(Path::new(path), source.as_bytes())
            .expect("fixture parses");
        assert_eq!(tree.lang, expected_lang);
        assert_eq!(tree.root_kind(), expected_root);
        assert!(!tree.has_error(), "{path} parsed with tree-sitter errors");
    }
}

#[test]
fn incremental_parse_reuses_previous_language_boundary() {
    let parsers = Parsers::new_default();
    let before = b"export const answer: number = 41;\n";
    let after = b"export const answer: number = 42;\n";
    let tree = parsers
        .parse_path(Path::new("demo.ts"), before)
        .expect("initial parse");
    let edit = InputEdit {
        start_byte: 30,
        old_end_byte: 32,
        new_end_byte: 32,
        start_position: Point { row: 0, column: 30 },
        old_end_position: Point { row: 0, column: 32 },
        new_end_position: Point { row: 0, column: 32 },
    };

    let reparsed = parsers
        .parse_incremental(&tree, after, &[edit])
        .expect("incremental parse");

    assert_eq!(reparsed.lang, "typescript");
    assert!(!reparsed.has_error());
}

#[test]
fn incremental_parse_supports_python_edits() {
    let parsers = Parsers::new_default();
    let before = b"def answer():\n    return 41\n";
    let after = b"def answer():\n    return 42\n";
    let tree = parsers
        .parse_path(Path::new("demo.py"), before)
        .expect("initial python parse");
    let edit = InputEdit {
        start_byte: 25,
        old_end_byte: 27,
        new_end_byte: 27,
        start_position: Point { row: 1, column: 11 },
        old_end_position: Point { row: 1, column: 13 },
        new_end_position: Point { row: 1, column: 13 },
    };

    let reparsed = parsers
        .parse_incremental(&tree, after, &[edit])
        .expect("incremental python parse");

    assert_eq!(reparsed.lang, "python");
    assert_eq!(reparsed.root_kind(), "module");
    assert!(!reparsed.has_error());
}
