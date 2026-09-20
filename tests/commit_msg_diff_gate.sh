#!/usr/bin/env bash
# Fixtures for tools/hooks/lib/commit-msg-diff-gate.sh
#
# FIRE  = gate must emit findings (and exit 1 under STRICT=1)
# QUIET = gate must be silent and exit 0
#
# 060b4e0-shaped failure: subject type=test + production module staged.
# Honest fleet subjects ([agent/workflow] type:) must parse — Claude's draft
# almost shipped without that; we lock it here.
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

set -euo pipefail

ROOT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
GATE="$ROOT_DIR/tools/hooks/lib/commit-msg-diff-gate.sh"

if [ ! -x "$GATE" ]; then
    printf 'missing executable gate: %s\n' "$GATE" >&2
    exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fail=0

write_msg() {
    local path="$1"
    shift
    printf '%s\n' "$@" >"$path"
}

write_staged() {
    local path="$1"
    shift
    printf '%s\n' "$@" >"$path"
}

# Run gate; capture exit + stderr. $1=label $2=expect_fire(0|1) $3=msg $4=staged
assert_case() {
    local label="$1"
    local expect_fire="$2"
    local msg="$3"
    local staged="$4"
    local out rc

    set +e
    out="$(LOCTREE_COMMIT_GATE_STRICT=1 "$GATE" "$msg" "$staged" 2>&1)"
    rc=$?
    set -e

    if [ "$expect_fire" -eq 1 ]; then
        if [ "$rc" -eq 0 ]; then
            printf 'FAIL %s: expected FIRE (exit 1), got exit 0\n%s\n' "$label" "$out" >&2
            fail=1
            return
        fi
        if ! printf '%s\n' "$out" | grep -q 'commit-msg-diff-gate:'; then
            printf 'FAIL %s: expected findings banner\n%s\n' "$label" "$out" >&2
            fail=1
            return
        fi
        printf 'OK   FIRE  %s\n' "$label"
    else
        if [ "$rc" -ne 0 ]; then
            printf 'FAIL %s: expected QUIET (exit 0), got %s\n%s\n' "$label" "$rc" "$out" >&2
            fail=1
            return
        fi
        if printf '%s\n' "$out" | grep -q 'commit-msg-diff-gate:'; then
            printf 'FAIL %s: expected silence, got findings\n%s\n' "$label" "$out" >&2
            fail=1
            return
        fi
        printf 'OK   QUIET %s\n' "$label"
    fi
}

# ---------------------------------------------------------------------------
# FIRE fixtures
# ---------------------------------------------------------------------------

# [F1] 060b4e0-shaped: test(makieta) + production overlay module
write_msg "$tmpdir/f1.msg" \
    '[grok/vc-workflow] test(makieta): A/B value eval — live model + judge' \
    '' \
    'Harness only. No touches to core (…overlay).' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: grok'
write_staged "$tmpdir/f1.staged" \
    'loctree-rs/src/overlay.rs' \
    'loctree-rs/src/pack.rs' \
    'loctree-rs/tests/makieta_ab.rs'
assert_case 'F1 060b4e0-shaped test+prod+no-touch' 1 "$tmpdir/f1.msg" "$tmpdir/f1.staged"

# [F2] type=chore with production Rust
write_msg "$tmpdir/f2.msg" \
    '[claude/vc-implement] chore: tidy comments' \
    '' \
    'Comment-only intention.' \
    '' \
    'Authored-By: claude <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: claude'
write_staged "$tmpdir/f2.staged" \
    'loctree-rs/src/analyzer/swift.rs'
assert_case 'F2 chore+prod source' 1 "$tmpdir/f2.msg" "$tmpdir/f2.staged"

# [F3] file-count lie: "1-file pack" but 3 staged
write_msg "$tmpdir/f3.msg" \
    '[grok/vc-workflow] feat(hooks): wire gate' \
    '' \
    '1-file pack only.' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: grok'
write_staged "$tmpdir/f3.staged" \
    'tools/hooks/commit-msg' \
    'tools/hooks/lib/commit-msg-diff-gate.sh' \
    'tests/commit_msg_diff_gate.sh'
assert_case 'F3 file-count claim mismatch' 1 "$tmpdir/f3.msg" "$tmpdir/f3.staged"

# [F4] docs type with .kt production
write_msg "$tmpdir/f4.msg" \
    '[codex/interactive] docs: describe tool window' \
    '' \
    'Docs only.' \
    '' \
    'Authored-By: codex <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: codex'
write_staged "$tmpdir/f4.staged" \
    'editors/jetbrains/src/main/kotlin/io/loct/intellij/toolwindow/ResultProjector.kt'
assert_case 'F4 docs+prod kotlin' 1 "$tmpdir/f4.msg" "$tmpdir/f4.staged"

