//! Real JSON-RPC stdio smoke for `loctree/symbolContext`.
//!
//! The in-process integration tests prove the handler logic; this proves the
//! WIRE: spawn the actual `loctree-lsp --stdio` binary, do a real LSP
//! `initialize` handshake, then send `loctree/symbolContext` and assert the
//! JSON shape a VS Code client will consume. Catches framing / serde / routing
//! regressions that helper tests cannot.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders ⓒ 2025-2026 Vetcoders

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

/// Write a single LSP message (`Content-Length` framed) to the server.
fn write_msg(stdin: &mut impl Write, value: &Value) {
    let body = serde_json::to_string(value).expect("serialize message");
    write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("write message");
    stdin.flush().expect("flush stdin");
}

/// Read one framed LSP message from the server's stdout.
fn read_msg(reader: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("read header line");
        if n == 0 {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().expect("parse Content-Length");
        }
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).expect("read message body");
    Some(serde_json::from_slice(&buf).expect("parse message json"))
}

/// Drain frames on a reader thread so a silent/stuck child cannot block the
/// test forever. ServerGuard reaps the owned child if the deadline panics.
fn response_stream(stdout: ChildStdout) -> std::sync::mpsc::Receiver<Value> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(message) = read_msg(&mut reader) {
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    receiver
}

/// Read framed messages until one with `id == want_id` (a response) arrives,
/// skipping interleaved notifications. Bounded so a stuck server fails the test
/// rather than hanging forever.
fn read_response(reader: &mut std::sync::mpsc::Receiver<Value>, want_id: i64) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    for _ in 0..500 {
        let msg = reader
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("server response within 15 seconds");
        if msg.get("id").and_then(Value::as_i64) == Some(want_id) {
            return msg;
        }
    }
    panic!("no response with id {want_id} after 500 messages");
}

struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn literal_protocol_semantics_use_shared_metadata_and_keep_full_pages() {
    let dir = tempfile::tempdir().expect("literal protocol project");
    let cache = tempfile::tempdir().expect("private cache");
    let files = [
        (
            "packs.ts",
            "export function getGlyphPack(id: string) { return id; }\ngetGlyphPack('a');\nfunction load(source: getGlyphPack = fallback) {}\nconst event = { getGlyphPack };\n// getGlyphPack\nconst label = 'getGlyphPack';\n",
        ),
        (
            "app.ts",
            "import { getGlyphPack as glyph } from './packs';\nglyph('b');\n",
        ),
        (
            "unrelated.ts",
            "getGlyphPack('heuristic-call');\nconst unresolved = getGlyphPack;\n",
        ),
        (
            "routes.rs",
            "pub fn detect_axum_routes() {}\nfn run() { detect_axum_routes(); }\n",
        ),
        (
            "transcribe.py",
            "def transcribe_audio():\n    pass\ntranscribe_audio()\n",
        ),
    ];
    for (path, source) in files {
        std::fs::write(dir.path().join(path), source).expect("write fixture");
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_loctree-lsp"))
        .args(["--stdio", "--root"])
        .arg(dir.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn literal server");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut reader = response_stream(child.stdout.take().expect("stdout"));
    let mut guard = ServerGuard(child);
    let mut id = 0;
    let mut request = |method: &str, params: Value| {
        id += 1;
        write_msg(
            &mut stdin,
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        );
        read_response(&mut reader, id)
    };
    let init = request(
        "initialize",
        json!({
            "rootUri": format!("file://{}", dir.path().display()), "capabilities": {},
            "initializationOptions": {"loctree.watcher.enabled":false, "loctree.workspaces.maxDepth":0}
        }),
    );
    assert!(init["result"]["capabilities"]["definitionProvider"].is_null());
    drop(request);
    write_msg(
        &mut stdin,
        &json!({"jsonrpc":"2.0", "method":"initialized", "params":{}}),
    );
    let mut request = |method: &str, params: Value| {
        id += 1;
        write_msg(
            &mut stdin,
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        );
        read_response(&mut reader, id)
    };
    let mut ready = None;
    for _ in 0..40 {
        let response = request(
            "loctree/find",
            json!({"query":"getGlyphPack", "mode":"literal", "limit":50}),
        );
        if response["error"]["code"] != -32001 {
            ready = Some(response);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let response = ready.expect("snapshot ready");
    assert!(response.get("error").is_none(), "{response}");
    let full = &response["result"]["literal_matches"];
    assert_eq!(full["total"], 9);
    let hits = full["occurrences"].as_array().expect("full hits");
    let hit = |file: &str, line: u64| {
        hits.iter()
            .find(|hit| hit["file"] == file && hit["line"] == line)
            .expect("expected hit")
    };
    assert_eq!(hit("packs.ts", 1)["match_role"], "definition");
    assert_eq!(hit("packs.ts", 1)["confidence"], "high");
    assert_eq!(hit("packs.ts", 3)["match_role"], "reference");
    assert_eq!(hit("packs.ts", 4)["match_role"], "field_emission");
    assert_eq!(hit("app.ts", 1)["match_role"], "import");
    assert_eq!(
        hit("app.ts", 1)["resolved_definition"]["name"],
        "getGlyphPack"
    );
    assert_eq!(hit("unrelated.ts", 1)["match_role"], "reference");
    // The shared engine permits a same-language call-shaped fallback, but a
    // bare read cannot resolve merely because a global name is unique.
    assert_eq!(
        hit("unrelated.ts", 1)["resolved_definition"]["name"],
        "getGlyphPack"
    );
    assert_eq!(hit("unrelated.ts", 2)["match_role"], "reference");
    assert!(hit("unrelated.ts", 2)["resolved_definition"].is_null());
    let repeat = request(
        "loctree/find",
        json!({"query":"getGlyphPack", "mode":"literal", "limit":50}),
    );
    assert_eq!(repeat["result"]["literal_matches"], *full, "cache hit");
    for (query, file) in [
        ("detect_axum_routes", "routes.rs"),
        ("transcribe_audio", "transcribe.py"),
    ] {
        let control = request(
            "loctree/find",
            json!({"query":query,"mode":"literal","file":file}),
        );
        assert_eq!(
            control["result"]["literal_matches"]["occurrences"][0]["match_role"], "definition",
            "{control}"
        );
        assert_eq!(
            control["result"]["literal_matches"]["occurrences"][0]["confidence"],
            "high"
        );
    }
    let alias = request("loctree/find", json!({"query":"glyph","mode":"literal"}));
    assert_eq!(
        alias["result"]["literal_matches"]["occurrences"][0]["match_role"],
        "import"
    );
    assert_eq!(
        alias["result"]["literal_matches"]["occurrences"][0]["resolved_definition"]["name"],
        "getGlyphPack"
    );
    let scoped_full = request(
        "loctree/find",
        json!({"query":"getGlyphPack","mode":"literal","file":"packs.ts","limit":50,"group_by_file":true}),
    );
    let page = request(
        "loctree/find",
        json!({"query":"getGlyphPack","mode":"literal","file":"packs.ts","limit":5,"chunk_size":5,"group_by_file":true}),
    );
    let page = &page["result"]["literal_matches"];
    assert_eq!(page["total"], 6);
    assert_eq!(page["emitted"], 5);
    assert_eq!(page["page"]["has_more"], true);
    let tail = request(
        "loctree/find",
        json!({"query":"getGlyphPack","mode":"literal","file":"packs.ts","limit":5,"offset":5,"group_by_file":true}),
    );
    let tail = &tail["result"]["literal_matches"];
    assert_eq!(tail["total"], 6);
    assert_eq!(tail["emitted"], 1);
    assert_eq!(tail["universe"], page["universe"]);
    assert_eq!(tail["role_summary"], page["role_summary"]);
    let mut combined = page["occurrences"].as_array().expect("page hits").clone();
    combined.extend(
        tail["occurrences"]
            .as_array()
            .expect("tail hits")
            .iter()
            .cloned(),
    );
    // Usage counts are scoped to the scan, so compare full rows against the
    // unpaged SAME file scope, rather than a repository-wide enrichment.
    let expected = scoped_full["result"]["literal_matches"]["occurrences"]
        .as_array()
        .expect("unpaged scoped hits");
    assert_eq!(
        &combined, expected,
        "full rows, including coordinates, survive paging"
    );
    let counts = request(
        "loctree/find",
        json!({"query":"getGlyphPack","mode":"literal","file":"packs.ts","count_only":true,"group_by_file":true}),
    );
    assert_eq!(counts["result"]["literal_matches"]["total"], 6);
    assert_eq!(
        counts["result"]["literal_matches"]["occurrences"],
        json!([])
    );
    assert_eq!(
        counts["result"]["literal_matches"]["role_summary"],
        page["role_summary"]
    );
    let context = request(
        "loctree/symbolContext",
        json!({"symbol":"getGlyphPack","file":"packs.ts","position":{"line":0,"character":16},"same_file_only":true,"occurrence_limit":5}),
    );
    assert_eq!(context["result"]["occurrences"]["same_file_total"], 6);
    assert_eq!(
        context["result"]["occurrences"]["returned"]
            .as_array()
            .expect("context page")
            .len(),
        5
    );
    assert_eq!(context["result"]["occurrences"]["next_offset"], 5);
    let context_tail = request(
        "loctree/symbolContext",
        json!({"symbol":"getGlyphPack","file":"packs.ts","position":{"line":0,"character":16},"same_file_only":true,"occurrence_limit":5,"offset":5}),
    );
    assert_eq!(
        context_tail["result"]["occurrences"]["returned"]
            .as_array()
            .expect("context tail")
            .len(),
        1
    );
    assert_eq!(context_tail["result"]["occurrences"]["has_more"], false);
    std::fs::write(
        dir.path().join("packs.ts"),
        "// getGlyphPack after saved dirty invalidation\n",
    )
    .expect("save dirty edit");
    let changed = request(
        "loctree/find",
        json!({"query":"getGlyphPack","mode":"literal","file":"packs.ts"}),
    );
    assert_eq!(changed["result"]["literal_matches"]["total"], 1);
    assert_eq!(
        changed["result"]["literal_matches"]["occurrences"][0]["match_role"],
        "comment"
    );
    assert_eq!(
        changed["result"]["literal_matches"]["occurrences"][0]["line"],
        1
    );
    assert_eq!(
        changed["result"]["literal_matches"]["occurrences"][0]["column"],
        4
    );
    let changed_context = request(
        "loctree/symbolContext",
        json!({"symbol":"getGlyphPack","file":"packs.ts","position":{"line":0,"character":4},"same_file_only":true}),
    );
    assert_eq!(
        changed_context["result"]["occurrences"]["same_file_total"],
        1
    );
    assert_eq!(
        changed_context["result"]["occurrences"]["returned"][0]["kind"],
        "comment"
    );
    let shutdown = request("shutdown", Value::Null);
    assert!(shutdown.get("error").is_none(), "{shutdown}");
    drop(request);
    write_msg(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"}));
    drop(stdin);
    let exit_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(status) = guard.0.try_wait().expect("server exit status") {
            assert!(status.success());
            break;
        }
        assert!(
            std::time::Instant::now() < exit_deadline,
            "server exit within 3 seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn symbol_context_over_real_stdio_jsonrpc() {
    // Tiny TS project: an EXPORTED `greet` calling a FILE-LOCAL `hello`.
    // Keep it non-git: editor users can open ordinary folders, and LSP
    // auto-scan must still write and reload a usable flat cache snapshot.
    let dir = tempfile::tempdir().expect("temp project");
    let cache = tempfile::tempdir().expect("isolated cache");
    std::fs::create_dir_all(dir.path().join("src")).expect("src dir");
    std::fs::write(
        dir.path().join("src/app.ts"),
        "import { greet } from './lib';\n\
         greet(\"a\");\ngreet(\"b\");\n",
    )
    .expect("write app.ts");
    // Second file: declares + exports `greet`, calling a file-local `hello`.
    // app.ts only USES greet → cross-file resolution via the import graph.
    std::fs::write(
        dir.path().join("src/lib.ts"),
        "export function greet(name: string): string {\n  return hello(name);\n}\n\
         function hello(n: string): string {\n  return \"hi \" + n;\n}\n",
    )
    .expect("write lib.ts");

    let root_uri = format!("file://{}", dir.path().display());

    let mut child = Command::new(env!("CARGO_BIN_EXE_loctree-lsp"))
        .arg("--stdio")
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_OPEN_BROWSER", "0")
        // The workspace is a TMPDIR fixture, outside any git checkout; the scan
        // guard (loctree snapshot.rs) refuses such roots unless this is set.
        // Without it the background scan never produces a snapshot and every
        // request below answers `-32001`, exhausting the retry budget.
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn loctree-lsp --stdio");

    let mut stdin = child.stdin.take().expect("child stdin");
    let mut reader = response_stream(child.stdout.take().expect("child stdout"));
    let _guard = ServerGuard(child);

    // initialize → wait for the result.
    write_msg(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "tmp" }]
            }
        }),
    );
    let init = read_response(&mut reader, 1);
    assert!(
        init["result"]["capabilities"].is_object(),
        "initialize must return capabilities: {init}"
    );

    write_msg(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    );

    // loctree/symbolContext on a USAGE of `greet` in app.ts (line 1, 0-based:
    // `greet("a");`). `greet` is declared+exported in src/lib.ts, so this
    // exercises CROSS-FILE resolution via the import graph: the response must
    // carry `exported=true` AND `defined_in=src/lib.ts`.
    //
    // The snapshot scan runs asynchronously after `initialized`, so an immediate
    // request can race it (`-32001 snapshot not loaded`) — exactly what a real
    // client sees. Retry until the snapshot settles (the VS Code gateway does the
    // same soft retry).
    let mut resp = None;
    for attempt in 0..40 {
        let id = 100 + attempt;
        write_msg(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0", "id": id, "method": "loctree/symbolContext",
                "params": {
                    "file": "src/app.ts",
                    "position": { "line": 1, "character": 0 },
                    "symbol": "greet"
                }
            }),
        );
        let candidate = read_response(&mut reader, id);
        let snapshot_pending = candidate["error"]["code"].as_i64() == Some(-32001);
        if !snapshot_pending {
            resp = Some(candidate);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let resp = resp.expect("snapshot must settle for non-git workspace within retry budget");

    assert!(
        resp.get("error").is_none(),
        "symbolContext must not error over the wire (after snapshot settles): {resp}"
    );
    let result = &resp["result"];
    assert!(result.is_object(), "symbolContext result object: {resp}");

    // Shape the VS Code client will consume.
    assert_eq!(result["symbol"], "greet", "resolved symbol name: {result}");
    assert_eq!(result["file"], "src/app.ts");
    assert_eq!(
        result["exported"], true,
        "exported greet must report exported=true (from identity, not literal stub): {result}"
    );
    // CROSS-FILE: greet is declared in src/lib.ts, used in src/app.ts. The
    // import graph must surface the declaring file.
    assert_eq!(
        result["defined_in"].as_str().map(|p| p.replace('\\', "/")),
        Some("src/lib.ts".to_string()),
        "imported greet must resolve defined_in=src/lib.ts via the import graph: {result}"
    );
    assert!(
        result["occurrences"].is_object(),
        "occurrences object present: {result}"
    );
    assert!(
        result["occurrences"]["total"].as_u64().unwrap_or(0) >= 1,
        "at least one literal occurrence of greet: {result}"
    );
    // CROSS-FILE BODY: the body is disambiguated to the DECLARING file (lib.ts)
    // and read via `query_symbol_body`'s root-resolving disk read, so it must be
    // PRESENT over the wire (regardless of the server's cwd) — never a
    // not_found_in_file error. This is the regression guard for the path-
    // resolution fix in loctree-rs::body::read_source.
    assert!(
        result.get("body_error").is_none(),
        "cross-file resolution must not yield not_found_in_file: {result}"
    );
    let body = &result["body"];
    assert!(
        body.is_object(),
        "cross-file body must be present over the wire (read from the declaring file): {result}"
    );
    assert!(
        body["source"]
            .as_str()
            .unwrap_or_default()
            .contains("function greet"),
        "body is lib.ts's greet declaration: {body}"
    );
    assert!(body["total_lines"].is_number(), "body.total_lines: {body}");

    // Clean shutdown.
    write_msg(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
    );
    let _ = read_response(&mut reader, 3);
    write_msg(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    );
}
