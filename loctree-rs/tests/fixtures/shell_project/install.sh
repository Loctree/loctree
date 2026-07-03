#!/bin/bash
# Entry point installer — sources shared helpers.

set -euo pipefail

source ./common.sh

export INSTALLER_NAME="loctree-installer"

install_all() {
    log_info "Installing ${LOCTREE_VERSION} to ${LOCTREE_PREFIX}"
    if is_installed curl; then
        log_info "curl present"
    else
        log_error "curl missing"
        exit 1
    fi
}

install_all "$@"
