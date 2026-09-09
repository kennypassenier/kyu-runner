#!/usr/bin/env bash
# Project-owned gates (chassis 1.6.0, M1): the kit's `chassis sync` rewrites
# .claude/hooks/gates.sh and never touches this file.
#
# Why this exists: the 0.2.0 move onto chassis moved two config validations
# into the kit, so the suite went 71 -> 69. Nothing re-derived the totals the
# documents quote, and TEST_PLAN.md kept claiming 71 while README.md still
# claimed 57 from an even earlier round (found 2026-09-09, kit 1.8.0 round).
# `cargo test` cannot catch that: the suite was green the whole time. Standing
# rule 11b says documentation lies in both directions; this makes the half
# that is countable mechanical.
#
# Scope is deliberate: only the two documents that make a LIVE claim about
# the suite. docs/REALIZATION_PLAN.md's gate log records what was true on a
# past date ("Suite: 66 -> 71 tests") and is history, not a claim about now.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# The suite total, counted the way cargo counts it: one test attribute is one
# test. Verified equal to `cargo test`'s own sum on 2026-09-09 (69).
real=$(cat src/*.rs tests/*.rs | grep -c '^[[:space:]]*#\[test\]\|^[[:space:]]*#\[tokio::test\]')

fail=0

# A · every "<N> tests" in a live document names the real total.
while IFS=: read -r file line claim; do
  [ -z "${claim:-}" ] && continue
  if [ "$claim" != "$real" ]; then
    echo "gate: $file:$line says '$claim tests', the suite has $real." >&2
    echo "      What now: correct the number, or move the sentence to the" >&2
    echo "      gate log in docs/REALIZATION_PLAN.md if it is dated history." >&2
    fail=1
  fi
done < <(grep -nEo '(^|[^A-Za-z0-9_])[0-9]+ tests' README.md docs/TEST_PLAN.md \
         | sed -E 's/^(.*:[0-9]+:)[^0-9]*([0-9]+) tests$/\1\2/')

# B · the per-suite numbers in TEST_PLAN's suite table add up to the total.
table_sum=$(sed -n '/^## The suites/,/^## /p' docs/TEST_PLAN.md \
            | grep -oE '\(([0-9]+)\)' | tr -d '()' \
            | awk '{s+=$1} END {print s+0}')
if [ "$table_sum" != "$real" ]; then
  echo "gate: the suite table in docs/TEST_PLAN.md adds up to $table_sum, the suite has $real." >&2
  echo "      What now: correct the per-suite count that moved." >&2
  fail=1
fi

exit "$fail"
