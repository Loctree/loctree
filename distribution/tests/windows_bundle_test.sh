#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TMP_ROOT=$(mktemp -d)
trap 'rm -rf "$TMP_ROOT"' EXIT

workflow="$ROOT_DIR/.github/workflows/release-bundles.yml"
grep -F 'bash_runner_temp="$(cygpath -u "$RUNNER_TEMP")"' "$workflow" >/dev/null
grep -F 'verify_dir="$BASH_RUNNER_TEMP/bundle-verify"' "$workflow" >/dev/null
grep -F 'extract_dir="$BASH_RUNNER_TEMP/version-smoke"' "$workflow" >/dev/null
grep -F 'work="$BASH_RUNNER_TEMP/lsp-asset"' "$workflow" >/dev/null
if grep -E '(verify_dir|extract_dir|work)="\$RUNNER_TEMP/(bundle-verify|version-smoke|lsp-asset)"' "$workflow" >/dev/null; then
  echo "Windows tar paths must use the Git Bash-normalized runner temp" >&2
  exit 1
fi

mkdir -p "$TMP_ROOT/bin" "$TMP_ROOT/release-payload"

cat > "$TMP_ROOT/bin/make" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

repo=""
staging=""
target=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -C) repo="$2"; shift 2 ;;
    STAGING_DIR=*) staging="${1#STAGING_DIR=}"; shift ;;
    TARGET=*) target="${1#TARGET=}"; shift ;;
    *) shift ;;
  esac
done
[[ -n "$repo" && -n "$staging" && -n "$target" ]]
mkdir -p "$staging/bin" "$staging/components"
bins=(loct loctree loctree-mcp loctree-lsp)
component=loctree
suffix=""
if [[ "$target" == *-windows-* ]]; then
  suffix=.exe
fi
for bin in "${bins[@]}"; do
  printf '#!/usr/bin/env bash\nprintf "%s test\\n"\n' "$bin" > "$staging/bin/$bin$suffix"
  chmod +x "$staging/bin/$bin$suffix"
done
printf '{"source":"test","components":[{"name":"%s"}]}\n' "$component" \
  > "$staging/components/$component.json"
SH
chmod +x "$TMP_ROOT/bin/make"

cat > "$TMP_ROOT/bin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

url=""
output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
[[ -n "$url" && -n "$output" ]]
cp "$FAKE_RELEASE_DIR/${url##*/}" "$output"
SH
chmod +x "$TMP_ROOT/bin/curl"

archive_name="aicx-v0.12.5-x86_64-pc-windows-msvc-slim.zip"
python3 - "$TMP_ROOT/release-payload/$archive_name" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1], "w") as archive:
    archive.writestr("aicx-v0.12.5/aicx.exe", "aicx test\n")
    archive.writestr("aicx-v0.12.5/aicx-mcp.exe", "aicx-mcp test\n")
PY
if command -v shasum >/dev/null 2>&1; then
  archive_sha=$(shasum -a 256 "$TMP_ROOT/release-payload/$archive_name" | awk '{print $1}')
else
  archive_sha=$(sha256sum "$TMP_ROOT/release-payload/$archive_name" | awk '{print $1}')
fi
printf '%s  %s\n' "$archive_sha" "$archive_name" \
  > "$TMP_ROOT/release-payload/$archive_name.sha256"

# Model Git Bash checksum behavior without depending on the host platform.
# The production helper must hash stdin; passing a Windows filename directly
# lets GNU checksum tools prefix an otherwise correct digest with `\`.
cat > "$TMP_ROOT/bin/shasum" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$#" -eq 2 && "$1" == "-a" && "$2" == "256" ]]
cat >/dev/null
printf '%s  -\n' "$FAKE_ARCHIVE_SHA"
SH
chmod +x "$TMP_ROOT/bin/shasum"

