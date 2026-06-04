# Loctree Build System
# Includes comprehensive MCP server management
#
# Created by M&K ⓒ 2025-2026 The Loctree Team

.PHONY: all build install clean test check precheck fmt help setup-protoc
.PHONY: version version-show version-check publish
.PHONY: mcp-build mcp-install mcp-test smoke-release-macos-arm64
.PHONY: ai-hooks ai-hooks-claude ai-hooks-codex ai-hooks-gemini git-hooks

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
# Install everything (CLI + MCP server + hooks)
install: install-cli install-mcp git-hooks
	@echo "Installed: loct, loctree, loctree-mcp → $(CARGO_BIN)"

# Install CLI only (loct + loctree binaries)
install-cli: setup-protoc
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
	@trap 'rm -f "$(LOCKFILE)"' EXIT; \
	cargo install --path loctree_rs --force

# Install MCP server only
install-mcp: setup-protoc
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
	@trap 'rm -f "$(LOCKFILE)"' EXIT; \
	cargo install --path loctree-mcp --force

# Setup protoc - check system or use Homebrew
setup-protoc:
	@which protoc > /dev/null 2>&1 || { \
		echo "protoc not found. Installing via Homebrew..."; \
		brew install protobuf; \
	}

# Run tests
test:
	cargo test --workspace

# Fast gate (fmt + clippy + check)
precheck:
	@echo "=== Fast Gate ==="
	@echo "[1/3] Checking formatting..."
	@cargo fmt --all --check || (echo "Run 'make fmt' to fix" && exit 1)
	@echo "[2/3] Running clippy..."
	@cargo clippy --workspace --all-targets -- -D warnings
	@echo "[3/3] Type checking..."
	@cargo check --workspace
	@echo "=== Fast gate passed ==="

