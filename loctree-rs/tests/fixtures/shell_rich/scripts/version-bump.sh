#!/usr/bin/env bash
# Flexible version bump script with scoped targets and full crates awareness.
# Usage: ./scripts/version-bump.sh [OPTIONS]
#
# Version options:
#   --patch           Bump patch version (default)
#   --minor           Bump minor version
#   --major           Bump major version
#   --set VERSION     Set exact version (e.g., --set 0.8.0)
#
# Scope options:
#   --all             All crates (default)
#   --loctree         Only loctree crate
#   --report          Only report-leptos crate
#   --landing         Only landing page crate
#   --mcp             Only loctree-mcp crate
#   --lsp             Only loctree-lsp crate
#   --report-wasm     Only report-wasm crate
#   --rmcp-common     Only rmcp-common crate
#
# Suffix options:
#   --dev             Add -dev suffix
#   --rc              Add -rc suffix
#   --alpha           Add -alpha suffix
#   --beta            Add -beta suffix
#
# Behavior options:
#   --deps            Update dependencies (cargo update + show outdated)
#   --tag             Create git tag after commit
#   --push            Push to remote after commit
#   --dry-run         Preview changes without applying
#   --check           Alias for --dry-run
#   --force           Skip dirty tree check
#   --no-test         Skip tests (faster, use carefully)
#   --no-publish      Skip cargo publish even if token available
#   --interactive     Confirm before publish
#   --show-deps       Show workspace dependency graph
#
# Examples:
#   ./scripts/version-bump.sh --minor --loctree --tag --push
#   ./scripts/version-bump.sh --deps --check
#   ./scripts/version-bump.sh --set 1.0.0 --all --tag
#   ./scripts/version-bump.sh --show-deps
#
# Created by M&K ⓒ 2025-2026 The Loctree Team

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m' # No Color

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Crate definitions (compatible with bash 3.x)
# Format: name|path|publishable|deps
# Current crate-publish reality stays on legacy crates.io names until the
# dedicated rename tracks land. Thin releases, npm, and Homebrew use the
# active loct / loct-mcp / loct-lsp contract separately.
# Publish order: report-leptos → loctree → loctree-mcp (dependency chain)
CRATE_LIST=(
  "report-leptos|reports|yes|"
  "loctree|loctree-rs|yes|report-leptos"
  "report-wasm|reports/wasm|no|report-leptos"
  "landing-page|landing|no|loctree,report-leptos"
  "loctree-mcp|loctree-mcp|yes|loctree"
  "loctree-lsp|loctree_lsp|no|loctree"
  "rmcp-common|rmcp-common|no|"
)

# Helper to get crate field
get_crate_field() {
  local name="$1"
  local field="$2"  # 1=path, 2=publishable, 3=deps
  for entry in "${CRATE_LIST[@]}"; do
    local crate_name="${entry%%|*}"
    if [[ "$crate_name" == "$name" ]]; then
      local rest="${entry#*|}"
      case "$field" in
        path) echo "${rest%%|*}" ;;
        pub)
          rest="${rest#*|}"
          echo "${rest%%|*}"
          ;;
        deps)
          rest="${rest#*|}"
          rest="${rest#*|}"
          echo "$rest"
          ;;
      esac
      return
    fi
  done
}

# Get all crate names
get_all_crates() {
  for entry in "${CRATE_LIST[@]}"; do
    echo "${entry%%|*}"
  done
}

# Default values
bump_type="patch"
bump_flag_set=false
explicit_version=""
scope="all"
sync_versions=true
dev_suffix=false
rc_suffix=false
alpha_suffix=false
beta_suffix=false
dry_run=false
force=false
update_deps=false
create_tag=false
push_after=false
skip_tests=false
skip_publish=false
interactive=false
show_deps_only=false

