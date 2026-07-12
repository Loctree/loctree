//! Real JSON-RPC stdio smoke for `loctree/symbolContext`.
//!
//! The in-process integration tests prove the handler logic; this proves the
//! WIRE: spawn the actual `loctree-lsp --stdio` binary, do a real LSP
//! `initialize` handshake, then send `loctree/symbolContext` and assert the
//! JSON shape a VS Code client will consume. Catches framing / serde / routing
//! regressions that helper tests cannot.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

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
fn read_msg(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("read header line");
        assert!(n > 0, "server closed stdout before a complete message");
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
    serde_json::from_slice(&buf).expect("parse message json")
}

/// Read framed messages until one with `id == want_id` (a response) arrives,
/// skipping interleaved notifications. Bounded so a stuck server fails the test
/// rather than hanging forever.
fn read_response(reader: &mut BufReader<ChildStdout>, want_id: i64) -> Value {
    for _ in 0..500 {
        let msg = read_msg(reader);
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn loctree-lsp --stdio");

    let mut stdin = child.stdin.take().expect("child stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("child stdout"));
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
