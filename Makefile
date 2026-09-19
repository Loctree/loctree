# Loctree Build System
# Includes comprehensive MCP server management
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

# Git hooks and wrappers may invoke Make with repository-local Git variables
# pointing at the caller's checkout. Keep those variables out of every recipe,
# including test prerequisites and publish steps that run before shell wrappers.
LOCTREE_GIT_LOCAL_ENV_VARS := $(shell git rev-parse --local-env-vars 2>/dev/null)
unexport $(LOCTREE_GIT_LOCAL_ENV_VARS)

# Git hooks and wrappers may invoke Make with repository-local Git variables
# pointing at the caller's checkout. Keep those variables out of every recipe,
# including test prerequisites and publish steps that run before shell wrappers.
LOCTREE_GIT_LOCAL_ENV_VARS := $(shell git rev-parse --local-env-vars 2>/dev/null)
unexport $(LOCTREE_GIT_LOCAL_ENV_VARS)

# --- Cargo PATH discovery ---------------------------------------------------
# When `make install` or release packaging is launched from a parent shell that
# did not source `~/.cargo/env`, cargo may be missing or Homebrew cargo may win
# PATH before rustup. Prefer rustup cargo when it exists so installed targets
# such as x86_64-unknown-linux-musl line up with the compiler cargo invokes.
ifneq (,$(wildcard $(HOME)/.cargo/bin/cargo))
  export PATH := $(HOME)/.cargo/bin:$(PATH)
else ifeq (,$(shell command -v cargo 2>/dev/null))
  $(warning cargo not found on PATH and $(HOME)/.cargo/bin/cargo is missing)
endif

.PHONY: all build release-binaries release-bundles release-pack smoke-release-macos-arm64 smoke-release-linux-gnu install install-all install-npm npm-install install-service uninstall-service clean test check precheck preflight semgrep fmt help setup-protoc
.PHONY: editors editors-full editors-vscode editors-vscode-package editors-neovim editors-jetbrains editors-jetbrains-full editors-jetbrains-verify editors-jetbrains-install
.PHONY: version version-show version-check version-assert publish npm-release-verify npm-release-publish
.PHONY: mcp-build mcp-install mcp-test
.PHONY: ai-hooks ai-hooks-claude ai-hooks-codex ai-hooks-gemini ai-hooks-all git-hooks test-git-hooks
# Landing extracted to ../loct-io standalone repo (was: landing landing-dev landing-clean landing-deploy)

# Default target
all: build

# Build all workspace members
build: setup-protoc
	cargo build --workspace --release

# Build only core loctree (no protobuf needed)
build-core:
	cargo build --release -p loctree

release-binaries: setup-protoc
	@if [ -z "$(STAGING_DIR)" ]; then \
		echo "STAGING_DIR is required. Usage: make release-binaries STAGING_DIR=/tmp/stage TARGET=$(TARGET)" >&2; \
		exit 1; \
	fi
	$(CARGO_BUILD) --locked --release --target "$(BUILD_TARGET)" --bin loct --bin loctree --bin loctree-mcp --bin loctree-lsp
	@mkdir -p "$(STAGING_DIR)/bin" "$(STAGING_DIR)/components"
	@for bin in $(RELEASE_BINARIES); do \
		file="$$bin$(RELEASE_BINARY_SUFFIX)"; \
		install -m 0755 "target/$(TARGET)/release/$$file" "$(STAGING_DIR)/bin/$$file"; \
		printf '  %s -> %s\n' "$$file" "$(STAGING_DIR)/bin/$$file"; \
	done
	@case "$(TARGET)" in \
		*apple-darwin) \
			if [ "$(CODESIGN)" = "0" ]; then \
				echo "  codesign skipped (CODESIGN=0)"; \
			elif [ -n "$${MACOS_DEVELOPER_ID_APPLICATION:-}" ]; then \
				MACOS_DEVELOPER_ID_APPLICATION="$$MACOS_DEVELOPER_ID_APPLICATION" bash distribution/macos/codesign-binaries.sh "$(STAGING_DIR)/bin"; \
			elif [ "$(CODESIGN)" = "1" ]; then \
				echo "MACOS_DEVELOPER_ID_APPLICATION is required for CODESIGN=1" >&2; \
				exit 1; \
			else \
				echo "  codesign skipped (set CODESIGN=1 and MACOS_DEVELOPER_ID_APPLICATION for release)"; \
			fi ;; \
	esac
	@python3 -c 'import json, pathlib, sys; staging=pathlib.Path(sys.argv[1]); version=sys.argv[2]; commit=sys.argv[3]; data={"source":"loctree-suite","commit":commit,"components":[{"name":"loct","version":version,"source":"loctree-suite"},{"name":"loctree","version":version,"source":"loctree-suite"},{"name":"loctree-mcp","version":version,"source":"loctree-suite"},{"name":"loctree-lsp","version":version,"source":"loctree-suite"}]}; path=staging/"components"/"loctree-suite.json"; path.write_text(json.dumps(data, indent=2)+"\n", encoding="utf-8"); print(f"  metadata -> {path}")' "$(STAGING_DIR)" "$$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')" "$$(git rev-parse --short=12 HEAD)"

