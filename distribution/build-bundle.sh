#!/usr/bin/env bash
set -euo pipefail

# Build the per-target Loctree release bundles: stage the suite binaries (plus
# AICX for full flavors), write bundle metadata and checksums, tar and optionally
# GPG-sign each artifact, then hand it to the loct.io release-index sync.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SUITE_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

# Targets built when the caller passes no --target: macOS arm64, Linux x64 in
# both gnu and musl flavors, and Windows x64 MSVC.
DEFAULT_TARGETS=(
  "aarch64-apple-darwin"
  "x86_64-unknown-linux-gnu"
  "x86_64-unknown-linux-musl"
  "x86_64-pc-windows-msvc"
)
# Binaries a bundle must contain: the Loctree set ships in every flavor, the
# AICX set only in full bundles.
LOCTREE_RELEASE_BINARIES=(loct loctree loctree-mcp loctree-lsp)
AICX_RELEASE_BINARIES=(aicx aicx-mcp)
# Canonical AICX release coordinates used in release mode; --aicx-repo /
# --aicx-version override them, --aicx-root bypasses the download entirely.
AICX_REPO_DEFAULT="Loctree/aicx"
AICX_VERSION_DEFAULT="0.12.5"

# Print the full CLI contract on --help or when no version argument is given.
usage() {
  cat <<'EOF'
Usage:
  distribution/build-bundle.sh <version> [options]

Options:
  --aicx-root <path>       Developer override: build AICX from source instead
                           of the canonical GitHub release asset.
  --aicx-version <version> AICX release version. Default: 0.12.5.
  --aicx-tag <tag>         AICX release tag. Default: v<aicx-version>.
  --aicx-repo <owner/repo> AICX GitHub repo. Default: Loctree/aicx.
  --loct-io-root <path>    Root containing scripts/sync_releases.py.
                           Defaults to LOCT_IO_ROOT or ../loct-io.
  --target <triple>        Build one target. Repeat to build multiple.
                           Default: mac arm64, Linux x64 gnu/musl, Windows x64.
  --print-aicx-asset <triple>
                           Print the canonical AICX asset name and exit. Used
                           by CI so addressability checks share this mapping.
  --bundle-flavor <name>   auto, full, or core. Default: auto.
                           auto builds full bundles except musl, which is core.
  --bundle-suffix <text>   Override the flavor-derived artifact-name suffix
                           ("" for none, "-core" for the core convention).
                           Requires exactly one --target. Use only to satisfy a
                           published asset-name contract; see the note below.
  --dist-dir <path>        Output directory for tarballs.
                           Default: ./dist/release-bundles/<version>.
  --work-dir <path>        Staging work directory.
                           Default: ./target/release-bundles/<version>.
  --gpg-key <key-id>       Optional GPG key id for detached tarball .sig.
                           Defaults to LOCTREE_GPG_KEY_ID.
  --signature-claim <text> Optional human-readable manifest signature claim.
  --channel <name>         stable, beta, or nightly. Default: stable.
  --make-current           Mark the version as current in loct.io index.
  --no-sync                Build artifacts but do not call loct.io sync script.
  --dry-run                Print planned actions without building or syncing.
  --allow-unsigned-macos   Do not require macOS codesign for apple-darwin target.
  -h, --help               Show this help.

Default release mode downloads AICX from:
  https://github.com/Loctree/aicx/releases/tag/v0.12.5

Developer override mode uses:
  make -C <aicx-root> release-binaries STAGING_DIR=<staging> TARGET=<triple>

Leave AICX_ROOT unset for release mode. Setting AICX_ROOT or --aicx-root is an
explicit developer-source override and skips the canonical GitHub asset.

Core bundles are explicitly Loctree-only. They are named with a -core suffix and
write README.md/components.json metadata that marks AICX as an optional runtime
dependency instead of silently omitting it.

--bundle-suffix exists for one case: x86_64-apple-darwin. AICX publishes no
release asset for that triple, so the bundle can only ever be core -- but the
shipped editor plugins hard-code the plain archive name
loctree-<ver>-x86_64-apple-darwin.tar.gz (editors/vscode/scripts/
fetch-release-lsp.js and editors/vscode/src/client.ts), and v0.13.1 published it
under exactly that name. The suffix is a filename convention; the honesty it was
added for lives in the bundle's own README.md and components.json, which still
declare AICX unbundled. Overriding the name does not weaken that.
EOF
}

