//! Import-role contracts shared by literal find, occurrences and library scans.

use assert_cmd::cargo::cargo_bin_cmd;
use loctree::analyzer::occurrences::{enrich_with_snapshot, scan_files};
use loctree::snapshot::Snapshot;
use serde_json::Value;
use tempfile::TempDir;

struct Project {
    root: TempDir,
    cache: TempDir,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let project = Self {
            root: tempfile::tempdir().expect("fixture source directory"),
            cache: tempfile::tempdir().expect("fixture cache directory"),
        };
        for (path, source) in files {
            std::fs::write(project.root.path().join(path), source).expect("write fixture source");
        }
        project
    }

    fn run(&self, args: &[&str]) -> Value {
        let output = cargo_bin_cmd!("loct")
            .current_dir(self.root.path())
            .env("LOCT_CACHE_DIR", self.cache.path())
            .env("LOCT_OPEN_BROWSER", "0")
            .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("CLI JSON response")
    }

    fn parity(&self, files: &[(&str, &str)], query: &str) -> Value {
        let occurrences = self.run(&["occurrences", query, "--json"]);
        let find = self.run(&["find", "--literal", query, "--json"]);
        for field in ["total", "occurrences", "role_summary"] {
            assert_eq!(
                occurrences[field], find["literal_matches"][field],
                "{field}"
            );
        }

        let snapshot_path = self.run(&["snapshot-path", "--json"]);
        let snapshot: Snapshot = serde_json::from_slice(
            &std::fs::read(
                snapshot_path["snapshot_path"]
                    .as_str()
                    .expect("snapshot path"),
            )
            .expect("read CLI snapshot"),
        )
        .expect("deserialize CLI snapshot");
        let mut library = scan_files(files.iter().copied(), query);
        let coordinates: Vec<_> = library
            .occurrences
            .iter()
            .map(|hit| (hit.file.clone(), hit.range, hit.matched_text.clone()))
            .collect();
        enrich_with_snapshot(&mut library, &snapshot);
        assert_eq!(
            coordinates,
            library
                .occurrences
                .iter()
                .map(|hit| (hit.file.clone(), hit.range, hit.matched_text.clone()))
                .collect::<Vec<_>>()
        );
        let mut library = serde_json::to_value(library).expect("serialize library result");
        for field in ["total", "role_summary"] {
            assert_eq!(occurrences[field], library[field], "library {field}");
        }
        // CLI ranking is definition-first; library scans retain lexical order.
        // Compare every row in full without imposing either surface's order.
        let mut cli_hits = occurrences["occurrences"]
            .as_array()
            .expect("CLI hits")
            .clone();
        let key = |hit: &Value| {
            (
                hit["file"].as_str().expect("hit file").to_string(),
                hit["line"].as_u64().expect("hit line"),
                hit["column"].as_u64().expect("hit column"),
            )
        };
        cli_hits.sort_by_key(&key);
        let library_hits = library["occurrences"].as_array_mut().expect("library hits");
        library_hits.sort_by_key(key);
        assert_eq!(&cli_hits, library_hits, "library occurrences");
        let page = self.run(&[
            "occurrences",
            query,
            "--limit",
            "1",
            "--offset",
            "1",
            "--json",
        ]);
        assert_eq!(page["total"], occurrences["total"]);
        assert_eq!(page["role_summary"], occurrences["role_summary"]);
        assert_eq!(page["occurrences"][0], occurrences["occurrences"][1]);
        for (path, source) in files {
            assert_eq!(
                std::fs::read_to_string(self.root.path().join(path))
                    .expect("read unchanged source"),
                *source
            );
        }
        occurrences
    }
}

fn hit<'a>(result: &'a Value, file: &str, line: u64) -> &'a Value {
    result["occurrences"]
        .as_array()
        .expect("result hits")
        .iter()
        .find(|hit| hit["file"] == file && hit["line"] == line)
        .unwrap_or_else(|| panic!("missing {file}:{line}: {result}"))
}