release-bundles:
	@if [ -z "$(VERSION)" ]; then \
		echo "VERSION is required. Usage: make release-bundles VERSION=0.14.4 [AICX_VERSION=0.12.5] [BUNDLE_TARGET=aarch64-apple-darwin|x86_64-unknown-linux-gnu|x86_64-pc-windows-msvc]" >&2; \
		exit 1; \
	fi
	@set --; \
	if [ -n "$(BUNDLE_TARGET)" ]; then \
		set -- "$$@" --target "$(BUNDLE_TARGET)"; \
	elif [ "$(origin TARGET)" = "command line" ] || [ "$(origin TARGET)" = "environment" ]; then \
		set -- "$$@" --target "$(TARGET)"; \
	fi; \
	bash distribution/build-bundle.sh "$(VERSION)" "$$@" $(if $(AICX_VERSION),--aicx-version "$(AICX_VERSION)") $(if $(DIST_DIR),--dist-dir "$(DIST_DIR)") $(if $(WORK_DIR),--work-dir "$(WORK_DIR)") $(if $(LOCT_IO_ROOT),--loct-io-root "$(LOCT_IO_ROOT)") $(if $(NO_SYNC),--no-sync) $(if $(MAKE_CURRENT),--make-current) $(if $(DRY_RUN),--dry-run)

# Determine install root. dragon keeps ~/.cargo/bin as rustup proxy space, so
# first-party Loctree binaries must install outside it by default.
CARGO_INSTALL_ROOT ?= $(HOME)/.local
CARGO_BIN ?= $(CARGO_INSTALL_ROOT)/bin

LOCKFILE ?= /tmp/loctree-make.lock
TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
BUNDLE_TARGET ?=
CODESIGN ?= auto
CARGO_BUILD ?= cargo build
RELEASE_BINARIES := loct loctree loctree-mcp loctree-lsp
RELEASE_BINARY_SUFFIX := $(if $(findstring -windows-,$(TARGET)),.exe,)

# Glibc (gnu) Linux binaries must run on OLDER distros (ubuntu-22.04 = glibc 2.35,
# debian-12 = 2.36), not just the build host (ops-linux = glibc 2.39, which made
# `loct` require GLIBC_2.39 and fail everywhere older). cargo-zigbuild links against
# a pinned older glibc via the `.<ver>` target suffix — 2.28 covers debian-10 /
# ubuntu-18.04 and everything newer. musl uses zigbuild too for an honest
# static target without reviving repo-local linker config.
# Requires `cargo install cargo-zigbuild` + a zig toolchain on the build host.
GLIBC_FLOOR ?= 2.28
ifneq (,$(findstring -linux-gnu,$(TARGET)))
  CARGO_BUILD := cargo zigbuild
  BUILD_TARGET := $(TARGET).$(GLIBC_FLOOR)
else ifneq (,$(findstring -linux-musl,$(TARGET)))
  CARGO_BUILD := cargo zigbuild
  BUILD_TARGET := $(TARGET)
else
  BUILD_TARGET := $(TARGET)
endif
RELEASE_SMOKE_BINARIES := loct loctree loctree-mcp loctree-lsp aicx aicx-mcp
EDITORS_VSCODE_DIR ?= editors/vscode
EDITORS_NVIM_FILE ?= editors/nvim/loctree.lua
EDITORS_JETBRAINS_DIR ?= editors/jetbrains
EDITORS_JETBRAINS_CONFIG ?= $(shell find "$(HOME)/Library/Application Support/JetBrains" -maxdepth 1 -type d -name 'IntelliJIdea*' ! -name '*-backup' 2>/dev/null | sort | tail -n 1)
EDITORS_JETBRAINS_PLUGIN_ID ?= loctree-intellij
NPM ?= npm
EDITORS_VSCODE_NPM_INSTALL ?= install
LUAC ?= $(shell command -v luac 2>/dev/null || echo "")
NVIM ?= nvim