# Abort the build with a message on stderr and a non-zero exit status.
die() {
  echo "error: $*" >&2
  exit 1
}

# Expand ~ and resolve one path to an absolute path, so directory-cleaning and
# containment checks later in the run compare canonical paths.
abs_path() {
  python3 - "$1" <<'PY'
import os, sys
print(os.path.abspath(os.path.expanduser(sys.argv[1])))
PY
}

# Print the SHA-256 of one file using whichever of shasum/sha256sum exists.
# Feed the file on stdin: GNU checksum tools prefix the digest with `\` when a
# Windows path needs escaping, which would make a correct digest compare unequal.
sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 < "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum < "$1" | awk '{print $1}'
  else
    die "missing shasum or sha256sum"
  fi
}

VERSION="${1:-}"
if [[ -z "$VERSION" || "$VERSION" == "-h" || "$VERSION" == "--help" ]]; then
  usage
  exit 0
fi
shift

AICX_ROOT="${AICX_ROOT:-}"
AICX_ROOT_SOURCE="${AICX_ROOT:+env}"
AICX_VERSION="${AICX_VERSION:-$AICX_VERSION_DEFAULT}"
AICX_TAG="${AICX_TAG:-}"
AICX_REPO="${AICX_REPO:-$AICX_REPO_DEFAULT}"
LOCT_IO_ROOT="${LOCT_IO_ROOT:-$SUITE_ROOT/../loct-io}"
DIST_DIR="$SUITE_ROOT/dist/release-bundles/$VERSION"
WORK_DIR="$SUITE_ROOT/target/release-bundles/$VERSION"
GPG_KEY_ID="${LOCTREE_GPG_KEY_ID:-}"
SIGNATURE_CLAIM="${SIGNATURE_CLAIM:-}"
CHANNEL="stable"
BUNDLE_FLAVOR="auto"
BUNDLE_SUFFIX_OVERRIDE=""
BUNDLE_SUFFIX_SET=0
MAKE_CURRENT=0
SYNC=1
DRY_RUN=0
ALLOW_UNSIGNED_MACOS=0
PRINT_AICX_ASSET_TARGET=""
TARGETS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --aicx-root)
      AICX_ROOT="${2:-}"; AICX_ROOT_SOURCE="cli"; shift 2 ;;
    --aicx-version)
      AICX_VERSION="${2:-}"; shift 2 ;;
    --aicx-tag)
      AICX_TAG="${2:-}"; shift 2 ;;
    --aicx-repo)
      AICX_REPO="${2:-}"; shift 2 ;;
    --loct-io-root)
      LOCT_IO_ROOT="${2:-}"; shift 2 ;;
    --target)
      TARGETS+=("${2:-}"); shift 2 ;;
    --print-aicx-asset)
      PRINT_AICX_ASSET_TARGET="${2:-}"; shift 2 ;;
    --dist-dir)
      DIST_DIR="${2:-}"; shift 2 ;;
    --work-dir)
      WORK_DIR="${2:-}"; shift 2 ;;
    --gpg-key)
      GPG_KEY_ID="${2:-}"; shift 2 ;;
    --signature-claim)
      SIGNATURE_CLAIM="${2:-}"; shift 2 ;;
    --channel)
      CHANNEL="${2:-}"; shift 2 ;;
    --bundle-flavor)
      BUNDLE_FLAVOR="${2:-}"; shift 2 ;;
    --bundle-suffix)
      BUNDLE_SUFFIX_OVERRIDE="${2-}"; BUNDLE_SUFFIX_SET=1; shift 2 ;;
    --make-current)
      MAKE_CURRENT=1; shift ;;
    --no-sync)
      SYNC=0; shift ;;
    --dry-run)
      DRY_RUN=1; shift ;;
    --allow-unsigned-macos)
      ALLOW_UNSIGNED_MACOS=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      die "unknown argument: $1" ;;
  esac