#[test]
fn typescript_imports_keep_roles_alias_identity_and_literal_contracts() {
    let files = [
        (
            "session.ts",
            "export class AgentSession {}\nexport type SessionType = AgentSession;\n",
        ),
        ("other.ts", "export class AgentSession {}\n"),
        (
            "multiline.ts",
            "import {\n  type AgentSession,\n} from './session';\n",
        ),
        (
            "runtime.ts",
            "import { AgentSession } from './session';\nconst event = { AgentSession };\nAgentSession = replacement;\n// AgentSession\nconst label = 'AgentSession';\nfunction load(source: AgentSession = this.session) {}\n",
        ),
        (
            "aliased.ts",
            "import type {\n  AgentSession as Session,\n} from './session';\nlet active: Session;\n",
        ),
        (
            "inline.ts",
            "import { type SessionType as Settings } from './session';\nlet active: Settings;\n",
        ),
        (
            "distinct.ts",
            "import { AgentSession as A } from './session';\nimport {\n  type AgentSession as B,\n} from './other';\n",
        ),
        (
            "external.ts",
            "import type { AgentSession } from 'external';\n",
        ),
        (
            "barrier.ts",
            "import { AgentSession as Local } from './session';\nimport { type AgentSession as Remote } from 'external';\n",
        ),
        (
            "same-line.ts",
            "import { AgentSession as A } from './session'; import { AgentSession as B } from './other';\n",
        ),
    ];
    let project = Project::new(&files);
    let result = project.parity(&files, "AgentSession");
    for (file, line, target) in [
        ("multiline.ts", 2, "session.ts"),
        ("runtime.ts", 1, "session.ts"),
        ("aliased.ts", 2, "session.ts"),
        ("distinct.ts", 1, "session.ts"),
        ("distinct.ts", 3, "other.ts"),
    ] {
        let import = hit(&result, file, line);
        assert_eq!(import["match_role"], "import", "{import}");
        assert_eq!(import["confidence"], "high");
        assert_eq!(import["resolved_definition"]["file"], target, "{import}");
        assert_eq!(import["resolved_definition"]["name"], "AgentSession");
    }
    assert_eq!(hit(&result, "session.ts", 1)["match_role"], "definition");
    assert_eq!(hit(&result, "session.ts", 2)["match_role"], "reference");
    for (line, role) in [
        (2, "field_emission"),
        (3, "mutation"),
        (4, "comment"),
        (5, "string_literal"),
        (6, "reference"),
    ] {
        assert_eq!(hit(&result, "runtime.ts", line)["match_role"], role);
    }
    let external = hit(&result, "external.ts", 1);
    assert_eq!(external["match_role"], "import");
    assert!(external["resolved_definition"].is_null());
    assert_eq!(
        hit(&result, "barrier.ts", 1)["resolved_definition"]["file"],
        "session.ts"
    );
    assert!(hit(&result, "barrier.ts", 2)["resolved_definition"].is_null());
    for import in result["occurrences"]
        .as_array()
        .expect("TypeScript hits")
        .iter()
        .filter(|hit| hit["file"] == "same-line.ts")
    {
        assert_eq!(import["match_role"], "import");
        assert!(import["resolved_definition"].is_null(), "{import}");
    }
    for (query, file, name) in [
        ("Session", "aliased.ts", "AgentSession"),
        ("Settings", "inline.ts", "SessionType"),
    ] {
        let alias = project.parity(&files, query);
        let import = &alias["occurrences"][0];
        assert_eq!(import["file"], file);
        assert_eq!(import["match_role"], "import");
        assert_eq!(import["resolved_definition"]["name"], name, "{import}");
    }
    let definition = project.parity(&files, "SessionType");
    assert_eq!(
        hit(&definition, "session.ts", 2)["match_role"],
        "definition"
    );
}

#[test]
fn python_and_rust_imports_preserve_definitions_and_write_controls() {
    for files in [
        vec![
            ("session.py", "class AgentSession:\n    pass\n"),
            (
                "consumer.py",
                "from session import (\n    AgentSession as Session,\n)\nAgentSession = replacement\n# AgentSession\nlabel = 'AgentSession'\n",
            ),
        ],
        vec![
            ("session.rs", "pub struct AgentSession;\n"),
            (
                "lib.rs",
                "mod session;\nuse crate::session::{\n    AgentSession,\n};\nfn run() {\n    Event { AgentSession };\n    AgentSession += 1;\n}\n",
            ),
        ],
    ] {
        let project = Project::new(&files);
        let result = project.parity(&files, "AgentSession");
        let summary = &result["role_summary"];
        assert_eq!(summary["definitions"], 1, "{result}");
        assert_eq!(summary["imports"], 1, "{result}");
        assert_eq!(summary["mutations"], 1, "{result}");
        let import = result["occurrences"]
            .as_array()
            .expect("fixture hits")
            .iter()
            .find(|hit| hit["match_role"] == "import")
            .expect("fixture import");
        assert_eq!(
            import["resolved_definition"]["name"], "AgentSession",
            "{import}"
        );
        assert_eq!(import["resolved_definition"]["line"], 1);
    }
}
