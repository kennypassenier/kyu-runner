#!/usr/bin/env bash
# Dev-procedure commit gate (option B): a PreToolUse hook on the Bash
# tool. Blocks `git commit` unless (1) the project's gates pass and
# (2) the commit message carries feature/milestone IDs in brackets.
#
# Contract: Claude Code pipes the tool call as JSON on stdin. Exit 0
# allows the command; exit 2 blocks it and feeds stderr back to Claude.
# Parse failures fail OPEN (exit 0) so a broken hook never bricks work.
#
# PROC-H1 (found on kp-themes 2026-09-04, fixed at its retrospective).
# Three faults lived in the old one-line test `case "$cmd" in
# *"git commit"*)`:
#
#   1. It matched the WORDS anywhere in the command, so any command that
#      merely quoted an example commit message was blocked. Writing a
#      document that quotes one is not committing. Heredoc bodies and
#      quoted strings are now masked before anything is matched, and the
#      match has to sit at a command position.
#   2. A PreToolUse hook runs BEFORE the command, so a compound command
#      that formats and then commits is always judged on the unformatted
#      tree and can never pass, however often it is retried.
#   3. Blocking a compound command blocks all of it, including setup that
#      had not run yet — a `git checkout -b` that never happened, after
#      which the retry lands the work on whatever branch was already
#      there. An L2 commit landed on `main` exactly this way.
#
# (2) and (3) cannot be fixed by gating harder, only by refusing early
# and saying so, which is what the `compound` verdict below does.
set -u

payload=$(cat) || exit 0
cmd=$(printf '%s' "$payload" | python3 -c '
import json,sys
try:
    print(json.load(sys.stdin).get("tool_input", {}).get("command", ""))
except Exception:
    pass
' 2>/dev/null) || exit 0

# One verdict, on one line: `not-a-commit`, `commit <repo-or-empty>`, or
# `compound <the segment that makes it unsafe to gate>`.
verdict=$(printf '%s' "$cmd" | python3 -c '
import re, sys

# Segments that may precede a commit in one command: they neither change
# file contents (so the gate verdict still holds when the command runs)
# nor carry a side effect that is lost when the command is blocked.
# A variable assignment qualifies on both counts.
SAFE_BEFORE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*=|cd\s|git\s+-C\s|git\s+add\b|git\s+status\b|git\s+diff\b|git\s+rev-parse\b|echo\s|true$)")
COMMIT = re.compile(r"^git\s+(?:-C\s+(?:\"[^\"]*\"|\x27[^\x27]*\x27|[^\s]+)\s+)?commit\b")
GIT_C = re.compile(r"git\s+-C\s+(\"[^\"]+\"|\x27[^\x27]+\x27|[^\s;&|]+)")
CD = re.compile(r"^cd\s+(\"[^\"]+\"|\x27[^\x27]+\x27|[^\s;&|]+)")


def blank(text):
    """Same length, no syntax: every offset stays valid."""
    return "".join("\n" if c == "\n" else "x" for c in text)


def mask(cmd):
    out = cmd
    # Heredocs first: their bodies routinely contain quotes and semicolons.
    for m in re.finditer(r"<<-?\s*([\x27\"]?)([A-Za-z_][A-Za-z0-9_]*)\1\n", cmd):
        end = re.search(r"^\s*" + re.escape(m.group(2)) + r"\s*$", cmd[m.end():], re.MULTILINE)
        stop = m.end() + (end.start() if end else len(cmd) - m.end())
        out = out[:m.end()] + blank(out[m.end():stop]) + out[stop:]
    for pattern in (r"\x27[^\x27]*\x27", r"\"(?:\\.|[^\"\\])*\""):
        out = re.sub(pattern, lambda m: blank(m.group(0)), out)
    return out


def segments(cmd):
    """(masked, original) per segment.

    Classification reads the MASKED half and reporting reads the original.
    Reading the original would undo the masking: a heredoc body line that
    happens to start with `git commit` — a document quoting one — is a
    separate line, and so a separate segment, whose original text starts
    with those words.
    """
    masked = mask(cmd)
    parts, start = [], 0
    for m in re.finditer(r"(\|\||&&|;|\||\n)", masked):
        parts.append((masked[start:m.start()], cmd[start:m.start()]))
        start = m.end()
    parts.append((masked[start:], cmd[start:]))
    return [(m.strip(), o.strip()) for m, o in parts if m.strip()]


cmd = sys.stdin.read()
parts = segments(cmd)
index = next((i for i, (m, _) in enumerate(parts) if COMMIT.match(m)), None)
if index is None:
    print("not-a-commit")
else:
    bad = next((o for m, o in parts[:index] if not SAFE_BEFORE.match(m)), None)
    if bad is not None:
        print("compound " + bad.splitlines()[0][:120])
    else:
        # Which repository the commit targets: an explicit `git -C`, else
        # the last `cd` before it. Variable indirection resolves to
        # nothing and the caller falls back to the project directory,
        # which is why standing rule 19 asks worktree commits to spell
        # the path out literally.
        target = ""
        for _, text in parts[:index + 1]:
            found = GIT_C.search(text) or CD.match(text)
            if found:
                target = found.group(1).strip("\"\x27")
        print("commit " + target)
' 2>/dev/null) || exit 0

case "$verdict" in
  not-a-commit|"") exit 0 ;;
  compound*)
    {
      echo "COMMIT BLOCKED — this command does more than commit, and NOTHING IN IT RAN."
      echo ""
      echo "  the step in the way: ${verdict#compound }"
      echo ""
      echo "This hook runs before the command, so it can only judge the tree as it"
      echo "stands now. A step that rewrites files first (a formatter, a build) would"
      echo "be judged on the unformatted tree and could never pass; a step with a side"
      echo "effect (creating a branch) would be lost, and the retry would land the"
      echo "commit on whatever branch is already checked out."
      echo ""
      echo "Run that step as its own command, then commit on its own. Only a variable"
      echo "assignment, cd, git -C, git add, git status, git diff, git rev-parse and"
      echo "echo may precede a commit in one command."
    } >&2
    exit 2
    ;;