# Full quality gate (fmt + clippy + check + semgrep) - run before push
check:
	@echo "=== Quality Gate ==="
	@$(MAKE) precheck
	@echo "[4/4] Semgrep security scan..."
	@if command -v semgrep >/dev/null 2>&1 || command -v pipx >/dev/null 2>&1; then \
		SEMGREP=$$(command -v semgrep || echo "pipx run semgrep"); \
		$$SEMGREP scan --config auto --error --quiet . 2>/dev/null || \
			echo "[!] Semgrep found issues (see above)"; \
	else \
		echo "[!] Semgrep not available, skipping (install: pipx install semgrep)"; \
	fi
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
# Help colors
HELP_C_CYAN   := \033[36m
HELP_C_GREEN  := \033[32m
HELP_C_YELLOW := \033[33m
HELP_C_RESET  := \033[0m

help:
	@printf '\n$(HELP_C_CYAN)%s$(HELP_C_RESET)\n' 'Loctree Build System'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'CORE COMMANDS'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'precheck' '- Pre-push validation (fmt+clippy+check) - RUN FIRST!'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'build' '- Build all (installs protobuf if needed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'build-core' '- Build only loctree (no protobuf needed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install' '- Install loct, loctree & loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test' '- Run all tests'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'check' '- Full quality gate (fmt+clippy+check+semgrep)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'fmt' '- Format all code'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'clean' '- Clean build artifacts'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'smoke-release-macos-arm64' 'Verify macOS arm64 release portability'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'VERSION MANAGEMENT'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version-show' '- Show all crate versions'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version-check' '- Check publish readiness (dry-run)'
	@printf '%s\n' '  make version SCOPE=X TYPE=Y  - Bump version'
	@printf '%s\n' '    SCOPE: loctree, report, mcp, lsp, all (default: all)'
	@printf '%s\n' '    TYPE:  patch (default), minor, major'
	@printf '%s\n' '    TAG=1, PUSH=1, FORCE=1, PUBLISH=1 - Additional options'
	@printf '%s\n' '  Examples:'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version' '- Bump all crates (patch)'
	@printf '%s\n' '    make version SCOPE=loctree         - Bump loctree only'
	@printf '%s\n' '    make version SCOPE=mcp TYPE=minor  - Minor bump loctree-mcp'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'PUBLISHING'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'publish' '- Publish current version to crates.io'
	@printf '%s\n' '  make publish BUMP=true               - Bump patch + publish'
	@printf '%s\n' '  make publish BUMP=true VERSION=minor - Bump minor + publish'
	@printf '%s\n' '    Cascade: report-leptos -> loctree -> loctree-mcp'
	@printf '%s\n' '    Requires: CARGO_REGISTRY_TOKEN env var'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'MCP BUILD & INSTALL'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'mcp-build' '- Build loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'mcp-install' '- Install loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'mcp-test' '- Test loctree-mcp via stdio'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'AI CLI INTEGRATION'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'git-hooks' '- Install git pre-push validation hook'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks' '- Interactive hook installer (Claude/Codex/Gemini)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks-claude' '- Install Claude Code hooks'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'QUICK START'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install' '- Install loct + loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'smoke-release-macos-arm64' 'Check macOS arm64 release binary portability'

# ============================================================================
# Version Management
# ============================================================================

VERSION_SCRIPT := ./scripts/version-bump.sh

# Default values (override via make version SCOPE=mcp TYPE=minor)
SCOPE ?= all
TYPE ?= patch

# Show all crate versions and dependency graph
version-show:
	@$(VERSION_SCRIPT) --show-deps

# Check publish readiness (dry-run)
# Usage: make version-check SCOPE=mcp
version-check:
	@$(VERSION_SCRIPT) --dry-run --$(SCOPE) --$(TYPE)

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
VERSION ?= patch

publish:
	@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then \
		echo "ERROR: CARGO_REGISTRY_TOKEN not set"; \
		echo "Usage: CARGO_REGISTRY_TOKEN=xxx make publish"; \
		exit 1; \
	fi
	@if [ -n "$$(git status --porcelain)" ]; then \
		echo "ERROR: Working tree is dirty. Commit or stash before publish."; \
		exit 1; \
	fi
	@if [ "$(BUMP)" = "true" ]; then \
		echo "=== Bumping version ($(VERSION)) ==="; \
		$(VERSION_SCRIPT) --all --$(VERSION) --no-publish; \
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
	cargo publish -p report-leptos || { echo "FATAL: report-leptos publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	echo "[4/5] Publishing loctree v$$VER..."; \
	cargo publish -p loctree || { echo "FATAL: loctree publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	echo "[5/5] Publishing loctree-mcp v$$VER..."; \
	cargo publish -p loctree-mcp || { echo "FATAL: loctree-mcp publish failed"; exit 1; }; \
	echo ""; \
	echo "=== All 3 crates published (v$$VER) ==="

# ============================================================================
# MCP Build & Install (loctree-mcp only)
# ============================================================================

# Build loctree-mcp
mcp-build:
	@printf '%s\n' 'Building loctree-mcp...'
	cargo build --release -p loctree-mcp
	@printf '%s\n' 'Done. Binary in target/release/'

# Install loctree-mcp (alias - use 'make install' instead)
mcp-install:
	cargo install --path loctree-mcp --force
	@printf '%s\n' 'Installed: loctree-mcp → $(CARGO_BIN)'

# Test loctree-mcp via stdio
mcp-test:
	@printf '%s\n' 'Testing loctree-mcp...'
	@echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"make-test","version":"1.0"}}}' \
		| $(CARGO_BIN)/loctree-mcp 2>/dev/null | head -1 || echo "Test failed"

# Verify the native macOS arm64 release story stays clean-room safe
smoke-release-macos-arm64:
	@if [ "$$(uname -s)" != "Darwin" ] || [ "$$(uname -m)" != "arm64" ]; then \
		echo "This smoke target must run on macOS arm64."; \
		exit 1; \
	fi
	cargo build --release -p loctree
	cargo build --release -p loctree-mcp
	bash distribution/macos/smoke-releaseability.sh target/release/loct target/release/loctree target/release/loctree-mcp

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
	@printf '%s\n' 'Installing git hooks...'
	@ln -sf ../../tools/hooks/pre-commit .git/hooks/pre-commit
	@ln -sf ../../tools/hooks/pre-push .git/hooks/pre-push
	@chmod +x tools/hooks/pre-commit tools/hooks/pre-push
	@printf '%s\n' '✓ pre-commit + pre-push hooks installed'
