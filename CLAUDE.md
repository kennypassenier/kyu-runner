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
| Current phase | all 11 phases done; **0.2.3** released 2026-09-10 on chassis 2.0.2 and running on CT 109 (measured 2026-09-26: `/healthz` on 10.10.10.9:8082 answers `0.2.3`, all four subsystems ok). A stray `/usr/local/bin/kyu-runner` 0.1.0 on CT 109 is not what the unit runs |
| Last completed gate | The chassis 2.0.2 round (2026-09-10): the bump was a dependency change and nothing else, and `chassis release` ran the whole chain itself for the first time since the CI trigger was restored |
| Next gate | Release 1.0.0, on Kenny's go. Its condition ("1.0.0 follows the first rollout that measures real HA") is met: measured 2026-09-26, the runner on CT 109 has delivered 306 `homelab-ops` and 13 `kyu-events` messages to the real HA (10.10.10.2) with 0 nacks, and HA answers 200 to an unknown webhook id (Q9) |
| Next action | waiting on Kenny: the decision form of 2026-09-26 (release 1.0.0; the stray 0.1.0 binary on CT 109). The `tool-help` measurement waits on the next chassis release (latest tag still v2.0.2, checked 2026-09-26) |
| AFK mode | off |

The AFK build's queue in `docs/PENDING_MINI_ROUNDS.md` is now fully
answered — it stays as the record of what was decided when. The real
hub on LXC 109 is still only touched as an agreed step (scope R1). The
scratch drill container LXC 191 no longer exists: `pct list` on pve and
the standalone Proxmox showed no 191 on 2026-09-26.

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

**CI runs on every branch** (restored 2026-09-10), so a branch push
produces the check branch protection waits for and `main` is reachable by
fast-forward again. `gates` is the only job, which is what narrowing the
trigger was really after. `chassis sync` still reports
`.github/workflows/ci.yml` as drift and should: the scaffold carries three
jobs more (`deny`, `image`, `coverage`) that this project deliberately does
not run. The three shared hooks are no longer sync's business since kit
2.0.2 — dev-procedure owns them.

## Scratch hub for development/tests

Tests spawn a local hub themselves (see `tests/support/`): the binary
at `KYU_BIN` (default: `~/Projects/kyu/target/release/kyu`)
or the public `ghcr.io/kennypassenier/kyu` image in CI. Never
`10.10.10.9:8080`.
