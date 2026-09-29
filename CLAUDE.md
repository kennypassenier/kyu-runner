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
| Current phase | all 11 phases done; **1.0.1** (chassis 2.2.1) tagged and built 2026-09-27, **not signed yet** and not `latest` (fix-10). CT 109 still runs **1.0.0** (`/healthz`), with 0.2.3 kept as `kyu-runner.prev`; `/usr/local/bin/kyu-runner` is a symlink to the unit's binary |
| Last completed gate | Kenny's form of 2026-09-26: `release` = release and roll out, `stray` = replace the old 0.1.0 with a link. Both done the same evening |
| Next gate | Signing, done for all four chassis projects at once from a separate thread (Kenny, 2026-09-27). `chassis release 1.0.1` stopped at `scripts/sign-release.sh v1.0.1` (minisign password). v1.0.0 stays unsigned on purpose: 1.0.1 supersedes it |
| Next action | nothing open for Kenny. **v1.0.1 signed 2026-09-27 08:57 local** (minisign OK, `releases/latest/download/VERSION` = 1.0.1) and **live on CT 109** via `homelab install-native stacks/kyu/kyu-runner v1.0.1`: `/healthz` 1.0.1 on 10.10.10.9:8082, `NRestarts=0`; 1.0.0 kept as `/opt/kyu-runner/bin/kyu-runner.prev-1.0.0` |
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

There is no GitHub Actions CI (removed 2026-09-29). Branch protection on
`main` still requires the `fmt · clippy · tests` check that only the
removed CI produced, so pushes to `main` are refused until Kenny drops
that requirement on GitHub (`chassis sync --protect` of 3.0.0 sets none).

The docker-backed suites (`l2_pump` and the other hub tests) start the
kyu hub from its published image when `KYU_BIN` is unset; on WSL the user
is in the `docker` group (a shell older than that group needs
`newgrp docker` until the next login).

Commits are blocked unless `.claude/hooks/gates.sh` passes and the
message carries IDs in brackets (`[K3, AR2]`, `[L1]`, `[meta]`).
Enforced twice over: `.githooks/pre-commit` + `.githooks/commit-msg`
(repo-scoped, any session) and `.claude/hooks/check-commit.sh` via
`.claude/settings.json`. `chassis release` re-runs the full gate (fmt,
clippy, tests, gates.project.sh, cargo-deny, image smoke) before it
commits; `chassis release <next> --dry-run` runs it without releasing.
`.claude/hooks/gates.project.sh` adds the project's own check: a test
total quoted in README.md or docs/TEST_PLAN.md must match the suite.

The image smoke (`docker build` + `--version` + a failing closed-port
`--healthcheck`) and coverage (informational) run in `chassis release`'s
gate; there is no workflow for `chassis sync` to report as drift any more.
The three shared hooks are no longer sync's business since kit
2.0.2 — dev-procedure owns them.

## Releasing

`chassis release <version>` (chassis-rs >= 3.0.0) builds and publishes
the release on this machine: gate, bump + tag, static musl binary in
docker (`ldd` refusal), `dist/kyu-runner` + `dist/SHA256SUMS`, image
`ghcr.io/kennypassenier/kyu-runner:v<version>` + `:latest`, then push,
`gh release create` (not `latest`) and `scripts/sign-release.sh`.
`--dry-run` stops before any commit, tag or upload. There is no release
workflow any more (removed 2026-09-29). The pin is still v2.2.1: run
`chassis upgrade 3.0.0` + `chassis sync --write` before the next release.

## Scratch hub for development/tests

Tests spawn a local hub themselves (see `tests/support/`): the binary
at `KYU_BIN` (default: `~/Projects/kyu/target/release/kyu`)
or the pinned public `ghcr.io/kennypassenier/kyu` image when it is
unset or empty. Never
`10.10.10.9:8080`.
