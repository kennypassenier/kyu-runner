# Realization plan — hub-bridge

Phase 5 output. L0 is the walking skeleton; every milestone ends
tested and committed, gates green.

> **AFK note (2026-08-28/29).** Plan drafted and executed during the
> AFK build; the Phase 5 approval form is queued
> (`PENDING_MINI_ROUNDS.md` Q6).

## Milestones

| ID | Features | Exit criteria |
|---|---|---|
| L0 | — (skeleton) | Repo + crate skeleton, enforcement live (git-native hooks + `.claude` hooks + CI workflow), `cargo test` green with a placeholder test, toolchain pinned. |
| L1 | K8, W2, K9 | Config: parse, validate fail-closed with remedies, `--check-config`; token from env, never logged. Unit tests per validation rule. |
| L2 | K1-K5 | The pump, E2E against the real hub binary + fake HA server: happy path, 500-no-ack (S1), backlog drain in order (S2), kill -9 drill (S3), permanent-failure dead-letter (S4). |
| L3 | K7, W1 | Hub-down resilience (backoff, transition-only logging, bounded volume) + graceful shutdown. E2E: hub stop/start under load; SIGTERM mid-delivery. |
| L4 | K6, W3, W4 | `mailbox.events` default route in shipped config; declarative route policy PUT at startup; opt-in `/healthz`. E2E each. |
| L5 | K10, K11, M1, M3 | Deployment: musl release build script, systemd unit + `ExecStartPre` check, numbered install/update/restore runbook, P8 wiring docs (Uptime Kuma, Grafana), release workflow. Restore drill on scratch. |

Order rationale: config before pump (the pump is unrunnable without
it), failure semantics before convenience features, packaging last
when the binary's shape is settled.

## Status

| Milestone | Status |
|---|---|
| L0 | pending |
| L1 | pending |
| L2 | pending |
| L3 | pending |
| L4 | pending |
| L5 | pending |

## Gate log (from Phase 7 onward; standing rule 5)

| Gate | Date | Decision | Recorded |
|---|---|---|---|
| — | | | |
