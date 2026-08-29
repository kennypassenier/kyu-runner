# Architecture reference — hub-bridge

The system as built (Phase 8). The decisions live in
ARCHITECTURE_DECISIONS.md; this is how they landed in code.

## Module map

```
src/main.rs      thin shell: CLI, tracing, runtime, signals,
                 supervision (AR1), healthz wiring, shutdown grace
src/config.rs    parse + fail-closed validation (K8), lease-budget
                 math (AR5), policy JSON rendering (W3) — pure, the
                 only I/O is the file read in load()
src/route.rs     the pump state machine: one loop per route (AR2),
                 outcome handling (AR3), circuit breaker (AR15),
                 birth replay (AR16), transition logging (AR6),
                 policy application (W3)
src/hub.rs       kyu client: next/ack/nack/put_policy, raw mode
                 (AR4), body cap (F1), id validation (F5), log
                 sanitisation (F4)
src/webhook.rs   HA client: POST with redirect::Policy::none (AR17),
                 connect-class vs status classification; the bare TCP
                 probe for the circuit breaker
src/health.rs    opt-in /healthz (W4/AR10): hand-rolled HTTP, bounded
                 (timeouts, semaphore, accept backoff — F2)
```

Tests: `tests/support/mod.rs` spawns a **real** hub (binary or docker
image) and a fake HA webhook server; `tests/l1..l4_*.rs` are the E2E
suites; unit tests live beside the pure code they exercise.

## The route loop

Per route, one task, one in-flight message. Each iteration:

```
poll GET /t/<topic>/next?as=<sub>&wait=25[&from=beginning]
├─ network error / 5xx → hub-down state: warn once, backoff
│                        500 ms → 30 s, retry            (K7/AR6)
├─ 401                 → auth-denied: warn once (with the /apps
│                        remedy), backoff, retry          (AR7/AR6)
├─ 404 topic unborn    → quiet wait, re-poll every 5 s; arm
│                        from=beginning for the birth poll (AR16)
├─ 204 empty           → apply pending policy (W3), loop
└─ 200 message         → apply pending policy, then deliver:
   ├─ POST 2xx         → ack (5 s timeout, one retry; a failed
   │                     ack = logged, duplicate is legal)  (AR3)
   └─ failure          → retry in-process on the same claim,
      backoff 1/2/4/8 s, until elapsed + next delay +
      webhook timeout > lease budget, then nack:
      ├─ connect-class → CIRCUIT OPEN: stop claiming, bare TCP
      │                  probe with backoff 1 s → 60 s; on
      │                  success resume (backlog unclaimed,
      │                  attempts preserved, order kept)   (AR15)
      └─ status-class  → loop (the hub schedules redelivery;
                         attempts burn ~1 per lease budget,
                         dead letter at exhaustion)     (K4/S4)
```

An `Oversize` outcome (body larger than the cap) bypasses delivery:
warn + nack unread; the attempts run out into a visible dead letter.

## The timing model (all defaults, all in one place)

| Number | Where | Why |
|---|---|---|
| 25 s | `poll_wait_ms` long-poll `wait` (wire unit: whole seconds) | under the hub's 30 s default and 300 s cap |
| wait + 10 s | poll client timeout | the poll must outlive its own wait |
| 5 s | settle client timeout (ack/nack/policy) | reserved inside the lease budget |
| 10 s | `webhook_timeout_ms` default, per-route override | one POST |
| 70% of lease | in-process retry budget (lease = `policy.lease_ms` or the hub default 30 000) | the claim is settled by its owner, never by expiry; K8 refuses `webhook_timeout + 5 s > budget` |
| 1/2/4/8 s (cap 8) | in-process delivery retries | head-of-line stays head-of-line |
| 500 ms → 30 s | hub-down backoff (×2, jitter ≤ 12.5%) | K7 |
| 1 s → 60 s | circuit probe backoff | AR15 |
| 5 s | route respawn pause after a panic | AR1 |
| max webhook timeout + 5 s | derived shutdown grace | AR9 — an in-flight delivery always fits |
| 16 MiB | `max_body_bytes` default (min 1024) | F1 — a hub cannot OOM the bridge; `MemoryMax=128M` in the unit as backstop |
| 5 s / 16 | healthz per-connection timeout / concurrent connections | F2 — a probe port cannot starve the pump |

## Supervision and shutdown

`main.rs::supervise` wraps every route task: a normal return ends it,
a panic is logged (`route loop died unexpectedly`) and the loop
respawns after 5 s with the same `RouteRunner` (AR1). Shutdown: a
`watch::channel(bool)` flips on SIGTERM/SIGINT; loops break at safe
points, in-flight deliveries finish inside the derived grace, stragglers
are aborted at the deadline; a second signal exits immediately (K5
makes that safe). Exit code 0 on an orderly stop.

## How the ARs landed (index)

- **AR1** `main.rs::supervise` · **AR2/AR3** `route.rs::run` +
  `deliver_with_budget` · **AR4** `hub.rs::next` (raw mode, header
  passthrough minus `kyu-notice`) · **AR5** `config.rs::lease_budget`
  + the K8 check · **AR6** the state booleans + `note_recovery` in
  `route.rs` · **AR7** token env in `main.rs`, `bearer_auth` in
  `hub.rs` · **AR8** `config.rs` schema · **AR9** derived grace in
  `config.rs::shutdown_grace` + the join loop in `main.rs::run` ·
  **AR10** `health.rs` · **AR11** no TLS stack compiled in (reqwest
  `default-features = false`) + K8 refusing non-http URLs · **AR12**
  the module map above · **AR13** no dedup anywhere, `kyu-id`
  forwarded as the consumer-side key · **AR14** payloads never
  formatted into log lines; hub-controlled text passes
  `hub.rs::printable` · **AR15** the circuit arm in `route.rs` +
  `webhook.rs::probe_origin` · **AR16** the `replay` flag in
  `route.rs` · **AR17** `webhook.rs` redirect policy + status
  classification.

The proof for each is the E2E suite — the map from feature/AR to test
function lives in USER_GUIDE.md (per feature) and TEST_PLAN.md (per
suite, including what is deliberately not covered).
