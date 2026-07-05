#!/bin/bash
# Sync version across release surfaces and hardcoded strings.
# Usage: ./scripts/sync-version.sh [new-version]
# If no version provided, reads from the workspace version in Cargo.toml.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Get version from workspace Cargo.toml or argument
if [ -n "$1" ]; then
    VERSION="$1"
else
    VERSION=$(awk '
        /^\[workspace.package\]$/ { in_section=1; next }
        in_section && /^version = / { gsub(/"/, "", $3); print $3; exit }
    ' "$ROOT_DIR/Cargo.toml")
fi

echo "Syncing version to: $VERSION"

update_file() {
    local file="$1"
    local pattern="$2"

    if [ -f "$file" ]; then
        # BSD sed (macOS) requires an extension for -i, empty string '' works
        # GNU sed (Linux) treats '' as the filename if provided as a separate arg
        if sed --version 2>/dev/null | grep -q GNU; then
             sed -i "$pattern" "$file"
        else
             sed -i '' "$pattern" "$file"
        fi
        echo "  Updated: $file"
    else
        echo "  Skipped (not found): $file"
    fi
}

# Update lib.rs docs link
update_file "$ROOT_DIR/loctree-rs/src/lib.rs" 's|html_root_url = "https://docs.rs/loctree/[^"]*"|html_root_url = "https://docs.rs/loctree/'$VERSION'"|'

# Update reports crate footer
update_file "$ROOT_DIR/reports/src/components/document.rs" 's/"loctree v[^"]*"/"loctree v'$VERSION'"/'

# Update landing page VERSION const (single source of truth for landing UI)
update_file "$ROOT_DIR/landing/src/sections/mod.rs" 's/pub const VERSION: \&str = "v[^"]*"/pub const VERSION: \&str = "v'$VERSION'"/'

# Update MCP agent index (landing/api/agent/index.json) — version field
update_file "$ROOT_DIR/landing/api/agent/index.json" 's/"version": *"[^"]*"/"version": "'$VERSION'"/'

# Sync canonical npm release surface
if [ -f "$ROOT_DIR/distribution/npm/sync-version.mjs" ]; then
    if command -v node >/dev/null 2>&1; then
        node "$ROOT_DIR/distribution/npm/sync-version.mjs" "$VERSION"
        echo "  Updated: distribution/npm/package.json"
    else
        echo "Node.js is required to sync distribution/npm version" >&2
        exit 1
    fi
fi

echo ""
echo "Version sync complete: v$VERSION"
echo ""
echo "Verify with:"
echo "  grep -r 'v$VERSION\|$VERSION' --include='*.rs' --include='Cargo.toml' --include='package.json' $ROOT_DIR | grep -v target | grep -v '#'"