log_info() { echo -e "${BLUE}ℹ${NC} $*"; }
log_success() { echo -e "${GREEN}✓${NC} $*"; }
log_warn() { echo -e "${YELLOW}⚠${NC} $*"; }
log_error() { echo -e "${RED}✗${NC} $*" >&2; }
log_step() { echo -e "\n${BOLD}${CYAN}==> $*${NC}"; }
log_dim() { echo -e "${DIM}$*${NC}"; }

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --patch|--minor|--major)
      bump_type="${1#--}"
      bump_flag_set=true
      shift
      ;;
    --set)
      explicit_version="$2"
      bump_flag_set=true
      bump_type="explicit"
      shift 2
      ;;
    --all|--loctree|--report|--report-wasm|--landing|--mcp|--lsp|--rmcp-common)
      scope="${1#--}"
      shift
      ;;
    --dev) dev_suffix=true; shift ;;
    --rc) rc_suffix=true; shift ;;
    --alpha) alpha_suffix=true; shift ;;
    --beta) beta_suffix=true; shift ;;
    --dry-run|--check) dry_run=true; shift ;;
    --force) force=true; shift ;;
    --deps) update_deps=true; shift ;;
    --tag) create_tag=true; shift ;;
    --push) push_after=true; shift ;;
    --no-test) skip_tests=true; shift ;;
    --no-publish) skip_publish=true; shift ;;
    --interactive|-i) interactive=true; shift ;;
    --show-deps) show_deps_only=true; shift ;;
    --help|-h)
      head -50 "$0" | tail -n +2 | sed 's/^# //' | sed 's/^#//'
      exit 0
      ;;
    *)
      log_error "Unknown option: $1"
      echo "Use --help for usage information"
      exit 1
      ;;
  esac
done

# Resolve scope aliases
resolve_scope() {
  case "$1" in
    report) echo "report-leptos" ;;
    report-wasm) echo "report-wasm" ;;
    landing) echo "landing-page" ;;
    mcp) echo "loctree-mcp" ;;
    lsp) echo "loctree-lsp" ;;
    rmcp-common) echo "rmcp-common" ;;
    *) echo "$1" ;;
  esac
}

# Check if crate is in scope
is_in_scope() {
  local crate="$1"
  if [[ "$scope" == "all" ]]; then
    return 0
  fi
  local resolved=$(resolve_scope "$scope")
  [[ "$crate" == "$resolved" ]]
}

# Enforce workspace-wide version sync
if [[ "$scope" != "all" ]]; then
  log_warn "Uniform versioning enabled: all workspace crates will be updated to the loctree version. Scope only affects checks/publish."
fi

# Show workspace dependency graph
show_dependency_graph() {
  echo ""
  echo -e "${BOLD}Workspace Dependency Graph${NC}"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo ""

  for entry in "${CRATE_LIST[@]}"; do
    local crate="${entry%%|*}"
    local path=$(get_crate_field "$crate" "path")
    local publishable=$(get_crate_field "$crate" "pub")
    local deps=$(get_crate_field "$crate" "deps")
    local cargo_toml="$ROOT_DIR/$path/Cargo.toml"

    # Get current version
    local version=""
    if [[ -f "$cargo_toml" ]]; then
      version=$(grep '^version = ' "$cargo_toml" | head -1 | cut -d'"' -f2 || true)
      if [[ -z "$version" ]] && grep -q '^version\.workspace = true' "$cargo_toml"; then
        version=$(grep '^version = ' "$ROOT_DIR/Cargo.toml" | head -1 | cut -d'"' -f2 || true)
      fi
    fi
    [[ -z "$version" ]] && version="?"

    # Format crate info
    local pub_badge=""
    [[ "$publishable" == "yes" ]] && pub_badge="${GREEN}[pub]${NC}" || pub_badge="${DIM}[local]${NC}"

    printf "  ${BOLD}%-18s${NC} %b  ${CYAN}v%-10s${NC}" "$crate" "$pub_badge" "$version"

    if [[ -n "$deps" ]]; then
      echo -e " ${DIM}← depends on:${NC} ${MAGENTA}${deps//,/, }${NC}"
    else
      echo ""
    fi
  done

  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

  # Show external dependencies summary
  echo ""
  echo -e "${BOLD}Key External Dependencies${NC}"
  echo ""

  # Parse workspace Cargo.toml for key deps
  if [[ -f "$ROOT_DIR/Cargo.toml" ]]; then
    for dep in oxc leptos tokio serde regex toml thiserror rmcp; do
      local ver=$(grep -E "^${dep}[^a-z].*version" "$ROOT_DIR/Cargo.toml" 2>/dev/null | head -1 | grep -oE '"[0-9]+\.[0-9]+[^"]*"' | tr -d '"' || echo "")
      if [[ -n "$ver" ]]; then
        printf "  %-15s ${CYAN}%s${NC}\n" "$dep" "$ver"
      fi
    done
  fi

  echo ""
}

