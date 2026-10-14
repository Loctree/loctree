#!/usr/bin/env bash
# Exercise the actual installer, stopping at an intentionally absent bundle.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
installer="${1:-$repo_root/public_dist/install.sh}"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/bin"
cat > "$scratch/bin/curl" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$INSTALLER_REQUESTS"
case "$*" in
  *index.json*) printf '{"current":"%s"}\n' "$INSTALLER_INDEX_VERSION" ;;
  *) exit 22 ;;
esac
SH
chmod +x "$scratch/bin/curl"
export INSTALLER_REQUESTS="$scratch/requests"
export INSTALLER_INDEX_VERSION=0.14.5
export PATH="$scratch/bin:$PATH"
export INSTALL_DIR="$scratch/install"
export LOCTREE_BASE_URL=https://installer.invalid/releases
export LOCTREE_NO_PROFILE_UPDATE=1
unset LOCTREE_VERSION LOCTREE_ALLOW_SOURCE_FALLBACK

if bash "$installer" > "$scratch/default.log" 2>&1; then
  echo 'an absent bundle must fail closed' >&2; exit 1
fi
grep -q 'version: 0.14.5' "$scratch/default.log"
grep -q '/releases/index.json' "$INSTALLER_REQUESTS"
grep -q '/releases/0.14.5/loctree-0.14.5-' "$INSTALLER_REQUESTS"
! grep -q '0.15.0-dev' "$INSTALLER_REQUESTS"

: > "$INSTALLER_REQUESTS"
if LOCTREE_VERSION=0.14.4 bash "$installer" > "$scratch/override.log" 2>&1; then
  exit 1
fi
grep -q 'version: 0.14.4' "$scratch/override.log"
! grep -q 'index.json' "$INSTALLER_REQUESTS"

: > "$INSTALLER_REQUESTS"
if LOCTREE_VERSION=0.15.0-dev bash "$installer" > "$scratch/invalid.log" 2>&1; then
  exit 1
fi
grep -q 'invalid stable release version' "$scratch/invalid.log"
test ! -s "$INSTALLER_REQUESTS"

if INSTALLER_INDEX_VERSION=0.15.0-dev bash "$installer" > "$scratch/index-invalid.log" 2>&1; then
  exit 1
fi
grep -q 'invalid stable release version' "$scratch/index-invalid.log"
echo 'installer registry contract: PASS'
