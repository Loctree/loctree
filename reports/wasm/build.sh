#!/bin/bash
# Build script for report-wasm
# Creates WASM package and generates assets for embedding in HTML
#
# Developed with 💀 by The Loctree Team ⓒ 2025-2026 

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "Building report-wasm..."

# Build WASM package
wasm-pack build --target web --release

# Create assets directory
mkdir -p assets

# Generate base64 encoded WASM
echo "Generating base64 encoded WASM..."
base64 < pkg/report_wasm_bg.wasm > assets/wasm.b64

# Copy JS glue code
echo "Copying JS glue code..."
cp pkg/report_wasm.js assets/report_wasm.js

# Print stats
echo ""
echo "Build complete!"
echo "  WASM size:   $(wc -c < pkg/report_wasm_bg.wasm | tr -d ' ') bytes"
echo "  Base64 size: $(wc -c < assets/wasm.b64 | tr -d ' ') bytes"
echo "  JS glue:     $(wc -c < assets/report_wasm.js | tr -d ' ') bytes"
echo ""
echo "Assets ready in: $SCRIPT_DIR/assets/"
