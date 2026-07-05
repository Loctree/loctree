//! Stdio-MCP transport for the AICX overlay.
//!
//! The public [`super::AicxClient`] stays synchronous because Loctree's context
//! composer is synchronous. This module owns a small Tokio runtime and a
//! persistent rmcp client session to `aicx-mcp --transport stdio`, then maps MCP
//! tool output back into the same typed records produced by the shell transport.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::runtime::{Builder, Runtime};

use super::{
    AicxIntent, AicxSearchResult, AicxSteerResult, SteerFilters,
    shell::{self},
};

/// Transport mode selector parsed by [`super::AicxClient::new`].
pub const AICX_MODE_ENV: &str = "LOCT_AICX_MODE";

/// Optional override for the stdio MCP server binary.
pub const AICX_MCP_BINARY_ENV: &str = "AICX_MCP_BINARY";

type McpService = RunningService<RoleClient, ()>;

pub(super) struct AicxMcpClient {
    runtime: Arc<Runtime>,
    service: Mutex<McpService>,
}

impl std::fmt::Debug for AicxMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AicxMcpClient").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) enum AicxMcpError {
    Runtime(std::io::Error),
    Spawn(std::io::Error),
    Init(String),
    Timeout(&'static str),
    Service(String),
    Tool(String),
    Parse(String),
    Poisoned,
}

impl std::fmt::Display for AicxMcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(error) => write!(f, "runtime init failed: {error}"),
            Self::Spawn(error) => write!(f, "spawn failed: {error}"),
            Self::Init(error) => write!(f, "initialize failed: {error}"),
            Self::Timeout(tool) => write!(f, "{tool} timed out after {:?}", mcp_call_timeout()),
            Self::Service(error) => write!(f, "service error: {error}"),
            Self::Tool(error) => write!(f, "tool error: {error}"),
            Self::Parse(error) => write!(f, "parse error: {error}"),
            Self::Poisoned => write!(f, "MCP service mutex poisoned"),
        }
    }
}

impl AicxMcpClient {
    /// `timeout_cap` tightens (never widens) the per-operation timeout for
    /// connect + handshake. `None` keeps the [`mcp_call_timeout`] default —
    /// budgeted callers (the bare-context auto-overlay) pass their
    /// remaining wall-clock budget so session start is never hostage to a
    /// slow `aicx-mcp` boot. The cap spans BOTH phases: the handshake
    /// consumes from it and the health check only gets what is left —
    /// otherwise a caller's N-ms budget could legally burn 2×N here.
    pub(super) fn connect_and_check(timeout_cap: Option<Duration>) -> Result<Self, AicxMcpError> {
        let started = Instant::now();
        let runtime = shared_runtime()?;

        let service = run_blocking_on(&runtime, async {
            let mut command = tokio::process::Command::new(aicx_mcp_binary());
            command.arg("--transport").arg("stdio");
            let (transport, _stderr) = TokioChildProcess::builder(command)
                .stderr(Stdio::null())
                .spawn()
                .map_err(AicxMcpError::Spawn)?;
            tokio::time::timeout(effective_timeout(timeout_cap), ().serve(transport))
                .await
                .map_err(|_| AicxMcpError::Init("timeout".to_string()))?
                .map_err(|error| AicxMcpError::Init(error.to_string()))
        })?;

        let client = Self {
            runtime,
            service: Mutex::new(service),
        };
        // A zero remainder makes the tokio timeout fire immediately, which
        // maps to the same timeout error a slow handshake would produce.
        let health_cap = timeout_cap.map(|cap| cap.saturating_sub(started.elapsed()));
        client.health_check(health_cap)?;
        Ok(client)
    }

    fn health_check(&self, timeout_cap: Option<Duration>) -> Result<(), AicxMcpError> {
        let service = self.service.lock().map_err(|_| AicxMcpError::Poisoned)?;
        let result = run_blocking_on(&self.runtime, async {
            tokio::time::timeout(effective_timeout(timeout_cap), service.list_all_tools()).await
        });
        let tools = match result {
            Ok(Ok(tools)) => tools,
            Ok(Err(error)) => return Err(AicxMcpError::Service(error.to_string())),
            Err(_) => return Err(AicxMcpError::Timeout("tools/list")),
        };

        let has_aicx_tool = tools.iter().any(|tool| {
            matches!(
                tool.name.as_ref(),
                "health" | "aicx_intents" | "aicx_search" | "aicx_steer"
            )
        });
        if has_aicx_tool {
            Ok(())
        } else {
            Err(AicxMcpError::Tool(
                "aicx-mcp did not expose expected tools".to_string(),
            ))
        }
    }

