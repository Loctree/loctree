use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

const EXPECTED_TOOLS: &[&str] = &[
    "context",
    "repo-view",
    "focus",
    "slice",
    "body",
    "find",
    "impact",
    "diff",
    "tree",
    "follow",
    "suppressions",
    "prism",
];
const TOOL_SURFACE_DIGEST: &str =
    "TOOLS: context,repo-view,focus,slice,body,find,impact,diff,tree,follow,suppressions,prism";

const MCP_RESPONSE_BUDGET_CHARS: usize = 38_000;
const MCP_RESPONSE_BUDGET_PROTOCOL: &str = "loctree.mcp.response_budget.v1";

#[test]
fn instructions_and_stdio_tools_list_are_bidirectionally_equal() {
    let mut server = StdioServer::start();

    let init = server.initialize();
    assert_compact_tool_digest(&init);
    let advertised = advertised_tool_names(&init);
    let callable = server.tools_list();

    assert_eq!(advertised, expected_tools(), "server instructions drifted");
    assert_eq!(callable, expected_tools(), "stdio tools/list drifted");
    assert_eq!(
        advertised, callable,
        "every advertised tool must be callable and every callable tool must be advertised"
    );
    assert!(
        callable.contains("find"),
        "find must be callable over stdio"
    );
}

#[test]
fn http_tools_list_matches_stdio_contract() {
    let server = HttpServer::start();

    let initialized = server.initialize();
    assert_compact_tool_digest(&initialized.init);
    let advertised = advertised_tool_names(&initialized.init);
    let callable = initialized.tools_list();

    assert_eq!(advertised, expected_tools(), "HTTP instructions drifted");
    assert_eq!(callable, expected_tools(), "HTTP tools/list drifted");
    assert_eq!(
        advertised, callable,
        "HTTP transport must expose the same advertised/callable surface"
    );
    assert!(callable.contains("find"), "find must be callable over HTTP");
}

#[test]
fn find_literal_roundtrips_through_stdio_server() {
    let project = sample_project();
    let mut server = StdioServer::start();
    server.initialize();
    server.initialized();

    let result = server.request(
        2,
        "tools/call",
        json!({
            "name": "find",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "name": "needle_literal_marker",
                "mode": "literal",
                "group_by_file": true
            }
        }),
    );

    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("tool result text");
    let body: Value = serde_json::from_str(text).expect("find result JSON");
    assert_eq!(body["mode"], "literal");
    assert_eq!(body["query"], "needle_literal_marker");
    assert_eq!(body["literal_matches"]["total"], 1);
    assert_eq!(body["literal_matches"]["files_matched"], 1);
    assert_eq!(
        body["literal_matches"]["by_file"][0]["file"], "src/lib.rs",
        "literal result should point at the fixture source file"
    );
}

#[test]
fn find_regex_and_body_roundtrip_through_stdio_server() {
    let project = sample_project();
    let mut server = StdioServer::start();
    server.initialize();
    server.initialized();

    let regex_result = server.request(
        2,
        "tools/call",
        json!({
            "name": "find",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "name": "needle_[a-z]+_marker",
                "mode": "regex"
            }
        }),
    );
    let regex_text = regex_result["result"]["content"][0]["text"]
        .as_str()
        .expect("regex result text");
    let regex_body: Value = serde_json::from_str(regex_text).expect("regex result JSON");
    assert_eq!(regex_body["mode"], "regex");
    assert_eq!(regex_body["regex_matches"]["total"], 1);
    assert!(
        regex_body["regex_matches"]["coverage_line"]
            .as_str()
            .is_some_and(|line| line.contains("scanned"))
    );

    let body_result = server.request(
        3,
        "tools/call",
        json!({
            "name": "body",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "symbol": "marker"
            }
        }),
    );
    let body_text = body_result["result"]["content"][0]["text"]
        .as_str()
        .expect("body result text");
    let body: Value = serde_json::from_str(body_text).expect("body result JSON");
    assert_eq!(body["total"], 1);
    assert_eq!(body["result"]["bodies"][0]["file"], "src/lib.rs");
    assert!(
        body["result"]["bodies"][0]["source"]
            .as_str()
            .is_some_and(|source| source.contains("pub fn marker")),
        "body should return the defining source: {body}"
    );
}