real_tar=$(command -v tar)
cat > "$TMP_ROOT/bin/tar" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
args=("$@")
creating=0
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[$i]}" == "-czf" ]]; then
    creating=1
    archive="${args[$((i + 1))]}"
    [[ "$archive" != */* && "$archive" != *:* ]]
  fi
done
if [[ -n "${TAR_LOG:-}" ]]; then
  printf '%s\n' "${args[@]}" >> "$TAR_LOG"
  if [[ "$creating" == 1 ]]; then
    printf 'COPYFILE_DISABLE=%s\n' "${COPYFILE_DISABLE:-}" >> "$TAR_LOG"
  fi
  printf '\n' >> "$TAR_LOG"
fi
exec "$REAL_TAR" "$@"
SH
chmod +x "$TMP_ROOT/bin/tar"

asset() {
  bash "$ROOT_DIR/distribution/build-bundle.sh" 0.14.4 \
    --aicx-version 0.12.5 \
    --no-sync \
    --dry-run \
    --print-aicx-asset "$1"
}

[[ "$(asset aarch64-apple-darwin)" == "aicx-v0.12.5-aarch64-apple-darwin-slim.zip" ]]
[[ "$(asset x86_64-unknown-linux-gnu)" == "aicx-v0.12.5-x86_64-linux-gnu-slim.tar.gz" ]]
[[ "$(asset x86_64-pc-windows-msvc)" == "aicx-v0.12.5-x86_64-pc-windows-msvc-slim.zip" ]]

FAKE_RELEASE_DIR="$TMP_ROOT/release-payload" \
FAKE_ARCHIVE_SHA="$archive_sha" \
REAL_TAR="$real_tar" \
TAR_LOG="$TMP_ROOT/tar-invocations.txt" \
LOCTREE_GPG_KEY_ID="" \
PATH="$TMP_ROOT/bin:$PATH" bash "$ROOT_DIR/distribution/build-bundle.sh" 0.14.4 \
  --aicx-version 0.12.5 \
  --target x86_64-pc-windows-msvc \
  --bundle-flavor full \
  --bundle-suffix "" \
  --dist-dir "$TMP_ROOT/dist" \
  --work-dir "$TMP_ROOT/work" \
  --no-sync

archive="$TMP_ROOT/dist/loctree-0.14.4-x86_64-pc-windows-msvc.tar.gz"
root="loctree-0.14.4-x86_64-pc-windows-msvc"
[[ -s "$archive" ]]
tar -tzf "$archive" > "$TMP_ROOT/contents.txt"
for bin in loct.exe loctree.exe loctree-mcp.exe loctree-lsp.exe aicx.exe aicx-mcp.exe; do
  grep -Fx "$root/bin/$bin" "$TMP_ROOT/contents.txt" >/dev/null
done
grep -Fx "$root/CHECKSUMS.sha256" "$TMP_ROOT/contents.txt" >/dev/null
grep -Fx "$root/components.json" "$TMP_ROOT/contents.txt" >/dev/null
grep -Fx "$root/README.md" "$TMP_ROOT/contents.txt" >/dev/null
if grep -Fx "$root/bin/loct" "$TMP_ROOT/contents.txt" >/dev/null; then
  echo "Windows bundle contains an unsuffixed loct binary" >&2
  exit 1
fi

# macOS bsdtar otherwise embeds com.apple.provenance. The packager must ask
# tar to leave extended attributes out, and set COPYFILE_DISABLE for the
# older Apple copyfile tar. GNU tar documents --no-xattrs, so this host must
# see that flag on the create invocation.
grep -Fx -- '--no-xattrs' "$TMP_ROOT/tar-invocations.txt" >/dev/null
grep -Fx 'COPYFILE_DISABLE=1' "$TMP_ROOT/tar-invocations.txt" >/dev/null
if tar -tzf "$archive" 2>"$TMP_ROOT/tar-list.err" >/dev/null; then
  if grep -F 'LIBARCHIVE.xattr' "$TMP_ROOT/tar-list.err" >/dev/null; then
    echo "bundle tarball still carries macOS extended-attribute headers" >&2
    exit 1
  fi
fi

echo "Windows release bundle contract passed"