done

if [[ -z "$AICX_TAG" ]]; then
  AICX_TAG="v$AICX_VERSION"
fi
case "$CHANNEL" in
  stable|beta|nightly) ;;
  *) die "--channel must be stable, beta, or nightly" ;;
esac
case "$BUNDLE_FLAVOR" in
  auto|full|core) ;;
  *) die "--bundle-flavor must be auto, full, or core" ;;
esac
if [[ "$BUNDLE_SUFFIX_SET" == "1" ]]; then
  case "$BUNDLE_SUFFIX_OVERRIDE" in
    ""|-[a-z0-9]*) ;;
    *) die "--bundle-suffix must be empty or start with '-'" ;;
  esac
  [[ ${#TARGETS[@]} -eq 1 ]] \
    || die "--bundle-suffix requires exactly one --target"
fi

if [[ ${#TARGETS[@]} -eq 0 ]]; then
  TARGETS=("${DEFAULT_TARGETS[@]}")
fi

# Decide whether a target ships a full (Loctree + AICX) or core (Loctree-only)
# bundle. In auto mode musl is always core, because AICX publishes no static
# musl asset.
bundle_flavor_for_target() {
  local target="$1"
  if [[ "$BUNDLE_FLAVOR" != "auto" ]]; then
    printf '%s\n' "$BUNDLE_FLAVOR"
    return 0
  fi
  case "$target" in
    *-musl) printf 'core\n' ;;
    *) printf 'full\n' ;;
  esac
}

# Resolve the artifact-name suffix for a flavor: empty for full, -core for core,
# unless --bundle-suffix overrode it to satisfy a published asset-name contract.
bundle_suffix_for_flavor() {
  local flavor="$1"
  # An explicit --bundle-suffix wins over the flavor convention. Callers use it
  # to satisfy a published asset-name contract, never to hide the flavor: the
  # bundle's README.md and components.json still say whether AICX is inside.
  if [[ "$BUNDLE_SUFFIX_SET" == "1" ]]; then
    printf '%s\n' "$BUNDLE_SUFFIX_OVERRIDE"
    return 0
  fi
  case "$flavor" in
    full) printf '\n' ;;
    core) printf -- '-core\n' ;;
    *) die "unknown bundle flavor: $flavor" ;;
  esac
}

# Report whether any requested target builds a full bundle, i.e. whether this run
# needs curl and the AICX release assets at all.
needs_aicx_release_download() {
  local target
  for target in "${TARGETS[@]}"; do
    if [[ "$(bundle_flavor_for_target "$target")" == "full" ]]; then
      return 0
    fi
  done
  return 1
}

# Fail fast unless the given repo exists and exposes a Makefile, since staging
# drives `make release-binaries` inside it.
require_repo_make() {
  local repo="$1"
  [[ -d "$repo" ]] || die "missing repo: $repo"
  [[ -f "$repo/Makefile" ]] || die "missing Makefile in repo: $repo"
}

if [[ -n "$AICX_ROOT" ]]; then
  [[ -n "${AICX_ROOT_SOURCE:-}" ]] || die "internal error: AICX_ROOT is set but source is unknown"
  AICX_ROOT=$(abs_path "$AICX_ROOT")
  echo "AICX developer-source override active ($AICX_ROOT_SOURCE): $AICX_ROOT"
  echo "Canonical GitHub release asset download is skipped for AICX."
fi
LOCT_IO_ROOT=$(abs_path "$LOCT_IO_ROOT")
DIST_DIR=$(abs_path "$DIST_DIR")
WORK_DIR=$(abs_path "$WORK_DIR")
AICX_BASE_URL="https://github.com/$AICX_REPO/releases/download/$AICX_TAG"

require_repo_make "$SUITE_ROOT"
if [[ -n "$AICX_ROOT" ]]; then
  require_repo_make "$AICX_ROOT"
elif [[ "$DRY_RUN" != "1" ]] && needs_aicx_release_download && ! command -v curl >/dev/null 2>&1; then
  die "curl is required to fetch AICX GitHub release assets"
fi
if [[ "$SYNC" == "1" ]]; then
  [[ -x "$LOCT_IO_ROOT/scripts/sync_releases.py" || -f "$LOCT_IO_ROOT/scripts/sync_releases.py" ]] \
    || die "missing loct.io sync script: $LOCT_IO_ROOT/scripts/sync_releases.py"
fi
if [[ -n "$GPG_KEY_ID" ]] && ! command -v gpg >/dev/null 2>&1; then
  die "gpg is required when --gpg-key or LOCTREE_GPG_KEY_ID is set"
fi

if [[ "$DRY_RUN" != "1" ]]; then
  mkdir -p "$DIST_DIR" "$WORK_DIR"
fi

# Flatten every components/*.json produced during staging into the bundle-level
# components.json, pushing source/commit/release_tag down onto entries that lack
# them so the shipped manifest states provenance per component.
merge_components() {
  local components_dir="$1"
  local output="$2"
  python3 - "$components_dir" "$output" <<'PY'
import json
import sys
from pathlib import Path

components_dir = Path(sys.argv[1])
output = Path(sys.argv[2])
components = []
for path in sorted(components_dir.glob("*.json")):
    data = json.loads(path.read_text())
    if isinstance(data, dict):
        source = data.get("source")
        commit = data.get("commit")
        release_tag = data.get("release_tag")
        for component in data.get("components", []):
            if isinstance(component, dict):
                component = dict(component)
                if source and "source" not in component:
                    component["source"] = source
                if commit and "commit" not in component:
                    component["commit"] = commit
                if release_tag and "release_tag" not in component:
                    component["release_tag"] = release_tag
            components.append(component)
    elif isinstance(data, list):
        components.extend(data)
output.write_text(json.dumps(components, indent=2) + "\n")
PY
}

# Write CHECKSUMS.sha256 covering every staged binary plus components.json and
# README.md, and abort if a binary the flavor promised was never staged.
write_bundle_checksums() {
  local staging="$1"
  local output="$2"
  shift 2
  local bin rel
  [[ $# -gt 0 ]] || die "internal error: no binaries passed to write_bundle_checksums"
  : > "$output"
  for bin in "$@"; do
    [[ -x "$staging/bin/$bin" ]] || die "missing staged binary: $staging/bin/$bin"
    printf "%s  %s\n" "$(sha256_file "$staging/bin/$bin")" "bin/$bin" >> "$output"
  done
  for rel in components.json README.md; do
    if [[ -f "$staging/$rel" ]]; then
      printf "%s  %s\n" "$(sha256_file "$staging/$rel")" "$rel" >> "$output"
    fi
  done
}

# Return the executable suffix used by a target. Keeping this in the bundle
# owner prevents Windows staging, checksums, README and CI from disagreeing.
binary_suffix_for_target() {
  local target="$1"
  case "$target" in
    *-windows-*) printf '.exe\n' ;;
    *) printf '\n' ;;
  esac
}

# Map a target triple to the AICX release asset that carries its binaries. musl
# and unmapped triples die here rather than silently producing a bundle that
# claims AICX but has none.
aicx_asset_name_for_target() {
  local target="$1"
  case "$target" in
    aarch64-apple-darwin)
      printf 'aicx-%s-aarch64-apple-darwin-slim.zip\n' "$AICX_TAG" ;;
    x86_64-unknown-linux-gnu)
      printf 'aicx-%s-x86_64-linux-gnu-slim.tar.gz\n' "$AICX_TAG" ;;
    x86_64-pc-windows-msvc)
      printf 'aicx-%s-x86_64-pc-windows-msvc-slim.zip\n' "$AICX_TAG" ;;
    *-musl)
      die "AICX has no static musl release asset for $target; build a core bundle instead" ;;
    *)
      die "no AICX release asset mapping for target: $target" ;;
  esac
}

