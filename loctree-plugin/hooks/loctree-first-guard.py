#!/usr/bin/env python3
"""Teach structural repo mapping Loctree-first without policing text work.

Two modes, selected by LOCTREE_FIRST_GUARD:

- **advisory** (default, the only mode shipped to external users): the hook
  never blocks a command and never alters its result. When a standalone
  grep/rg sweeps the entire git repository containing the session cwd — the
  one shape that usually stands in for a structural question — it lets the
  command run and attaches a short Loctree recipe as extra context. File
  listing, explicit file targets, explicit subdirectory targets, searches
  outside that repo, downstream pipe filters, and ``command grep`` /
  ``command rg`` (a deliberate broad search) draw no note at all.
- **strict** (LOCTREE_FIRST_GUARD=strict; what our own product machines run,
  wired by scripts/install-ai-hooks.sh): zero fallback. Any grep/rg whose
  concrete scope lives inside the working repo is denied — including the
  ``command``-prefixed spelling — with the full loct recipe map and the
  loctree-feedback.md feedback contract. Pipe filters, file listing (rg --files),
  stdin filters, and out-of-repo searches stay allowed.

LOCTREE_FIRST_GUARD=0 (or "off") disables the hook entirely. Any parsing
uncertainty stays silent/fails open in both modes: this is workflow doctrine,
not a security boundary.
"""

import glob as globmod
import json
import os
import re
import shlex
import sys


GREP_RE = re.compile(r"(grep|rg|egrep|fgrep)\b")
DELIBERATE_FALLBACK_RE = re.compile(r"command\s+(grep|rg|egrep|fgrep)\b")
PREFIX_RE = re.compile(r"^(sudo\s+|env(\s+\w+=\S*)*\s+)+")
_HEREDOC_WORD_STOP = frozenset(" \t\n\r;&|<>(){}")


def _read_heredoc_delim(command, index):
    """Parse `<<[-][quote]DELIM[quote]` at `index`. Return None when it is not a heredoc."""
    length = len(command)
    if index + 1 >= length or command[index + 1] != "<":
        return None
    if index + 2 < length and command[index : index + 3] == "<<<":
        return None
    cursor = index + 2
    strip_tabs = False
    if cursor < length and command[cursor] == "-":
        strip_tabs = True
        cursor += 1
    while cursor < length and command[cursor] in " \t":
        cursor += 1
    if cursor >= length:
        return None
    if command[cursor] in "'\"":
        quote = command[cursor]
        cursor += 1
        start = cursor
        while cursor < length and command[cursor] != quote:
            cursor += 1
        delim = command[start:cursor]
        if cursor < length:
            cursor += 1
        return delim, strip_tabs, cursor
    if command[cursor] == "\\" and cursor + 1 < length:
        start = cursor + 1
        cursor += 2
        while cursor < length and command[cursor] not in _HEREDOC_WORD_STOP:
            cursor += 1
        return command[start:cursor], strip_tabs, cursor
    start = cursor
    while cursor < length and command[cursor] not in _HEREDOC_WORD_STOP:
        cursor += 1
    delim = command[start:cursor]
    if not delim:
        return None
    return delim, strip_tabs, cursor


def _consume_heredoc_body(command, start, delim, strip_tabs):
    """Skip one heredoc body starting at `start`. Return the index after the closer."""
    length = len(command)
    cursor = start
    while cursor <= length:
        newline = command.find("\n", cursor)
        line = command[cursor:] if newline == -1 else command[cursor:newline]
        if line.endswith("\r"):
            line = line[:-1]
        candidate = line.lstrip("\t") if strip_tabs else line
        if candidate == delim:
            return length if newline == -1 else newline + 1
        if newline == -1:
            return length
        cursor = newline + 1
    return length


def split_command_segments(command):
    """Split on unquoted `&&`, `;`, and newlines; skip heredoc bodies.

    Quote-blind `re.split(r"&&|;|\\n")` cut commit messages and heredoc
    contents, which produced false positives. Any parse error fail-opens
    to a single segment (the original command).
    """
    try:
        return _split_command_segments(command)
    except Exception:
        return [command]


