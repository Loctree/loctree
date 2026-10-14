#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/bin"
CALLS="$TEST_ROOT/cargo.calls"
SLEEPS="$TEST_ROOT/sleep.calls"
COUNT="$TEST_ROOT/count"

cat >"$TEST_ROOT/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$CALLS"
count=0
[[ ! -f "$COUNT" ]] || count="$(cat "$COUNT")"
count=$((count + 1))
printf '%s\n' "$count" >"$COUNT"
[[ "$count" -ge "${SUCCEED_ON:-999}" ]]
EOF

cat >"$TEST_ROOT/bin/sleep" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$SLEEPS"
EOF
chmod +x "$TEST_ROOT/bin/cargo" "$TEST_ROOT/bin/sleep"

export CALLS SLEEPS COUNT
export PATH="$TEST_ROOT/bin:$PATH"
export CRATES_IO_MAX_ATTEMPTS=5
export CRATES_IO_POLL_INTERVAL_SECONDS=0
export SUCCEED_ON=3

"$ROOT_DIR/scripts/wait-for-crates-io-version.sh" loctree 9.8.7
[[ "$(wc -l <"$CALLS" | tr -d ' ')" -eq 3 ]]
[[ "$(wc -l <"$SLEEPS" | tr -d ' ')" -eq 2 ]]
grep -Fxq 'info --registry crates-io loctree@9.8.7' "$CALLS"

: >"$CALLS"
: >"$SLEEPS"
: >"$COUNT"
export CRATES_IO_MAX_ATTEMPTS=2
export SUCCEED_ON=999
if "$ROOT_DIR/scripts/wait-for-crates-io-version.sh" loctree-mcp 9.8.7 \
	>"$TEST_ROOT/failure.out" 2>&1; then
	echo "permanent index miss unexpectedly succeeded" >&2
	exit 1
fi
grep -Fq 'did not reach the crates.io index after 2 attempts' "$TEST_ROOT/failure.out"
[[ "$(wc -l <"$CALLS" | tr -d ' ')" -eq 2 ]]
[[ "$(wc -l <"$SLEEPS" | tr -d ' ')" -eq 1 ]]

if "$ROOT_DIR/scripts/wait-for-crates-io-version.sh" '../bad' 9.8.7 >/dev/null 2>&1; then
	echo "unsafe crate name unexpectedly accepted" >&2
	exit 1
fi

# Publisher ordering belongs to the canonical suite, not this source mirror.
[[ ! -e "$ROOT_DIR/.github/workflows/publish.yml" ]]

echo "crates.io index wait contract: PASS"