if [[ -n "$PRINT_AICX_ASSET_TARGET" ]]; then
  aicx_asset_name_for_target "$PRINT_AICX_ASSET_TARGET"
  exit 0
fi

# Flags for `tar -czf`. macOS bsdtar stores extended attributes in pax headers.
# Release bundles packed on macOS carry com.apple.provenance as
# LIBARCHIVE.xattr.com.apple.provenance and SCHILY.xattr.com.apple.provenance,
# and GNU tar warns once per member:
#   tar: Ignoring unknown extended header keyword 'LIBARCHIVE.xattr...'
# Pass a suppression flag only when this tar documents it. GNU tar accepts
# --no-xattrs and already omits xattrs unless --xattrs is set.
# --no-mac-metadata is bsdtar-only. COPYFILE_DISABLE covers Apple's older
# copyfile-based tar and is a no-op for GNU tar.
tar_create_flags() {
  local help flags
  help="$(tar --help 2>&1 || true)"
  flags=""
  if printf '%s\n' "$help" | grep -q -- '--no-xattrs'; then
    flags="$flags --no-xattrs"
  fi
  if printf '%s\n' "$help" | grep -q -- '--no-mac-metadata'; then
    flags="$flags --no-mac-metadata"
  fi
  printf '%s' "${flags# }"
}