# ---------------------------------------------------------------------------
# QUIET fixtures
# ---------------------------------------------------------------------------

# [Q1] honest feat with matching prod + correct file count
write_msg "$tmpdir/q1.msg" \
    '[grok/vc-workflow] feat(hooks): message-vs-diff gate' \
    '' \
    'Wire advisory gate into commit-msg; 3 files changed.' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: grok'
write_staged "$tmpdir/q1.staged" \
    'tools/hooks/commit-msg' \
    'tools/hooks/lib/commit-msg-diff-gate.sh' \
    'tests/commit_msg_diff_gate.sh'
assert_case 'Q1 honest feat 3-file claim' 0 "$tmpdir/q1.msg" "$tmpdir/q1.staged"

# [Q2] test type with ONLY test files (fleet subject)
write_msg "$tmpdir/q2.msg" \
    '[claude/vc-implement] test(makieta): harness only' \
    '' \
    'Fixtures and harness; no production modules.' \
    '' \
    'Authored-By: claude <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: claude'
write_staged "$tmpdir/q2.staged" \
    'loctree-rs/tests/makieta_ab.rs' \
    'loctree-rs/tests/fixtures/makieta/repo1/src/lib.rs'
assert_case 'Q2 test type + tests only' 0 "$tmpdir/q2.msg" "$tmpdir/q2.staged"

# [Q3] pack fix honesty (1d58fb4c shape): feat/fix + 1 file + no false no-touch
write_msg "$tmpdir/q3.msg" \
    '[grok/vc-workflow] fix(pack): emit Hotspots once in context md' \
    '' \
    'Removes late Risk table dupe. Files: loctree-rs/src/pack.rs only (1 file).' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:50:00+02:00' \
    'runtime: grok'
write_staged "$tmpdir/q3.staged" \
    'loctree-rs/src/pack.rs'
assert_case 'Q3 honest 1-file pack fix' 0 "$tmpdir/q3.msg" "$tmpdir/q3.staged"

# [Q4] advisory mode: would FIRE under strict but exits 0 without STRICT
write_msg "$tmpdir/q4.msg" \
    '[grok/vc-workflow] test: should warn only' \
    '' \
    'Body.' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: grok'
write_staged "$tmpdir/q4.staged" \
    'loctree-rs/src/lib.rs'
set +e
out="$(LOCTREE_COMMIT_GATE_STRICT=0 "$GATE" "$tmpdir/q4.msg" "$tmpdir/q4.staged" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then
    printf 'FAIL Q4 advisory: expected exit 0, got %s\n%s\n' "$rc" "$out" >&2
    fail=1
elif ! printf '%s\n' "$out" | grep -q 'type-vs-content'; then
    printf 'FAIL Q4 advisory: expected findings text\n%s\n' "$out" >&2
    fail=1
else
    printf 'OK   ADVISORY Q4 test+prod warns but exit 0\n'
fi

# [Q5] pure semantic "No dupe" is a known blind spot — gate stays QUIET
# (structural tests own this; gate documents honesty)
write_msg "$tmpdir/q5.msg" \
    '[grok/vc-workflow] fix(pack): synthesis-first risk' \
    '' \
    'No dupe, no late emission. Memory follows.' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: grok'
write_staged "$tmpdir/q5.staged" \
    'loctree-rs/src/pack.rs'
assert_case 'Q5 blind-spot semantic No dupe stays quiet' 0 "$tmpdir/q5.msg" "$tmpdir/q5.staged"

# [I1] commit-msg via .git-style symlink still finds lib/ (install path)
hook_tmp="$tmpdir/fake-git-hooks"
mkdir -p "$hook_tmp"
ln -sf "$ROOT_DIR/tools/hooks/commit-msg" "$hook_tmp/commit-msg"
write_msg "$tmpdir/i1.msg" \
    '[grok/vc-workflow] feat(hooks): symlink resolve' \
    '' \
    'Body for shape validation.' \
    '' \
    'Authored-By: grok <agents@vetcoders.io>' \
    'session_id: 019e93be-379d-7303-9ad4-ffae468db99f' \
    'time: 2026-07-23T07:00:00+02:00' \
    'runtime: grok'
# Must not exit 2 / missing gate; shape-valid message exits 0.
set +e
out="$("$hook_tmp/commit-msg" "$tmpdir/i1.msg" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then
    printf 'FAIL I1 symlink commit-msg: expected exit 0, got %s\n%s\n' "$rc" "$out" >&2
    fail=1
else
    printf 'OK   INTEG I1 commit-msg via symlink finds diff-gate\n'
fi

if [ "$fail" -ne 0 ]; then
    printf '\ncommit_msg_diff_gate: FAILED\n' >&2
    exit 1
fi
printf '\ncommit_msg_diff_gate: all fixtures passed\n'
