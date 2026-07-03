#!/bin/bash
# ============================================================================
# install-ai-hooks.sh - Interactive AI CLI hooks installer
# ============================================================================
#
# Installs loctree and rmcp-memex hooks for:
#   - Claude Code (~/.claude/)
#   - OpenAI Codex CLI (~/.codex/)
#   - Google Gemini CLI (~/.gemini/)
#
# Hook packages:
#   - loctree: Structural code analysis (impact, consumers, symbols)
#   - memex: Memory/RAG augmentation (past conversations, knowledge)
#
# Usage:
#   make ai-hooks                    # Interactive mode
#   make ai-hooks CLI=all            # Install all detected CLIs
#   make ai-hooks CLI=claude,gemini  # Install specific CLIs
#   make ai-hooks HOOKS=loctree      # Only loctree hooks
#   make ai-hooks HOOKS=memex        # Only memex hooks
#   make ai-hooks HOOKS=all          # All hooks
#
# ============================================================================
# Part of loctree - https://loct.io
# Created by M&K ⓒ 2025-2026 The Loctree Team
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOKS_DIR="${SCRIPT_DIR}/../ai-hooks"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
NC='\033[0m' # No Color
BOLD='\033[1m'

# Track what to install
INSTALL_LOCTREE=false
INSTALL_MEMEX=false

# ============================================================================
# Helpers
# ============================================================================

info() { echo -e "${BLUE}ℹ${NC} $1"; }
success() { echo -e "${GREEN}✓${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }
error() { echo -e "${RED}✗${NC} $1"; }
header() { echo -e "\n${BOLD}${CYAN}━━━ $1 ━━━${NC}\n"; }

confirm() {
    local prompt="$1"
    local default="${2:-y}"
    local response

    if [[ "$default" == "y" ]]; then
        read -rp "$(echo -e "${YELLOW}?${NC} $prompt [Y/n]: ")" response
        response=${response:-y}
    else
        read -rp "$(echo -e "${YELLOW}?${NC} $prompt [y/N]: ")" response
        response=${response:-n}
    fi

    [[ "$response" =~ ^[Yy]$ ]]
}

# ============================================================================
# Tool Detection
# ============================================================================

detect_claude() {
    command -v claude &>/dev/null || [[ -d "$HOME/.claude" ]]
}

detect_codex() {
    command -v codex &>/dev/null || [[ -d "$HOME/.codex" ]]
}

detect_gemini() {
    command -v gemini &>/dev/null || [[ -d "$HOME/.gemini" ]]
}

detect_loct() {
    command -v loct &>/dev/null
}

detect_memex() {
    command -v rmcp-memex &>/dev/null
}

# ============================================================================
# Hook Selection (Interactive)
# ============================================================================

select_hooks() {
    header "Select Hook Packages"

    local loct_status memex_status

    if detect_loct; then
        loct_status="${GREEN}installed${NC}"
    else
        loct_status="${YELLOW}not installed${NC}"
    fi

    if detect_memex; then
        memex_status="${GREEN}installed${NC}"
    else
        memex_status="${YELLOW}not installed${NC}"
    fi

    echo -e "Available hook packages:"
    echo ""
    echo -e "  ${BOLD}1)${NC} ${CYAN}loctree${NC} - Structural code analysis ($loct_status)"
    echo -e "     Adds: impact analysis, dependency tracking, symbol lookup"
    echo ""
    echo -e "  ${BOLD}2)${NC} ${MAGENTA}memex${NC} - Memory/RAG augmentation ($memex_status)"
    echo -e "     Adds: past conversation context, institutional knowledge"
    echo ""
    echo -e "  ${BOLD}3)${NC} ${GREEN}both${NC} - Full suite (recommended)"
    echo ""

    local choice
    read -rp "$(echo -e "${YELLOW}?${NC} Select packages [1/2/3]: ")" choice

    case "$choice" in
        1)
            INSTALL_LOCTREE=true
            ;;
        2)
            INSTALL_MEMEX=true
            ;;
        3|"")
            INSTALL_LOCTREE=true
            INSTALL_MEMEX=true
            ;;
        *)
            warn "Invalid choice, installing both"
            INSTALL_LOCTREE=true
            INSTALL_MEMEX=true
            ;;
    esac

    # Check dependencies and offer installation
    if $INSTALL_LOCTREE && ! detect_loct; then
        echo ""
        warn "loct CLI not found"
        if confirm "Install loctree with 'make install'?" "n"; then
            info "Run: cd $(dirname "$SCRIPT_DIR") && make install"
            info "Then re-run: make ai-hooks"
            exit 0
        fi
    fi

    if $INSTALL_MEMEX && ! detect_memex; then
        echo ""
        warn "rmcp-memex not found"
        if confirm "Install rmcp-memex with cargo?" "y"; then
            info "Installing rmcp-memex..."
            if cargo install rmcp-memex 2>/dev/null; then
                success "rmcp-memex installed"
            else
                warn "cargo install failed - trying from git..."
                if cargo install --git https://github.com/Loctree/rmcp-memex 2>/dev/null; then
                    success "rmcp-memex installed from git"
                else
                    error "Failed to install rmcp-memex"
                    info "Install manually: cargo install --path /path/to/rmcp-memex"
                    INSTALL_MEMEX=false
                fi
            fi
        else
            warn "Skipping memex hooks (rmcp-memex not available)"
            INSTALL_MEMEX=false
        fi
    fi
}