# Extract the canonical AICX archive format for the selected platform.
extract_aicx_archive() {
  local archive="$1"
  local output="$2"
  case "$archive" in
    *.tar.gz) tar -xzf "$archive" -C "$output" ;;
    *.zip)
      command -v unzip >/dev/null 2>&1 || die "unzip is required to extract $archive"
      unzip -q "$archive" -d "$output" ;;
    *) die "unsupported AICX release archive: $archive" ;;
  esac
}

# Fetch one AICX release URL to a local path, failing the build if curl cannot
# retrieve it.
download_aicx_asset() {
  local url="$1"
  local output="$2"
  curl -fsSL "$url" -o "$output" || die "failed to download AICX release asset: $url"
}

# Record the AICX repo, tag and asset this bundle actually consumed into the
# staged components/ tree, so the merged manifest can prove where AICX came from.
write_aicx_release_metadata() {
  local output="$1"
  local asset="$2"
  python3 - "$output" "$AICX_VERSION" "$AICX_REPO" "$AICX_TAG" "$asset" <<'PY'
import json
import sys
from pathlib import Path

output = Path(sys.argv[1])
version = sys.argv[2]
repo = sys.argv[3]
tag = sys.argv[4]
asset = sys.argv[5]
data = {
    "source": repo,
    "version": version,
    "release_tag": tag,
    "asset": asset,
    "components": [
        {"name": "aicx", "version": version, "source": repo, "release_tag": tag},
        {"name": "aicx-mcp", "version": version, "source": repo, "release_tag": tag},
    ],
}
output.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
print(f"  metadata -> {output}")
PY
}