smoke-release-macos-arm64:
	@if [ -z "$(SMOKE_BIN_DIR)" ]; then \
		echo "SMOKE_BIN_DIR is required. Usage: make smoke-release-macos-arm64 SMOKE_BIN_DIR=/path/to/staged/bin" >&2; \
		exit 1; \
	fi
	@bash distribution/macos/smoke-releaseability.sh $(foreach bin,$(RELEASE_SMOKE_BINARIES),"$(SMOKE_BIN_DIR)/$(bin)")

smoke-release-linux-gnu:
	@if [ -z "$(SMOKE_BIN_DIR)" ]; then \
		echo "SMOKE_BIN_DIR is required. Usage: make smoke-release-linux-gnu SMOKE_BIN_DIR=/path/to/staged/bin" >&2; \
		exit 1; \
	fi
	@if ! command -v readelf >/dev/null 2>&1; then \
		echo "readelf is required for Linux glibc portability smoke" >&2; \
		exit 1; \
	fi
	@floor="$(GLIBC_FLOOR)"; \
	for bin in $(RELEASE_SMOKE_BINARIES); do \
		path="$(SMOKE_BIN_DIR)/$$bin"; \
		if [ ! -x "$$path" ]; then \
			echo "missing executable: $$path" >&2; \
			exit 1; \
		fi; \
		if ! "$$path" --version >/dev/null; then \
			echo "$$bin failed to start" >&2; \
			exit 1; \
		fi; \
		max_glibc=$$(readelf --version-info "$$path" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -n 1); \
		if [ -n "$$max_glibc" ]; then \
			highest=$$(printf '%s\n%s\n' "$$floor" "$$max_glibc" | sort -V | tail -n 1); \
			if [ "$$highest" != "$$floor" ]; then \
				echo "$$bin requires GLIBC_$$max_glibc, above floor GLIBC_$$floor" >&2; \
				exit 1; \
			fi; \
			printf '  %s starts; max GLIBC_%s <= GLIBC_%s\n' "$$bin" "$$max_glibc" "$$floor"; \
		else \
			printf '  %s starts; no GLIBC version requirements found\n' "$$bin"; \
		fi; \
	done

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
	cargo install --root "$(CARGO_INSTALL_ROOT)" --path loctree-rs --locked --force; \
	cargo install --root "$(CARGO_INSTALL_ROOT)" --path loctree-mcp --locked --force; \
	if [ "$$(uname -s)" = "Darwin" ]; then \
		bash distribution/macos/codesign-binaries.sh "$(CARGO_BIN)"; \
		if [ "$${LOCTREE_SKIP_SERVICE:-0}" != "1" ] && [ -f "tools/install-mcp-service.sh" ]; then \
			bash tools/install-mcp-service.sh || true; \
		fi; \
	fi; \
	echo "Installed: loct, loctree, loctree-mcp → $(CARGO_BIN)"

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
	cargo install --root "$(CARGO_INSTALL_ROOT)" --path loctree-rs --locked --force; \
	cargo install --root "$(CARGO_INSTALL_ROOT)" --path loctree-mcp --locked --force; \
	cargo install --root "$(CARGO_INSTALL_ROOT)" --path loctree-lsp --locked --force; \
	if [ "$$(uname -s)" = "Darwin" ]; then \
		bash distribution/macos/codesign-binaries.sh "$(CARGO_BIN)"; \
		if [ "$${LOCTREE_SKIP_SERVICE:-0}" != "1" ] && [ -f "tools/install-mcp-service.sh" ]; then \
			bash tools/install-mcp-service.sh || true; \
		fi; \
	fi; \
	echo "Installed: loct, loctree, loctree-mcp, loctree-lsp → $(CARGO_BIN)"

# Background HTTP MCP service daemon (launchd, macOS)
install-service:
	bash tools/install-mcp-service.sh

uninstall-service:
	bash tools/install-mcp-service.sh --uninstall

