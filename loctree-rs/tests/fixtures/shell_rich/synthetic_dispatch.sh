#!/usr/bin/env bash
# Synthetic edge cases for shell semantic analyzer.
# Each block exercises a class of false positives that T1 ShellSemantics must handle.

set -euo pipefail

# --- 1. Sourced library APIs (must NOT be flagged as dead) ---
# shellcheck source=./lib_helpers.sh
. "$(dirname "$0")/lib_helpers.sh"

# --- 2. Idiom-only helpers (must classify as idiom, not flag as unused) ---
usage() {
    cat <<EOF
Usage: $0 <command> [args]
Commands: deploy | rollback | status
EOF
    exit 0
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

# --- 3. Case dispatch graph (handlers reachable through dispatch) ---
deploy_impl() {
    echo "Deploying..."
    _check_health || die "health check failed"
}

rollback_impl() {
    echo "Rolling back..."
}

status_impl() {
    echo "Status: ok"
}

main() {
    [[ $# -gt 0 ]] || usage
    case "$1" in
        deploy)   shift; deploy_impl "$@"   ;;
        rollback) shift; rollback_impl "$@" ;;
        status)   shift; status_impl "$@"   ;;
        *)        usage                     ;;
    esac
}

# --- 4. Function-pointer dispatch (handler held in variable) ---
choose_handler() {
    local kind="$1"
    local handler
    case "$kind" in
        prod)  handler="deploy_impl"  ;;
        stage) handler="rollback_impl" ;;
        *)     handler="usage"        ;;
    esac
    "$handler"
}

main "$@"