# ============================================================================
# Claude Code Installation
# ============================================================================

install_claude() {
    header "Installing Claude Code hooks"

    local hooks_dir="$HOME/.claude/hooks"
    local settings="$HOME/.claude/settings.json"

    # Create hooks directory
    mkdir -p "$hooks_dir"
    success "Created $hooks_dir"

    # Track hooks to register
    local grep_hooks=()
    local startup_hooks=()
    local compact_hooks=()

    # ── Loctree hooks ──
    if $INSTALL_LOCTREE; then
        cp "$HOOKS_DIR/loct-grep-augment.sh" "$hooks_dir/"
        chmod +x "$hooks_dir/loct-grep-augment.sh"
        success "Copied loct-grep-augment.sh"
        grep_hooks+=("loct-grep-augment.sh")

        if [[ -f "$HOOKS_DIR/loct-smart-suggest.sh" ]]; then
            cp "$HOOKS_DIR/loct-smart-suggest.sh" "$hooks_dir/"
            chmod +x "$hooks_dir/loct-smart-suggest.sh"
            success "Copied loct-smart-suggest.sh"
        fi
    fi

    # ── Memex hooks ──
    if $INSTALL_MEMEX; then
        cp "$HOOKS_DIR/memex-context.sh" "$hooks_dir/"
        chmod +x "$hooks_dir/memex-context.sh"
        success "Copied memex-context.sh"
        grep_hooks+=("memex-context.sh")

        cp "$HOOKS_DIR/memex-startup.sh" "$hooks_dir/"
        chmod +x "$hooks_dir/memex-startup.sh"
        success "Copied memex-startup.sh"
        startup_hooks+=("memex-startup.sh")

        if [[ -f "$HOOKS_DIR/memory-on-compact.sh" ]]; then
            cp "$HOOKS_DIR/memory-on-compact.sh" "$hooks_dir/"
            chmod +x "$hooks_dir/memory-on-compact.sh"
            success "Copied memory-on-compact.sh"
            compact_hooks+=("memory-on-compact.sh")
        fi
    fi

    # ── Update settings.json ──
    # Use ${array[*]+...} syntax to handle empty arrays with set -u
    update_claude_settings "$settings" "${grep_hooks[*]}" "${startup_hooks[*]+"${startup_hooks[*]}"}" "${compact_hooks[*]+"${compact_hooks[*]}"}"

    success "Claude Code installation complete!"
    info "Restart Claude Code to apply changes"
}

