# Vetcoders HTTP MCP Services Runbook

**Version:** 1.0.0 (August 2026)
**Status:** Canonical Multi-Agent & Multi-Machine Architecture
**Scope:** `aicx-mcp` (Memory & Intent Search) + `loctree-mcp` / `loct watch --http` (Structural Code Perception)

---

## 1. Why Streamable HTTP instead of Stdio?

In multi-agent environments (fleet of 10–50 agents: Codex, Claude, Gemini, Antigravity, subagents):

| Feature | Stdio Mode | Streamable HTTP Mode (Recommended) |
| :--- | :--- | :--- |
| **Number of processes** | 1 process per agent (50 agents = 50 processes in `ps aux`) | **1 central daemon per host** |
| **RAM usage** | Memory duplication for each process (50 × 50MB+) | **Shared RAM (~30–60MB per process)** |
| **Cache management** | Disk contention over `.cache` files | **Single in-memory `Arc<Snapshot>` in RAM** |
| **Query speed** | Binary startup + cold read per call | **< 1–5 ms (instant RAM graph lookup)** |
| **Remote access** | Only locally on the working machine | **Network / Tailscale access (`dragon`, `div0`, `sztudio`)** |
| **Scalability** | Overwhelms CPU and file descriptor limits | **Tokio + Axum easily handles thousands of concurrent requests** |

---

## 2. Service 1: `aicx-mcp` (Memory & Intent Search Engine)

The installed service binds to `127.0.0.1` by default. Tailnet exposure is an
explicit operator choice:

```bash
AICX_MCP_HOST="$(tailscale ip -4)" \
  AICX_MCP_ALLOWED_HOSTS="$(hostname -s),$(tailscale ip -4),localhost,127.0.0.1" \
  make install-service
```

Keep client URLs on loopback unless the client actually runs on another trusted
Tailnet machine. Never commit the generated token value; use the token file or a
client-specific environment/header mechanism.

### 2.1. Live Refresh Behavior
* **Is `aicx serve` live?** **Yes.** `aicx serve` does not freeze a stale in-memory copy of the index. Every tool invocation (`aicx_search`, `aicx_steer`, `aicx_intents`) reads the live Tantivy and vector store state from disk (`~/.aicx/`).
* **Ingestion cadence:** HTTP mode owns a bounded async refresh loop: every 5 minutes it refreshes the hot 48-hour catalog window and incrementally publishes the lexical index. Blocking filesystem/index work runs outside Tokio request workers. Use `--refresh-interval-seconds` to tune it or `--no-auto-refresh` only when another explicit writer owns freshness.

### 2.2. Installation & Makefile Targets
* **Standard install (with wizard):**
  ```bash
  cd /Volumes/vc-workspace/Loctree/aicx
  make install
  ```
  *(Runs `install.sh`, installs binaries, sets up MCP clients, and registers the launchd MCP HTTP service. That server owns index refresh).*

* **Explicit service management:**
  ```bash
  make install-service    # Installs and starts com.loctree.aicx.mcp LaunchAgent
  make uninstall-service  # Stops and unregisters com.loctree.aicx.mcp LaunchAgent
  make install-schedule   # Legacy: standalone refresh only when no HTTP server is installed
  ```

### 2.3. Smoke Test
```bash
# 1. Healthcheck
curl -s http://127.0.0.1:8044/health && echo " (Health: OK)"

# 2. Handshake MCP
curl -s \
  -H "Authorization: Bearer $(cat ~/.aicx/auth-token)" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:8044/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'
```

---

## 3. Service 2: `loctree-mcp` (Universal Code Perception)

### 3.0. Authentication and bind policy (read this before exposing anything)

`loctree-mcp --transport http` serves 12 filesystem-reading MCP tools plus
`/context_pack`. Both routes read whatever project directory the caller names,
so the listener is a code-read surface, not a health endpoint.

The server picks its posture from the bind address, at startup, **before the
socket is created**:

| `--bind` | tokens configured | `--allow-unauthenticated` | behaviour |
| :--- | :--- | :--- | :--- |
| loopback (`127.0.0.0/8`, `::1`) | no | — | starts open — the zero-config local dev path |
| loopback | yes | — | bearer auth enforced |
| non-loopback (incl. `0.0.0.0`, `::`) | yes | — | bearer auth enforced |
| non-loopback | no | yes | starts open, logs a loud `SECURITY:` warning |
| **non-loopback** | **no** | **no** | **refuses to start; no port is opened** |

A bind string that cannot be resolved is treated as non-loopback (fail-safe).
`--allow-unauthenticated` is ignored when tokens exist: configured auth is never
silently downgraded.