# Daily dev loop: build the suite binaries and reinstall the global npm package
# from THIS checkout — same on-disk layout as `npm install -g @loctree/loctree`
# (bin symlinks in <npm prefix>/bin, natives in the scoped platform package).
# install-npm is an alias of npm-install. Throwaway prefix for testing:
#   npm_config_prefix=/tmp/loctree-npm make npm-install
NPM_INSTALL_WORK ?= $(or $(TMPDIR),/tmp)/loctree-npm-install

install-npm: npm-install

npm-install: setup-protoc
	$(MAKE) release-binaries STAGING_DIR="$(NPM_INSTALL_WORK)/native"
	NPM_LEGACY_NAMES="loctree" bash distribution/npm/install-local.sh \
		--wrapper distribution/npm/loct \
		--native-bin-dir "$(NPM_INSTALL_WORK)/native/bin"

# Setup protoc - check system or use Homebrew
setup-protoc:
	@which protoc > /dev/null 2>&1 || { \
		echo "protoc not found. Attempting platform-aware install..."; \
		if command -v brew >/dev/null 2>&1; then \
			brew install protobuf; \
		elif command -v apt-get >/dev/null 2>&1; then \
			sudo apt-get update -qq && sudo apt-get install -y protobuf-compiler; \
		elif command -v dnf >/dev/null 2>&1; then \
			sudo dnf install -y protobuf-compiler; \
		elif command -v pacman >/dev/null 2>&1; then \
			sudo pacman -S --needed --noconfirm protobuf; \
		elif command -v apk >/dev/null 2>&1; then \
			sudo apk add --no-cache protobuf-dev; \
		else \
			echo "ERROR: no supported package manager. Install protobuf-compiler manually."; \
			echo "See: https://grpc.io/docs/protoc-installation/"; \
			exit 1; \
		fi; \
	}

# Run tests
test: test-git-hooks
	sh tools/test.sh

test-git-hooks:
	sh tests/git_hooks_install.sh
	sh tests/git_hooks_behavior.sh
	sh tests/commit_msg_hook.sh
	bash tests/commit_msg_diff_gate.sh
	sh tests/preflight_contract.sh

ifneq ($(LOCTREE_GIT_ENV_ISOLATION_NESTED),1)
	sh tests/preflight_git_env_isolation.sh
endif

# Quick check (compilation only)
check:
	cargo check --workspace

# Quick explicit validation (fmt + clippy + check) - FAST, run before build!
# This catches 90% of issues in seconds instead of waiting for 20min build
precheck:
	@echo "=== Quick Validation ==="
	@echo "[1/3] Checking formatting..."
	@cargo fmt --all --check || (echo "Run 'make fmt' to fix" && exit 1)
	@echo "[2/3] Running clippy..."
	@cargo clippy --workspace --all-targets -- -D warnings
	@echo "[3/3] Type checking..."
	@cargo check --workspace
	@echo "=== All checks passed ==="

# Canonical security gate (the `semgrep` step the release runbook names).
#
# Runs the SAME rule surface as .github/workflows/semgrep.yml (auto + p/rust
# + p/typescript) so a local `make semgrep` sees what Code Scanning sees. It
# fails on WARNING and ERROR findings. INFO-level audit rules (unsafe-usage,
# current-exe, args) are review prompts, not defects: they still upload to
# Code Scanning from CI, but they do not block here. No rule is excluded;
# the former html integrity exclusion went away with the generated
# `public_dist/**` pages that caused it. Suppressing a finding in source
# (`nosemgrep`) is not an accepted fix in this repository — fix the sink.
#
# Overridable: SEMGREP_CONFIGS, SEMGREP_SEVERITY, SEMGREP_TARGET.
SEMGREP ?= semgrep
SEMGREP_CONFIGS ?= auto p/rust p/typescript
SEMGREP_SEVERITY ?= WARNING ERROR
SEMGREP_TARGET ?= .

semgrep:
	@if ! command -v $(SEMGREP) >/dev/null 2>&1; then \
		echo "ERROR: semgrep not found. Install it with: pip install semgrep (or brew install semgrep)" >&2; \
		exit 1; \
	fi
	@echo "=== Semgrep security gate (configs=$(SEMGREP_CONFIGS); blocking on $(SEMGREP_SEVERITY)) ==="
	$(SEMGREP) scan $(foreach c,$(SEMGREP_CONFIGS),--config $(c)) \
		$(foreach s,$(SEMGREP_SEVERITY),--severity $(s)) \
		--error --quiet $(SEMGREP_TARGET)
	@echo "=== Semgrep clean ==="