# Download the AICX release asset, verify it against its published sha256 sidecar,
# install aicx/aicx-mcp into the staging bin/, codesign them for macOS targets,
# and write the provenance metadata. A checksum mismatch aborts the release.
stage_aicx_from_release() {
  local target="$1"
  local staging="$2"
  local scratch="$3"
  local codesign_mode="$4"
  local asset archive sidecar expected actual extract_dir found suffix bin file

  asset=$(aicx_asset_name_for_target "$target")
  archive="$scratch/$asset"
  sidecar="$scratch/$asset.sha256"
  extract_dir="$scratch/aicx-release"
  mkdir -p "$scratch" "$extract_dir"

  echo "  AICX source: https://github.com/$AICX_REPO/releases/tag/$AICX_TAG"
  echo "  AICX asset:  $asset"
  download_aicx_asset "$AICX_BASE_URL/$asset.sha256" "$sidecar"
  download_aicx_asset "$AICX_BASE_URL/$asset" "$archive"

  expected=$(awk '{print $1; exit}' "$sidecar")
  actual=$(sha256_file "$archive")
  [[ -n "$expected" ]] || die "empty AICX sha256 sidecar: $AICX_BASE_URL/$asset.sha256"
  [[ "$actual" == "$expected" ]] || die "AICX sha256 mismatch for $asset: expected $expected, got $actual"
  echo "  AICX sha256 ok: $actual"

  extract_aicx_archive "$archive" "$extract_dir"
  mkdir -p "$staging/bin" "$staging/components"
  suffix=$(binary_suffix_for_target "$target")
  for bin in aicx aicx-mcp; do
    file="$bin$suffix"
    found=""
    while IFS= read -r candidate; do
      found="$candidate"
      break
    done < <(find "$extract_dir" -type f -name "$file")
    [[ -n "$found" ]] || die "AICX release asset missing binary: $file"
    install -m 0755 "$found" "$staging/bin/$file"
    printf '  %s -> %s\n' "$file" "$staging/bin/$file"
  done
  case "$target" in
    *apple-darwin)
      if [[ "$codesign_mode" == "0" ]]; then
        echo "  AICX codesign skipped (CODESIGN=0)"
      elif [[ -n "${MACOS_DEVELOPER_ID_APPLICATION:-}" ]]; then
        for bin in aicx aicx-mcp; do
          codesign --force --timestamp --options runtime --sign "$MACOS_DEVELOPER_ID_APPLICATION" "$staging/bin/$bin"
          codesign --verify --verbose=2 "$staging/bin/$bin" >/dev/null
          printf '  AICX codesigned %s\n' "$bin"
        done
      elif [[ "$codesign_mode" == "1" ]]; then
        die "MACOS_DEVELOPER_ID_APPLICATION is required to codesign AICX release binaries"
      else
        echo "  AICX codesign skipped (set CODESIGN=1 and MACOS_DEVELOPER_ID_APPLICATION for release)"
      fi ;;
  esac
  write_aicx_release_metadata "$staging/components/loctree-aicx.json" "$asset"
}

# For core bundles, declare AICX as an unbundled optional runtime dependency with
# the reason and install hints, instead of silently omitting it from the manifest.
write_aicx_optional_metadata() {
  local output="$1"
  local target="$2"
  python3 - "$output" "$AICX_VERSION" "$AICX_REPO" "$AICX_TAG" "$target" <<'PY'
import json
import sys
from pathlib import Path

output = Path(sys.argv[1])
version = sys.argv[2]
repo = sys.argv[3]
tag = sys.argv[4]
target = sys.argv[5]
if target.endswith("-musl"):
    reason = (
        "AICX does not publish a static musl release asset; this core bundle keeps "
        "Loctree musl-static and leaves AICX memory features as an optional runtime dependency."
    )
else:
    reason = (
        f"AICX does not publish a release asset for {target}; this core bundle leaves "
        "AICX memory features as an optional runtime dependency."
    )
data = {
    "source": repo,
    "version": version,
    "release_tag": tag,
    "target": target,
    "bundle_flavor": "core",
    "bundled": False,
    "reason": reason,
    "components": [
        {
            "name": "aicx",
            "version": version,
            "source": repo,
            "release_tag": tag,
            "bundled": False,
            "optional_runtime_dependency": True,
            "install_hint": "Install aicx on PATH to enable AICX-backed memory features.",
        },
        {
            "name": "aicx-mcp",
            "version": version,
            "source": repo,
            "release_tag": tag,
            "bundled": False,
            "optional_runtime_dependency": True,
            "install_hint": "Install aicx-mcp on PATH when MCP access to AICX is required.",
        },
    ],
}
output.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
print(f"  metadata -> {output}")
PY
}

