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
| Current phase | all 11 phases done; **0.2.2** released 2026-09-10 (static musl again); adopted on CT 109 |
| Last completed gate | The chassis 2.0.0 round (2026-09-10): three pieces of hand-written base moved into the kit, the artifact is static musl again, and the hook/CI ownership question was decided in this project's favour |
| Next gate | The rollout, when Kenny wants it: pick the LXC, create the HA automation FIRST, then the route, then a smoke test — and measure whether HA really answers 200 to an unknown webhook id. 1.0.0 follows that. Deploying 0.2.1 itself is **Later** by decision (U1): it changes no behaviour, so it rides along with the next rollout that matters |
| AFK mode | off |

The AFK build's queue in `docs/PENDING_MINI_ROUNDS.md` is now fully
answered — it stays as the record of what was decided when. The real
hub on LXC 109 is still only touched as an agreed step (scope R1); the
scratch container for drills is LXC 191 on the Proxmox host, kept until
the rollout at Kenny's request.

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

`main` on GitHub requires the `gates` and `deny` checks and requires the
branch to be up to date, admins included — so a fresh commit cannot be
pushed straight to `main`: branch, wait for green, fast-forward.

Commits are blocked unless `.claude/hooks/gates.sh` passes and the
message carries IDs in brackets (`[K3, AR2]`, `[L1]`, `[meta]`).
Enforced twice over: `.githooks/pre-commit` + `.githooks/commit-msg`
(repo-scoped, any session) and `.claude/hooks/check-commit.sh` via
`.claude/settings.json`. CI re-runs the gates on every push.
`.claude/hooks/gates.project.sh` adds the project's own check: a test
total quoted in README.md or docs/TEST_PLAN.md must match the suite.

**CI triggers on `main` and pull requests only**, so a plain branch push
produces no checks and `main` is reachable only through a pull request —
including for a release. `chassis release` therefore cannot be used here
as it stands: it pushes a `release-*` branch and waits for checks that
never run. Releases go the pull-request way, verifying each step (rule
36). `chassis sync` reports `ci.yml` and the two commit hooks as drift on
purpose: the scaffold's copies are older than this repository's, and the
kit has been asked to catch up.

## Scratch hub for development/tests

Tests spawn a local hub themselves (see `tests/support/`): the binary
at `KYU_BIN` (default: `~/Projects/kyu/target/release/kyu`)
or the public `ghcr.io/kennypassenier/kyu` image in CI. Never
`10.10.10.9:8080`.
