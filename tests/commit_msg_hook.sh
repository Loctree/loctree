#!/bin/sh
set -eu

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HOOK="$ROOT_DIR/tools/hooks/commit-msg"
if [ ! -x "$HOOK" ]; then
  HOOK="$ROOT_DIR/.git/hooks/commit-msg"
fi

tmpdir="$(mktemp -d)"
trap 'rm -R "$tmpdir"' EXIT

run_hook() {
  msg="$1"
  "$HOOK" "$msg"
}

expect_reject() {
  msg="$1"
  why="$2"
  if run_hook "$msg" >/dev/null 2>&1; then
    echo "expected hook to reject: $why" >&2
    exit 1
  fi
}

# Valid agent commit: mandatory prefix, matching Authored-By, session_id,
# time: with numeric offset, runtime, explanatory body. Prior invalid
# history quoted in the body is text and must not be validated.
cat > "$tmpdir/valid-agent.msg" <<'MSG'
[claude/interactive] fix: restore native toolbar, Replace, Share + tab UX

Addresses six findings from a ScreenScribe review of the editor.

Prior invalid history is just body text and must not be validated here:
docs(spec): VS Code Context-King surface design

Authored-By: claude <agents@vetcoders.io>
session_id: 9c13d55e-1af1-4dc0-ae3b-382659e4f766
time: 2026-06-04T15:36:27-06:00
runtime: claude-code
MSG

run_hook "$tmpdir/valid-agent.msg"

# Zulu offset is an equally valid time: form.
cat > "$tmpdir/valid-agent-zulu.msg" <<'MSG'
[grok/implement] test(hooks): cover zulu time trailer

Adds the Z variant to the hook contract fixtures.

Authored-By: grok <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
time: 2026-06-05T12:52:47Z
runtime: grok
MSG

run_hook "$tmpdir/valid-agent-zulu.msg"

# Vendor footers are forbidden; Authored-By is the only authorship channel.
cat > "$tmpdir/vendor-footer.msg" <<'MSG'
[junie/implement] feat(mcp): single-instance lock

Locks the MCP server per project root.

Authored-By: junie <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
time: 2026-06-05T12:52:47-06:00
runtime: junie
Co-Authored-By: Junie <junie@jetbrains.com>
MSG

expect_reject "$tmpdir/vendor-footer.msg" "vendor Co-Authored-By footer"

# Legacy date: trailer no longer satisfies the time: requirement.
cat > "$tmpdir/legacy-date.msg" <<'MSG'
[codex/interactive] fix: repair release commit message validation

Explains the change.

Authored-By: codex <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
date: 2026-06-04T15:36:27 MDT
runtime: codex
MSG

expect_reject "$tmpdir/legacy-date.msg" "legacy date: trailer without time:"

# Legacy timestamp: trailer is rejected explicitly.
cat > "$tmpdir/legacy-timestamp.msg" <<'MSG'
[codex/interactive] fix: repair release commit message validation

Explains the change.

Authored-By: codex <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
timestamp: 2026_0604_1536_MDT
time: 2026-06-04T15:36:27-06:00
runtime: codex
MSG

expect_reject "$tmpdir/legacy-timestamp.msg" "legacy timestamp: trailer"

# The agent/runtime prefix is mandatory; there is no human escape hatch.
cat > "$tmpdir/plain-conventional.msg" <<'MSG'
fix(hooks): make worktree installation safe
MSG

expect_reject "$tmpdir/plain-conventional.msg" "plain conventional subject without [agent/runtime] prefix"

cat > "$tmpdir/garbage-subject.msg" <<'MSG'
update hooks
MSG

expect_reject "$tmpdir/garbage-subject.msg" "non-conventional subject"

# Missing session metadata is rejected.
cat > "$tmpdir/missing-current-metadata.msg" <<'MSG'
[codex/interactive] fix: repair release commit message validation

Explains the change.

Authored-By: codex <agents@vetcoders.io>
runtime: codex
MSG

expect_reject "$tmpdir/missing-current-metadata.msg" "agent commit without session_id/time:"

# Subject prefix and Authored-By must name the same agent (fleet truth).
cat > "$tmpdir/agent-mismatch.msg" <<'MSG'
[claude/implement] fix(atlas): status from snapshot truth

Derives atlas status from snapshot presence and staleness.

Authored-By: grok <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
time: 2026-06-05T12:52:47-06:00
runtime: grok
MSG

expect_reject "$tmpdir/agent-mismatch.msg" "subject claims claude while Authored-By says grok"

# Trailers without an explanatory body are rejected.
cat > "$tmpdir/no-body.msg" <<'MSG'
[codex/interactive] fix: repair release commit message validation

Authored-By: codex <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
time: 2026-06-04T15:36:27-06:00
runtime: codex
MSG

expect_reject "$tmpdir/no-body.msg" "agent commit without explanatory body"

# The make-version flow stays compliant: time: in UTC Z form.
cat > "$tmpdir/version-flow.msg" <<'MSG'
[codex/interactive] chore(release): bump versions

loctree=0.12.2 loctree-mcp=0.12.2 loctree-lsp=0.12.2

Authored-By: codex <agents@vetcoders.io>
session_id: 019e93be-379d-7303-9ad4-ffae468db99f
time: 2026-06-04T15:36:27Z
runtime: make-version
MSG

run_hook "$tmpdir/version-flow.msg"