# Write the bundle README.md: which binaries are inside, and whether AICX ships
# with this artifact or has to be installed separately on PATH.
write_bundle_readme() {
  local output="$1"
  local target="$2"
  local flavor="$3"
  local suffix
  suffix=$(binary_suffix_for_target "$target")

  {
    printf "# Loctree %s bundle\n\n" "$flavor"
    printf -- "- Target: \`%s\`\n" "$target"
    printf -- "- Flavor: \`%s\`\n\n" "$flavor"
    printf "## Included binaries\n\n"
    printf -- "- \`loct%s\`\n- \`loctree%s\`\n- \`loctree-mcp%s\`\n- \`loctree-lsp%s\`\n" "$suffix" "$suffix" "$suffix" "$suffix"
    if [[ "$flavor" == "full" ]]; then
      printf -- "- \`aicx%s\`\n- \`aicx-mcp%s\`\n\n" "$suffix" "$suffix"
      printf "AICX is bundled from \`%s\` release \`%s\`.\n" "$AICX_REPO" "$AICX_TAG"
    else
      printf "\n## AICX / memory features\n\n"
      printf "AICX is not bundled in this core artifact. This is intentional for musl targets because AICX does not publish a static musl release asset.\n\n"
      printf "Install \`aicx\` and \`aicx-mcp\` separately on \`PATH\` when AICX-backed memory features are required. The Loctree binaries in this bundle are usable without AICX.\n"
    fi
  } > "$output"
}

TAR_CREATE_FLAGS="$(tar_create_flags)"