def _split_command_segments(command):
    segments = []
    buf = []
    index = 0
    length = len(command)
    in_single = False
    in_double = False
    pending = []

    def flush():
        text = "".join(buf)
        buf.clear()
        if text.strip():
            segments.append(text)

    while index < length:
        char = command[index]
        if in_single:
            buf.append(char)
            if char == "'":
                in_single = False
            index += 1
            continue
        if in_double:
            buf.append(char)
            if char == "\\" and index + 1 < length:
                buf.append(command[index + 1])
                index += 2
                continue
            if char == '"':
                in_double = False
            index += 1
            continue
        if char == "'":
            in_single = True
            buf.append(char)
            index += 1
            continue
        if char == '"':
            in_double = True
            buf.append(char)
            index += 1
            continue
        if char == "\\" and index + 1 < length:
            buf.append(char)
            buf.append(command[index + 1])
            index += 2
            continue
        if char == "<":
            parsed = _read_heredoc_delim(command, index)
            if parsed is not None:
                delim, strip_tabs, new_index = parsed
                pending.append((delim, strip_tabs))
                buf.append(command[index:new_index])
                index = new_index
                continue
        if char == "&" and index + 1 < length and command[index + 1] == "&":
            flush()
            index += 2
            continue
        if char == ";":
            flush()
            index += 1
            continue
        if char in "\n\r":
            flush()
            if char == "\r" and index + 1 < length and command[index + 1] == "\n":
                index += 1
            body_start = index + 1
            if pending:
                for delim, strip_tabs in pending:
                    body_start = _consume_heredoc_body(
                        command, body_start, delim, strip_tabs
                    )
                pending.clear()
                index = body_start
            else:
                index += 1
            continue
        buf.append(char)
        index += 1
    flush()
    return segments


def pipeline_head(segment):
    """Return the pipeline producer: text before the first unquoted `|`."""
    try:
        return _pipeline_head(segment)
    except Exception:
        return segment


def _pipeline_head(segment):
    index = 0
    length = len(segment)
    in_single = False
    in_double = False
    while index < length:
        char = segment[index]
        if in_single:
            if char == "'":
                in_single = False
            index += 1
            continue
        if in_double:
            if char == "\\" and index + 1 < length:
                index += 2
                continue
            if char == '"':
                in_double = False
            index += 1
            continue
        if char == "'":
            in_single = True
            index += 1
            continue
        if char == '"':
            in_double = True
            index += 1
            continue
        if char == "\\" and index + 1 < length:
            index += 2
            continue
        if char == "|":
            nxt = segment[index + 1] if index + 1 < length else ""
            if nxt in "|&":
                index += 2
                continue
            return segment[:index]
        index += 1
    return segment
FILE_LIST_FLAGS = {"--files"}

# grep/rg flags that consume the NEXT token as their argument. Without the
# skip that argument would be misread as the pattern (shifting the real
# pattern into the targets) or as a search target.
ARG_FLAGS = {
    "-e",
    "--regexp",
    "-f",
    "--file",
    "-m",
    "--max-count",
    "-A",
    "--after-context",
    "-B",
    "--before-context",
    "-C",
    "--context",
    "-g",
    "--glob",
    "--iglob",
    "-t",
    "--type",
    "-T",
    "--type-not",
    "--include",
    "--exclude",
    "--exclude-dir",
    "-d",
    "--directories",
    "-D",
    "--devices",
}

# Flags that supply the PATTERN themselves; every bare positional is then a
# target. Otherwise the FIRST bare positional is the pattern, never a target.
PATTERN_FLAGS = {"-e", "--regexp", "-f", "--file"}


def repo_root(path: str):
    """Return the nearest ancestor containing .git, or None."""
    try:
        candidate = os.path.realpath(os.path.expanduser(path))
        if not os.path.isdir(candidate):
            candidate = os.path.dirname(candidate)
        while True:
            if os.path.exists(os.path.join(candidate, ".git")):
                return candidate
            parent = os.path.dirname(candidate)
            if parent == candidate:
                return None
            candidate = parent
    except Exception:
        return None