The refusal looks like this:

```console
$ loctree-mcp --transport http --bind 0.0.0.0:5174
[loctree-mcp] Error: refusing to start: --bind 0.0.0.0:5174 is not loopback and no bearer tokens are configured.
...
Pick one:
  1. Mint a token, then restart: loctree-mcp token create --id <name> --scope context-read
  2. Export one shared token: LOCTREE_MCP_AUTH_TOKEN=<secret>
  3. Keep it local: --bind 127.0.0.1:5174
  4. Accept the risk out loud: --allow-unauthenticated (or LOCTREE_MCP_ALLOW_UNAUTHENTICATED=1)
```

#### Minting a token

```bash
# Default store: ~/.rmcp-servers/loctree-mcp/tokens.json (argon2id-hashed at rest)
loctree-mcp token create --id dragon-tailnet --scope context-read

# id:         dragon-tailnet
# store:      ~/.rmcp-servers/loctree-mcp/tokens.json
# scopes:     context-read
# namespaces: *
# expires:    never
# token:      loct_2f1c…            <- shown once, never recoverable

loctree-mcp token list
loctree-mcp token rotate --id dragon-tailnet
loctree-mcp token revoke --id dragon-tailnet
```

Useful flags: `--expires-in-days N`, repeatable `--scope` / `--namespace`,
`--token-store PATH` (or `LOCTREE_MCP_TOKEN_STORE`). All 12 MCP tools are
read-only, so `context-read` is the whole surface; `tool-execute` / `cli-full` /
`admin` are reserved for a future write side.

#### Environment variables

| Variable | Effect |
| :--- | :--- |
| `LOCTREE_MCP_TOKEN_STORE` | token store path (same as `--token-store`) |
| `LOCTREE_MCP_AUTH_TOKEN` | single shared token, no store file needed; maps to a wildcard-admin entry and logs a deprecation warning |
| `LOCTREE_MCP_ALLOW_UNAUTHENTICATED` | `1`/`true`/`yes`/`on` — same as `--allow-unauthenticated` |
| `LOCTREE_MCP_ALLOWED_ROOTS` | pre-existing, orthogonal: restricts which project roots any caller may name |

`tools/install-mcp-service.sh` forwards all three auth variables into the
LaunchAgent's `EnvironmentVariables` (launchd does not inherit your shell), and
warns when `LOCTREE_MCP_BIND` is not loopback.

#### What is NOT protected

Be blunt about the boundary:

* **No TLS.** The process speaks plain HTTP. Tokens on a non-loopback bind travel
  in cleartext unless something in front terminates TLS. Remote exposure is a
  reverse-proxy (`caddy`/`nginx`) or tailnet responsibility — not this binary's.
* **No namespace enforcement.** Tokens carry a `namespaces` ACL and the store
  honours it, but the HTTP boundary authenticates and checks scope only. `/mcp`
  carries the project inside a JSON-RPC body the middleware does not parse, so
  half-enforcing on `/context_pack` alone would be worse than not enforcing.
  Treat every valid token as reaching every project the server process can read.
  Use `LOCTREE_MCP_ALLOWED_ROOTS` if you need a hard boundary.
* **No rate limiting, no audit log, no per-request authorization.** Denials are
  logged; successful reads are not.
* **stdio transport has no auth and needs none** — it is a child process of the
  client, with the client's own privileges.
* **Loopback is still trust-by-locality.** Any local process or user on the host
  can call an unauthenticated loopback server.

### 3.1. Two Models of Operation

#### Mode A: Universal Central Server (Shared across all repositories)
One server on port `5174` handles all repositories. Every tool call (`slice`, `impact`, `find`, `focus`, `repo-view`, `tree`) accepts the `project: "/path/to/repo"` parameter.
* **Capacity:** Runs with `--snapshot-cache-capacity 20` to keep up to 20 repository snapshots in RAM concurrently.

#### Mode B: Dedicated per-repo Watcher (`loct watch --http`)
In the active project directory:
```bash
loct watch --http --port 5174 &
```
* Watches files for modifications and recalculates graph snapshots on save.
* Supervises the child `loctree-mcp` process on `127.0.0.1:5174/mcp`.

### 3.2. Installation & Makefile Targets
* **Standard install in `loctree-suite`:**
  ```bash
  cd /Volumes/vc-workspace/Loctree/loctree-suite
  make install-all
  ```
  *(Compiles `loct`, `loctree`, `loctree-mcp`, `loctree-lsp`, codesigns, and registers the `com.loctree.loctree.mcp` launchd service).*

