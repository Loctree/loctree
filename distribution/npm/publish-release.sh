#!/usr/bin/env bash
set -euo pipefail

# Signed-release npm publisher: trusted GitHub OIDC or explicit local bootstrap.
# It stages packages only from checksum-verified immutable suite bundles, never
# from host-built binaries. Future releases publish the same package graph from
# publish.yml through trusted OIDC.

VERSION="${VERSION:-}"
KEYS_DIR="${KEYS:-$HOME/.keys}"
ASSET_DIR="${ASSET_DIR:-}"
RELEASE_REPO="${RELEASE_REPO:-Loctree/loctree-release}"
PUBLISH_CONFIRM="${PUBLISH_CONFIRM:-0}"
AUTH_MODE="${AUTH_MODE:-token}"
case "$AUTH_MODE" in
  token|oidc) ;;
  *) echo "AUTH_MODE must be token or oidc" >&2; exit 2 ;;
esac
if [[ "$PUBLISH_CONFIRM" == "1" && "$AUTH_MODE" == "oidc" ]]; then
  [[ "${GITHUB_ACTIONS:-}" == "true" && -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" && -n "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ]] || {
    echo "OIDC publication requires the GitHub Actions trusted-publisher identity" >&2
    exit 2
  }
  unset NODE_AUTH_TOKEN NPM_TOKEN 2>/dev/null || true
fi

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "VERSION=x.y.z is required" >&2
  exit 2
fi

publisher_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
tag="v$VERSION"
git rev-parse --verify --quiet "refs/tags/$tag^{commit}" >/dev/null || {
  echo "signed release tag $tag is not present locally" >&2
  exit 2
}

workspace_version="$(awk '
  /^\[workspace.package\]$/ { in_section=1; next }
  in_section && /^version = / { gsub(/"/, "", $3); print $3; exit }
' Cargo.toml)"
[[ "$workspace_version" == "$VERSION" ]] || {
  echo "workspace version $workspace_version does not match $VERSION" >&2
  exit 2
}

work="$(mktemp -d "${TMPDIR:-/tmp}/loctree-npm-${VERSION}.XXXXXX")"
cleanup() {
  unset NODE_AUTH_TOKEN 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

if [[ -z "$ASSET_DIR" ]]; then
  ASSET_DIR="$work/assets"
  mkdir -p "$ASSET_DIR"
  gh release download "$tag" --repo "$RELEASE_REPO" --dir "$ASSET_DIR"
fi
ASSET_DIR="$(cd "$ASSET_DIR" && pwd)"

archives=(
  "loctree-${VERSION}-aarch64-apple-darwin.tar.gz"
  # The x64 macOS payload is core-only, but its public filename intentionally
  # stays unsuffixed because editor clients already consume that contract.
  "loctree-${VERSION}-x86_64-apple-darwin.tar.gz"
  "loctree-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
  "loctree-${VERSION}-x86_64-pc-windows-msvc.tar.gz"
)

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

for archive_name in "${archives[@]}"; do
  archive="$ASSET_DIR/$archive_name"
  sidecar="$archive.sha256"
  [[ -s "$archive" && -s "$sidecar" ]] || {
    echo "missing release asset or checksum: $archive_name" >&2
    exit 2
  }
  expected="$(awk 'NR == 1 { print $1 }' "$sidecar")"
  actual="$(sha256_file "$archive")"
  [[ "$actual" == "$expected" ]] || {
    echo "checksum mismatch: $archive_name" >&2
    exit 2
  }
  printf 'verified %s\n' "$archive_name"
done

tag_tree="$work/tag-tree"
mkdir -p "$tag_tree"
git archive "$tag" distribution/npm/loct | tar -x -C "$tag_tree"
stage="$work/npm"
cp -R "$tag_tree/distribution/npm/loct" "$stage"

stage_platform() {
  local platform="$1"
  local archive_name="$2"
  local root_name="${archive_name%.tar.gz}"
  local suffix="${3:-}"
  local extract="$work/extract-$platform"
  local bin_dir="$stage/platform-packages/$platform/bin"
  mkdir -p "$extract"
  tar -xzf "$ASSET_DIR/$archive_name" -C "$extract"
  rm -rf "$bin_dir"
  mkdir -p "$bin_dir"
  for binary in loct loctree loctree-mcp loctree-lsp; do
    install -m 0755 "$extract/$root_name/bin/${binary}${suffix}" "$bin_dir/${binary}${suffix}"
  done
}

stage_platform darwin-arm64 "${archives[0]}"
stage_platform darwin-x64 "${archives[1]}"
stage_platform linux-x64-gnu "${archives[2]}"
stage_platform win32-x64-msvc "${archives[3]}" .exe

node distribution/npm/sync-version.mjs --check "$VERSION"
node --test "$publisher_dir/publish-if-missing.test.mjs"

for platform_dir in "$stage"/platform-packages/*; do
  for binary in loct loctree loctree-mcp loctree-lsp; do
    suffix=""
    [[ "$platform_dir" == *win32* ]] && suffix=".exe"
    target="$platform_dir/bin/${binary}${suffix}"
    [[ -s "$target" ]] || {
      echo "missing staged binary: $target" >&2
      exit 2
    }
    size="$(wc -c < "$target" | tr -d ' ')"
    [[ "$size" -ge 1048576 ]] || {
      echo "staged binary is suspiciously small: $target ($size bytes)" >&2
      exit 2
    }
  done
done

if [[ "$PUBLISH_CONFIRM" == "1" && "$AUTH_MODE" == "token" ]]; then
  credential="$KEYS_DIR/.npm"
  [[ -s "$credential" ]] || {
    echo "operator npm credential missing at $credential" >&2
    exit 2
  }
  NODE_AUTH_TOKEN="$(tr -d '\r\n' < "$credential")"
  export NODE_AUTH_TOKEN
  [[ ${#NODE_AUTH_TOKEN} -ge 20 ]] || {
    echo "operator npm credential is malformed" >&2
    exit 2
  }
  npm_config="$work/npmrc"
  printf '%s\n' \
    'registry=https://registry.npmjs.org/' \
    "//registry.npmjs.org/:_authToken=\${NODE_AUTH_TOKEN}" > "$npm_config"
  chmod 0600 "$npm_config"
  export NPM_CONFIG_USERCONFIG="$npm_config"
  npm whoami --registry=https://registry.npmjs.org/ >/dev/null
fi

publish_package() {
  local package_dir="$1"
  if [[ "$PUBLISH_CONFIRM" == "1" ]]; then
    provenance=false
    [[ "$AUTH_MODE" != "oidc" ]] || provenance=true
    node "$publisher_dir/publish-if-missing.mjs" \
      --package-dir "$package_dir" --version "$VERSION" --provenance "$provenance"
  else
    (cd "$package_dir" && npm pack --dry-run >/dev/null)
  fi
}

for platform_dir in "$stage"/platform-packages/*; do
  publish_package "$platform_dir"
done

wrapper_names=("@loctree/loctree" "@loctree/loct" "loctree")
for package_name in "${wrapper_names[@]}"; do
  safe_name="${package_name//@/}"
  safe_name="${safe_name//\//-}"
  wrapper="$work/wrapper-$safe_name"
  cp -R "$stage" "$wrapper"
  (cd "$wrapper" && npm pkg set "name=$package_name" >/dev/null)
  publish_package "$wrapper"
done

if [[ "$PUBLISH_CONFIRM" != "1" ]]; then
  echo "npm release verification passed; set PUBLISH_CONFIRM=1 to publish"
  exit 0
fi

echo "npm release $VERSION published or already present under all seven identities"