def grep_scope(head: str, cwd: str):
    """Return (command, existing targets, recursive), or None when unparseable."""
    try:
        tokens = shlex.split(head)
    except ValueError:
        return None

    command_name = os.path.basename(tokens[0]) if tokens else ""
    if command_name == "rg" and any(token in FILE_LIST_FLAGS for token in tokens[1:]):
        return None
    targets = []
    recursive = False
    # The pattern is an argument, never a target: `grep -c . file` counts
    # every character of `file` — its `.` must not be read as "the repo
    # root" just because the path happens to exist (2026-09-28 field
    # false-positive).
    pattern_consumed = any(
        token in PATTERN_FLAGS or token.split("=", 1)[0] in PATTERN_FLAGS
        for token in tokens[1:]
    )
    skip_next = False
    for token in tokens[1:]:
        if skip_next:
            skip_next = False
            continue
        if token in ("-r", "-R", "--recursive") or (
            token.startswith("-")
            and not token.startswith("--")
            and ("r" in token or "R" in token)
        ):
            recursive = True
        if token.startswith("-") and token != "-":
            if token in ARG_FLAGS:
                skip_next = True
            continue
        if token == "-":
            # stdin marker, not a filesystem target
            continue
        if not pattern_consumed:
            pattern_consumed = True
            continue
        expanded = os.path.expanduser(token)
        candidate = expanded if os.path.isabs(expanded) else os.path.join(cwd, expanded)
        if any(character in candidate for character in "*?["):
            matches = globmod.glob(candidate)[:8]
            # An unmatched glob still says WHERE the search points; keep the
            # literal path so the judgment stays on that location.
            targets.extend(matches if matches else [candidate])
        else:
            # Keep an explicitly named target even when the path does not
            # exist (grep merely errors on it). Dropping it left `targets`
            # empty, and the repo-wide fallback then judged `cwd` instead —
            # blocking explicit OUT-of-repo searches (2026-09-29 field
            # false-positive on a not-yet-created ledger directory).
            targets.append(candidate)

    # ripgrep searches cwd by default; classic grep only walks cwd when asked
    # to recurse and otherwise commonly consumes stdin.
    if not targets and not recursive and command_name != "rg":
        return None
    return command_name, targets, recursive


def is_repo_wide(scope, cwd: str, work_repo: str) -> bool:
    """True only for a search whose concrete scope is the whole repo root."""
    _command_name, targets, _recursive = scope
    concrete = targets if targets else [cwd]
    return any(os.path.realpath(target) == work_repo for target in concrete)


def is_in_repo(scope, cwd: str, work_repo: str) -> bool:
    """True when any concrete search target lives inside the working repo."""
    _command_name, targets, _recursive = scope
    concrete = targets if targets else [cwd]
    return any(repo_root(target) == work_repo for target in concrete)


def guard_mode() -> str:
    """Resolve LOCTREE_FIRST_GUARD to 'off', 'strict', or 'advisory'."""
    value = os.environ.get("LOCTREE_FIRST_GUARD", "advisory").strip().lower()
    if value in ("0", "off", "false", "disabled"):
        return "off"
    if value in ("strict", "block", "enforce"):
        return "strict"
    return "advisory"


