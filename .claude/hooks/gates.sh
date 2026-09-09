#!/usr/bin/env bash
# Project quality gates: format, clippy with warnings as errors, the full
# test suite, and the clean-tree check. Called by .githooks/pre-commit for
# every commit and by .claude/hooks/check-commit.sh before Claude's
# commits; non-zero exit blocks the commit. cargo-deny runs in CI only.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Git exports GIT_DIR (absolute in a linked worktree), GIT_INDEX_FILE and
# friends to this hook. The suite below spawns git itself (the scaffold E2E
# runs `chassis new`, which inits and commits a fresh repository); a child
# inheriting them acts on THIS repository instead — the first commit from a
# worktree under .claude/worktrees/ put the scaffold on the committer's
# branch (2026-09-07). The toplevel is resolved above, so drop them here.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_COMMON_DIR

# Standing rule 7: a gate that does not predict the build is not a gate.
# The checks rewrite files (cargo refreshes Cargo.lock); anything rewritten
# AFTER `git add` is green here and absent from the commit, so the tree is
# fingerprinted before and after and a moved tree is refused.
gate_tree_fingerprint() {
  { git status --porcelain; git diff; } | sha256sum | cut -d' ' -f1
}
gate_tree_before=$(gate_tree_fingerprint)

cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test

# Project-owned gates (chassis 1.6.0, M1): a project keeps its own checks
# in .claude/hooks/gates.project.sh — a module-boundary grep, a version
# consistency script, a SQL guard. This file is the kit's and `chassis
# sync` rewrites it; that one is never touched.
if [ -x .claude/hooks/gates.project.sh ]; then
  .claude/hooks/gates.project.sh
fi

if [ "$(gate_tree_fingerprint)" != "$gate_tree_before" ]; then
  {
    echo "gates: the checks rewrote the working tree while they ran."
    echo "What this commit carries is NOT what was just tested (usually"
    echo "Cargo.lock). Changed paths:"
    git status --porcelain
    echo "What now: stage the changed files and commit again."
  } >&2
  exit 1
fi
