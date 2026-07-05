# Loctree Build System
# Includes comprehensive MCP server management
#
# Created by M&K ⓒ 2025-2026 The Loctree Team

.PHONY: all build install clean test check precheck fmt help setup-protoc
.PHONY: version version-show version-check publish
.PHONY: mcp-build mcp-install mcp-test
.PHONY: ai-hooks ai-hooks-claude ai-hooks-codex ai-hooks-gemini git-hooks
.PHONY: landing landing-dev landing-clean

# Default target
all: build

# Setup vendored protoc path
PROTOC_VENDOR := $(shell cargo run --quiet --package protoc-bin-vendored --example get-path 2>/dev/null || echo "")

# Build all workspace members
build: setup-protoc
	cargo build --workspace --release

# Build only core loctree (no protobuf needed)
build-core:
	cargo build --release -p loctree

# Determine cargo bin dir
CARGO_BIN ?= $(if $(CARGO_HOME),$(CARGO_HOME)/bin,$(HOME)/.cargo/bin)

LOCKFILE ?= /tmp/loctree-make.lock

# Install loctree CLI + MCP server
# Lock is auto-cleaned on success, failure, or if stale (dead PID)
install: setup-protoc
	@if [ -f "$(LOCKFILE)" ]; then \
		old_pid=$$(cat "$(LOCKFILE)" 2>/dev/null); \
		if [ -n "$$old_pid" ] && kill -0 "$$old_pid" 2>/dev/null; then \
			echo "Another build running (PID $$old_pid). Aborting."; \
			exit 1; \
		fi; \
		echo "Removing stale lock (PID $$old_pid dead)"; \
		rm -f "$(LOCKFILE)"; \
	fi
	@echo $$$$ > "$(LOCKFILE)"
	@trap 'rm -f $(LOCKFILE)' EXIT; \
	set -e; \
	cargo install --path loctree-rs --force; \
	cargo install --path loctree-mcp --force; \
	echo "Installed: loct, loctree, loctree-mcp → $(CARGO_BIN)"; \
	$(MAKE) git-hooks

# Install all CLI binaries (loct/loctree, loctree-mcp, loctree-lsp)
install-all: setup-protoc
	@if [ -f "$(LOCKFILE)" ]; then \
		old_pid=$$(cat "$(LOCKFILE)" 2>/dev/null); \
		if [ -n "$$old_pid" ] && kill -0 "$$old_pid" 2>/dev/null; then \
			echo "Another build running (PID $$old_pid). Aborting."; \
			exit 1; \
		fi; \
		echo "Removing stale lock (PID $$old_pid dead)"; \
		rm -f "$(LOCKFILE)"; \
	fi
	@echo $$$$ > "$(LOCKFILE)"
	@trap 'rm -f $(LOCKFILE)' EXIT; \
	set -e; \
	cargo install --path loctree-rs --force; \
	cargo install --path loctree-mcp --force; \
	cargo install --path loctree_lsp --force; \
	echo "Installed: loct, loctree, loctree-mcp, loctree-lsp → $(CARGO_BIN)"; \
	$(MAKE) git-hooks

# Setup protoc - check system or use Homebrew
setup-protoc:
	@which protoc > /dev/null 2>&1 || { \
		echo "protoc not found. Installing via Homebrew..."; \
		brew install protobuf; \
	}

# Run tests
test:
	cargo test --workspace

# Quick check (compilation only)
check:
	cargo check --workspace

# Full pre-push validation (fmt + clippy + check) - FAST, run before build!
# This catches 90% of issues in seconds instead of waiting for 20min build
precheck:
	@echo "=== Pre-push Check ==="
	@echo "[1/3] Checking formatting..."
	@cargo fmt --all --check || (echo "Run 'make fmt' to fix" && exit 1)
	@echo "[2/3] Running clippy..."
	@cargo clippy --workspace --all-targets -- -D warnings
	@echo "[3/3] Type checking..."
	@cargo check --workspace
	@echo "=== All checks passed ==="

# Format code
fmt:
	cargo fmt --all

# Clean build artifacts
clean:
	cargo clean

# Remove stale build lock
unlock:
	@rm -f "$(LOCKFILE)" && echo "Lock removed" || echo "No lock"