update_claude_settings() {
    local settings="$1"
    local grep_hooks="$2"
    local startup_hooks="$3"
    local compact_hooks="$4"

    # Backup existing settings
    if [[ -f "$settings" ]]; then
        cp "$settings" "$settings.backup.$(date +%Y%m%d%H%M%S)"
        success "Backed up settings.json"
    fi

    if ! command -v jq &>/dev/null; then
        warn "jq not installed - generating manual instructions"
        generate_manual_config "$grep_hooks" "$startup_hooks" "$compact_hooks"
        return
    fi

    # Initialize settings if not exists
    if [[ ! -f "$settings" ]]; then
        echo '{}' > "$settings"
    fi

    local tmp_settings="${settings}.tmp"
    cp "$settings" "$tmp_settings"

    # Add PostToolUse hooks for Grep
    for hook in $grep_hooks; do
        if ! grep -q "$hook" "$tmp_settings" 2>/dev/null; then
            jq --arg hook "$hook" '
                .hooks.PostToolUse = (.hooks.PostToolUse // []) + [{
                    "matcher": "Grep",
                    "hooks": [{
                        "type": "command",
                        "command": ("~/.claude/hooks/" + $hook)
                    }]
                }]
            ' "$tmp_settings" > "${tmp_settings}.new" && mv "${tmp_settings}.new" "$tmp_settings"
            success "Registered $hook for PostToolUse:Grep"
        else
            warn "$hook already registered"
        fi
    done

    # Add SessionStart hooks
    for hook in $startup_hooks; do
        if ! grep -q "$hook" "$tmp_settings" 2>/dev/null; then
            jq --arg hook "$hook" '
                .hooks.SessionStart = (.hooks.SessionStart // []) + [{
                    "type": "command",
                    "command": ("~/.claude/hooks/" + $hook)
                }]
            ' "$tmp_settings" > "${tmp_settings}.new" && mv "${tmp_settings}.new" "$tmp_settings"
            success "Registered $hook for SessionStart"
        else
            warn "$hook already registered"
        fi
    done

    # Add Notification hooks (for compact/summary)
    for hook in $compact_hooks; do
        if ! grep -q "$hook" "$tmp_settings" 2>/dev/null; then
            jq --arg hook "$hook" '
                .hooks.Notification = (.hooks.Notification // []) + [{
                    "matcher": "compact",
                    "hooks": [{
                        "type": "command",
                        "command": ("~/.claude/hooks/" + $hook)
                    }]
                }]
            ' "$tmp_settings" > "${tmp_settings}.new" && mv "${tmp_settings}.new" "$tmp_settings"
            success "Registered $hook for Notification:compact"
        else
            warn "$hook already registered"
        fi
    done

    mv "$tmp_settings" "$settings"
    success "Updated settings.json"
}

generate_manual_config() {
    local grep_hooks="$1"
    local startup_hooks="$2"
    local compact_hooks="$3"

    echo ""
    warn "Add this to ~/.claude/settings.json manually:"
    echo ""
    cat <<'MANUAL'
{
  "hooks": {
    "PostToolUse": [
MANUAL

    for hook in $grep_hooks; do
        cat <<HOOK
      {
        "matcher": "Grep",
        "hooks": [{"type": "command", "command": "~/.claude/hooks/$hook"}]
      },
HOOK
    done

    cat <<'MANUAL'
    ],
    "SessionStart": [
MANUAL

    for hook in $startup_hooks; do
        cat <<HOOK
      {"type": "command", "command": "~/.claude/hooks/$hook"},
HOOK
    done

    cat <<'MANUAL'
    ],
    "Notification": [
MANUAL

    for hook in $compact_hooks; do
        cat <<HOOK
      {
        "matcher": "compact",
        "hooks": [{"type": "command", "command": "~/.claude/hooks/$hook"}]
      },
HOOK
    done

    cat <<'MANUAL'
    ]
  }
}
MANUAL
}

# ============================================================================
# Codex CLI Installation
# ============================================================================

install_codex() {
    header "Installing Codex CLI integration"

    local config="$HOME/.codex/config.toml"

    mkdir -p "$HOME/.codex"

    if [[ -f "$config" ]]; then
        cp "$config" "$config.backup.$(date +%Y%m%d%H%M%S)"
        success "Backed up config.toml"
    fi

    # Add loctree MCP server
    if $INSTALL_LOCTREE; then
        if ! grep -q "loctree" "$config" 2>/dev/null; then
            cat >> "$config" <<'EOF'

# Loctree MCP Server - structural code analysis
[[mcp.servers]]
name = "loctree"
command = "loctree-mcp"
args = []

EOF
            success "Added loctree MCP server"
        else
            warn "loctree already configured"
        fi
    fi

    # Add memex MCP server
    if $INSTALL_MEMEX; then
        if ! grep -q "memex" "$config" 2>/dev/null; then
            cat >> "$config" <<'EOF'

# Memex MCP Server - memory/RAG augmentation
[[mcp.servers]]
name = "memex"
command = "rmcp-memex"
args = ["serve"]

EOF
            success "Added memex MCP server"
        else
            warn "memex already configured"
        fi
    fi

    success "Codex CLI installation complete!"
}

# ============================================================================
# Gemini CLI Installation
# ============================================================================

