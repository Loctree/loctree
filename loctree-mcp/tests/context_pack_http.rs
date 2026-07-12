use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

struct TestServer {
    child: Child,
    addr: SocketAddr,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn context_pack_walks_all_six_cards() {
    let project = sample_project();
    let server = start_server(project.path());

    let mut cursor = None;
    let mut cards = Vec::new();
    loop {
        let path = match cursor {
            Some(ref cursor) => format!(
                "/context_pack?project={}&cursor={}",
                url_query(project.path()),
                cursor
            ),
            None => format!("/context_pack?project={}", url_query(project.path())),
        };
        let response = http_get(server.addr, &path);
        assert_eq!(response.status, 200, "body: {}", response.body);
        let json: Value = serde_json::from_str(&response.body).expect("json body");
        cards.push(json["card"].as_str().expect("card").to_string());
        cursor = json["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            assert_eq!(json["next_cursor"], Value::Null);
            break;
        }
    }

    assert_eq!(
        cards,
        vec![
            "core",
            "structural",
            "runtime",
            "memory",
            "verification",
            "risk"
        ]
    );
}

#[test]
fn context_pack_cards_filter_skips_intermediate_cards() {
    let project = sample_project();
    let server = start_server(project.path());

    let first = http_get(
        server.addr,
        &format!(
            "/context_pack?project={}&cards=core,risk",
            url_query(project.path())
        ),
    );
    assert_eq!(first.status, 200, "body: {}", first.body);
    let first_json: Value = serde_json::from_str(&first.body).expect("json body");
    assert_eq!(first_json["section"], 0);
    assert_eq!(first_json["card"], "core");
    assert_eq!(first_json["total_sections"], 2);

    let cursor = first_json["next_cursor"].as_str().expect("next cursor");
    let second = http_get(
        server.addr,
        &format!(
            "/context_pack?project={}&cursor={}",
            url_query(project.path()),
            cursor
        ),
    );
    assert_eq!(second.status, 200, "body: {}", second.body);
    let second_json: Value = serde_json::from_str(&second.body).expect("json body");
    assert_eq!(second_json["section"], 1);
    assert_eq!(second_json["card"], "risk");
    assert_eq!(second_json["next_cursor"], Value::Null);
}

#[test]
fn context_pack_returns_gone_when_atlas_fingerprint_changes_mid_cursor() {
    let project = sample_project();
    let server = start_server(project.path());

    let first = http_get(
        server.addr,
        &format!("/context_pack?project={}", url_query(project.path())),
    );
    assert_eq!(first.status, 200, "body: {}", first.body);
    let first_json: Value = serde_json::from_str(&first.body).expect("json body");
    let cursor = first_json["next_cursor"].as_str().expect("next cursor");

    let manifest_path = project.path().join(".loctree/context-atlas/manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("manifest json");
    manifest["generated_at"] = Value::String("2099-01-01T00:00:00Z".to_string());
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("manifest serialize"),
    )
    .expect("rewrite manifest");

    let response = http_get(
        server.addr,
        &format!(
            "/context_pack?project={}&cursor={}",
            url_query(project.path()),
            cursor
        ),
    );
    assert_eq!(response.status, 410, "body: {}", response.body);
}

struct HttpResponse {
    status: u16,
    body: String,
}

fn sample_project() -> TempDir {
    let tmp = TempDir::new().expect("temp dir");
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"context-pack-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    fs::write(
        tmp.path().join("src/lib.rs"),
        "pub fn alpha() -> &'static str { beta() }\nfn beta() -> &'static str { \"beta\" }\n",
    )
    .expect("write lib.rs");
    tmp
}

fn start_server(project: &Path) -> TestServer {
    let exe = env!("CARGO_BIN_EXE_loctree-mcp");
    // Bind an ephemeral port *inside* the child and let it announce the result,
    // rather than pre-reserving a port here and handing the number to the child.
    // The old reserve-then-spawn handshake (`bind(:0)` -> read `local_addr` ->
    // `drop` -> child re-binds the same number) left a window where two parallel
    // tests could reserve the same just-freed port: the loser died on
    // EADDRINUSE while the winner's server was silently shared, then killed by
    // the first test's `Drop` mid-request — surfacing as a flaky `read response`
    // reset in the *other* test. Letting the OS assign the port and the child
    // hold it from bind onward removes that shared resource entirely.
    let mut child = Command::new(exe)
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--log-level",
            "error",
        ])
        .env("LOCT_CACHE_DIR", project.join(".loctree-cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn loctree-mcp");

    let addr = read_announced_addr(&mut child);
    TestServer { child, addr }
}

/// Read the `listening on <addr>` line the server prints to stdout once it has
/// bound its socket. The announcement doubles as a readiness signal: a bound
/// socket is already accepting, so no separate connect-poll is needed.
///
/// The read is bounded by a deadline. A child that binds but then wedges before
/// announcing (deadlock, a panic that leaves stdout open, a runtime stall) would
/// otherwise block `read_line` forever — and `cargo test` has no per-test
/// timeout, so one hung child hangs the whole suite until the CI job is reaped.
/// The old reserve-then-spawn handshake carried a 15s `wait_until_ready`
/// deadline; this restores that failure bound on the race-free child-binds path.
/// The blocking `read_line` runs on a dedicated thread so this side can bound it
/// with `recv_timeout`, since std has no portable read timeout for a pipe.
fn read_announced_addr(child: &mut Child) -> SocketAddr {
    const PREFIX: &str = "loctree-mcp http listening on ";
    const DEADLINE: Duration = Duration::from_secs(15);

    let stdout = child.stdout.take().expect("child stdout piped");
    let (tx, rx) = mpsc::channel::<Result<SocketAddr, String>>();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(Err(
                        "server exited before announcing a listening address".into()
                    ));
                    return;
                }
                Ok(_) => {
                    if let Some(rest) = line.trim().strip_prefix(PREFIX) {
                        let _ = tx.send(rest.parse::<SocketAddr>().map_err(|e| {
                            format!("parse announced listening address {rest:?}: {e}")
                        }));
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("read server stdout: {e}")));
                    return;
                }
            }
        }
    });

    match rx.recv_timeout(DEADLINE) {
        Ok(Ok(addr)) => addr,
        Ok(Err(msg)) => panic!("{msg}"),
        Err(_) => {
            // Timeout or reader thread gone: kill the wedged child so its `Drop`
            // does not block, then fail fast instead of hanging the whole suite.
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not announce a listening address within {DEADLINE:?}");
        }
    }
}

fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap_or_else(|e| {
        panic!("read response from {addr} path={path}: {e}; raw_so_far={raw:?}")
    });
    parse_response(&raw)
}

fn parse_response(raw: &str) -> HttpResponse {
    let (head, body) = raw.split_once("\r\n\r\n").expect("http separator");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .expect("status code");
    HttpResponse {
        status,
        body: body.to_string(),
    }
}

fn url_query(path: &Path) -> String {
    percent_encode(path.to_string_lossy().as_ref())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
