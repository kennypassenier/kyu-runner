# kyu-runner

A stateless Rust daemon that lets Home Assistant consume from the
kyu hub: long-poll configured topics, forward each message to an
HA webhook, ack only on HA's 2xx — so the hub's retry → dead-letter
machinery works for the HA delivery. Implements P1 + P8 of the Kyu
Integration Study.

This project follows the dev procedure in `~/Projects/dev-procedure/`
(`/project-flow`). Standing rules apply to every change:
`~/Projects/dev-procedure/STANDING_RULES.md`.
Enforcement is **git-native** (`.githooks/` via `core.hooksPath`), so
gates hold from any session or terminal. After a fresh clone, run:
`git config core.hooksPath .githooks`.

## Procedure status

| Field | Value |
|---|---|
| Current phase | AFK build COMPLETE through Phase 8; waiting for Kenny |
| Last completed gate | Phase 0 (2026-08-28): scope approved item-by-item, B1 out of scope |
| Next gate | Combined ratification round (Q1-Q13) — see docs/PENDING_MINI_ROUNDS.md; then Phase 9 "Tag & release?" (always Kenny's explicit go) |
| AFK mode | **on** (Kenny, 2026-08-28: "doe alles wat je kan zonder mijn input") |

**AFK contract:** phases 1-5 decisions are taken on the documented
recommendations and are PROVISIONAL until ratified; every gate is
queued as a form in `docs/PENDING_MINI_ROUNDS.md`, which is the FIRST
thing presented when Kenny returns. The real hub on LXC 109 is never
touched (scope R1).

## Project documents

| Doc | Purpose |
|---|---|
| docs/SCOPE.md | goals, non-goals, success criteria, constraints + build-vs-buy (Phases 0-1) |
| docs/FEATURES.md | rated feature list with permanent IDs (Phase 2) |
| docs/ARCHITECTURE_DECISIONS.md | AR decisions incl. tech choice (Phases 3-4) |
| docs/REALIZATION_PLAN.md | milestones + status table + gate log (Phase 5+) |
| docs/PENDING_MINI_ROUNDS.md | queued gates/ratifications from the AFK build |
| docs/TEST_PLAN.md | what is proven where + accepted limitations (Phase 7) |

## Gates (enforced)

Commits are blocked unless `.claude/hooks/gates.sh` passes and the
message carries IDs in brackets (`[K3, AR2]`, `[L1]`, `[meta]`).
Enforced twice over: `.githooks/pre-commit` + `.githooks/commit-msg`
(repo-scoped, any session) and `.claude/hooks/check-commit.sh` via
`.claude/settings.json`. CI re-runs the gates on every push.

## Scratch hub for development/tests

Tests spawn a local hub themselves (see `tests/support/`): the binary
at `KYU_BIN` (default: `~/Projects/kyu/target/release/kyu`)
or the public `ghcr.io/kennypassenier/kyu` image in CI. Never
`10.10.10.9:8080`.
