#!/usr/bin/env bash

_check_health() {
    echo "Health ok"
    return 0
}

_info() {
    echo "INFO: $*"
}

_warn() {
    echo "WARN: $*" >&2
}