# Help
help:
	@echo "Loctree Build System"
	@echo ""
	@echo "Core Commands:"
	@echo "  make precheck     - Pre-push validation (fmt+clippy+check) - RUN FIRST!"
	@echo "  make build        - Build all (installs protobuf if needed)"
	@echo "  make build-core   - Build only loctree (no protobuf needed)"
	@echo "  make install      - Install loct, loctree & loctree-mcp"
	@echo "  make install-all  - Install loct, loctree, loctree-mcp & loctree-lsp"
	@echo "  make test         - Run all tests"
	@echo "  make check        - Quick type check (no clippy)"
	@echo "  make fmt          - Format all code"
	@echo "  make clean        - Clean build artifacts"
	@echo ""
	@echo "Version Management:"
	@echo "  make version-show       - Show all crate versions"
	@echo "  make version-check      - Check publish readiness (dry-run)"
	@echo "  make version SCOPE=X TYPE=Y  - Bump version"
	@echo "    SCOPE: loctree, report, mcp, lsp, all (default: all)"
	@echo "    TYPE:  patch (default), minor, major"
	@echo "    TAG=1, PUSH=1, FORCE=1, PUBLISH=1 - Additional options"
	@echo "  Examples:"
	@echo "    make version                       - Bump all crates (patch)"
	@echo "    make version SCOPE=loctree         - Bump loctree only"
	@echo "    make version SCOPE=mcp TYPE=minor  - Minor bump loctree-mcp"
	@echo ""
	@echo "Publishing:"
	@echo "  make publish                         - Publish to crates.io"
	@echo "  make publish BUMP=true               - Bump patch + publish"
	@echo "  make publish BUMP=true VERSION=minor - Bump minor + publish"
	@echo "  make publish TAG=true                - Publish + tag (triggers npm + brew + binaries)"
	@echo "    Cascade: report-leptos -> loctree -> loctree-mcp -> [tag -> CI: npm + brew + binaries]"
	@echo "    Requires: CARGO_REGISTRY_TOKEN env var"
	@echo ""
	@echo "MCP Build & Install:"
	@echo "  make mcp-build         - Build loctree-mcp"
	@echo "  make mcp-install       - Install loctree-mcp"
	@echo "  make mcp-test          - Test loctree-mcp via stdio"
	@echo ""
	@echo "Landing Page:"
	@echo "  make landing           - Build for production (→ public_dist/)"
	@echo "  make landing-dev       - Dev server with hot reload"
	@echo "  make landing-clean     - Clean build artifacts"
	@echo ""
	@echo "AI CLI Integration:"
	@echo "  make git-hooks         - Install git pre-push validation hook"
	@echo "  make ai-hooks          - Interactive hook installer (Claude/Codex/Gemini)"
	@echo "  make ai-hooks-claude   - Install Claude Code hooks"
	@echo ""
	@echo "Quick start:"
	@echo "  make install           - Install loct + loctree-mcp"
	@echo "  make install-all       - Install loct + loctree-mcp + loctree-lsp"

# ============================================================================
# Version Management
# ============================================================================

VERSION_SCRIPT := ./scripts/version-bump.sh
PYTHON ?= $(shell command -v python3 2>/dev/null || command -v python 2>/dev/null || echo python3)

# Default values (override via make version SCOPE=mcp TYPE=minor)
SCOPE ?= all
TYPE ?= patch

# Show all crate versions and dependency graph
version-show:
	@$(VERSION_SCRIPT) --show-deps

# Check publish readiness (dry-run)
# Usage: make version-check SCOPE=mcp
version-check:
	@$(VERSION_SCRIPT) --dry-run --$(SCOPE) --$(TYPE) $(if $(FORCE),--force)

# Bump version
# Usage: make version SCOPE=loctree TYPE=minor
#        make version SCOPE=mcp TYPE=patch TAG=1 PUSH=1
# Options: SCOPE (all|loctree|mcp|report|lsp)
#          TYPE  (patch|minor|major)
#          TAG   (1 to create git tag)
#          PUSH  (1 to push to remote)
#          FORCE (1 to skip dirty tree check)
#          PUBLISH (1 to publish to crates.io, default: skip)
version:
	@$(VERSION_SCRIPT) --$(SCOPE) --$(TYPE) $(if $(TAG),--tag) $(if $(PUSH),--push) $(if $(FORCE),--force) $(if $(PUBLISH),,--no-publish)

# Publish to crates.io (cascade: report-leptos → loctree → loctree-mcp)
# Usage: make publish                              - Publish current version
#        make publish BUMP=true                     - Bump patch, then publish
#        make publish BUMP=true VERSION=minor       - Bump minor, then publish
# Requires: CARGO_REGISTRY_TOKEN env var
BUMP ?= false

