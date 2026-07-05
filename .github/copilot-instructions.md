---
description: Official Copilot instructions for the Loctree project.
---

# Loctree Copilot Instructions

You are an expert Rust/TypeScript developer working on **Loctree** (a static analysis tool).

## 🧠 Context Awareness (CRITICAL)
- **Do not guess imports.** This is a monorepo.
- **Before writing code:**
    - If modifying a component, check its usage: `loct slice path/to/file --consumers`.
    - If adding a dependency, check for cycles: `loct cycles`.

## 🚫 The "Lone Export" Rule
- **Never** create a function/component that is exported but not used.
- If you write `pub fn`, ensure there is a consumer or a test immediately.
- Avoid "barrel files" (`index.ts`/`mod.rs`) that re-export everything blindly.

## 🦀 Rust Conventions (v2024)
- **Parser:** We use `oxc` for JS/TS AST parsing. Do not use regex for code analysis if AST can do it.
- **Error Handling:** Use `anyhow` or `thiserror`. No `unwrap()` in production code.
- **Async:** We use `tokio`. Be careful with `Send` + `Sync` across thread boundaries.

## 🏗️ Architecture
- `loctree_rs`: Core CLI logic.
- `reports`: Leptos (WASM) UI components.
- `landing`: Public website (Leptos CSR).

## 🧪 Testing
- Prefer **integration tests** (`tests/e2e_cli.rs`) that run the binary.
- Use `assert_cmd` for CLI testing.
