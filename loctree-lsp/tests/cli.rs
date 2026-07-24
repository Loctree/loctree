use std::process::Command;

fn loctree_lsp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_loctree-lsp"))
}

#[test]
fn version_prints_and_exits() {
    let output = loctree_lsp()
        .arg("--version")
        .output()
        .expect("loctree-lsp --version should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("version stdout should be utf8");
    assert!(stdout.contains(env!("LOCTREE_LSP_BUILD_VERSION")));
}

#[test]
fn help_prints_usage_and_exits() {
    let output = loctree_lsp()
        .arg("--help")
        .output()
        .expect("loctree-lsp --help should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help stdout should be utf8");
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--capabilities"));
    assert!(stdout.contains("--debug"));
}

#[test]
fn capabilities_prints_initialize_result_json_and_exits() {
    let output = loctree_lsp()
        .arg("--capabilities")
        .output()
        .expect("loctree-lsp --capabilities should run");

    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("capabilities stdout should be json");

    assert_eq!(value["serverInfo"]["name"], "Loctree Language Server");
    assert_eq!(
        value["serverInfo"]["version"],
        serde_json::Value::String(env!("LOCTREE_LSP_BUILD_VERSION").to_string())
    );
    // Phase 1 surface cut: hover / references / definition are intentionally
    // NOT advertised — rust-analyzer / tsserver own those in the IDE; Loctree's
    // structural context lives in the Context Pill. The capability keys are
    // absent (None → omitted from JSON → indexes to Null).
    assert_eq!(
        value["capabilities"]["hoverProvider"],
        serde_json::Value::Null
    );
    assert_eq!(
        value["capabilities"]["referencesProvider"],
        serde_json::Value::Null
    );
    assert_eq!(
        value["capabilities"]["definitionProvider"],
        serde_json::Value::Null
    );
    // Code lenses are opt-in (off by default): the static `--capabilities`
    // view reflects the default-off surface, so the key is absent. Clients
    // turn it on via `initializationOptions.codeLens = true`.
    assert_eq!(
        value["capabilities"]["codeLensProvider"],
        serde_json::Value::Null
    );
    assert_eq!(
        value["capabilities"]["experimental"]["loctree/documentChanged"]["available"],
        true
    );
}

#[test]
fn capabilities_advertise_request_schemas_for_typed_namespaces() {
    // Every loctree/* namespace whose params type derives JsonSchema
    // must publish a `requestSchema` so editor-side LSP APIs (JetBrains
    // LSP, vscode-languageclient typed bindings, custom builders) can
    // render forms and validate calls without shipping duplicate type
    // definitions. Notification-only namespaces (`refresh`,
    // `scanProgress`, `documentChanged`, `symbolChanged`,
    // `openAtlasCard`) intentionally skip the schema field.
    let output = loctree_lsp()
        .arg("--capabilities")
        .output()
        .expect("loctree-lsp --capabilities should run");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("capabilities stdout should be json");

    let typed_namespaces = [
        "loctree/contextAtlas",
        "loctree/contextPack",
        "loctree/slice",
        "loctree/impact",
        "loctree/find",
        "loctree/follow",
        "loctree/health",
        "loctree/workspaces",
        "loctree/diff",
        "loctree/semantic",
        "loctree/aicx",
    ];

    let experimental = &value["capabilities"]["experimental"];
    for ns in typed_namespaces {
        let cap = &experimental[ns];
        assert_eq!(cap["available"], true, "{ns} must be available");
        assert!(
            cap["requestSchema"].is_object(),
            "{ns} must publish a requestSchema object, got: {cap}"
        );
        assert!(
            cap["requestSchema"]["$schema"].is_string()
                || cap["requestSchema"]["type"].is_string()
                || cap["requestSchema"]["properties"].is_object(),
            "{ns} requestSchema must look like a JSON Schema document"
        );
    }
}

#[test]
fn slice_request_schema_pins_target_field() {
    // Pin a representative shape end-to-end: SliceParams is the
    // simplest typed namespace, and `target: PathBuf` should serialize
    // as a non-null entry under properties so a typed client can render
    // it as a path field.
    let output = loctree_lsp()
        .arg("--capabilities")
        .output()
        .expect("loctree-lsp --capabilities should run");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("capabilities stdout should be json");

    let schema = &value["capabilities"]["experimental"]["loctree/slice"]["requestSchema"];
    assert!(
        schema["properties"]["target"].is_object(),
        "SliceParams.target must surface in requestSchema.properties, got: {schema}"
    );
    assert!(
        schema["required"]
            .as_array()
            .map(|r| r.iter().any(|v| v == "target"))
            .unwrap_or(false),
        "SliceParams.target must be marked required, got: {schema}"
    );
}