#[test]
fn diff_accepts_git_refs_and_sees_live_worktree_exports() {
    let project = sample_project();
    commit_project(project.path());
    fs::write(
        project.path().join("src/lib.rs"),
        "pub fn marker() -> &'static str { \"needle_literal_marker\" }\npub fn added_after_head() {}\n",
    )
    .expect("modify lib.rs");

    let mut server = StdioServer::start();
    server.initialize();
    server.initialized();
    let result = server.request(
        2,
        "tools/call",
        json!({
            "name": "diff",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "since": "HEAD"
            }
        }),
    );
    let text = result["result"]["content"][0]["text"]
        .as_str()
        .expect("diff result text");
    let body: Value = serde_json::from_str(text).expect("diff result JSON");
    assert_eq!(body["since"], "HEAD");
    assert!(
        body["diff"]["exports"]["added"]
            .as_array()
            .is_some_and(|exports| exports
                .iter()
                .any(|item| item["name"] == "added_after_head")),
        "diff should include the dirty-worktree export: {body}"
    );
}

#[test]
fn existing_tools_match_cli_depth_and_filter_controls() {
    let project = sample_project();
    fs::create_dir_all(project.path().join("src/nested/deep")).expect("nested fixture dir");
    fs::write(
        project.path().join("src/nested/deep/helper.rs"),
        "pub fn nested_helper() {}\n",
    )
    .expect("nested fixture file");

    let mut server = StdioServer::start();
    server.initialize();
    server.initialized();

    let tree_result = server.request(
        2,
        "tools/call",
        json!({
            "name": "tree",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "files": true,
                "match": "nested/.+helper\\.rs$"
            }
        }),
    );
    let tree: Value = serde_json::from_str(
        tree_result["result"]["content"][0]["text"]
            .as_str()
            .expect("tree result text"),
    )
    .expect("tree result JSON");
    assert!(
        tree["depth"].is_null(),
        "omitted tree depth must be unlimited"
    );
    assert_eq!(tree["files"].as_array().map(Vec::len), Some(1));
    assert_eq!(tree["files"][0]["path"], "src/nested/deep/helper.rs");

    let slice_result = server.request(
        3,
        "tools/call",
        json!({
            "name": "slice",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "file": "src/lib.rs",
                "depth": 1,
                "rescan": true
            }
        }),
    );
    let slice: Value = serde_json::from_str(
        slice_result["result"]["content"][0]["text"]
            .as_str()
            .expect("slice result text"),
    )
    .expect("slice result JSON");
    assert_eq!(slice["depth"], 1);

    let impact_result = server.request(
        4,
        "tools/call",
        json!({
            "name": "impact",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "file": "src/lib.rs",
                "depth": 1
            }
        }),
    );
    let impact: Value = serde_json::from_str(
        impact_result["result"]["content"][0]["text"]
            .as_str()
            .expect("impact result text"),
    )
    .expect("impact result JSON");
    assert_eq!(impact["depth"], 1);

    let focus_result = server.request(
        5,
        "tools/call",
        json!({
            "name": "focus",
            "arguments": {
                "project": project.path().to_string_lossy(),
                "force_no_git": true,
                "directory": "src",
                "depth": 1,
                "no_consumers": true
            }
        }),
    );
    let focus: Value = serde_json::from_str(
        focus_result["result"]["content"][0]["text"]
            .as_str()
            .expect("focus result text"),
    )
    .expect("focus result JSON");
    assert_eq!(focus["depth"], 1);
    assert_eq!(focus["consumers_included"], false);
}

