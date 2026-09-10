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
# A file whose job is to record old names is excluded from this check —
# this convention's own ID_TRANSLATIONS.md and a project's existing
# ID_MAP.md alike. The first drill blocked a perfectly good map for
# containing exactly what it exists to contain, and binary-puzzle-toolkit's
# real one then blocked every commit in that repository.
born=$(git diff --cached --unified=0 -- '*.md' \
  ':(exclude)*ID_TRANSLATIONS.md' ':(exclude)*ID_MAP.md' 2>/dev/null \
  | grep -E '^\+' | grep -vE '^\+\+\+' \
  | grep -oE '^\+(\| ?|#{2,4} )[A-Z]{1,4}[0-9]+[a-z]?\b' \
  | grep -oE '[A-Z]{1,4}[0-9]+[a-z]?$' | sort -u || true)

# An ID that already exists in the previous commit is not being born; the
# row is only being edited. Git reports a changed line as an added one, so
# without this, turning "open" into "closed" on a years-old row forces a
# rename in the middle of unrelated work -- measured twice on 2026-09-10
# in kp-themes, on `HA3` and on `KT16-M1`, both of them a status change
# and nothing else. The comment above promised "existing tables stay
# exactly as they are"; this is what makes that true.
if [ -n "$born" ]; then
  fresh=""
  for id in $born; do
    git -C "$root" grep -qE "\\b${id}\\b" HEAD -- '*.md' 2>/dev/null || fresh="$fresh $id"
  done
  born=$(printf '%s\n' $fresh | sed '/^$/d' | sort -u)
fi
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
# docs/ID_TRANSLATIONS.md holds one row per translated identifier:
# | old | new |. Two safeguards, both paid for on 2026-09-09:
#
#   * The name is this convention's own. The first version read
#     docs/ID_MAP.md, a name binary-puzzle-toolkit already used for
#     something else — a record of which pre-merge commits carry which
#     old numbers — and reading it as "these may no longer appear" locked
#     that repository out of committing entirely.
#   * The file must opt in with an exact header line, so a file that
#     merely shares the name never activates the gate by accident.
map="$root/docs/ID_TRANSLATIONS.md"
if [ -f "$map" ] && ! head -3 "$map" | grep -qx '<!-- id-translations: enforced -->'; then
  map=""
fi
if [ -n "$map" ] && [ -f "$map" ]; then
  while IFS= read -r old_id; do
    [ -n "$old_id" ] || continue
    # The map itself is where the old name is supposed to live.
    if git -C "$root" grep -qE "\\b${old_id}\\b" -- '*.md' '*.rs' '*.js' '*.ts' '*.cs' '*.php' ':(exclude)*ID_TRANSLATIONS.md' 2>/dev/null; then
      {
        echo "COMMIT BLOCKED — $old_id is recorded as translated but still appears:"
        git -C "$root" grep -nE "\\b${old_id}\\b" -- '*.md' '*.rs' '*.js' '*.ts' '*.cs' '*.php' ':(exclude)*ID_TRANSLATIONS.md' 2>/dev/null | head -5 | sed 's/^/  /'
        echo "A half-finished translation leaves two names for one thing."
      } >&2
      fail=1
    fi
  done < <(grep -oE '^\| *[A-Z]{1,4}[0-9]+[a-z]? *\|' "$map" 2>/dev/null | tr -d '| ' || true)
fi

exit $fail