install_gemini() {
    header "Installing Gemini CLI hooks"

    local settings="$HOME/.gemini/settings.json"
    local hooks_dir="$HOME/.gemini/hooks"

    mkdir -p "$hooks_dir"
    success "Created $hooks_dir"

    if $INSTALL_LOCTREE; then
        sed 's/PostToolUse/AfterTool/g' "$HOOKS_DIR/loct-grep-augment.sh" > "$hooks_dir/loct-grep-augment.sh"
        chmod +x "$hooks_dir/loct-grep-augment.sh"
        success "Copied and adapted loct-grep-augment.sh"
    fi

    if $INSTALL_MEMEX; then
        sed 's/PostToolUse/AfterTool/g' "$HOOKS_DIR/memex-context.sh" > "$hooks_dir/memex-context.sh"
        chmod +x "$hooks_dir/memex-context.sh"
        success "Copied and adapted memex-context.sh"
    fi

    # Update settings.json (simplified - Gemini uses different format)
    if [[ -f "$settings" ]]; then
        cp "$settings" "$settings.backup.$(date +%Y%m%d%H%M%S)"
    fi

    warn "Gemini settings.json format varies - check documentation"
    info "Hooks copied to $hooks_dir"

    success "Gemini CLI installation complete!"
}

# ============================================================================
# Main
# ============================================================================

main() {
    echo ""
    echo -e "${BOLD}${CYAN}🌳 AI Hooks Installer${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "loctree + rmcp-memex integration"
    echo ""

    # Check if hooks source exists
    if [[ ! -d "$HOOKS_DIR" ]]; then
        error "Hooks directory not found: $HOOKS_DIR"
        exit 1
    fi

    # Handle HOOKS environment variable
    local hooks_arg="${HOOKS:-}"
    if [[ -n "$hooks_arg" ]]; then
        case "$hooks_arg" in
            loctree) INSTALL_LOCTREE=true ;;
            memex) INSTALL_MEMEX=true ;;
            all|both) INSTALL_LOCTREE=true; INSTALL_MEMEX=true ;;
            *) warn "Unknown HOOKS value: $hooks_arg, using interactive" ;;
        esac
    fi

    # Interactive hook selection if not specified
    if ! $INSTALL_LOCTREE && ! $INSTALL_MEMEX; then
        select_hooks
    fi

    # Detect available CLIs
    local claude_available=false
    local codex_available=false
    local gemini_available=false

    detect_claude && claude_available=true
    detect_codex && codex_available=true
    detect_gemini && gemini_available=true

    header "Detected AI CLIs"
    $claude_available && echo -e "  ${GREEN}●${NC} Claude Code" || echo -e "  ${RED}○${NC} Claude Code"
    $codex_available && echo -e "  ${GREEN}●${NC} Codex CLI" || echo -e "  ${RED}○${NC} Codex CLI"
    $gemini_available && echo -e "  ${GREEN}●${NC} Gemini CLI" || echo -e "  ${RED}○${NC} Gemini CLI"
    echo ""

    if ! $claude_available && ! $codex_available && ! $gemini_available; then
        warn "No AI CLIs detected"
        exit 0
    fi

    # Check for CLI argument
    local cli_arg="${CLI:-}"

    if [[ -n "$cli_arg" ]]; then
        # Non-interactive mode
        if [[ "$cli_arg" == "all" ]]; then
            $claude_available && install_claude
            $codex_available && install_codex
            $gemini_available && install_gemini
        else
            IFS=',' read -ra CLIS <<< "$cli_arg"
            for cli in "${CLIS[@]}"; do
                case "$cli" in
                    claude) $claude_available && install_claude || warn "Claude not detected" ;;
                    codex) $codex_available && install_codex || warn "Codex not detected" ;;
                    gemini) $gemini_available && install_gemini || warn "Gemini not detected" ;;
                    *) warn "Unknown CLI: $cli" ;;
                esac
            done
        fi
    else
        # Interactive mode
        if $claude_available; then
            if confirm "Install hooks for Claude Code?"; then
                install_claude
            fi
        fi

        if $codex_available; then
            if confirm "Install for Codex CLI?"; then
                install_codex
            fi
        fi

        if $gemini_available; then
            if confirm "Install hooks for Gemini CLI?"; then
                install_gemini
            fi
        fi
    fi

    echo ""
    echo -e "${BOLD}${GREEN}━━━ Installation Complete ━━━${NC}"
    echo ""
    echo "Installed:"
    $INSTALL_LOCTREE && echo -e "  ${CYAN}●${NC} loctree hooks (structural analysis)"
    $INSTALL_MEMEX && echo -e "  ${MAGENTA}●${NC} memex hooks (memory augmentation)"
    echo ""
    echo "Next steps:"
    echo "  1. Restart your AI CLI to load hooks"
    $INSTALL_LOCTREE && echo "  2. Run 'loct scan' in your project"
    $INSTALL_MEMEX && echo "  3. Start memex daemon: rmcp-memex serve --http-port 6660"
    echo ""
}

main "$@"