#[test]
fn tool_budget_contract_keeps_all_stdio_tool_outputs_under_default_budget() {
    let project = fat_project();
    let project_path = project.path().to_string_lossy().to_string();
    let mut server = StdioServer::start();
    server.initialize();
    server.initialized();

    let cases = vec![
        (
            "context",
            json!({
                "project": project_path,
                "force_no_git": true,
                "format": "markdown",
                "no_aicx": true
            }),
        ),
        (
            "repo-view",
            json!({ "project": project_path, "force_no_git": true }),
        ),
        (
            "focus",
            json!({ "project": project_path, "force_no_git": true, "directory": "src" }),
        ),
        (
            "slice",
            json!({ "project": project_path, "force_no_git": true, "file": "src/lib.rs", "consumers": true }),
        ),
        (
            "body",
            json!({ "project": project_path, "force_no_git": true, "symbol": "needle_literal_marker" }),
        ),
        (
            "find",
            json!({ "project": project_path, "force_no_git": true, "name": "fat_symbol", "mode": "symbols", "limit": 1000 }),
        ),
        (
            "impact",
            json!({ "project": project_path, "force_no_git": true, "file": "src/lib.rs" }),
        ),
        (
            "diff",
            json!({ "project": project_path, "force_no_git": true, "since": "HEAD" }),
        ),
        (
            "tree",
            json!({ "project": project_path, "force_no_git": true, "depth": 4, "loc_threshold": 1 }),
        ),
        (
            "follow",
            json!({ "project": project_path, "force_no_git": true, "scope": "all", "limit": 200 }),
        ),
        (
            "suppressions",
            json!({ "project": project_path, "force_no_git": true, "include_fixtures": true }),
        ),
        (
            "prism",
            json!({
                "project": project_path,
                "force_no_git": true,
                "no_aicx": true,
                "task": ["large symbol budget contract", "large response cap contract"],
                "limit": 50
            }),
        ),
    ];

    for (idx, (tool, arguments)) in cases.into_iter().enumerate() {
        let result = server.request(
            100 + idx as u64,
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        let text = result["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool} result text"));
        assert_budgeted_tool_text(tool, text);
    }
}

