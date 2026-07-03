#!/usr/bin/env bash
set -euo pipefail

FORMULA_NAME=${1:?"formula name required (loct, loct-mcp, or loct-lsp)"}
VERSION=${2:?"version required (e.g. 0.9.0)"}
OUTPUT_PATH=${3:?"output path required"}

mkdir -p "$(dirname "$OUTPUT_PATH")"

case "$FORMULA_NAME" in
  loct)
    CLASS_NAME="Loct"
    DESCRIPTION="Fast, language-aware codebase analyzer for AI agents"
    RELEASE_REPO="${CLI_RELEASE_REPO:-Loctree/loct}"
    DARWIN_ARM_ASSET="loct-darwin-aarch64.tar.gz"
    DARWIN_INTEL_ASSET="loct-darwin-x86_64.tar.gz"
    LINUX_INTEL_ASSET="loct-linux-x86_64.tar.gz"
    DARWIN_ARM_SHA="${CLI_DARWIN_AARCH64_SHA:?CLI_DARWIN_AARCH64_SHA is required}"
    DARWIN_INTEL_SHA="${CLI_DARWIN_X86_64_SHA:?CLI_DARWIN_X86_64_SHA is required}"
    LINUX_INTEL_SHA="${CLI_LINUX_X86_64_SHA:?CLI_LINUX_X86_64_SHA is required}"
    INSTALL_SNIPPET='    bin.install "loct"'
    TEST_SNIPPET=$'    assert_match version.to_s, shell_output("#{bin}/loct --version")\n\n    (testpath/"test.js").write("export const answer = 42;\\n")\n    output = shell_output("#{bin}/loct #{testpath}")\n    assert_match "test.js", output'
    ;;
  loct-mcp)
    CLASS_NAME="LoctMcp"
    DESCRIPTION="MCP server for loct structural analysis"
    RELEASE_REPO="${MCP_RELEASE_REPO:-Loctree/loct-mcp}"
    DARWIN_ARM_ASSET="loct-mcp-darwin-aarch64.tar.gz"
    DARWIN_INTEL_ASSET="loct-mcp-darwin-x86_64.tar.gz"
    LINUX_INTEL_ASSET="loct-mcp-linux-x86_64.tar.gz"
    DARWIN_ARM_SHA="${MCP_DARWIN_AARCH64_SHA:?MCP_DARWIN_AARCH64_SHA is required}"
    DARWIN_INTEL_SHA="${MCP_DARWIN_X86_64_SHA:?MCP_DARWIN_X86_64_SHA is required}"
    LINUX_INTEL_SHA="${MCP_LINUX_X86_64_SHA:?MCP_LINUX_X86_64_SHA is required}"
    INSTALL_SNIPPET='    bin.install "loct-mcp"'
    TEST_SNIPPET=$'    assert_match version.to_s, shell_output("#{bin}/loct-mcp --version")\n    assert_match "Usage", shell_output("#{bin}/loct-mcp --help")'
    ;;
  loct-lsp)
    CLASS_NAME="LoctLsp"
    DESCRIPTION="Language server for loct structural analysis"
    RELEASE_REPO="${LSP_RELEASE_REPO:-Loctree/loct-lsp}"
    DARWIN_ARM_ASSET="loct-lsp-darwin-aarch64.tar.gz"
    DARWIN_INTEL_ASSET="loct-lsp-darwin-x86_64.tar.gz"
    LINUX_INTEL_ASSET="loct-lsp-linux-x86_64.tar.gz"
    DARWIN_ARM_SHA="${LSP_DARWIN_AARCH64_SHA:?LSP_DARWIN_AARCH64_SHA is required}"
    DARWIN_INTEL_SHA="${LSP_DARWIN_X86_64_SHA:?LSP_DARWIN_X86_64_SHA is required}"
    LINUX_INTEL_SHA="${LSP_LINUX_X86_64_SHA:?LSP_LINUX_X86_64_SHA is required}"
    INSTALL_SNIPPET='    bin.install "loct-lsp"'
    TEST_SNIPPET=$'    assert_match version.to_s, shell_output("#{bin}/loct-lsp --version")\n    assert_match "Usage", shell_output("#{bin}/loct-lsp --help")'
    ;;
  *)
    echo "Unknown formula: $FORMULA_NAME" >&2
    exit 1
    ;;
esac

cat > "$OUTPUT_PATH" <<EOF
class ${CLASS_NAME} < Formula
  desc "${DESCRIPTION}"
  homepage "https://loct.io"
  version "${VERSION}"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/${RELEASE_REPO}/releases/download/v${VERSION}/${DARWIN_ARM_ASSET}"
      sha256 "${DARWIN_ARM_SHA}"
    end

    on_intel do
      url "https://github.com/${RELEASE_REPO}/releases/download/v${VERSION}/${DARWIN_INTEL_ASSET}"
      sha256 "${DARWIN_INTEL_SHA}"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/${RELEASE_REPO}/releases/download/v${VERSION}/${LINUX_INTEL_ASSET}"
      sha256 "${LINUX_INTEL_SHA}"
    end
  end

  def install
${INSTALL_SNIPPET}
  end

  test do
${TEST_SNIPPET}
  end
end
EOF