# Show deps and exit if requested
if $show_deps_only; then
  show_dependency_graph
  exit 0
fi

# Validate semver format
validate_semver() {
  local ver="$1"
  if [[ ! "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?$ ]]; then
    log_error "Invalid semver format: $ver"
    exit 1
  fi
}

if [[ -n "$explicit_version" ]]; then
  validate_semver "$explicit_version"
fi

# If --dev/--rc/--alpha/--beta is set without an explicit bump flag, keep current version
if { $dev_suffix || $rc_suffix || $alpha_suffix || $beta_suffix; } && ! $bump_flag_set; then
  bump_type="none"
fi

# Verify we're in the right directory
if [[ ! -f "$ROOT_DIR/loctree-rs/Cargo.toml" ]]; then
  log_error "Run this script from the repository root."
  exit 1
fi

# Check for clean tree (unless --force)
if ! $force; then
  if ! git -C "$ROOT_DIR" diff --quiet || ! git -C "$ROOT_DIR" diff --cached --quiet; then
    log_error "Working tree is dirty. Commit/stash changes first, or use --force."
    exit 1
  fi
fi

# Version manipulation functions
bump_version() {
  local current="$1" kind="$2"
  # Strip existing suffixes
  current="${current%-dev}"
  current="${current%-rc}"
  current="${current%-alpha}"
  current="${current%-beta}"

  if [[ "$kind" == "none" ]]; then
    echo "$current"
    return
  fi
  if [[ "$kind" == "explicit" ]]; then
    echo "$explicit_version"
    return
  fi

  IFS='.' read -r major minor patch <<<"$current"
  case "$kind" in
    patch) patch=$((patch + 1)) ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    major) major=$((major + 1)); minor=0; patch=0 ;;
  esac
  echo "${major}.${minor}.${patch}"
}

read_version() {
  local file="$1"
  # All crates use version.workspace = true — read from workspace root
  if grep -q 'version\.workspace\s*=\s*true' "$file" 2>/dev/null; then
    grep '^version = ' "$ROOT_DIR/Cargo.toml" | head -1 | cut -d'"' -f2
  else
    grep '^version = ' "$file" | head -1 | cut -d'"' -f2
  fi
}

update_sed() {
  local file="$1" pattern="$2"
  if [[ -f "$file" ]]; then
    if $dry_run; then
      log_info "Would update: $file"
    else
      if sed --version 2>/dev/null | grep -q GNU; then
        sed -i "$pattern" "$file"
      else
        sed -i '' "$pattern" "$file"
      fi
      log_success "Updated: $file"
    fi
  fi
}

# Apply suffixes to version
apply_suffix() {
  local ver="$1"
  $dev_suffix && ver="${ver%-dev}-dev"
  $rc_suffix && ver="${ver%-rc}-rc"
  $alpha_suffix && ver="${ver%-alpha}-alpha"
  $beta_suffix && ver="${ver%-beta}-beta"
  echo "$ver"
}

# Build version arrays using temp files (bash 3.x compatible)
VERSIONS_FILE=$(mktemp)
NEW_VERSIONS_FILE=$(mktemp)
trap "rm -f $VERSIONS_FILE $NEW_VERSIONS_FILE" EXIT

loctree_current_ver=$(read_version "$ROOT_DIR/loctree-rs/Cargo.toml")
loctree_target_ver=$(bump_version "$loctree_current_ver" "$bump_type")
loctree_target_ver=$(apply_suffix "$loctree_target_ver")

for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  path=$(get_crate_field "$crate" "path")
  cargo_toml="$ROOT_DIR/$path/Cargo.toml"

  if [[ -f "$cargo_toml" ]]; then
    current_ver=$(read_version "$cargo_toml")
  else
    current_ver="0.0.0"
  fi

  echo "$crate=$current_ver" >> "$VERSIONS_FILE"
  echo "$crate=$loctree_target_ver" >> "$NEW_VERSIONS_FILE"
done

# Helper to get version from file
get_version() {
  local crate="$1"
  local file="$2"
  grep "^${crate}=" "$file" | cut -d'=' -f2
}

