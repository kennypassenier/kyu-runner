#!/usr/bin/env bash
# hub-bridge quality gates (standing rules 6/7): format, lint with
# warnings as errors, full test suite. Called by .githooks/pre-commit
# and .claude/hooks/check-commit.sh; non-zero exit blocks the commit.
set -euo pipefail
cd "${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel)}"

# Standing rule 7 (mailbox retro 2026-08-28): a gate that lets the tree
# change while it runs is green locally and wrong in the commit — cargo
# rewrites Cargo.lock, and anything rewritten after `git add` is absent
# from what gets committed. Snapshot before, compare after.
snapshot() { { git status --porcelain=v1; git diff; git diff --cached; } | sha256sum; }
before=$(snapshot)

if [ -f Cargo.toml ]; then
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test --all
else
  # Loud, not silent (standing rule 12): before L0 there is no crate yet.
  echo "gates: no Cargo.toml yet (pre-L0) — Rust gates SKIPPED." >&2
fi

after=$(snapshot)
if [ "$before" != "$after" ]; then
  {
    echo "GATE FAILED — the working tree changed while the gates ran (standing rule 7)."
    echo "Something (cargo?) rewrote a file after staging. Run: git add -A, then commit again."
  } >&2
  exit 1
fi