# Explicit full validation before a PR or release. This is intentionally not
# wired to pre-push so normal pushes stay fast and offline-friendly.
preflight: test-git-hooks
	sh tools/preflight.sh

# Format code
fmt:
	cargo fmt --all

# ============================================================================
# Editor Integrations (canonical per-tier builders)
# ============================================================================

# Daily editor integration checks. Release packaging and verifier lanes are
# explicit opt-ins below so local agent/dev loops stay fast by default.
editors: editors-vscode editors-neovim editors-jetbrains

# Release/full editor integration checks.
editors-full: editors-vscode-package editors-neovim editors-jetbrains-full

# VS Code daily tier: install/update dependencies and compile TypeScript.
editors-vscode:
	@echo "=== Editor tier: VS Code ==="
	@if [ ! -d "$(EDITORS_VSCODE_DIR)" ]; then \
		echo "ERROR: missing $(EDITORS_VSCODE_DIR)" >&2; \
		exit 1; \
	fi
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) $(EDITORS_VSCODE_NPM_INSTALL)
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) run check-types
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) run compile
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) test

# VS Code release tier: exact dependency install, binary bundling, and VSIX package.
editors-vscode-package:
	@echo "=== Editor tier: VS Code package ==="
	@if [ ! -d "$(EDITORS_VSCODE_DIR)" ]; then \
		echo "ERROR: missing $(EDITORS_VSCODE_DIR)" >&2; \
		exit 1; \
	fi
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) ci
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) run lint
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) test
	cd "$(EDITORS_VSCODE_DIR)" && $(NPM) run package

# Neovim tier: validate Lua syntax and load the config with a minimal lspconfig stub.
editors-neovim:
	@echo "=== Editor tier: Neovim ==="
	@if [ ! -f "$(EDITORS_NVIM_FILE)" ]; then \
		echo "ERROR: missing $(EDITORS_NVIM_FILE)" >&2; \
		exit 1; \
	fi
	@if [ -z "$(LUAC)" ]; then \
		echo "ERROR: luac is required for Neovim syntax validation" >&2; \
		exit 1; \
	fi
	$(LUAC) -p "$(EDITORS_NVIM_FILE)"
	$(LUAC) -p "editors/nvim/runtime_contract_test.lua"
	@if command -v "$(NVIM)" >/dev/null 2>&1; then \
		"$(NVIM)" --headless -u NONE -i NONE \
			+'lua package.loaded["lspconfig"]={util={root_pattern=function(...) return function(_) return nil end end},loctree={setup=function(_) end}}; package.loaded["lspconfig.configs"]={}' \
			+'luafile $(EDITORS_NVIM_FILE)' \
			+'luafile editors/nvim/runtime_contract_test.lua' \
			+qa; \
	else \
		echo "nvim not found; syntax validation passed, runtime smoke skipped"; \
	fi

# JetBrains daily tier: unit tests plus local plugin ZIP build.
editors-jetbrains:
	@echo "=== Editor tier: JetBrains ==="
	@if [ ! -x "$(EDITORS_JETBRAINS_DIR)/gradlew" ]; then \
		echo "ERROR: missing executable $(EDITORS_JETBRAINS_DIR)/gradlew" >&2; \
		exit 1; \
	fi
	cd "$(EDITORS_JETBRAINS_DIR)" && ./gradlew test buildPlugin --console=plain

# JetBrains verifier tier: expensive compatibility lane for release confidence.
editors-jetbrains-verify:
	@echo "=== Editor tier: JetBrains verifier ==="
	@if [ ! -x "$(EDITORS_JETBRAINS_DIR)/gradlew" ]; then \
		echo "ERROR: missing executable $(EDITORS_JETBRAINS_DIR)/gradlew" >&2; \
		exit 1; \
	fi
	cd "$(EDITORS_JETBRAINS_DIR)" && ./gradlew verifyPlugin --console=plain

# JetBrains full tier: daily build plus Plugin Verifier.
editors-jetbrains-full: editors-jetbrains editors-jetbrains-verify

