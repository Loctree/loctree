#!/usr/bin/env bash
# Additional utilities, also sourced.

. ./common.sh

export UTILS_LOADED=1

fetch_tarball() {
    log_info "fetching $1"
}
