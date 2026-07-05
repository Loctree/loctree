# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.8.x   | Yes                |
| < 0.8   | No                 |

## Reporting a Vulnerability

If you discover a security vulnerability in Loctree, please report it responsibly.

**Do not open a public issue.**

Instead, email: **security@loctree.dev**

We will acknowledge receipt within 48 hours and provide a timeline for a fix.

## Scope

Loctree is a local analysis tool. It reads source files and caches structural data.
It does not execute analyzed code, make network requests during analysis, or store
credentials.

The MCP server (`loctree-mcp`) listens on stdio only — it does not open network ports.

## Known Trust Boundaries

- Snapshot cache files are written to `~/Library/Caches/loctree/` (macOS) or
  `$XDG_CACHE_HOME/loctree/` (Linux). These contain file paths and structural
  metadata, not source code content.
- The HTML report renderer (`report-leptos`) generates static HTML with no
  external resource loading.