* **Explicit service management:**
  ```bash
  make install-service    # Installs and starts com.loctree.loctree.mcp LaunchAgent
  make uninstall-service  # Stops and unregisters com.loctree.loctree.mcp LaunchAgent
  ```

* **Tailnet exposure (explicit, authenticated):**
  ```bash
  loctree-mcp token create --id dragon-tailnet --scope context-read   # copy the token
  LOCTREE_MCP_BIND="$(tailscale ip -4):5174" make install-service
  ```
  Without a token in the store this install produces a service that refuses to
  start — by design. Check `~/.loctree/logs/loctree-serve-http.log`.

### 3.3. Smoke Test
```bash
# Loopback, no tokens configured — no header needed.
curl -s \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:5174/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'

# Anywhere auth is configured — /mcp and /context_pack both require the header.
curl -s \
  -H "Authorization: Bearer $LOCTREE_MCP_TOKEN" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:5174/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'

# Expect 401 with `WWW-Authenticate: Bearer` when the header is missing:
curl -s -o /dev/null -w '%{http_code}\n' \
  "http://127.0.0.1:5174/context_pack?project=/path/to/repo"
```
---

## 4. Client Configuration Matrix

The `Authorization` headers below are required whenever the server has tokens
configured (always, for any non-loopback bind). On a loopback server with no
tokens they are simply ignored, so the same client config works in both modes.

### 4.1. Antigravity / Gemini IDE (`~/.gemini/config/mcp_config.json`)

```json
{
  "mcpServers": {
    "aicx-http": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:8044/mcp",
      "headers": {
        "Authorization": "Bearer <AICX_LOCAL_TOKEN>"
      }
    },
    "loctree-http": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:5174/mcp",
      "headers": {
        "Authorization": "Bearer <LOCTREE_MCP_TOKEN>"
      }
    }
  }
}
```

### 4.2. Claude Code (`~/.claude/mcp.json` or project `.mcp.json`)

```json
{
  "mcpServers": {
    "aicx": {
      "url": "http://127.0.0.1:8044/mcp",
      "headers": {
        "Authorization": "Bearer <AICX_LOCAL_TOKEN>"
      }
    },
    "loctree": {
      "url": "http://127.0.0.1:5174/mcp",
      "headers": {
        "Authorization": "Bearer <LOCTREE_MCP_TOKEN>"
      }
    }
  }
}
```

### 4.3. Codex (`~/.codex/config.toml`)

```toml
[mcp_servers.aicx]
url = "http://127.0.0.1:8044/mcp"
bearer_token_env_var = "AICX_MCP_TOKEN"

[mcp_servers.aicx.tools.aicx_search]
approval_mode = "approve"

[mcp_servers.aicx.tools.aicx_steer]
approval_mode = "approve"

[mcp_servers.aicx.tools.aicx_rank]
approval_mode = "approve"

[mcp_servers.loctree]
url = "http://127.0.0.1:5174/mcp"
bearer_token_env_var = "LOCTREE_MCP_TOKEN"
```

---

## 5. macOS launchd LaunchAgents

The services are configured as LaunchAgents in `~/Library/LaunchAgents/` with `RunAtLoad=true` and `KeepAlive=true`:

| Service Label | Command | Default Port | Log Destination |
| :--- | :--- | :--- | :--- |
| **`com.loctree.aicx.mcp`** | `aicx serve --transport http` | `8044` | `~/.aicx/logs/aicx-serve-http.log` |
| **`com.loctree.loctree.mcp`** | `loctree-mcp --transport http` | `5174` | `~/.loctree/logs/loctree-serve-http.log` |

Both services bind loopback by default and neither terminates TLS in-process.

The former `io.vetcoders.aicx.reindex` timer is removed when the HTTP service is installed, preventing two competing index writers.

---

## 6. Useful Aliases and Operational Helpers

Add the following aliases to `~/.zshrc`:

```bash
# Check status of listening MCP ports
alias mcp-status='lsof -nP -iTCP:8044,5174 -sTCP:LISTEN'

# View live MCP logs
alias mcp-logs='tail -f ~/.aicx/logs/aicx-serve-http.log ~/.loctree/logs/loctree-serve-http.log'

# Check launchd registration
alias mcp-launchd='launchctl list | grep -E "vetcoders|loctree"'

# Restart all launchd MCP services
alias mcp-restart='launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.loctree.aicx.mcp.plist 2>/dev/null; \
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.loctree.aicx.mcp.plist; \
  launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.loctree.loctree.mcp.plist 2>/dev/null; \
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.loctree.loctree.mcp.plist; \
  echo "🚀 Services restarted under launchd"'
```
