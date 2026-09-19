#!/usr/bin/env bash
# install-local.sh — "npm install -g" from a local checkout, for repos that ship
# a wrapper package plus per-platform binary packages (the esbuild/swc pattern).
#
# Daily-driver counterpart of publish-release.sh: where that script ships
# checksum-verified release assets to the registry, this one stages the SAME
# wrapper+platform package graph from freshly built native binaries and runs a
# real `npm install -g` against local tarballs. The machine ends up with exactly
# the layout end users get: bin symlinks in <npm prefix>/bin, native binaries
# in the scoped platform package under <prefix>/lib/node_modules.
#
# Universal by construction — nothing below is Loctree-specific:
#   * the wrapper package.json provides package name/version and bin entry names
#   * the platform package skeleton (wrapper/platform-packages/<key>/) provides
#     the native binary list via the bin/* entries in its files[]
#   * the host platform key is detected from uname (override with --platform)
#   * native binaries come from --native-bin-dir (default: the cargo target dir)
#
# Reuse in another repo (e.g. AICX): keep the same distribution/npm/<wrapper>/
# layout with platform-packages/<key>/ inside and copy this script next to it,
# or pass --wrapper explicitly. Global packages from older naming schemes that
# own the same bin links can be uninstalled first via --legacy-names.
#
# Usage:
#   install-local.sh [--wrapper DIR] [--native-bin-dir DIR] [--platform KEY]
#                    [--legacy-names "pkg ..."] [--dry-run]
#
# Env overrides: WRAPPER_DIR, NATIVE_BIN_DIR, PLATFORM_KEY, NPM_LEGACY_NAMES,
#                SKIP_BIN_CHECK=1, SKIP_VERIFY=1.

set -euo pipefail

say()  { printf '==> %s\n' "$*"; }
warn() { printf 'warn: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

WRAPPER_DIR="${WRAPPER_DIR:-$SCRIPT_DIR/loct}"
NATIVE_BIN_DIR="${NATIVE_BIN_DIR:-}"
PLATFORM_KEY="${PLATFORM_KEY:-}"
LEGACY_NAMES="${NPM_LEGACY_NAMES:-}"
DRY_RUN=0

while [ $# -gt 0 ]; do
  case "$1" in
    --wrapper)        WRAPPER_DIR="${2:?--wrapper needs a value}"; shift 2 ;;
    --native-bin-dir) NATIVE_BIN_DIR="${2:?--native-bin-dir needs a value}"; shift 2 ;;
    --platform)       PLATFORM_KEY="${2:?--platform needs a value}"; shift 2 ;;
    --legacy-names)   LEGACY_NAMES="${2:?--legacy-names needs a value}"; shift 2 ;;
    --dry-run)        DRY_RUN=1; shift ;;
    -h|--help)        sed -n '2,30p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *)                die "unknown argument: $1 (try --help)" ;;
  esac
done

command -v node >/dev/null 2>&1 || die "node is required (it powers npm and JSON reads)"
command -v npm  >/dev/null 2>&1 || die "npm is required"

WRAPPER_DIR="$(cd "$WRAPPER_DIR" 2>/dev/null && pwd)" || die "wrapper dir not found: $WRAPPER_DIR"
[ -f "$WRAPPER_DIR/package.json" ] || die "no package.json in wrapper dir: $WRAPPER_DIR"

# Host platform key, matching one platform-packages/<key>/ skeleton.
detect_platform_key() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin)
      case "$arch" in
        arm64)  printf 'darwin-arm64' ;;
        x86_64) printf 'darwin-x64' ;;
        *)      return 1 ;;
      esac ;;
    Linux)
      case "$arch" in
        x86_64|amd64)  printf 'linux-x64-gnu' ;;
        aarch64|arm64) printf 'linux-arm64-gnu' ;;
        *)             return 1 ;;
      esac ;;
    *) return 1 ;;
  esac
}

if [ -z "$PLATFORM_KEY" ]; then
  PLATFORM_KEY="$(detect_platform_key)" \
    || die "unsupported host: $(uname -s)-$(uname -m); pass --platform explicitly"
fi
PLATFORM_SKELETON="$WRAPPER_DIR/platform-packages/$PLATFORM_KEY"
[ -f "$PLATFORM_SKELETON/package.json" ] \
  || die "no platform package skeleton for $PLATFORM_KEY (looked in $PLATFORM_SKELETON)"

# Native binaries: explicit flag/env wins, else the cargo target dirs.
if [ -z "$NATIVE_BIN_DIR" ]; then
  repo_root="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
  host_triple="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p' || true)"
  for candidate in \
    ${repo_root:+"$repo_root/target/${host_triple:-none}/release"} \
    ${repo_root:+"$repo_root/target/release"}; do
    if [ -d "$candidate" ]; then NATIVE_BIN_DIR="$candidate"; break; fi
  done
fi
[ -n "$NATIVE_BIN_DIR" ] && [ -d "$NATIVE_BIN_DIR" ] \
  || die "native bin dir not found; pass --native-bin-dir (e.g. make release-binaries STAGING_DIR=... first)"