fn expected_tools() -> BTreeSet<String> {
    EXPECTED_TOOLS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

fn advertised_tool_names(init_response: &Value) -> BTreeSet<String> {
    let instructions = init_response["result"]["instructions"]
        .as_str()
        .expect("initialize response includes instructions");
    let mut names = BTreeSet::new();

    for line in instructions.lines() {
        let Some(rest) = line.trim().strip_prefix("- ") else {
            continue;
        };
        let Some(name) = rest
            .split_once('(')
            .map(|(name, _)| name)
            .or_else(|| rest.split_once(' ').map(|(name, _)| name))
        else {
            continue;
        };
        if EXPECTED_TOOLS.contains(&name) {
            names.insert(name.to_string());
        }
    }

    names
}

fn assert_compact_tool_digest(init_response: &Value) {
    let instructions = init_response["result"]["instructions"]
        .as_str()
        .expect("initialize response includes instructions");
    let digest_position = instructions
        .find(TOOL_SURFACE_DIGEST)
        .expect("instructions include compact tool digest");
    let start_position = instructions
        .find("START:")
        .expect("instructions include detailed start section");
    assert!(
        digest_position < start_position,
        "compact tool digest should precede the detailed catalogue"
    );
}

fn assert_budgeted_tool_text(tool: &str, text: &str) {
    assert!(
        text.chars().count() <= MCP_RESPONSE_BUDGET_CHARS,
        "{tool} response exceeded MCP budget: {} chars",
        text.chars().count()
    );

    let Ok(body) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if body["protocol"] == MCP_RESPONSE_BUDGET_PROTOCOL {
        assert_eq!(body["tool"], tool, "budget marker must identify the tool");
        let artifact_path = body["full_payload"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool} budget marker must include artifact path"));
        assert!(
            artifact_path.ends_with(".full.json"),
            "{tool} artifact must be a concrete .full.json sibling: {artifact_path}"
        );
        assert!(
            fs::metadata(artifact_path).is_ok(),
            "{tool} full payload artifact must exist: {artifact_path}"
        );
    }
}

fn tool_names(value: &Value) -> BTreeSet<String> {
    value["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect()
}

fn sample_project() -> TempDir {
    let tmp = TempDir::new().expect("temp dir");
    // The scan guard refuses roots outside a git checkout.
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(tmp.path())
        .status()
        .expect("git init sample project");
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"tool-surface-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    fs::write(
        tmp.path().join("src/lib.rs"),
        "pub fn marker() -> &'static str { \"needle_literal_marker\" }\n",
    )
    .expect("write lib.rs");
    tmp
}

fn commit_project(path: &std::path::Path) {
    for args in [
        vec!["config", "user.email", "tool-surface@example.invalid"],
        vec!["config", "user.name", "Tool Surface"],
        vec!["add", "."],
        vec!["commit", "-qm", "fixture base"],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(path)
            .status()
            .unwrap_or_else(|_| panic!("git {args:?}"));
        assert!(status.success(), "git {args:?} failed");
    }
}

fn fat_project() -> TempDir {
    let tmp = TempDir::new().expect("temp dir");
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"tool-budget-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::create_dir_all(tmp.path().join("src/nested/deep")).expect("src dir");

    let mut lib = String::new();
    for module in 0..90 {
        lib.push_str(&format!("pub mod module_{module:03};\n"));
    }
    for module in 0..90 {
        lib.push_str(&format!("pub use module_{module:03}::*;\n"));
    }
    lib.push_str("pub fn needle_literal_marker() -> &'static str { \"needle_literal_marker\" }\n");
    fs::write(tmp.path().join("src/lib.rs"), lib).expect("write lib.rs");

    for module in 0..90 {
        let mut body = String::new();
        body.push_str(&format!("use crate::module_{:03}::*;\n", (module + 1) % 90));
        for symbol in 0..24 {
            body.push_str(&format!(
                "pub fn fat_symbol_{module:03}_{symbol:03}() -> &'static str {{ \"fat-symbol-{module:03}-{symbol:03}-needle_literal_marker\" }}\n"
            ));
        }
        fs::write(tmp.path().join(format!("src/module_{module:03}.rs")), body)
            .expect("write module");
    }

    fs::write(
        tmp.path().join("src/nested/deep/helper.rs"),
        "pub fn nested_helper() -> &'static str { \"nested\" }\n",
    )
    .expect("write nested helper");
    tmp
}

struct StdioServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    _cache_dir: Option<tempfile::TempDir>,
}

impl StdioServer {
    fn start() -> Self {
        let temp = tempfile::tempdir().ok();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"));
        cmd.args(["--log-level", "error"]);
        if let Some(ref t) = temp {
            cmd.env("LOCT_CACHE_DIR", t.path().join("cache"));
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn loctree-mcp stdio");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        Self {
            child,
            stdin,
            stdout,
            _cache_dir: temp,
        }
    }