STRICT_MESSAGE = (
    "LOCTREE FIRST — name the QUESTION, not the pattern. grep/rg on the "
    "working repo is blocked (ZERO FALLBACK: 'command grep'/'command rg' do "
    "not bypass). Route the question:\n"
    "  shape of a module         loct focus DIR      "
    "(files, internal edges, external deps)\n"
    "  a file and its orbit      loct slice FILE     "
    "(deps + consumers — before any edit)\n"
    "  what breaks if I touch    loct impact FILE    "
    "(direct + transitive blast radius)\n"
    "  repo-wide signals         loct follow SCOPE   "
    "(dead, cycles, twins, hotspots, trace)\n"
    "  only then, text truth     loct find NAME      "
    "(exact boundary; multi-OR: A B; --where-symbol definition; "
    "--who-imports importers; -l: --group-by-file)\n"
    "                            loct find --regex 'PAT'  "
    "(full metachars, (?i); raw text incl. markdown/comments/configs; "
    "-c: --count-only; --json · --limit/--offset · --all)\n"
    "Most greps vanish once the question is named; what remains is literal "
    "text, and --regex covers it with a coverage line (scanned X of Y) — "
    "absence is trustworthy, unlike grep silence.\n"
    "Caveats: scan universe = indexed snapshot — 'loct scan' after creating "
    "files; in --regex mode ignore --lang/--file scoping (unreliable) and "
    "filter output instead.\n"
    "If loct genuinely cannot express the query, do NOT fall back to grep — "
    "append the gap to ~/internal-artifacts/loctree/loctree-feedback.md and report the "
    "blocker. Pipe filters (cmd | grep), file listing (rg --files), and "
    "searches outside the working repo remain allowed.\n"
)

ADVISORY = (
    "LOCTREE RECIPE (advisory - the search ran untouched): a repo-wide text "
    "sweep often stands in for a structural question loct answers precisely: "
    "symbols: loct find NAME (multi-OR: A B; --where-symbol definition; "
    "--who-imports importers) | raw text/regex: loct find --regex 'PAT' "
    "(full metachars, (?i); the coverage line makes absence trustworthy) | "
    "file + orbit before an edit: loct slice FILE | blast radius: "
    "loct impact FILE | module shape: loct focus DIR. When loct cannot "
    "express a query this search answered, append the gap to "
    "~/internal-artifacts/loctree/loctree-feedback.md - that ledger drives the roadmap."
)


def emit_advisory() -> None:
    """Attach the recipe as context without touching the command or its result.

    stdout carries the Claude Code PreToolUse JSON contract
    (hookSpecificOutput.additionalContext reaches the model on exit 0);
    stderr carries the same text for harnesses that surface it. Unknown JSON
    fields are ignored by other harnesses, so both channels are safe no-ops
    where unsupported.
    """
    try:
        sys.stdout.write(
            json.dumps(
                {
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "additionalContext": ADVISORY,
                    }
                }
            )
            + "\n"
        )
    except Exception:
        pass
    sys.stderr.write(ADVISORY + "\n")


def main() -> int:
    """Advisory (default): always 0, attach a recipe on a repo-wide sweep.
    Strict (our machines): exit 2 with the recipe map for any in-repo search.

    Advisory keeps ``command``-prefixed fallbacks, pipe filters, file/subdir
    targets, and out-of-repo searches silent; strict denies every in-repo
    content search including the ``command`` spelling. File listing
    (rg --files), pipes, stdin filters, out-of-repo searches, and every parse
    failure pass in both modes (fail-open by design). Advisory never blocks
    and never mutates output.
    """
    mode = guard_mode()
    if mode == "off":
        return 0

    try:
        payload = json.load(sys.stdin)
        command = (payload.get("tool_input") or {}).get("command", "")
        cwd = payload.get("cwd") or os.getcwd()
    except Exception:
        return 0

    if not command:
        return 0
    work_repo = repo_root(cwd)
    if work_repo is None:
        return 0

    for segment in split_command_segments(command):
        head = pipeline_head(segment)
        head = re.sub(r"^[\s({]+", "", head)
        head = PREFIX_RE.sub("", head)
        if DELIBERATE_FALLBACK_RE.match(head):
            if mode == "advisory":
                continue
            # Strict is zero-fallback: the `command` spelling routes the same.
            head = re.sub(r"^command\s+", "", head)
        if not GREP_RE.match(head):
            continue
        scope = grep_scope(head, cwd)
        if scope is None:
            continue
        if mode == "strict":
            if is_in_repo(scope, cwd, work_repo):
                sys.stderr.write(STRICT_MESSAGE)
                return 2
        elif is_repo_wide(scope, cwd, work_repo):
            emit_advisory()
            return 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
