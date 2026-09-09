#!/usr/bin/env bash
# HOOK_VERSION=3
# The ID-scheme gate (standing rule 4, policy set 2026-09-09).
#
# The large rename was cancelled after measuring what it would cost: an ID
# means different things in different projects, and a document quotes a
# sibling project's IDs beside its own, so an ID-keyed rewrite silently
# repoints references. Kenny's replacement is narrower and self-dosing:
# new items are born in the new shape, and an old one is translated only
# when it surfaces by itself. This gate holds up the two halves of that
# which a machine can hold.
#
#   1. A NEW identifier may not be born in the old shape. Only ADDED
#      lines are read, so existing tables stay exactly as they are.
#   2. A translation is whole or it is refused. Once a project's ID map
#      records old -> new, the old name may not survive anywhere in the
#      tracked files; half a rename is worse than none, because the two
#      documents then disagree and neither says which is real.
#
# Plain bash and git. No Claude, no network, no dev-procedure checkout —
# same independence as the rest of the git-native layer.
set -u

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[ -n "$root" ] || exit 0

fail=0

# ---- 1. no new identifier in the old shape -------------------------------
# An ID is "born" on a line that defines one: a table row or a heading that
# starts with it. A mere mention is not a birth, so prose keeps working.
# ID_MAP.md is excluded on purpose: it is the one file whose whole job is
# to carry old names beside their replacements, and the first drill of
# this gate blocked a perfectly good map for containing exactly that.
born=$(git diff --cached --unified=0 -- '*.md' ':(exclude)*ID_MAP.md' 2>/dev/null \
  | grep -E '^\+' | grep -vE '^\+\+\+' \
  | grep -oE '^\+(\| ?|#{2,4} )[A-Z]{1,4}[0-9]+[a-z]?\b' \
  | grep -oE '[A-Z]{1,4}[0-9]+[a-z]?$' | sort -u || true)
if [ -n "$born" ]; then
  {
    echo "COMMIT BLOCKED — a new identifier is being created in the old shape:"
    printf '  %s\n' $born
    echo "The house scheme is a kind word, an optional domain and a number:"
    echo "  feat-<domain>-<n>  arch-<n>  tech-<n>  step-<n>"
    echo "  fix-<n>  gap-<n>  scope-<n>  ask-<n>"
    echo "Existing rows may stay as they are; only new ones must follow it."
  } >&2
  fail=1
fi

# ---- 2. a translation is whole or refused --------------------------------
# docs/ID_MAP.md holds one row per translated identifier: | old | new |
map="$root/docs/ID_MAP.md"
if [ -f "$map" ]; then
  while IFS= read -r old_id; do
    [ -n "$old_id" ] || continue
    # The map itself is where the old name is supposed to live.
    if git -C "$root" grep -qE "\\b${old_id}\\b" -- '*.md' '*.rs' '*.js' '*.ts' '*.cs' '*.php' ':(exclude)*ID_MAP.md' 2>/dev/null; then
      {
        echo "COMMIT BLOCKED — $old_id is recorded as translated but still appears:"
        git -C "$root" grep -nE "\\b${old_id}\\b" -- '*.md' '*.rs' '*.js' '*.ts' '*.cs' '*.php' ':(exclude)*ID_MAP.md' 2>/dev/null | head -5 | sed 's/^/  /'
        echo "A half-finished translation leaves two names for one thing."
      } >&2
      fail=1
    fi
  done < <(grep -oE '^\| *[A-Z]{1,4}[0-9]+[a-z]? *\|' "$map" 2>/dev/null | tr -d '| ' || true)
fi

exit $fail
