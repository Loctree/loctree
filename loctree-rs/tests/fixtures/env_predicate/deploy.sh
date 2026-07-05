#!/usr/bin/env bash
# W2-c fixture: locally-assigned SHOUTING_CASE vars + ANSI colors + shell
# builtins must produce ZERO orphan env reads; $DEPLOY_TOKEN stays a real one.
set -euo pipefail

APP_NAME="codescribe"
GREEN='\033[0;32m'
NC='\033[0m'
export RELEASE_CHANNEL="stable"

for TARGET in linux darwin; do
    echo "building ${TARGET}"
done

if [[ "${1:-}" =~ ^v([0-9]+) ]]; then
    echo "major ${BASH_REMATCH[1]}"
fi

echo "completing: ${COMP_WORDS[0]:-none}"
echo -e "${GREEN}${APP_NAME} ready${NC} (channel: ${RELEASE_CHANNEL})"

# Genuine environment contract — never assigned in this file.
echo "deploy key: ${DEPLOY_TOKEN}"