for target in "${TARGETS[@]}"; do
  bundle_flavor=$(bundle_flavor_for_target "$target")
  bundle_suffix=$(bundle_suffix_for_flavor "$bundle_flavor")
  bundle_name="loctree-$VERSION-$target$bundle_suffix"
  target_work="$WORK_DIR/$target"
  staging="$target_work/$bundle_name"
  tarball="$DIST_DIR/$bundle_name.tar.gz"
  tarball_tmp="$target_work/$bundle_name.tar.gz"
  components_json="$staging/components.json"
  binary_suffix=$(binary_suffix_for_target "$target")
  bundle_binaries=()
  for bin in "${LOCTREE_RELEASE_BINARIES[@]}"; do
    bundle_binaries+=("$bin$binary_suffix")
  done
  if [[ "$bundle_flavor" == "full" ]]; then
    for bin in "${AICX_RELEASE_BINARIES[@]}"; do
      bundle_binaries+=("$bin$binary_suffix")
    done
  elif [[ -n "$AICX_ROOT" ]]; then
    die "--bundle-flavor core cannot be combined with --aicx-root; core bundles do not include AICX"
  fi

  echo "==> Building $bundle_name ($bundle_flavor)"
  codesign_mode="${CODESIGN:-auto}"
  if [[ "$target" == *apple-darwin && "$ALLOW_UNSIGNED_MACOS" != "1" && "$codesign_mode" == "auto" ]]; then
    codesign_mode=1
  fi

  if [[ "$DRY_RUN" == "1" ]]; then
    echo "  [dry-run] make -C $SUITE_ROOT release-binaries STAGING_DIR=$staging TARGET=$target CODESIGN=$codesign_mode"
    if [[ "$bundle_flavor" == "core" ]]; then
      echo "  [dry-run] write AICX optional-runtime metadata for core bundle"
    elif [[ -n "$AICX_ROOT" ]]; then
      echo "  [dry-run] make -C $AICX_ROOT release-binaries STAGING_DIR=$staging TARGET=$target CODESIGN=$codesign_mode"
    else
      aicx_asset=$(aicx_asset_name_for_target "$target")
      echo "  [dry-run] AICX source: https://github.com/$AICX_REPO/releases/tag/$AICX_TAG"
      echo "  [dry-run] curl -fsSL $AICX_BASE_URL/$aicx_asset.sha256 -o <scratch>/$aicx_asset.sha256"
      echo "  [dry-run] curl -fsSL $AICX_BASE_URL/$aicx_asset -o <scratch>/$aicx_asset"
      echo "  [dry-run] verify sha256 and stage aicx/aicx-mcp into $staging/bin"
      if [[ "$target" == *apple-darwin && "$codesign_mode" == "1" ]]; then
        echo "  [dry-run] codesign staged AICX binaries with MACOS_DEVELOPER_ID_APPLICATION"
      fi
    fi
    echo "  [dry-run] (cd $target_work && COPYFILE_DISABLE=1 tar $TAR_CREATE_FLAGS -czf $bundle_name.tar.gz $bundle_name)"
    if [[ -n "$GPG_KEY_ID" ]]; then
      echo "  [dry-run] gpg --batch --yes --local-user $GPG_KEY_ID --detach-sign --output $tarball.sig $tarball"
    fi
    if [[ "$SYNC" == "1" ]]; then
      echo "  [dry-run] python3 $LOCT_IO_ROOT/scripts/sync_releases.py --version $VERSION --target $target --tarball $tarball --components-json $components_json"
    fi
    continue
  fi

  [[ "$WORK_DIR" != "/" && "$target_work" == "$WORK_DIR/"* ]] \
    || die "refusing to clean unsafe work dir: $target_work"
  rm -rf "$target_work"
  mkdir -p "$staging/bin" "$staging/components"

  make -C "$SUITE_ROOT" release-binaries \
    STAGING_DIR="$staging" \
    TARGET="$target" \
    CODESIGN="$codesign_mode"
  if [[ "$bundle_flavor" == "core" ]]; then
    write_aicx_optional_metadata "$staging/components/loctree-aicx.json" "$target"
  elif [[ -n "$AICX_ROOT" ]]; then
    make -C "$AICX_ROOT" release-binaries \
      STAGING_DIR="$staging" \
      TARGET="$target" \
      CODESIGN="$codesign_mode"
  else
    stage_aicx_from_release "$target" "$staging" "$target_work/aicx-download" "$codesign_mode"
  fi

  for bin in "${bundle_binaries[@]}"; do
    [[ -x "$staging/bin/$bin" ]] || die "missing staged binary: $staging/bin/$bin"
  done

  merge_components "$staging/components" "$components_json"
  write_bundle_readme "$staging/README.md" "$target" "$bundle_flavor"
  write_bundle_checksums "$staging" "$staging/CHECKSUMS.sha256" "${bundle_binaries[@]}"

  rm -f "$tarball" "$tarball.sha256" "$tarball.sig" "$tarball_tmp"
  # Keep the archive operand relative. GNU tar treats `D:\...` as remote
  # `host:path` syntax on Git Bash even though it is a local Windows drive.
  # Word-splitting on TAR_CREATE_FLAGS is intentional: tar_create_flags emits
  # only flag tokens (--no-xattrs, --no-mac-metadata).
  # shellcheck disable=SC2086
  (cd "$target_work" && COPYFILE_DISABLE=1 tar $TAR_CREATE_FLAGS -czf "$bundle_name.tar.gz" "$bundle_name")
  mv "$tarball_tmp" "$tarball"
  printf "%s  %s\n" "$(sha256_file "$tarball")" "$(basename "$tarball")" > "$tarball.sha256"
  echo "  tarball -> $tarball"
  echo "  sha256  -> $tarball.sha256"

  signature_args=()
  if [[ -n "$GPG_KEY_ID" ]]; then
    gpg --batch --yes --local-user "$GPG_KEY_ID" --detach-sign --output "$tarball.sig" "$tarball"
    signature_args+=(--signature-file "$tarball.sig")
    echo "  sig     -> $tarball.sig"
  fi
  if [[ -n "$SIGNATURE_CLAIM" ]]; then
    signature_args+=(--signature "$SIGNATURE_CLAIM")
  fi

  if [[ "$SYNC" == "1" ]]; then
    sync_args=(
      --version "$VERSION"
      --target "$target"
      --tarball "$tarball"
      --components-json "$components_json"
      --channel "$CHANNEL"
    )
    if [[ "$MAKE_CURRENT" == "1" ]]; then
      sync_args+=(--make-current)
    fi
    python3 "$LOCT_IO_ROOT/scripts/sync_releases.py" "${sync_args[@]}" "${signature_args[@]}"
  fi
done

echo "Done: $DIST_DIR"
