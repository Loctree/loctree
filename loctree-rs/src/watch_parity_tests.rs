use super::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write(root: &Path, path: &str, source: &str) -> PathBuf {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(&path, source).expect("write fixture");
    path
}

fn full(roots: &[PathBuf], parsed: &ParsedArgs) -> Snapshot {
    let mut parsed = parsed.clone();
    parsed.full_scan = true;
    snapshot::build_snapshot_for_strategy(
        roots,
        &parsed,
        true,
        snapshot::SnapshotRootStrategy::Project,
        false,
    )
    .expect("canonical full scan")
}

fn normalized(snapshot: &Snapshot) -> serde_json::Value {
    fn sort(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => map.values_mut().for_each(sort),
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(sort);
                if items.iter().all(|v| v.is_object())
                    || items.iter().all(|v| v.is_array())
                    || items.iter().all(|v| v.is_string())
                {
                    items.sort_by_key(|v| v.to_string());
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(snapshot).expect("serialize snapshot");
    let metadata = value["metadata"]
        .as_object_mut()
        .expect("snapshot metadata");
    for key in ["generated_at", "scan_duration_ms"] {
        metadata.remove(key);
    }
    sort(&mut value);
    value
}

fn assert_parity(snap: &Snapshot, roots: &[PathBuf], parsed: &ParsedArgs) {
    let expected = normalized(&full(roots, parsed));
    let actual = normalized(snap);
    for key in expected.as_object().expect("snapshot object").keys() {
        assert_eq!(actual[key], expected[key], "canonical parity: {key}");
    }
}

fn initial(roots: &[PathBuf], parsed: &ParsedArgs) -> (Snapshot, ScanInfra) {
    let infra = setup_scan_infra(roots, parsed).expect("watch infrastructure");
    let snap = snapshot::build_snapshot_with_cache(
        roots,
        parsed,
        true,
        snapshot::SnapshotRootStrategy::Project,
        false,
        Some(&infra.raw_cache),
    )
    .expect("initial watch scan");
    assert_parity(&snap, roots, parsed);
    (snap, infra)
}

fn args() -> ParsedArgs {
    ParsedArgs {
        force_non_git: true,
        full_scan: true,
        use_gitignore: false,
        ..Default::default()
    }
}

#[test]
fn single_event_tauri_rename_matches_full_pipeline() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let root = temp.path().canonicalize()?;
    let roots = vec![root.clone()];
    let parsed = args();
    write(
        &root,
        "src-tauri/src/main.rs",
        "#[tauri::command]\nfn greet() {}\nfn main() { tauri::Builder::default().invoke_handler(tauri::generate_handler![greet]); }\n",
    );
    let app = write(
        &root,
        "src/App.tsx",
        "import { invoke } from '@tauri-apps/api/core';\ninvoke('greet');\n",
    );
    let (mut snap, mut infra) = initial(&roots, &parsed);
    assert!(
        snap.command_bridges
            .iter()
            .any(|b| b.name == "greet" && b.is_called)
    );
    fs::write(
        &app,
        "import { invoke } from '@tauri-apps/api/core';\ninvoke('greet_v2');\n",
    )?;
    patch_snapshot(
        &mut snap,
        &mut infra,
        &HashSet::from([app.clone()]),
        &parsed,
    )?;
    assert_parity(&snap, &roots, &parsed);
    assert!(
        snap.command_bridges
            .iter()
            .any(|b| b.name == "greet" && !b.is_called)
    );
    assert!(
        snap.command_bridges
            .iter()
            .any(|b| b.name == "greet_v2" && b.is_called && !b.has_handler)
    );
    // An event invalidates even when the stat-only cache would accept the file.
    let before = fs::metadata(&app)?.modified()?;
    fs::write(
        &app,
        "import { invoke } from '@tauri-apps/api/core';\ninvoke('greet_v3');\n",
    )?;
    fs::File::options()
        .write(true)
        .open(&app)?
        .set_times(fs::FileTimes::new().set_modified(before))?;
    patch_snapshot(
        &mut snap,
        &mut infra,
        &HashSet::from([app.clone()]),
        &parsed,
    )?;
    assert_parity(&snap, &roots, &parsed);
    assert!(
        snap.command_bridges
            .iter()
            .any(|b| b.name == "greet_v3" && b.is_called)
    );
    assert!(!snap.command_bridges.iter().any(|b| b.name == "greet_v2"));
    fs::remove_file(&app)?;
    patch_snapshot(&mut snap, &mut infra, &HashSet::from([app]), &parsed)?;
    assert_parity(&snap, &roots, &parsed);
    assert!(!snap.command_bridges.iter().any(|b| b.name == "greet_v3"));
    Ok(())
}

#[test]
fn events_reexports_graph_and_noop_match_full_pipeline() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let root = temp.path().canonicalize()?;
    let roots = vec![root.clone()];
    let parsed = args();
    let constants = write(&root, "src/events.ts", "export const READY = 'ready';\n");
    write(
        &root,
        "src/listener.ts",
        "import { listen } from '@tauri-apps/api/event';\nimport { READY } from './events';\nlisten(READY, () => {});\n",
    );
    write(
        &root,
        "src/emitter.ts",
        "import { emit } from '@tauri-apps/api/event';\nemit('ready');\n",
    );
    let barrel = write(&root, "src/index.ts", "export { READY } from './events';\n");
    let (mut snap, mut infra) = initial(&roots, &parsed);
    assert!(!snap.event_bridges.is_empty());
    assert!(!snap.barrels.is_empty());
    assert_parity(&snap, &roots, &parsed);
    fs::write(&constants, "export const READY = 'ready_v2';\n")?;
    patch_snapshot(
        &mut snap,
        &mut infra,
        &HashSet::from([constants.clone()]),
        &parsed,
    )?;
    assert_parity(&snap, &roots, &parsed);
    patch_snapshot(
        &mut snap,
        &mut infra,
        &HashSet::from([constants.clone()]),
        &parsed,
    )?;
    assert_parity(&snap, &roots, &parsed);
    fs::write(&barrel, "export const own = 1;\n")?;
    patch_snapshot(&mut snap, &mut infra, &HashSet::from([barrel]), &parsed)?;
    assert_parity(&snap, &roots, &parsed);
    assert!(snap.barrels.is_empty());
    fs::remove_file(&constants)?;
    patch_snapshot(&mut snap, &mut infra, &HashSet::from([constants]), &parsed)?;
    assert_parity(&snap, &roots, &parsed);
    Ok(())
}