    pub(super) fn intents(
        &self,
        project: Option<&str>,
        hours: u64,
        limit: usize,
        timeout_cap: Option<Duration>,
    ) -> Result<Vec<AicxIntent>, AicxMcpError> {
        // `aicx_intents` MCP tool models project as `Option<String>` — None
        // = null on the wire = AICX-side "all projects". `json_object`
        // already filters `Value::Null` entries, so a None project is
        // dropped from the params object entirely.
        let text = self.call_json_tool(
            "aicx_intents",
            json_object([
                ("project", project.map(|p| json!(p)).unwrap_or(Value::Null)),
                ("hours", json!(hours)),
                ("limit", json!(limit)),
                ("emit", json!("json")),
            ]),
            timeout_cap,
        )?;
        Ok(shell::parse_intents(&text))
    }

    /// Search AICX with `projects` carrying the wrapper's scope.
    ///
    /// `projects.len() == 0` → AICX-side "all projects" (we omit both
    /// `project` and `projects` from the params); `len() == 1` → AICX's
    /// historical `project: <name>` shape; `len() > 1` → AICX's
    /// multi-project `projects: [...]` shape introduced by the
    /// "Extend semantic project scopes across public surfaces" cut. The
    /// AICX server fans out across the listed buckets and merges results.
    pub(super) fn search(
        &self,
        projects: &[String],
        query: &str,
        hours: u64,
        limit: usize,
        timeout_cap: Option<Duration>,
    ) -> Result<Vec<AicxSearchResult>, AicxMcpError> {
        let project_value: Value = match projects {
            [] => Value::Null,
            [single] => json!(single),
            _ => Value::Null,
        };
        let projects_value: Value = if projects.len() > 1 {
            json!(projects)
        } else {
            Value::Null
        };
        let text = self.call_json_tool(
            "aicx_search",
            json_object([
                ("query", json!(query)),
                ("project", project_value),
                ("projects", projects_value),
                ("hours", json!(hours)),
                ("limit", json!(limit)),
            ]),
            timeout_cap,
        )?;
        Ok(shell::parse_search(&text))
    }

    pub(super) fn steer(
        &self,
        project: Option<&str>,
        filters: &SteerFilters,
        timeout_cap: Option<Duration>,
    ) -> Result<Vec<AicxSteerResult>, AicxMcpError> {
        let mut args = json_object([
            ("project", project.map(|p| json!(p)).unwrap_or(Value::Null)),
            ("run_id", json!(filters.run_id)),
            ("prompt_id", json!(filters.prompt_id)),
            ("kind", json!(filters.kind)),
            ("agent", json!(filters.agent)),
            ("date", json!(filters.date)),
            ("frame_kind", json!(filters.frame_kind)),
        ]);
        if let Some(limit) = filters.limit {
            args.insert("limit".to_string(), json!(limit));
        }

        let text = self.call_json_tool("aicx_steer", args, timeout_cap)?;
        Ok(parse_mcp_steer(&text))
    }

    fn call_json_tool(
        &self,
        name: &'static str,
        arguments: JsonObject,
        timeout_cap: Option<Duration>,
    ) -> Result<String, AicxMcpError> {
        let service = self.service.lock().map_err(|_| AicxMcpError::Poisoned)?;
        let params = CallToolRequestParams::new(name).with_arguments(arguments);

        let result = run_blocking_on(&self.runtime, async {
            tokio::time::timeout(effective_timeout(timeout_cap), service.call_tool(params)).await
        });
        let tool_result = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => return Err(AicxMcpError::Service(error.to_string())),
            Err(_) => return Err(AicxMcpError::Timeout(name)),
        };
        tool_result_to_json_text(tool_result)
    }
}

fn aicx_mcp_binary() -> PathBuf {
    std::env::var(AICX_MCP_BINARY_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("aicx-mcp"))
}