# Print summary
echo ""
echo -e "${BOLD}Version Bump Summary${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
printf "%-18s │ %-12s │ %-12s │ %-8s │ %s\n" "Crate" "Current" "New" "Status" "Deps"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Print in dependency order
for crate in loctree report-leptos report-wasm loctree-mcp loctree-lsp rmcp-common landing-page; do
  old=$(get_version "$crate" "$VERSIONS_FILE")
  new=$(get_version "$crate" "$NEW_VERSIONS_FILE")
  deps=$(get_crate_field "$crate" "deps")

  if [[ "$old" != "$new" ]]; then
    status="bump"
    color="$GREEN"
  else
    status="keep"
    color="$BLUE"
  fi

  printf "%-18s │ %-12s │ %-12s │ ${color}%-8s${NC} │ ${DIM}%s${NC}\n" "$crate" "$old" "$new" "$status" "${deps:-none}"
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Show options
loctree_new_ver=$(get_version "loctree" "$NEW_VERSIONS_FILE")
echo -e "${BOLD}Options:${NC} bump=$bump_type scope=$scope"
$dry_run && echo -e "  ${YELLOW}--dry-run${NC} mode (no changes will be made)"
$update_deps && echo -e "  ${CYAN}--deps${NC} will update dependencies"
$create_tag && echo -e "  ${CYAN}--tag${NC} will create git tag v$loctree_new_ver"
$push_after && echo -e "  ${CYAN}--push${NC} will push to remote"
$skip_tests && echo -e "  ${YELLOW}--no-test${NC} skipping tests"
$skip_publish && echo -e "  ${YELLOW}--no-publish${NC} skipping publish"
echo ""

# Dry run exits here
if $dry_run; then
  log_warn "Dry run - no changes made"
  exit 0
fi

# Update dependencies if requested
if $update_deps; then
  log_step "Updating dependencies"
  cargo update --manifest-path "$ROOT_DIR/Cargo.toml"

  log_step "Checking for outdated dependencies"
  if command -v cargo-outdated &> /dev/null; then
    cargo outdated --manifest-path "$ROOT_DIR/Cargo.toml" --depth 1 || true
  else
    log_warn "cargo-outdated not installed. Install with: cargo install cargo-outdated"
    log_info "Checking key dependencies manually..."

    echo ""
    echo -e "${BOLD}Checking key dependencies:${NC}"
    for dep in oxc_parser leptos tokio; do
      latest=$(cargo search "$dep" --limit 1 2>/dev/null | head -1 | grep -oE '"[0-9]+\.[0-9]+\.[0-9]+"' | tr -d '"' || echo "?")
      printf "  %-20s latest: ${CYAN}%s${NC}\n" "$dep" "$latest"
    done
  fi
fi

# Generate changelog BEFORE updating versions
# Scans conventional commits since last tag (max 100 commits to avoid hanging)
generate_changelog_entry() {
  local version="$1"
  local today
  today=$(date +%Y-%m-%d)
  local last_tag
  last_tag=$(git -C "$ROOT_DIR" describe --tags --abbrev=0 2>/dev/null || echo "")

  echo "## [$version] - $today"
  echo ""

  local added=""
  local changed=""
  local fixed=""
  local security=""

  # Get commits - use temp file to avoid process substitution issues with set -e
  local tmp_commits
  tmp_commits=$(mktemp)

  if [[ -n "$last_tag" ]]; then
    git -C "$ROOT_DIR" log --oneline -100 "${last_tag}..HEAD" 2>/dev/null > "$tmp_commits" || true
  else
    git -C "$ROOT_DIR" log --oneline -50 2>/dev/null > "$tmp_commits" || true
  fi

  while IFS= read -r commit || [[ -n "$commit" ]]; do
    [[ -z "$commit" ]] && continue
    local subject="${commit#* }"
    local msg=""

    case "$subject" in
      feat:*|feat\(*\):*)
        msg="${subject#feat}"
        msg="${msg#\(*\):}"
        msg="${msg#:}"
        msg="${msg# }"
        added="${added}- ${msg}"$'\n'
        ;;
      fix:*|fix\(*\):*)
        msg="${subject#fix}"
        msg="${msg#\(*\):}"
        msg="${msg#:}"
        msg="${msg# }"
        fixed="${fixed}- ${msg}"$'\n'
        ;;
      refactor:*|refactor\(*\):*|perf:*|perf\(*\):*)
        msg="${subject#refactor}"
        msg="${msg#perf}"
        msg="${msg#\(*\):}"
        msg="${msg#:}"
        msg="${msg# }"
        changed="${changed}- ${msg}"$'\n'
        ;;
      security:*|security\(*\):*)
        msg="${subject#security}"
        msg="${msg#\(*\):}"
        msg="${msg#:}"
        msg="${msg# }"
        security="${security}- ${msg}"$'\n'
        ;;
      *BREAKING*|*breaking*|*!:*)
        changed="${changed}- **BREAKING**: ${subject}"$'\n'
        ;;
    esac
  done < "$tmp_commits"

  rm -f "$tmp_commits"

  [[ -n "$added" ]] && echo "### Added" && printf "%s\n" "$added"
  [[ -n "$changed" ]] && echo "### Changed" && printf "%s\n" "$changed"
  [[ -n "$fixed" ]] && echo "### Fixed" && printf "%s\n" "$fixed"
  [[ -n "$security" ]] && echo "### Security" && printf "%s\n" "$security"

  # Ensure function returns 0 (last [[ -n ]] might return 1 if empty)
  return 0
}

