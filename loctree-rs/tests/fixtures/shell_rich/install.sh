#!/usr/bin/env bash
set -euo pipefail
umask 022

# loctree install script
# Usage:
#   curl -fsSL https://loct.io/install.sh | sh
# Env overrides:
#   INSTALL_DIR   where to place the runnable `loctree` wrapper (default: $HOME/.local/bin)
#   CARGO_HOME    override cargo home (default: ~/.cargo)
#   INSTALL_MCP   install `loctree-mcp` too (default: 1, set 0 for CLI-only)

INSTALL_DIR=${INSTALL_DIR:-"$HOME/.local/bin"}
CARGO_HOME=${CARGO_HOME:-"$HOME/.cargo"}
CARGO_BIN="$CARGO_HOME/bin"
INSTALL_MCP=${INSTALL_MCP:-1}
REPO_URL="https://github.com/Loctree/Loctree"
# Allow pinning a branch, tag, or commit; defaults to 'main' regardless of which branch this script is fetched from.
LOCTREE_REF=${LOCTREE_REF:-"main"}

info() { printf "[loctree] %s\n" "$*"; }
warn() { printf "[loctree][warn] %s\n" "$*" >&2; }

command -v cargo >/dev/null 2>&1 || {
  warn "cargo not found. Install Rust (e.g. https://rustup.rs) then re-run.";
  exit 1;
}

crate_args=(loctree)
if [[ "$INSTALL_MCP" != "0" ]]; then
  crate_args+=(loctree-mcp)
fi

info "Installing ${crate_args[*]} from crates.io"
# Install from crates.io (faster than git); --locked keeps stranger installs reproducible.
cargo install --locked "${crate_args[@]}" --force 2>&1 | grep -v "Compiling\|Downloading\|Downloaded" || true

installed_bin="$CARGO_BIN/loctree"
if [[ ! -x $installed_bin ]]; then
  warn "loctree binary not found at $installed_bin after install";
  exit 1;
fi

if [[ "$INSTALL_MCP" != "0" && ! -x "$CARGO_BIN/loctree-mcp" ]]; then
  warn "loctree-mcp binary not found at $CARGO_BIN/loctree-mcp after install"
  exit 1
fi

mkdir -p "$INSTALL_DIR"
wrapper="$INSTALL_DIR/loctree"
cat >"$wrapper" <<WRAP
#!/usr/bin/env bash
exec "$installed_bin" "\$@"
WRAP
chmod +x "$wrapper"

# Create short alias 'loct' -> 'loctree'
loct_wrapper="$INSTALL_DIR/loct"
ln -sf "$wrapper" "$loct_wrapper" 2>/dev/null || cp "$wrapper" "$loct_wrapper"

info "Installed binary: $installed_bin"
if [[ "$INSTALL_MCP" != "0" ]]; then
  info "Installed MCP server: $CARGO_BIN/loctree-mcp"
fi
info "Wrapper: $wrapper"
info "Short alias: $loct_wrapper (loct)"

# Ensure PATH contains cargo/bin and INSTALL_DIR (wrapper), in that order.
ensure_path_line() {
  local file="$1"
  local cargo="$CARGO_BIN"
  local install="$INSTALL_DIR"
  local tag="# loctree installer"

  if [ ! -w "$file" ]; then
    warn "Cannot update PATH in $file (not writable). Add manually: export PATH=\"$cargo:$install:\$PATH\""
    return
  fi

  # Avoid duplicating our block.
  if grep -q "loctree installer" "$file"; then
    return
  fi

  printf '\n%s\nexport PATH="%s:%s:$PATH"\n' "$tag" "$cargo" "$install" >>"$file"
  warn "Appended PATH to $file; reload shell or run: source $file"
}

case ":$PATH:" in
  *":$CARGO_BIN:"*) :;;
  *) warn "cargo bin not in PATH; adding to ~/.zshrc"; ensure_path_line "$HOME/.zshrc";;
esac

case ":$PATH:" in
  *":$INSTALL_DIR:"*) :;;
  *) warn "loctree wrapper dir not in PATH; adding to ~/.zshrc"; ensure_path_line "$HOME/.zshrc";;
esac

if [[ "$INSTALL_MCP" != "0" ]]; then
  info "Done. Try: loct --help && loctree-mcp --version"
else
  info "Done. Try: loct --help"
fi