esac

target="${verdict#commit }"
[ "$target" = "commit" ] && target=""
project_dir="${CLAUDE_PROJECT_DIR:-$PWD}"

# R2 (latch v2 retro, 2026-08-29): gates must run against the tree the
# commit actually targets. A commit issued from a git worktree used to be
# gated against CLAUDE_PROJECT_DIR — the MAIN checkout — silently
# approving code that was not the code being committed.
if [ -n "$target" ]; then
  case "$target" in
    "~"*) target="$HOME${target#\~}" ;;
    '$HOME'*) target="$HOME${target#\$HOME}" ;;
  esac
  root=$(git -C "$target" rev-parse --show-toplevel 2>/dev/null || true)
  [ -n "$root" ] && project_dir="$root"
fi

# Gate 1: the project's own quality gates (fmt/lint/tests). The project
# defines what that means in .claude/hooks/gates.sh (see gates.example.sh).
gates="$project_dir/.claude/hooks/gates.sh"
if [ -x "$gates" ]; then
  if ! out=$(cd "$project_dir" && "$gates" 2>&1); then
    {
      echo "COMMIT BLOCKED — gates failed (standing rule 7). Fix, then retry."
      echo "$out" | tail -30
    } >&2
    exit 2
  fi
fi

# Gate 2: traceability (standing rule 4). The message must contain IDs
# in brackets, e.g. [W12, AR9] or [L4b] — or [meta] for infra commits.
#
# PROC-H2 (kp-themes, 2026-09-04): this reads the COMMAND, so a message
# handed over with `-F <file>` carries no IDs the check can see and was
# blocked however well it was written. Same shape as facet 1 — it fails
# closed, so it was noise rather than a hole, but noise that has to be
# worked around teaches people to work around the gate.
#
# So: the haystack is the command plus, when the command names one, the
# file it takes the message from. A heredoc needs nothing extra, because
# its body is already part of the command text.
haystack="$cmd"
message_file=$(printf '%s' "$cmd" | python3 -c '
import re, sys
m = re.search(r"(?:-F|--file)[ =]+(\"[^\"]+\"|\x27[^\x27]+\x27|[^\s;&|]+)", sys.stdin.read())
print(m.group(1).strip("\"\x27") if m else "")
' 2>/dev/null) || message_file=""
if [ -n "$message_file" ] && [ "$message_file" != "-" ] && [ -r "$message_file" ]; then
  haystack="$cmd
$(cat "$message_file")"
fi

if ! printf '%s' "$haystack" | grep -qE '\[(meta|[A-Za-z]{1,4}[0-9])[^]]*\]'; then
  {
    echo "COMMIT BLOCKED — message lacks feature/milestone IDs (standing rule 4)."
    echo "Add the IDs this commit implements, e.g.: feat(L4b): groups [W12a-d, AR9]"
    echo "Pure infrastructure commits use [meta]."
  } >&2
  exit 2
fi

exit 0