# Update changelog first (so it's included in the commit)
if [[ -f "$ROOT_DIR/CHANGELOG.md" ]]; then
  log_step "Generating changelog entry"
  changelog_entry=$(generate_changelog_entry "$loctree_new_ver")

  if [[ -n "$changelog_entry" ]]; then
    # Insert new entry before the first version heading (## [x.y.z])
    # Use temp file for multiline entry (awk -v breaks on newlines)
    entry_file=$(mktemp)
    echo "$changelog_entry" > "$entry_file"

    awk -v entry_file="$entry_file" '
      /^## \[[0-9]/ && !inserted {
        while ((getline line < entry_file) > 0) print line
        close(entry_file)
        print ""
        inserted = 1
      }
      { print }
    ' "$ROOT_DIR/CHANGELOG.md" > "$ROOT_DIR/CHANGELOG.md.tmp"

    rm -f "$entry_file"
    mv "$ROOT_DIR/CHANGELOG.md.tmp" "$ROOT_DIR/CHANGELOG.md"
    log_success "Updated: CHANGELOG.md"
  else
    log_info "No conventional commits found since last tag"
  fi
fi

# Update loctree + UI (only when loctree is included)
if [[ -n "$loctree_target_ver" ]]; then
  log_step "Updating loctree version"
  "$ROOT_DIR/scripts/sync-version.sh" "$loctree_target_ver"
fi

