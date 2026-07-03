use std::fs;
use std::io::{self, BufRead, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::thread;

/// Opt-out gate for `open_in_browser` auto-launch.
///
/// Set `LOCT_OPEN_BROWSER=0` (also accepts `false` / `no`, case-insensitive) to
/// suppress the OS-level `open`/`xdg-open`/`Start-Process` spawn that follows
/// report writes. Intended for non-interactive contexts: e2e tests that spawn
/// the `loct` binary against a `tempfile::TempDir`, CI pipelines, scripts.
///
/// Default behavior (env unset or any other value) preserves the human-operator
/// auto-open: a fresh browser window pops on `loct report --serve` / explicit
/// `--report-path` / `loct auto` with a configured artifact location.
pub(crate) const LOCT_OPEN_BROWSER_ENV: &str = "LOCT_OPEN_BROWSER";

static OPEN_SERVER_BASE: OnceLock<String> = OnceLock::new();

/// Returns `true` when `LOCT_OPEN_BROWSER` is set to a falsy value.
///
/// Falsy values (case-insensitive): `0`, `false`, `no`. Any other value
/// (including unset) returns `false`, preserving the default open behavior.
fn auto_open_disabled() -> bool {
    match std::env::var(LOCT_OPEN_BROWSER_ENV) {
        Ok(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no"
        ),
        Err(_) => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorKind {
    Code,
    Cursor,
    Windsurf,
    Jetbrains,
    None,
    Auto,
}

#[derive(Clone, Debug)]
pub struct EditorConfig {
    pub kind: EditorKind,
    pub command_template: Option<String>,
}

impl EditorConfig {
    pub fn from_args(kind: Option<String>, cmd_tpl: Option<String>) -> Self {
        let parsed_kind = kind
            .as_deref()
            .map(|v| match v.to_lowercase().as_str() {
                "code" | "vscode" | "vs" => EditorKind::Code,
                "cursor" => EditorKind::Cursor,
                "windsurf" => EditorKind::Windsurf,
                "jetbrains" | "jb" => EditorKind::Jetbrains,
                "none" => EditorKind::None,
                _ => EditorKind::Auto,
            })
            .unwrap_or(EditorKind::Auto);

        Self {
            kind: parsed_kind,
            command_template: cmd_tpl,
        }
    }
}

pub(crate) fn url_encode_component(input: &str) -> String {
    input
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

pub(crate) fn url_decode_component(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut iter = input.as_bytes().iter().cloned();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let hi = iter.next()?;
            let lo = iter.next()?;
            let hex = [hi, lo];
            let s = std::str::from_utf8(&hex).ok()?;
            let v = u8::from_str_radix(s, 16).ok()?;
            out.push(v as char);
        } else {
            out.push(b as char);
        }
    }
    Some(out)
}

pub(crate) fn open_in_browser(path: &Path) {
    if auto_open_disabled() {
        eprintln!(
            "[loctree] Skipping browser auto-open ({}=0): {}",
            LOCT_OPEN_BROWSER_ENV,
            path.display()
        );
        return;
    }

    let Ok(canon) = path.canonicalize() else {
        eprintln!(
            "[loctree][warn] Could not resolve report path for auto-open: {}",
            path.display()
        );
        return;
    };

    let target = canon.to_string_lossy().to_string();
    if target.bytes().any(|b| b < 0x20) {
        eprintln!(
            "[loctree][warn] Skipping auto-open for suspicious path: {}",
            target
        );
        return;
    }

    #[cfg(target_os = "macos")]
    let try_cmds = vec![("open", vec![target.as_str()])];
    #[cfg(target_os = "windows")]
    let try_cmds = vec![(
        "powershell",
        vec![
            "-NoProfile",
            "-Command",
            "Start-Process",
            "-FilePath",
            target.as_str(),
        ],
    )];
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let try_cmds = vec![("xdg-open", vec![target.as_str()])];

    for (program, args) in try_cmds {
        if Command::new(program).args(args.clone()).spawn().is_ok() {
            return;
        }
    }
    eprintln!(
        "[loctree][warn] Could not open report automatically: {}",
        target
    );
}

pub(crate) fn start_open_server(
    roots: Vec<PathBuf>,
    editor_cfg: EditorConfig,
    report_path: Option<PathBuf>,
    port_hint: Option<u16>,
) -> Option<(String, thread::JoinHandle<()>)> {
    // Prefer stable public-ish bind: 0.0.0.0:5075, fall back to loopback random port.
    let mut attempts = Vec::new();
    if let Some(p) = port_hint {
        attempts.push(format!("0.0.0.0:{p}"));
        attempts.push(format!("127.0.0.1:{p}"));
    } else {
        attempts.push("0.0.0.0:5075".to_string());
        attempts.push("127.0.0.1:0".to_string());
    }

    let (listener, _bind_addr) = attempts
        .into_iter()
        .find_map(|addr| TcpListener::bind(&addr).ok().map(|l| (l, addr)))?;

    let bound_addr = listener.local_addr().ok()?;
    let port = bound_addr.port();
    let base = if bound_addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{port}")
    } else {
        format!("http://{}:{port}", bound_addr.ip())
    };
    let _ = OPEN_SERVER_BASE.set(base.clone());

    let handle = thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = String::new();
            let mut reader = io::BufReader::new(&stream);
            if reader.read_line(&mut buf).is_ok() {
                handle_request(
                    &mut stream,
                    &roots,
                    &editor_cfg,
                    report_path.as_ref(),
                    buf.trim(),
                );
            }
        }
    });
    Some((base, handle))
}