    fn initialize(&mut self) -> Value {
        self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "tool-surface-contract", "version": "0.0.1" }
            }),
        )
    }

    fn initialized(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn tools_list(&mut self) -> BTreeSet<String> {
        self.initialized();
        let response = self.request(2, "tools/list", json!({}));
        tool_names(&response)
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        self.read_response(id)
    }

    fn send(&mut self, value: Value) {
        writeln!(
            self.stdin,
            "{}",
            serde_json::to_string(&value).expect("serialize request")
        )
        .expect("write request");
        self.stdin.flush().expect("flush request");
    }

    fn read_response(&mut self, id: u64) -> Value {
        let mut line = String::new();
        for _ in 0..128 {
            line.clear();
            let bytes = self.stdout.read_line(&mut line).expect("read response");
            assert!(bytes > 0, "server exited before response id={id}");
            let value: Value = serde_json::from_str(&line).expect("response JSON");
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
        panic!("response id={id} not received");
    }
}

impl Drop for StdioServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpServer {
    child: Child,
    addr: SocketAddr,
    session_id: String,
    _cache_dir: Option<tempfile::TempDir>,
}

impl HttpServer {
    fn start() -> Self {
        let temp = tempfile::tempdir().ok();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"));
        cmd.args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--log-level",
            "error",
        ]);
        if let Some(ref t) = temp {
            cmd.env("LOCT_CACHE_DIR", t.path().join("cache"));
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn loctree-mcp http");
        let addr = read_announced_addr(&mut child);
        Self {
            child,
            addr,
            session_id: String::new(),
            _cache_dir: temp,
        }
    }

    fn initialize(mut self) -> InitializedHttpServer {
        let response = self.post(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "tool-surface-contract", "version": "0.0.1" }
                }
            }),
            None,
        );
        assert_eq!(response.status, 200, "body: {}", response.body);
        let session_id = response
            .header("mcp-session-id")
            .expect("mcp-session-id header")
            .to_string();
        self.session_id = session_id;
        let init = sse_json(&response.body, 1);
        InitializedHttpServer { server: self, init }
    }

    fn post(&self, value: Value, session_id: Option<&str>) -> HttpResponse {
        let body = serde_json::to_string(&value).expect("serialize request");
        let mut stream =
            TcpStream::connect_timeout(&self.addr, Duration::from_secs(2)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("read timeout");

        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.addr,
            body.len()
        )
        .expect("write headers");
        if let Some(session_id) = session_id {
            write!(stream, "Mcp-Session-Id: {session_id}\r\n").expect("write session header");
        }
        write!(stream, "\r\n{body}").expect("write body");

        let mut raw = String::new();
        stream.read_to_string(&mut raw).expect("read response");
        parse_http_response(&raw)
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct InitializedHttpServer {
    server: HttpServer,
    init: Value,
}

impl InitializedHttpServer {
    fn tools_list(&self) -> BTreeSet<String> {
        let notify = self.server.post(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            Some(&self.server.session_id),
        );
        assert_eq!(notify.status, 202, "body: {}", notify.body);

        let response = self.server.post(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            Some(&self.server.session_id),
        );
        assert_eq!(response.status, 200, "body: {}", response.body);
        tool_names(&sse_json(&response.body, 2))
    }
}

fn read_announced_addr(child: &mut Child) -> SocketAddr {
    const PREFIX: &str = "loctree-mcp http listening on ";
    const DEADLINE: Duration = Duration::from_secs(15);

    let stdout = child.stdout.take().expect("child stdout piped");
    let (tx, rx) = mpsc::channel::<Result<SocketAddr, String>>();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = tx.send(Err("server exited before announcing address".into()));
            }
            Ok(_) => {
                let Some(rest) = line.trim().strip_prefix(PREFIX) else {
                    let _ = tx.send(Err(format!("unexpected server announcement: {line:?}")));
                    return;
                };
                let _ = tx.send(rest.parse::<SocketAddr>().map_err(|e| e.to_string()));
            }
            Err(e) => {
                let _ = tx.send(Err(format!("read server stdout: {e}")));
            }
        }
    });

    match rx.recv_timeout(DEADLINE) {
        Ok(Ok(addr)) => addr,
        Ok(Err(msg)) => panic!("{msg}"),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not announce a listening address within {DEADLINE:?}");
        }
    }
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn parse_http_response(raw: &str) -> HttpResponse {
    let (head, body) = raw.split_once("\r\n\r\n").expect("http separator");
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .expect("status code");
    let headers = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    HttpResponse {
        status,
        headers,
        body: body.to_string(),
    }
}

fn sse_json(body: &str, id: u64) -> Value {
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(data).expect("SSE data JSON");
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
    panic!("SSE response id={id} not found in body: {body}");
}