fn mcp_call_timeout() -> Duration {
    std::env::var(shell::AICX_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(shell::DEFAULT_TIMEOUT)
}

/// Resolve the effective timeout for one MCP operation: the env-tunable
/// default, tightened (never widened) by the caller's remaining budget.
fn effective_timeout(timeout_cap: Option<Duration>) -> Duration {
    let base = mcp_call_timeout();
    timeout_cap.map(|cap| cap.min(base)).unwrap_or(base)
}

/// Process-wide shared Tokio runtime for the AICX MCP transport.
///
/// All [`AicxMcpClient`] instances share a single multi-thread runtime so that
/// long-running processes that re-instantiate the client (tests, repeated
/// `loct context` calls) do not leak threads. Initialised lazily on first use.
fn shared_runtime() -> Result<Arc<Runtime>, AicxMcpError> {
    static RUNTIME: OnceLock<Result<Arc<Runtime>, String>> = OnceLock::new();

    let result = RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .map(Arc::new)
            .map_err(|error| error.to_string())
    });

    result
        .as_ref()
        .map(Arc::clone)
        .map_err(|error| AicxMcpError::Runtime(std::io::Error::other(error.clone())))
}

/// Drive `future` to completion on `runtime`, regardless of whether the caller
/// is already inside another Tokio runtime.
///
/// Plain `runtime.block_on(...)` panics with "Cannot start a runtime from
/// within a runtime" when invoked from within an async context. Callers like
/// `loctree-mcp` (which embeds `AicxClient` inside `#[tokio::main]`) hit
/// exactly that path. When an ambient runtime is detected we suspend the
/// current worker via `block_in_place` and drive the future on our owned
/// runtime so the rmcp service's I/O drivers stay attached to the runtime
/// that created them.
fn run_blocking_on<F, T>(runtime: &Runtime, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime.block_on(future))
    } else {
        runtime.block_on(future)
    }
}

fn json_object(items: impl IntoIterator<Item = (&'static str, Value)>) -> JsonObject {
    items
        .into_iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn tool_result_to_json_text(result: CallToolResult) -> Result<String, AicxMcpError> {
    if result.is_error.unwrap_or(false) {
        return Err(AicxMcpError::Tool(
            serde_json::to_string(&result).unwrap_or_else(|_| "tool returned error".to_string()),
        ));
    }

    let value: Value = result
        .into_typed()
        .map_err(|error| AicxMcpError::Parse(error.to_string()))?;
    Ok(match value {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

#[derive(Debug, Deserialize)]
struct McpSteerEnvelope {
    #[serde(default)]
    items: Vec<McpSteerItem>,
}

#[derive(Debug, Deserialize)]
struct McpSteerItem {
    project: String,
    agent: String,
    date: String,
    kind: String,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    path: String,
}

fn parse_mcp_steer(text: &str) -> Vec<AicxSteerResult> {
    if let Ok(envelope) = serde_json::from_str::<McpSteerEnvelope>(text.trim()) {
        return envelope
            .items
            .into_iter()
            .map(|item| AicxSteerResult {
                project: item.project,
                agent: item.agent,
                date: item.date,
                kind: item.kind,
                run_id: item.run_id,
                prompt_id: item.prompt_id,
                model: item.model,
                source_chunk_path: item.path,
            })
            .collect();
    }

    shell::parse_steer(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_steer_json_maps_to_cli_shape() {
        let payload = r#"{
            "results": 1,
            "items": [
                {
                    "project": "Loctree/loctree-suite",
                    "agent": "codex",
                    "date": "2026-04-28",
                    "kind": "reports",
                    "run_id": "impl-1",
                    "prompt_id": "cut5",
                    "model": "gpt",
                    "path": "/tmp/chunk.md"
                }
            ]
        }"#;
        let parsed = parse_mcp_steer(payload);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].project, "Loctree/loctree-suite");
        assert_eq!(parsed[0].run_id.as_deref(), Some("impl-1"));
        assert_eq!(parsed[0].source_chunk_path, "/tmp/chunk.md");
    }

    #[test]
    fn mcp_call_timeout_honors_aicx_timeout_env() {
        let old_value = std::env::var(shell::AICX_TIMEOUT_ENV).ok();
        unsafe {
            std::env::set_var(shell::AICX_TIMEOUT_ENV, "60");
        }
        assert_eq!(mcp_call_timeout(), Duration::from_secs(60));

        match old_value {
            Some(value) => unsafe {
                std::env::set_var(shell::AICX_TIMEOUT_ENV, value);
            },
            None => unsafe {
                std::env::remove_var(shell::AICX_TIMEOUT_ENV);
            },
        }
    }
}