pub(crate) fn current_open_base() -> Option<String> {
    OPEN_SERVER_BASE.get().cloned()
}

fn open_file_in_editor(full_path: &Path, line: usize, cfg: &EditorConfig) -> io::Result<()> {
    if cfg.kind == EditorKind::None {
        return Err(io::Error::other("editor disabled (--editor none)"));
    }

    let template_result = if let Some(tpl) = &cfg.command_template {
        let replaced = tpl
            .replace("{file}", full_path.to_string_lossy().as_ref())
            .replace("{line}", &line.to_string());
        let parts: Vec<String> = replaced.split_whitespace().map(|s| s.to_string()).collect();
        parts
            .split_first()
            .map(|(prog, args)| (prog.clone(), args.to_vec()))
    } else {
        None
    };

    let try_commands = |program: &str, args: &[String]| -> io::Result<bool> {
        let status = Command::new(program).args(args).status()?;
        Ok(status.success())
    };

    if let Some((prog, args)) = template_result
        && try_commands(&prog, &args)?
    {
        return Ok(());
    }

    let location_arg = format!("{}:{}", full_path.to_string_lossy(), line.max(1));
    let mut tried = false;

    let mut attempt_editor = |binary: &str| -> io::Result<bool> {
        tried = true;
        try_commands(binary, &[String::from("-g"), location_arg.clone()])
    };

    match cfg.kind {
        EditorKind::Code => {
            if attempt_editor("code")? {
                return Ok(());
            }
        }
        EditorKind::Cursor => {
            if attempt_editor("cursor")? {
                return Ok(());
            }
        }
        EditorKind::Windsurf => {
            if attempt_editor("windsurf")? {
                return Ok(());
            }
        }
        EditorKind::Jetbrains => {
            let url = format!(
                "jetbrains://idea/navigate/reference?path={}&line={}&column=1",
                url_encode_component(full_path.to_string_lossy().as_ref()),
                line.max(1)
            );
            let launcher = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            if try_commands(launcher, &[url])? {
                return Ok(());
            }
            tried = true;
        }
        EditorKind::Auto | EditorKind::None => {}
    }

    if cfg.kind == EditorKind::Auto {
        // Try common binaries in order.
        for bin in ["code", "cursor", "windsurf"] {
            if try_commands(bin, &[String::from("-g"), location_arg.clone()])? {
                return Ok(());
            }
        }
        // JetBrains URI
        let url = format!(
            "jetbrains://idea/navigate/reference?path={}&line={}&column=1",
            url_encode_component(full_path.to_string_lossy().as_ref()),
            line.max(1)
        );
        let launcher = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if try_commands(launcher, &[url])? {
            return Ok(());
        }
    }

    #[cfg(target_os = "macos")]
    let fallback = Command::new("open")
        .arg(full_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    #[cfg(target_os = "windows")]
    let fallback = Command::new("cmd")
        .args(["/C", "start", full_path.to_string_lossy().as_ref()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let fallback = Command::new("xdg-open")
        .arg(full_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if fallback {
        Ok(())
    } else if tried {
        Err(io::Error::other("could not open file via editor"))
    } else {
        Err(io::Error::other(
            "no editor command succeeded (try --editor-cmd)",
        ))
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    include_body: bool,
) {
    let header = format!(
        "{status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if include_body {
        let _ = stream.write_all(body);
    }
}

fn handle_open_request(
    stream: &mut TcpStream,
    roots: &[PathBuf],
    editor_cfg: &EditorConfig,
    target: &str,
    head_only: bool,
) -> bool {
    if !target.starts_with("/open?") {
        return false;
    }

    let query = &target[6..];
    let mut file = None;
    let mut line = 1usize;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "f" => file = url_decode_component(v),
                "l" => {
                    line = v.parse::<usize>().unwrap_or(1).max(1);
                }
                _ => {}
            }
        }
    }
    let Some(rel_or_abs) = file else {
        write_response(
            stream,
            "HTTP/1.1 400 Bad Request",
            "text/plain",
            b"missing file",
            true,
        );
        return true;
    };

    let mut candidate = None;
    let path_obj = PathBuf::from(&rel_or_abs);
    if path_obj.is_absolute() {
        if let Ok(canon) = path_obj.canonicalize()
            && roots.iter().any(|r| canon.starts_with(r))
        {
            candidate = Some(canon);
        }
    } else {
        for root in roots {
            let joined = root.join(&path_obj);
            if let Ok(canon) = joined.canonicalize()
                && canon.starts_with(root)
            {
                candidate = Some(canon);
                break;
            }
        }
    }

    let Some(full) = candidate else {
        write_response(
            stream,
            "HTTP/1.1 404 Not Found",
            "text/plain",
            b"not found",
            true,
        );
        return true;
    };

    let status = open_file_in_editor(&full, line, editor_cfg);
    let (status_line, body) = if status.is_ok() {
        ("HTTP/1.1 200 OK", b"opened".as_slice())
    } else {
        (
            "HTTP/1.1 500 Internal Server Error",
            b"failed to open in editor".as_slice(),
        )
    };
    write_response(stream, status_line, "text/plain", body, !head_only);
    true
}

fn serve_report(
    stream: &mut TcpStream,
    req_path: &str,
    report_path: &Path,
    head_only: bool,
) -> bool {
    let (path_only, _) = req_path.split_once('?').unwrap_or((req_path, ""));
    let target = path_only.trim_start_matches('/');

    let base_dir = report_path.parent().unwrap_or(Path::new("."));
    let base_canon = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());

    let requested_path = if target.is_empty() {
        report_path.to_path_buf()
    } else {
        let decoded = url_decode_component(target).unwrap_or_else(|| target.to_string());
        base_dir.join(decoded)
    };

    let Ok(canon) = requested_path.canonicalize() else {
        return false;
    };

    if !canon.starts_with(&base_canon) {
        write_response(
            stream,
            "HTTP/1.1 403 Forbidden",
            "text/plain",
            b"forbidden",
            true,
        );
        return true;
    }

    if !canon.is_file() {
        return false;
    }

    let content_type = match canon.extension().and_then(|e| e.to_str()) {
        Some("js") => "application/javascript; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    };

    match fs::read(&canon) {
        Ok(bytes) => {
            write_response(stream, "HTTP/1.1 200 OK", content_type, &bytes, !head_only);
            true
        }
        Err(_) => false,
    }
}

fn handle_request(
    stream: &mut TcpStream,
    roots: &[PathBuf],
    editor_cfg: &EditorConfig,
    report_path: Option<&PathBuf>,
    request_line: &str,
) {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let is_head = method.eq_ignore_ascii_case("head");

    if !(method.eq_ignore_ascii_case("get") || is_head) {
        write_response(
            stream,
            "HTTP/1.1 405 Method Not Allowed",
            "text/plain",
            b"method not allowed",
            true,
        );
        return;
    }

    if handle_open_request(stream, roots, editor_cfg, target, is_head) {
        return;
    }

    if let Some(report) = report_path
        && serve_report(stream, target, report, is_head)
    {
        return;
    }

    write_response(
        stream,
        "HTTP/1.1 404 Not Found",
        "text/plain",
        b"not found",
        !is_head,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode_simple() {
        assert_eq!(url_encode_component("hello"), "hello");
        assert_eq!(url_encode_component("Hello123"), "Hello123");
    }

    #[test]
    fn test_url_encode_special_chars() {
        assert_eq!(url_encode_component("hello world"), "hello%20world");
        assert_eq!(url_encode_component("path/to/file"), "path%2Fto%2Ffile");
        assert_eq!(url_encode_component("a=b&c=d"), "a%3Db%26c%3Dd");
    }

    #[test]
    fn test_url_encode_unicode() {
        let encoded = url_encode_component("żółć");
        assert!(encoded.contains('%'));
    }

    #[test]
    fn test_url_encode_allowed_chars() {
        // These should NOT be encoded
        assert_eq!(url_encode_component("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn test_url_decode_simple() {
        assert_eq!(url_decode_component("hello"), Some("hello".to_string()));
    }

    #[test]
    fn test_url_decode_encoded() {
        assert_eq!(
            url_decode_component("hello%20world"),
            Some("hello world".to_string())
        );
        assert_eq!(
            url_decode_component("path%2Fto%2Ffile"),
            Some("path/to/file".to_string())
        );
    }

    #[test]
    fn test_url_decode_invalid() {
        // Incomplete percent encoding
        assert_eq!(url_decode_component("hello%2"), None);
        assert_eq!(url_decode_component("hello%"), None);
        // Invalid hex
        assert_eq!(url_decode_component("hello%GG"), None);
    }

    #[test]
    fn test_url_roundtrip() {
        let original = "path/to/file with spaces.ts";
        let encoded = url_encode_component(original);
        let decoded = url_decode_component(&encoded);
        assert_eq!(decoded, Some(original.to_string()));
    }

    #[test]
    fn test_editor_config_from_args_defaults() {
        let cfg = EditorConfig::from_args(None, None);
        assert_eq!(cfg.kind, EditorKind::Auto);
        assert!(cfg.command_template.is_none());
    }

    #[test]
    fn test_editor_config_from_args_code() {
        let cfg = EditorConfig::from_args(Some("code".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::Code);

        let cfg2 = EditorConfig::from_args(Some("vscode".to_string()), None);
        assert_eq!(cfg2.kind, EditorKind::Code);

        let cfg3 = EditorConfig::from_args(Some("vs".to_string()), None);
        assert_eq!(cfg3.kind, EditorKind::Code);
    }

    #[test]
    fn test_editor_config_from_args_cursor() {
        let cfg = EditorConfig::from_args(Some("cursor".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::Cursor);
    }

    #[test]
    fn test_editor_config_from_args_windsurf() {
        let cfg = EditorConfig::from_args(Some("windsurf".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::Windsurf);
    }

    #[test]
    fn test_editor_config_from_args_jetbrains() {
        let cfg = EditorConfig::from_args(Some("jetbrains".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::Jetbrains);

        let cfg2 = EditorConfig::from_args(Some("jb".to_string()), None);
        assert_eq!(cfg2.kind, EditorKind::Jetbrains);
    }

    #[test]
    fn test_editor_config_from_args_none() {
        let cfg = EditorConfig::from_args(Some("none".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::None);
    }

    #[test]
    fn test_editor_config_from_args_case_insensitive() {
        let cfg = EditorConfig::from_args(Some("CODE".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::Code);

        let cfg2 = EditorConfig::from_args(Some("JetBrains".to_string()), None);
        assert_eq!(cfg2.kind, EditorKind::Jetbrains);
    }

    #[test]
    fn test_editor_config_from_args_unknown() {
        let cfg = EditorConfig::from_args(Some("unknown_editor".to_string()), None);
        assert_eq!(cfg.kind, EditorKind::Auto);
    }

    #[test]
    fn test_editor_config_with_template() {
        let template = Some("myeditor {file}:{line}".to_string());
        let cfg = EditorConfig::from_args(None, template.clone());
        assert_eq!(cfg.command_template, template);
    }

    #[test]
    fn test_editor_kind_equality() {
        assert_eq!(EditorKind::Code, EditorKind::Code);
        assert_ne!(EditorKind::Code, EditorKind::Cursor);
    }

    #[test]
    fn test_url_decode_empty() {
        assert_eq!(url_decode_component(""), Some("".to_string()));
    }

    #[test]
    fn test_url_encode_empty() {
        assert_eq!(url_encode_component(""), "");
    }

    /// Serial mutex for env-var tests — `std::env::set_var` is process-global
    /// and racing with parallel test threads would flake.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn auto_open_disabled_respects_falsy_values() {
        let _guard = env_lock();
        let prev = std::env::var(LOCT_OPEN_BROWSER_ENV).ok();

        for raw in ["0", "false", "FALSE", "False", "no", "NO", " 0 ", "false "] {
            // SAFETY: env mutation guarded by env_lock() for serial access.
            unsafe { std::env::set_var(LOCT_OPEN_BROWSER_ENV, raw) };
            assert!(
                auto_open_disabled(),
                "expected {raw:?} to disable browser auto-open"
            );
        }

        // Truthy / unrecognized values keep the default open behavior.
        for raw in ["1", "true", "yes", "on", "", "anything-else"] {
            // SAFETY: env mutation guarded by env_lock() for serial access.
            unsafe { std::env::set_var(LOCT_OPEN_BROWSER_ENV, raw) };
            assert!(
                !auto_open_disabled(),
                "expected {raw:?} to leave auto-open enabled"
            );
        }

        // SAFETY: env mutation guarded by env_lock() for serial access.
        unsafe { std::env::remove_var(LOCT_OPEN_BROWSER_ENV) };
        assert!(
            !auto_open_disabled(),
            "unset env must preserve default (auto-open enabled)"
        );

        // Restore prior value if any so neighbouring tests are unaffected.
        if let Some(prev) = prev {
            // SAFETY: env mutation guarded by env_lock() for serial access.
            unsafe { std::env::set_var(LOCT_OPEN_BROWSER_ENV, prev) };
        }
    }
}
