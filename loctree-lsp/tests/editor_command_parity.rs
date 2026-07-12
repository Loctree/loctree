//! W3 parity guard: every `loctree.*` command the LSP emits must be contributed
//! by both editors/vscode (package.json) and editors/jetbrains (loctree-lsp.xml
//! and/or LoctreeLspCommandRouter).
//!
//! Prevents silent drift that surfaces as IntelliJ "Cannot execute 'loctree.*'".

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("loctree-lsp parent")
        .to_path_buf()
}

fn extract_lsp_emitted_commands(sources: &[&str]) -> BTreeSet<String> {
    let mut commands = BTreeSet::new();
    let needle = r#"command: ""#;
    for rel in sources {
        let path = repo_root().join(rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        for line in content.lines() {
            let Some(idx) = line.find(needle) else {
                continue;
            };
            let rest = &line[idx + needle.len()..];
            let end = rest.find('"').expect("closing quote");
            let cmd = &rest[..end];
            if cmd.starts_with("loctree.") {
                commands.insert(cmd.to_string());
            }
        }
    }
    commands
}

fn vscode_contributes(commands: &BTreeSet<String>) -> Vec<String> {
    let pkg = repo_root().join("editors/vscode/package.json");
    let content = fs::read_to_string(&pkg).expect("read vscode package.json");
    commands
        .iter()
        .filter(|cmd| !content.contains(&format!("\"command\": \"{cmd}\"")))
        .cloned()
        .collect()
}

fn jetbrains_xml_contributes(commands: &BTreeSet<String>) -> Vec<String> {
    let xml = repo_root().join("editors/jetbrains/src/main/resources/META-INF/loctree-lsp.xml");
    let content = fs::read_to_string(&xml).expect("read loctree-lsp.xml");
    commands
        .iter()
        .filter(|cmd| !content.contains(&format!(r#"id="{cmd}""#)))
        .cloned()
        .collect()
}

fn jetbrains_router_handles(commands: &BTreeSet<String>) -> Vec<String> {
    let router = repo_root()
        .join("editors/jetbrains/src/main/kotlin/io/loct/intellij/lsp/LoctreeLspCommandRouter.kt");
    let content = fs::read_to_string(&router).expect("read LoctreeLspCommandRouter.kt");
    commands
        .iter()
        .filter(|cmd| !content.contains(&format!("\"{cmd}\"")))
        .cloned()
        .collect()
}

#[test]
fn lsp_emitted_loctree_commands_have_vscode_and_jetbrains_contributions() {
    let emitted = extract_lsp_emitted_commands(&[
        "loctree-lsp/src/actions/refactor.rs",
        "loctree-lsp/src/actions/quickfix.rs",
    ]);
    assert!(
        emitted.len() >= 9,
        "expected at least 9 loctree.* commands from LSP actions, got {emitted:?}"
    );

    let missing_vscode = vscode_contributes(&emitted);
    assert!(
        missing_vscode.is_empty(),
        "VS Code package.json missing commands: {missing_vscode:?}"
    );

    let missing_xml = jetbrains_xml_contributes(&emitted);
    assert!(
        missing_xml.is_empty(),
        "JetBrains loctree-lsp.xml missing action ids: {missing_xml:?}"
    );

    let missing_router = jetbrains_router_handles(&emitted);
    assert!(
        missing_router.is_empty(),
        "JetBrains LoctreeLspCommandRouter missing when branches: {missing_router:?}"
    );
}