# JetBrains local reinstall: build plugin ZIP and unpack it into the selected
# IntelliJ config plugin directory. Restart the IDE after this target.
editors-jetbrains-install: editors-jetbrains
	@echo "=== Editor tier: JetBrains install ==="
	@if [ -z "$(EDITORS_JETBRAINS_CONFIG)" ]; then \
		echo "ERROR: no IntelliJ config found. Set EDITORS_JETBRAINS_CONFIG=/path/to/IntelliJIdeaYYYY.N" >&2; \
		exit 1; \
	fi
	@zip_path=$$(ls -t "$(EDITORS_JETBRAINS_DIR)"/build/distributions/*.zip 2>/dev/null | head -n 1); \
	if [ -z "$$zip_path" ]; then \
		echo "ERROR: no plugin ZIP under $(EDITORS_JETBRAINS_DIR)/build/distributions" >&2; \
		exit 1; \
	fi; \
	plugins_dir="$(EDITORS_JETBRAINS_CONFIG)/plugins"; \
	install_dir="$$plugins_dir/$(EDITORS_JETBRAINS_PLUGIN_ID)"; \
	mkdir -p "$$plugins_dir"; \
	rm -rf "$$install_dir"; \
	unzip -q "$$zip_path" -d "$$plugins_dir"; \
	echo "Installed $$zip_path -> $$install_dir"; \
	echo "Restart IntelliJ IDEA to load the reinstalled plugin."

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
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'precheck' '- Quick workspace validation (fmt+clippy+check)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'preflight' '- Full opt-in validation before PR/release'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'build' '- Build all (installs protobuf if needed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'build-core' '- Build only loctree (no protobuf needed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-pack' '- Full distribution pack (version gate + editor packages + release bundles)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install' '- Install loct, loctree & loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install-all' '- Install loct, loctree, loctree-mcp & loctree-lsp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'npm-install' '- Build + reinstall the global npm package from this checkout (alias: install-npm)'
	@printf '%s\n' '  make release-bundles VERSION=X - Build combined Loctree+AICX release tarballs'
	@printf '%s\n' '      Optional: AICX_VERSION=0.12.5 BUNDLE_TARGET=x86_64-unknown-linux-gnu'
	@printf '%s\n' '      Windows: BUNDLE_TARGET=x86_64-pc-windows-msvc builds the full six-binary .tar.gz bundle'
	@printf '%s\n' '      Musl: BUNDLE_TARGET=x86_64-unknown-linux-musl builds a -core tarball without bundled AICX'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test' '- Run all tests'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'check' '- Quick type check (no clippy)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'semgrep' '- Security gate (same rules as CI)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'fmt' '- Format all code'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'clean' '- Clean build artifacts'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'EDITOR INTEGRATIONS'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors' '- Daily editor checks (VS Code compile, Neovim smoke, JetBrains test+build)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-full' '- Release editor checks (VSIX package + JetBrains verifier)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-vscode' '- npm install + compile VS Code extension'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-vscode-package' '- npm ci + package VSIX'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-neovim' '- Lua syntax + headless Neovim smoke when available'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-jetbrains' '- Gradle test + buildPlugin'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-jetbrains-full' '- Gradle test + buildPlugin + Plugin Verifier'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'editors-jetbrains-install' '- Build and reinstall into EDITORS_JETBRAINS_CONFIG'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'VERSION MANAGEMENT'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version-show' '- Show all crate versions'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version-assert' '- Assert Cargo, editors, and web installer versions match'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version-check' '- Assert versions + check publish readiness (dry-run)'
	@printf '%s\n' '  make version SCOPE=X TYPE=Y  - Bump version'
	@printf '%s\n' '    SCOPE: loctree, report, mcp, lsp, all (default: all)'
	@printf '%s\n' '    TYPE:  patch (default), minor, major'
	@printf '%s\n' '    VERSION: exact semver (e.g. 0.13.0-dev) - wins over TYPE'
	@printf '%s\n' '    TAG=1, PUSH=1, FORCE=1, PUBLISH=1 - Additional options'
	@printf '%s\n' '  Examples:'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version' '- Bump all crates (patch)'
	@printf '%s\n' '    make version SCOPE=loctree         - Bump loctree only'
	@printf '%s\n' '    make version SCOPE=mcp TYPE=minor  - Minor bump loctree-mcp'
	@printf '%s\n' '    make version VERSION=0.13.0-dev    - Set exact version everywhere'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'PUBLISHING'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'publish' '- Publish to crates.io'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'npm-release-verify' '- Verify immutable release assets and dry-pack all 7 npm identities'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'npm-release-publish' '- Operator bootstrap from verified assets using KEYS/.npm'
	@printf '%s\n' '  make publish BUMP=true               - Bump patch + publish'
	@printf '%s\n' '  make publish BUMP=true VERSION=minor - Bump minor + publish'
	@printf '%s\n' '  make publish TAG=true                - Publish + tag (triggers npm + brew + binaries)'
	@printf '%s\n' '    Cascade: report-leptos -> loctree -> loctree-mcp -> [tag -> CI: npm + brew + binaries]'
	@printf '%s\n' '    Requires: CARGO_REGISTRY_TOKEN env var'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'MCP BUILD & INSTALL'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'mcp-build' '- Build loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'mcp-install' '- Install loctree-mcp'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install-service' '- Install & start loctree-mcp launchd service (HTTP :5174)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'uninstall-service' '- Stop & remove loctree-mcp launchd service'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'mcp-test' '- Test loctree-mcp via stdio'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'LANDING PAGE (EXTRACTED TO STANDALONE REPO)'
	@printf '%s\n' '  See ../loct-io (origin: github.com/Loctree/loct-io)'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'AI CLI INTEGRATION'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'git-hooks' '- Enable lightweight repo-local Git hooks'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks' '- Interactive hook installer (Claude/Codex/Gemini)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks-claude' '- Install Claude Code hooks'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks-codex' '- Install Codex hooks'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks-gemini' '- Install Gemini hooks'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ai-hooks-all' '- Install hooks for all detected AI CLIs'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'QUICK START'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install' '- Install loct + loctree-mcp + local HTTP service on macOS'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install-all' '- Install loct + loctree-mcp + loctree-lsp'

# ============================================================================
# Version Management
# ============================================================================

VERSION_SCRIPT := ./scripts/version-bump.sh
PYTHON ?= $(shell command -v python3 2>/dev/null || command -v python 2>/dev/null || echo python3)
PACK_VERSION ?= $(shell awk '/^\[workspace.package\]/{p=1; next} /^\[/{p=0} p && /^version = /{gsub(/"/, "", $$3); print $$3; exit}' Cargo.toml)

# Default values (override via make version SCOPE=mcp TYPE=minor)
SCOPE ?= all
TYPE ?= patch

# Show all crate versions and dependency graph
version-show:
	@$(VERSION_SCRIPT) --show-deps

# Check publish readiness (dry-run)
# Usage: make version-check SCOPE=mcp
version-check: version-assert
	@$(VERSION_SCRIPT) --dry-run --$(SCOPE) --$(TYPE) $(if $(FORCE),--force)

# Assert that the suite version is coherent across Cargo, editors, and installer.
version-assert:
	@$(VERSION_SCRIPT) --assert-synced

# Full distribution pack: editor artifacts plus combined CLI/MCP/AICX bundles.
# Uses PACK_VERSION from Cargo by default; override with PACK_VERSION=0.x.y.
release-pack: version-assert editors-full
	@$(MAKE) release-bundles VERSION="$(PACK_VERSION)" $(if $(AICX_VERSION),AICX_VERSION="$(AICX_VERSION)") $(if $(BUNDLE_TARGET),BUNDLE_TARGET="$(BUNDLE_TARGET)") $(if $(DIST_DIR),DIST_DIR="$(DIST_DIR)") $(if $(WORK_DIR),WORK_DIR="$(WORK_DIR)") $(if $(LOCT_IO_ROOT),LOCT_IO_ROOT="$(LOCT_IO_ROOT)") $(if $(NO_SYNC),NO_SYNC="$(NO_SYNC)") $(if $(MAKE_CURRENT),MAKE_CURRENT="$(MAKE_CURRENT)") $(if $(DRY_RUN),DRY_RUN="$(DRY_RUN)")

# Bump version
# Usage: make version SCOPE=loctree TYPE=minor
#        make version SCOPE=mcp TYPE=patch TAG=1 PUSH=1
#        make version VERSION=0.13.0-dev FORCE=1   - Set exact version (overrides TYPE)
# Options: SCOPE (all|loctree|mcp|report|lsp)
#          TYPE  (patch|minor|major)
#          VERSION (exact semver, e.g. 0.13.0-dev; maps to --set, wins over TYPE)
#          TAG   (1 to create git tag)
#          PUSH  (1 to push to remote)
#          FORCE (1 to skip dirty tree check)
#          PUBLISH (1 to publish to crates.io, default: skip)
version:
	@$(VERSION_SCRIPT) --$(SCOPE) $(if $(VERSION),--set $(VERSION),--$(TYPE)) $(if $(TAG),--tag) $(if $(PUSH),--push) $(if $(FORCE),--force) $(if $(PUBLISH),,--no-publish)

# Publish to crates.io (cascade: loctree-ast → report-leptos → loctree [→ loctree-mcp gdy publish!=false])
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
	sh tools/test.sh || exit 1; \
	echo ""; \
	echo "[3/6] Publishing loctree-ast v$$VER..."; \
	cargo publish -p loctree-ast --allow-dirty || { echo "FATAL: loctree-ast publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	echo "[4/6] Publishing report-leptos v$$VER..."; \
	cargo publish -p report-leptos --allow-dirty || { echo "FATAL: report-leptos publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	echo "[5/6] Publishing loctree v$$VER..."; \
	cargo publish -p loctree --allow-dirty || { echo "FATAL: loctree publish failed"; exit 1; }; \
	echo "Waiting for crates.io index (15s)..."; \
	sleep 15; \
	echo ""; \
	if grep -q '^publish = false' loctree-mcp/Cargo.toml; then \
		echo "[6/6] Skipping loctree-mcp (publish = false — vendored-dep story pending)"; \
	else \
		echo "[6/6] Publishing loctree-mcp v$$VER..."; \
		cargo publish -p loctree-mcp --allow-dirty || { echo "FATAL: loctree-mcp publish failed"; exit 1; }; \
	fi; \
	echo ""; \
	echo "=== Engine crates published (v$$VER) ==="; \
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

# Deterministic npm release path. The default target never authenticates or
# publishes; the explicit publish target reads the operator token inside the
# script and never copies it into the repository or GitHub Actions.
npm-release-verify:
	@if [ -z "$(VERSION)" ]; then \
		echo "VERSION is required. Usage: make npm-release-verify VERSION=0.14.4 [ASSET_DIR=/path/to/assets]" >&2; \
		exit 1; \
	fi
	@VERSION="$(VERSION)" KEYS="$(KEYS)" ASSET_DIR="$(ASSET_DIR)" RELEASE_REPO="$(or $(RELEASE_REPO),Loctree/loctree-release)" \
		bash distribution/npm/publish-release.sh

npm-release-publish:
	@if [ -z "$(VERSION)" ]; then \
		echo "VERSION is required. Usage: make npm-release-publish VERSION=0.14.4 KEYS=/path/to/operator-keys" >&2; \
		exit 1; \
	fi
	@VERSION="$(VERSION)" KEYS="$(KEYS)" ASSET_DIR="$(ASSET_DIR)" RELEASE_REPO="$(or $(RELEASE_REPO),Loctree/loctree-release)" PUBLISH_CONFIRM=1 \
		bash distribution/npm/publish-release.sh

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
	cargo install --root "$(CARGO_INSTALL_ROOT)" --path loctree-mcp --locked --force
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		bash distribution/macos/codesign-binaries.sh "$(CARGO_BIN)"; \
	fi
	@printf '%s\n' 'Installed: loctree-mcp → $(CARGO_BIN)'

# Test loctree-mcp via stdio
mcp-test:
	@printf '%s\n' 'Testing loctree-mcp...'
	@echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"make-test","version":"1.0"}}}' \
		| $(CARGO_BIN)/loctree-mcp 2>/dev/null | head -1 || echo "Test failed"

# ============================================================================
# Landing Page (Leptos/Trunk WASM)
# ============================================================================

# Landing page extracted to standalone repo at ../loct-io
# (origin: github.com/Loctree/loct-io). Build/deploy targets live there.

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
	@CLI=claude HOOKS=all $(AI_HOOKS_SCRIPT)

ai-hooks-codex:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=codex HOOKS=all $(AI_HOOKS_SCRIPT)

ai-hooks-gemini:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=gemini HOOKS=loctree $(AI_HOOKS_SCRIPT)

# Install all detected CLIs (non-interactive)
ai-hooks-all:
	@chmod +x $(AI_HOOKS_SCRIPT)
	@CLI=all HOOKS=all $(AI_HOOKS_SCRIPT)

# ============================================================================
# Git Hooks Installation
# ============================================================================

# Explicitly install a committed snapshot of the lightweight hooks in the
# repository's common Git directory. Heavy validation stays opt-in via
# `make preflight`; binary installation never changes Git hooks.
git-hooks:
	@printf '%s\n' 'Installing git hooks...'
	@sh tools/install-git-hooks.sh