#[test]
fn type_edges_survive_canonical_watch_event_reduction() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let root = temp.path().canonicalize()?;
    let roots = vec![root.clone()];
    let parsed = args();
    let a = write(
        &root,
        "a.ts",
        "import type { B } from './b'; export interface A { b: B }\n",
    );
    write(
        &root,
        "b.ts",
        "import type { A } from './a'; export interface B { a: A }\n",
    );
    let (mut snap, mut infra) = initial(&roots, &parsed);
    assert_eq!(
        snap.edges
            .iter()
            .filter(|e| e.label == "type_import")
            .count(),
        2
    );
    fs::write(
        &a,
        "import type { B } from './b'; export interface A { next: B }\n",
    )?;
    patch_snapshot(&mut snap, &mut infra, &HashSet::from([a]), &parsed)?;
    assert_parity(&snap, &roots, &parsed);
    assert_eq!(
        snap.edges
            .iter()
            .filter(|e| e.label == "type_import")
            .count(),
        2
    );
    let edges = snap
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect::<Vec<_>>();
    let (strict, _) = crate::analyzer::cycles::find_cycles_with_lazy(&edges);
    assert!(
        strict.is_empty(),
        "type navigation must not become a runtime cycle"
    );
    Ok(())
}

#[test]
fn rust_python_and_symbol_identity_survive_edit_add_delete()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let root = temp.path().canonicalize()?;
    let roots = vec![root.clone()];
    let parsed = args();
    write(
        &root,
        "Cargo.toml",
        "[package]\nname = 'watch_fixture'\nversion = '0.1.0'\nedition = '2024'\n",
    );
    write(
        &root,
        "src/lib.rs",
        "mod alpha;\nmod beta;\npub fn caller() { crate::alpha::run(); }\n",
    );
    let alpha = write(&root, "src/alpha.rs", "pub fn run() {}\n");
    write(&root, "src/beta.rs", "pub fn run() {}\n");
    write(&root, "pkg/__init__.py", "");
    write(
        &root,
        "pkg/main.py",
        "from .worker import work\ndef main():\n    work()\n",
    );
    let worker = write(&root, "pkg/worker.py", "def work():\n    return 1\n");
    let symbol = write(&root, "Widget.swift", "public struct Widget {}\n");
    write(&root, "Use.swift", "func use() { let _ = Widget() }\n");
    let (mut snap, mut infra) = initial(&roots, &parsed);
    assert!(
        snap.edges
            .iter()
            .any(|e| e.from == "src/lib.rs" && e.to == "src/alpha.rs")
    );
    fs::write(&alpha, "pub fn renamed() {}\n")?;
    patch_snapshot(
        &mut snap,
        &mut infra,
        &HashSet::from([alpha.clone()]),
        &parsed,
    )?;
    assert_parity(&snap, &roots, &parsed);
    for path in [alpha, worker, symbol] {
        fs::remove_file(&path)?;
        patch_snapshot(&mut snap, &mut infra, &HashSet::from([path]), &parsed)?;
        assert_parity(&snap, &roots, &parsed);
    }
    let worker = write(&root, "pkg/worker.py", "def work():\n    return 2\n");
    patch_snapshot(&mut snap, &mut infra, &HashSet::from([worker]), &parsed)?;
    assert_parity(&snap, &roots, &parsed);
    Ok(())
}

