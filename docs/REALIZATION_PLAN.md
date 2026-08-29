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
| L4 | K6, W3, W4 | `kyu.events` default route in shipped config; declarative route policy PUT at startup; opt-in `/healthz`. E2E each. |
| L5 | K10, K11, M1, M3 | Deployment: musl release build script, systemd unit + `ExecStartPre` check, numbered install/update/restore runbook, P8 wiring docs (Uptime Kuma, Grafana), release workflow. Restore drill on scratch. |

Order rationale: config before pump (the pump is unrunnable without
it), failure semantics before convenience features, packaging last
when the binary's shape is settled.

## Status

| Milestone | Status |
|---|---|
| L0 | done 2026-08-29 — skeleton, enforcement live (git-native hooks + CI + toolchain pin), gates green |
| L1 | done 2026-08-29 — config domain; 16 unit tests + 5 binary tests, every rejection carries a remedy |
| L2 | done 2026-08-29 — the pump; 8 E2E scenarios against the real hub binary (S1-S4 + AR15/AR16/AR17 + token hygiene) |
| L3 | done 2026-08-29 — hub stop/start drill (one transition line, auto-recovery) + SIGTERM mid-delivery exits zero |
| L4 | done 2026-08-29 — kyu.events route (sweeper event E2E), W3 policy PUT with values-in-force log, /healthz |
| L5 | done 2026-08-29 — static musl artifact, systemd unit, release workflow (E2Es the shipped artifact), runbook; restore-from-zero drill DRILL-OK on scratch (dead letter → warning webhook via the musl binary, acked, clean stop) |

## Gate log (from Phase 7 onward; standing rule 5)

| Gate | Date | Decision | Recorded |
|---|---|---|---|
| Phase 7 · test-gap audit | 2026-08-29 | AFK-provisional: 20 gaps found; closed 14 in the same pass (AR1 supervision built + panic drill, multi-route independence E2E, AR16 from-now E2E, binary payload + full header asserts, trace/json token scans on failure paths, K7 log-volume bound, AR15 5xx boundary assert, W2 no-network assert, shipped-config parse test, W3 refused-policy E2E, policy_json unit tests, three K8 arms, Backoff units, release workflow → 3 suites vs artifact); 6 accepted → TEST_PLAN.md; ratification queued (Q11) | TEST_PLAN.md |
| Phase 7 · security pass (interim for /security-review, see Q10) | 2026-08-29 | AFK-provisional: 12 findings, 0 critical/high, all code fixes landed (F1 16 MiB body cap streamed + MemoryMax backstop, F2 healthz timeout/semaphore/accept-backoff, F3 SHA-pinned actions + read-only CI token + image digest, F4 control-char sanitisation of hub text, F5 kyu-id charset, F6 real URL validation, F7 policy JSON constraints + lease bound, F8 read -rs token entry, F9 systemd hardening set, F12 healthz bind guidance); ratification queued (Q12) | this row + commits |
| Phase 7 · reasoned-vs-measured sweep | 2026-08-29 | Docker harness path + image measured (full suite via KYU_IMAGE), restore drill measured (DRILL-OK); remaining argued claims all live in the Q9 real-HA/LXC step | TEST_PLAN.md |
| Phase 8 · documentation | 2026-08-29 | AFK-provisional: USER_GUIDE + DEBUGGING_GUIDE + ARCHITECTURE_REFERENCE written from code/tests; README honesty pass; "Proven by" names mechanically verified (33 references, 0 missing); approval form queued (Q13) | docs/ + Q13 |
