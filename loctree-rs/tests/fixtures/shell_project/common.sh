#!/bin/bash
# Shared helpers sourced by other scripts.

export LOCTREE_VERSION="0.9.0"
export LOCTREE_PREFIX="/usr/local"

log_info() {
    echo "[info] $*"
}

log_error() {
    echo "[error] $*" 1>&2
}

function is_installed() {
    command -v "$1" >/dev/null 2>&1
}