#[test]
fn resolver_configuration_change_invalidates_unchanged_importers()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let root = temp.path().canonicalize()?;
    let roots = vec![root.clone()];
    let parsed = args();
    let config = write(
        &root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@model":["src/a.ts"]}}}"#,
    );
    write(&root, "src/a.ts", "export const model = 'a';\n");
    write(&root, "src/b.ts", "export const model = 'b';\n");
    write(
        &root,
        "src/main.ts",
        "import { model } from '@model';\nconsole.log(model);\n",
    );
    let (mut snap, mut infra) = initial(&roots, &parsed);
    assert!(
        snap.edges
            .iter()
            .any(|e| e.from == "src/main.ts" && e.to == "src/a.ts")
    );
    fs::write(
        &config,
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@model":["src/b.ts"]}}}"#,
    )?;
    patch_snapshot(&mut snap, &mut infra, &HashSet::from([config]), &parsed)?;
    assert_parity(&snap, &roots, &parsed);
    assert!(
        snap.edges
            .iter()
            .any(|e| e.from == "src/main.ts" && e.to == "src/b.ts")
    );
    assert!(
        !snap
            .edges
            .iter()
            .any(|e| e.from == "src/main.ts" && e.to == "src/a.ts")
    );
    Ok(())
}

#[test]
fn multi_root_cache_keeps_root_identity_and_failed_build_keeps_old_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let left = TempDir::new()?;
    let right = TempDir::new()?;
    let roots = vec![left.path().canonicalize()?, right.path().canonicalize()?];
    let parsed = args();
    write(&roots[0], "src/api.ts", "export const left = 1;\n");
    let changed = write(&roots[1], "src/api.ts", "export const right = 2;\n");
    let (mut snap, mut infra) = initial(&roots, &parsed);
    assert_eq!(infra.raw_cache.lock().expect("cache lock").len(), 2);
    fs::write(&changed, "export const updated = 3;\n")?;
    patch_snapshot(
        &mut snap,
        &mut infra,
        &HashSet::from([changed.clone()]),
        &parsed,
    )?;
    assert_parity(&snap, &roots, &parsed);
    let before = serde_json::to_value(&snap)?;
    let restricted = ParsedArgs {
        force_non_git: false,
        ..parsed.clone()
    };
    assert!(
        patch_snapshot(
            &mut snap,
            &mut infra,
            &HashSet::from([changed]),
            &restricted
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&snap)?, before);
    Ok(())
}