# JSON reads go through node — guaranteed present wherever npm is.
read_json() {
  node -e 'const j=require(process.argv[1]);let v=j;for(const k of process.argv[2].split("."))v=v?.[k];process.stdout.write(typeof v==="string"?v:JSON.stringify(v??""))' "$1" "$2"
}

wrapper_name="$(read_json "$WRAPPER_DIR/package.json" name)"
wrapper_version="$(read_json "$WRAPPER_DIR/package.json" version)"
[ -n "$wrapper_name" ] && [ -n "$wrapper_version" ] \
  || die "wrapper package.json lacks name/version"

bin_names="$(node -e 'process.stdout.write(Object.keys(require(process.argv[1]).bin||{}).join(" "))' "$WRAPPER_DIR/package.json")"
[ -n "$bin_names" ] || die "wrapper package.json has no bin entries"

# The platform package declares the native binaries it ships as bin/* files[].
platform_bins="$(node -e 'const f=require(process.argv[1]).files||[];process.stdout.write(f.filter(p=>p.startsWith("bin/")).map(p=>p.slice(4)).join(" "))' "$PLATFORM_SKELETON/package.json")"
[ -n "$platform_bins" ] || die "platform package $PLATFORM_KEY lists no bin/* files"

say "wrapper:  $wrapper_name@$wrapper_version"
say "platform: $PLATFORM_KEY ($platform_bins)"
say "natives:  $NATIVE_BIN_DIR"

work="$(mktemp -d "${TMPDIR:-/tmp}/npm-install-local.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM

say "staging wrapper package"
mkdir -p "$work/wrapper"
( cd "$WRAPPER_DIR" && tar -cf - --exclude platform-packages --exclude node_modules --exclude '*.tgz' . ) \
  | tar -xf - -C "$work/wrapper"

say "staging $PLATFORM_KEY platform package"
mkdir -p "$work/platform/bin"
( cd "$PLATFORM_SKELETON" && tar -cf - . ) | tar -xf - -C "$work/platform"

# The wrapper's optionalDependencies pin the platform package by exact version,
# so the staged copy must carry the wrapper's version regardless of what the
# in-repo skeleton says (skeletons are synced at release time, not daily).
node -e 'const fs=require("fs");const p=process.argv[1],v=process.argv[2];const j=JSON.parse(fs.readFileSync(p,"utf8"));j.version=v;fs.writeFileSync(p,JSON.stringify(j,null,2)+"\n")' \
  "$work/platform/package.json" "$wrapper_version"

exe_suffix=""
case "$PLATFORM_KEY" in win32-*) exe_suffix=".exe" ;; esac
for bin in $platform_bins; do
  src="$NATIVE_BIN_DIR/${bin}${exe_suffix}"
  [ -f "$src" ] || die "missing native binary: $src"
  install -m 0755 "$src" "$work/platform/bin/${bin}${exe_suffix}"
done

if [ "${SKIP_BIN_CHECK:-0}" != "1" ]; then
  say "binary smoke check (--version)"
  for bin in $platform_bins; do
    "$work/platform/bin/${bin}${exe_suffix}" --version >/dev/null 2>&1 \
      || die "staged binary failed --version: $bin (SKIP_BIN_CHECK=1 to bypass)"
  done
fi

say "npm pack"
wrapper_tgz="$work/wrapper/$(cd "$work/wrapper" && npm pack --silent 2>/dev/null | tail -n 1)"
platform_tgz="$work/platform/$(cd "$work/platform" && npm pack --silent 2>/dev/null | tail -n 1)"
[ -s "$wrapper_tgz" ] && [ -s "$platform_tgz" ] || die "npm pack produced no tarballs"
say "packed: $(basename "$wrapper_tgz") + $(basename "$platform_tgz")"

if [ "$DRY_RUN" = "1" ]; then
  say "dry run — staged tree kept at $work"
  trap - EXIT INT TERM
  exit 0
fi

# Absolute tarball paths are mandatory: a relative path whose first segment
# looks like an owner gets parsed by npm as a git shorthand (owner/repo).
for legacy in $LEGACY_NAMES; do
  say "removing legacy global package: $legacy"
  npm uninstall -g "$legacy" >/dev/null 2>&1 || true
done

say "npm install -g (wrapper + platform)"
npm install -g "$wrapper_tgz" "$platform_tgz"

prefix="$(npm prefix -g)"
say "verifying bin entries under $prefix/bin"
fail=0
for bin in $bin_names; do
  link="$prefix/bin/$bin"
  if [ ! -x "$link" ]; then warn "missing bin entry: $link"; fail=1; continue; fi
  if [ "${SKIP_VERIFY:-0}" != "1" ]; then
    version_line="$("$link" --version 2>&1 | head -n 1 || true)"
    printf '    %-14s %s\n' "$bin" "${version_line:-<no --version output>}"
  fi
  resolved="$(command -v "$bin" 2>/dev/null || true)"
  if [ -n "$resolved" ] && [ "$resolved" != "$link" ]; then
    warn "$bin is shadowed on PATH by $resolved (earlier than $link)"
  fi
done
[ "$fail" = "0" ] || die "one or more bin entries did not install"

say "done — $wrapper_name@$wrapper_version installed via npm ($PLATFORM_KEY)"
