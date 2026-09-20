#!/usr/bin/env python3
"""Keep repo mapping Loctree-first without policing deliberate fallbacks.

The hook blocks standalone grep/rg commands only when they map the git repo
containing the Codex session cwd. Searches outside that repo and grep used as
a downstream pipe filter remain valid. ``command grep`` and ``command rg`` are
explicit operator-grade fallbacks: the extra word proves the choice was
deliberate, so the hook allows them. Any uncertainty fails open because this is
workflow guidance, not a security boundary.
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


def grep_targets(head: str, cwd: str):
    """Return existing path arguments, or None when the command is not mapping."""
    try:
        tokens = shlex.split(head)
    except ValueError:
        return None

    command_name = os.path.basename(tokens[0]) if tokens else ""
    targets = []
    recursive = False
    for token in tokens[1:]:
        if token in ("-r", "-R", "--recursive") or (
            token.startswith("-")
            and not token.startswith("--")
            and ("r" in token or "R" in token)
        ):
            recursive = True
        if token.startswith("-"):
            continue
        expanded = os.path.expanduser(token)
        candidate = expanded if os.path.isabs(expanded) else os.path.join(cwd, expanded)
        if any(character in candidate for character in "*?["):
            targets.extend(globmod.glob(candidate)[:8])
        elif os.path.exists(candidate):
            targets.append(candidate)

    # ripgrep searches cwd by default; classic grep only walks cwd when asked
    # to recurse and otherwise commonly consumes stdin.
    if not targets and not recursive and command_name != "rg":
        return None
    return targets


def main() -> int:
    """Deny one PreToolUse shell command when it maps the session's own git repo.

    Returns 2 for a bare grep/rg/egrep/fgrep whose targets resolve inside the repo
    holding ``cwd``; returns 0 for ``command``-prefixed fallbacks, pipe filters,
    out-of-repo searches, and every parse failure (fail-open by design).
    """
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
        if DELIBERATE_FALLBACK_RE.match(head):
            continue
        head = PREFIX_RE.sub("", head)
        if not GREP_RE.match(head):
            continue
        targets = grep_targets(head, cwd)
        if targets is None:
            continue
        scope = targets if targets else [cwd]
        if any(repo_root(target) == work_repo for target in scope):
            sys.stderr.write(
                "LOCTREE FIRST: first-choice repo mapping with grep/rg is paused, "
                "not forbidden. "
                "Use loct find '<pattern>' (literal by default), loct occurrences, "
                "loct body, or loct slice. If the scope is already mapped or "
                "Loctree cannot answer cleanly, retry deliberately with "
                "'command grep ...' or 'command rg ...'. When that fallback "
                "reveals a Loctree miss, append it to "
                "~/.vibecrafted/loctree/loctree-fail.md. Pipe filters and "
                "searches outside the working repo remain allowed.\n"
            )
            return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