# Update workspace root Cargo.toml — [workspace.package] version
# (subcrates with `version.workspace = true` inherit from this)
if [[ -n "$loctree_target_ver" ]]; then
  log_step "Updating workspace root Cargo.toml"

  # Update [workspace.package] version — awk with section awareness
  awk -v new_ver="$loctree_target_ver" '
    /^\[workspace\.package\]$/ { in_pkg = 1; print; next }
    /^\[/ && in_pkg { in_pkg = 0 }
    in_pkg && /^version = / { print "version = \"" new_ver "\""; next }
    { print }
  ' "$ROOT_DIR/Cargo.toml" > "$ROOT_DIR/Cargo.toml.tmp" \
    && mv "$ROOT_DIR/Cargo.toml.tmp" "$ROOT_DIR/Cargo.toml"
  log_dim "  Updated [workspace.package] version → $loctree_target_ver"

  # Update [workspace.dependencies] internal path deps (loctree, report-leptos)
  # These carry { path = "...", version = "X" } — keep in sync with workspace.package
  for dep in loctree report-leptos; do
    dep_ver=$(get_version "$dep" "$NEW_VERSIONS_FILE")
    [[ -z "$dep_ver" ]] && dep_ver="$loctree_target_ver"
    if sed --version 2>/dev/null | grep -q GNU; then
      sed -i -E "s|(^${dep}[[:space:]]*=[[:space:]]*\{[^}]*version[[:space:]]*=[[:space:]]*\")[^\"]*(\")|\\1${dep_ver}\\2|" "$ROOT_DIR/Cargo.toml"
    else
      sed -i '' -E "s|(^${dep}[[:space:]]*=[[:space:]]*\{[^}]*version[[:space:]]*=[[:space:]]*\")[^\"]*(\")|\\1${dep_ver}\\2|" "$ROOT_DIR/Cargo.toml"
    fi
    log_dim "  Updated workspace.dependencies.${dep} path-dep version → $dep_ver"
  done
fi

# Update other crates' Cargo.toml versions
# Note: crates using version.workspace = true inherit from root (already updated by sync-version.sh)
log_step "Updating crate versions"

for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  if [[ "$crate" != "loctree" ]]; then
    path=$(get_crate_field "$crate" "path")
    cargo_toml="$ROOT_DIR/$path/Cargo.toml"
    new_ver=$(get_version "$crate" "$NEW_VERSIONS_FILE")
    # Skip crates that inherit version from workspace
    if [[ -f "$cargo_toml" ]] && grep -q 'version\.workspace\s*=\s*true' "$cargo_toml" 2>/dev/null; then
      log_dim "  $crate: inherits version.workspace (skipped)"
    else
      update_sed "$cargo_toml" 's/^version = ".*"/version = "'"$new_ver"'"/'
    fi
  fi
done

# Update internal dependency references
log_step "Updating internal dependency references"

update_internal_dep() {
  local cargo_toml="$1"
  local dep_name="$2"
  local new_ver="$3"

  if [[ -f "$cargo_toml" ]] && grep -q "$dep_name" "$cargo_toml"; then
    if sed --version 2>/dev/null | grep -q GNU; then
      sed -i -E "s/^(${dep_name}[[:space:]]*=[[:space:]]*\")([^\"]*)\"/\\1${new_ver}\"/" "$cargo_toml"
      sed -i -E "s/(${dep_name}[[:space:]]*=\\s*\\{[^}]*version\\s*=\\s*\")([^\"]*)\"/\\1${new_ver}\"/" "$cargo_toml"
    else
      sed -i '' -E "s/^(${dep_name}[[:space:]]*=[[:space:]]*\")([^\"]*)\"/\\1${new_ver}\"/" "$cargo_toml"
      sed -i '' -E "s/(${dep_name}[[:space:]]*=\\s*\\{[^}]*version\\s*=\\s*\")([^\"]*)\"/\\1${new_ver}\"/" "$cargo_toml"
    fi
    log_dim "  Updated $dep_name → v$new_ver in $cargo_toml"
  fi
}

# Update cross-references for all bumped crates
for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  new_ver=$(get_version "$crate" "$NEW_VERSIONS_FILE")

  # Find all crates that depend on this one
  for other_entry in "${CRATE_LIST[@]}"; do
    other_crate="${other_entry%%|*}"
    deps=$(get_crate_field "$other_crate" "deps")
    if [[ "$deps" == *"$crate"* ]]; then
      path=$(get_crate_field "$other_crate" "path")
      update_internal_dep "$ROOT_DIR/$path/Cargo.toml" "$crate" "$new_ver"
    fi
  done
done

# Refresh Cargo.lock so it reflects bumped workspace crate versions.
# Per-crate clippy with --quiet may not trigger a full workspace lock refresh
# when only the workspace root version changed, so we force it here.
log_step "Refreshing Cargo.lock"
if cargo check --workspace --all-targets --quiet --manifest-path "$ROOT_DIR/Cargo.toml" 2>&1 | tail -3; then
  log_success "Cargo.lock refreshed"
else
  log_warn "cargo check during lock refresh emitted warnings; continuing"
fi

# Quality checks
log_step "Running quality checks"

# Format all in parallel
log_info "Formatting..."
for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  if is_in_scope "$crate"; then
    path=$(get_crate_field "$crate" "path")
    cargo fmt --manifest-path "$ROOT_DIR/$path/Cargo.toml" 2>/dev/null &
  fi
done
wait
log_success "Format complete"

# Clippy
log_info "Running clippy..."
for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  if is_in_scope "$crate"; then
    path=$(get_crate_field "$crate" "path")
    echo -e "  ${DIM}Checking $crate...${NC}"
    cargo clippy --manifest-path "$ROOT_DIR/$path/Cargo.toml" --all-targets --quiet -- -D warnings
  fi
done
log_success "Clippy passed"

# Tests (unless --no-test)
if ! $skip_tests; then
  log_info "Running tests..."
  for entry in "${CRATE_LIST[@]}"; do
    crate="${entry%%|*}"
    if is_in_scope "$crate"; then
      path=$(get_crate_field "$crate" "path")
      echo -e "  ${DIM}Testing $crate...${NC}"
      # Some crates only build, don't have tests
      if [[ "$crate" == "loctree-server" ]] || [[ "$crate" == "landing-page" ]]; then
        cargo build --manifest-path "$ROOT_DIR/$path/Cargo.toml" --quiet
      else
        cargo test --manifest-path "$ROOT_DIR/$path/Cargo.toml" --quiet
      fi
    fi
  done
  log_success "Tests passed"
else
  log_warn "Tests skipped (--no-test)"
fi

# Publish crates (in dependency order)
if ! $skip_publish; then
  for crate in report-leptos loctree loctree-mcp; do
    if is_in_scope "$crate"; then
      publishable=$(get_crate_field "$crate" "pub")
      if [[ "$publishable" != "yes" ]]; then
        continue
      fi

      path=$(get_crate_field "$crate" "path")
      new_ver=$(get_version "$crate" "$NEW_VERSIONS_FILE")

      log_step "Build release ($crate)"
      cargo build --manifest-path "$ROOT_DIR/$path/Cargo.toml" --release --quiet

      if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
        log_warn "CARGO_REGISTRY_TOKEN not set; skipping publish for $crate"
        continue
      fi

      if $interactive; then
        echo ""
        read -p "Publish $crate v$new_ver to crates.io? [y/N] " -n 1 -r
        echo ""
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
          log_warn "Publish skipped for $crate"
          continue
        fi
      fi

      log_step "Publishing $crate v$new_ver"
      cargo publish --manifest-path "$ROOT_DIR/$path/Cargo.toml" --locked || {
        log_warn "Publish failed for $crate (may already exist)"
      }
      log_success "Published $crate to crates.io"

      # Wait for crates.io index to update before publishing dependents
      if [[ "$crate" != "loctree-mcp" ]]; then
        log_info "Waiting for crates.io index update (15s)..."
        sleep 15
      fi
    fi
  done
fi

# Git commit
log_step "Git commit"
git -C "$ROOT_DIR" add -A

# Build commit message with all changed versions
commit_parts=""
for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  if is_in_scope "$crate"; then
    new_ver=$(get_version "$crate" "$NEW_VERSIONS_FILE")
    commit_parts="$commit_parts$crate=$new_ver "
  fi
done

git -C "$ROOT_DIR" commit -m "chore(release): bump versions

$commit_parts

Created by M&K ⓒ 2025-2026 The Loctree Team"

log_success "Committed version bump"

# Create tag (based on loctree version)
if $create_tag; then
  tag_name="v$loctree_new_ver"

  if git -C "$ROOT_DIR" rev-parse "$tag_name" >/dev/null 2>&1; then
    log_warn "Tag $tag_name already exists, skipping"
  else
    log_step "Creating tag $tag_name"
    git -C "$ROOT_DIR" tag -a "$tag_name" -m "Release $tag_name"
    log_success "Created tag $tag_name"
  fi
fi

# Push
if $push_after; then
  log_step "Pushing to remote"
  git -C "$ROOT_DIR" push origin HEAD
  log_success "Pushed commits"

  if $create_tag; then
    git -C "$ROOT_DIR" push origin "v$loctree_new_ver" 2>/dev/null || true
    log_success "Pushed tag v$loctree_new_ver"
  fi
fi

# Final summary
echo ""
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}${GREEN}  Version bump complete!${NC}"
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

if ! $push_after; then
  log_info "Remember to push:"
  echo "  git push origin HEAD"
  $create_tag && echo "  git push origin v$loctree_new_ver"
fi

if ! $create_tag; then
  log_info "To create a tag:"
  echo "  git tag -a v$loctree_new_ver -m 'Release v$loctree_new_ver'"
  echo "  git push origin v$loctree_new_ver"
fi

# Show what was bumped
echo ""
log_info "Bumped crates:"
for entry in "${CRATE_LIST[@]}"; do
  crate="${entry%%|*}"
  if is_in_scope "$crate"; then
    old=$(get_version "$crate" "$VERSIONS_FILE")
    new=$(get_version "$crate" "$NEW_VERSIONS_FILE")
    echo -e "  ${CYAN}$crate${NC}: $old → ${GREEN}$new${NC}"
  fi
done