publish:
	@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then \
		echo "ERROR: CARGO_REGISTRY_TOKEN not set"; \
		echo "Usage: CARGO_REGISTRY_TOKEN=xxx make publish"; \
		exit 1; \
	fi
	@if [ "$(BUMP)" = "true" ]; then \
		echo "=== Bumping version ($(VERSION)) ==="; \
		$(VERSION_SCRIPT) --all --$(VERSION) --no-publish --force; \
	fi
	@VER=$$(grep '^version = ' Cargo.toml | head -1 | cut -d'"' -f2); \
	echo "=== Publishing loctree workspace v$$VER to crates.io ==="; \
	echo ""; \
	echo "[1/5] Pre-publish validation (fmt + clippy + check)..."; \
	$(MAKE) precheck || exit 1; \
	echo ""; \
	echo "[2/5] Running tests..."; \
	cargo test --workspace || exit 1; \
	echo ""; \
	echo "[3/5] Publishing report-leptos v$$VER..."; \
	cargo publish -p report-leptos --allow-dirty || { echo "FATAL: report-leptos publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	echo "[4/5] Publishing loctree v$$VER..."; \
	cargo publish -p loctree --allow-dirty || { echo "FATAL: loctree publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	echo "[5/5] Publishing loctree-mcp v$$VER..."; \
	cargo publish -p loctree-mcp --allow-dirty || { echo "FATAL: loctree-mcp publish failed"; exit 1; }; \
	echo ""; \
	echo "=== All 3 crates published (v$$VER) ==="; \
	echo ""; \
	if [ "$(TAG)" = "true" ] || [ "$(RELEASE)" = "true" ]; then \
		echo "[6/6] Creating release tag v$$VER..."; \
		git tag -a "v$$VER" -m "Release v$$VER" 2>/dev/null || echo "Tag v$$VER already exists"; \
		git push origin "v$$VER" 2>/dev/null || echo "Tag push failed (check remote access)"; \
		echo "=== Tag v$$VER pushed — CI will build binaries + npm + homebrew ==="; \
	else \
		echo "Tip: Run 'make publish TAG=true' or push tag manually to trigger binary + npm + brew release:"; \
		echo "  git tag -a v$$VER -m 'Release v$$VER' && git push origin v$$VER"; \
	fi

# ============================================================================
# MCP Build & Install (loctree-mcp only)
# ============================================================================

# Build loctree-mcp
mcp-build:
	@echo "Building loctree-mcp..."
	cargo build --release -p loctree-mcp
	@echo "Done. Binary in target/release/"

# Install loctree-mcp (alias - use 'make install' instead)
mcp-install:
	cargo install --path loctree-mcp --force
	@echo "Installed: loctree-mcp → $(CARGO_BIN)"

# Test loctree-mcp via stdio
mcp-test:
	@echo "Testing loctree-mcp..."
	@echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"make-test","version":"1.0"}}}' \
		| $(CARGO_BIN)/loctree-mcp 2>/dev/null | head -1 || echo "Test failed"

# ============================================================================
# Landing Page (Leptos/Trunk WASM)
# ============================================================================

# Build landing page for production (outputs to public_dist/)
landing:
	@echo "=== Building Loctree Landing Page ==="
	@cd landing && env -u NO_COLOR GLIBC_TUNABLES=glibc.rtld.optional_static_tls=16384 TRUNK_TOOLS_WASM_BINDGEN=0.2.118 trunk build --release
	@rm -rf public_dist
	@cp -r landing/dist public_dist
	@$(PYTHON) scripts/build_landing_static.py public_dist
	@echo "=== Build complete! Ready in public_dist/ ==="

# Development server with hot reload
landing-dev:
	@echo "Starting landing page dev server..."
	@cd landing && env -u NO_COLOR GLIBC_TUNABLES=glibc.rtld.optional_static_tls=16384 TRUNK_TOOLS_WASM_BINDGEN=0.2.118 trunk serve --open

# Clean landing build artifacts
landing-clean:
	@rm -rf landing/dist public_dist
	@echo "Landing build artifacts cleaned"

# ============================================================================
# AI Hooks Installation (Claude, Codex, Gemini)
# ============================================================================

AI_HOOKS_SCRIPT := ./scripts/install-ai-hooks.sh

# Interactive installation for all detected CLIs
ai-hooks:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@$(AI_HOOKS_SCRIPT)

# Install for specific CLIs (non-interactive)
ai-hooks-claude:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=claude $(AI_HOOKS_SCRIPT)

ai-hooks-codex:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=codex $(AI_HOOKS_SCRIPT)

ai-hooks-gemini:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=gemini $(AI_HOOKS_SCRIPT)

# Install all detected CLIs (non-interactive)
ai-hooks-all:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=all $(AI_HOOKS_SCRIPT)

# ============================================================================
# Git Hooks Installation
# ============================================================================

# Install git hooks (pre-commit fmt + pre-push validation)
git-hooks:
	@echo "Installing git hooks..."
	@ln -sf ../../tools/hooks/pre-commit .git/hooks/pre-commit
	@ln -sf ../../tools/hooks/pre-push .git/hooks/pre-push
	@chmod +x tools/hooks/pre-commit tools/hooks/pre-push
	@echo "✓ pre-commit + pre-push hooks installed"
