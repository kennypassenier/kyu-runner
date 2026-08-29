# Test plan — hub-bridge

Phase 7 output. What is proven where, what each test double cannot
express, and what is not covered **by decision**. Maintained from here
on; the AFK ratification round (PENDING_MINI_ROUNDS.md) may still move
items between "covered" and "accepted".

## The suites (57 tests)

| Suite | Scope |
|---|---|
| `src/config.rs` unit tests (27) | K8 fail-closed validation, one test per rule, every rejection asserted to carry a remedy; AR5 lease-budget math; W3 policy JSON rendering; the shipped `deploy/config.toml` parses. |
| `src/route.rs` unit tests (2) | AR6 backoff shape (doubling, cap, bounded jitter, reset). |
| `tests/l1_check_config.rs` (6) | W2 at the binary boundary: exit codes, remedies, `--version`, and the no-network guarantee (a held listener proves `--check-config` never connects). |
| `tests/l2_pump.rs` (11) | The pump E2E against a **real hub** (binary or docker image): S1-S4, byte-for-byte incl. a non-UTF-8 payload and all four metadata headers, AR15 circuit (both halves: connect-class opens it, a 500 does not), AR16 birth replay, AR17 redirect refusal, F1 oversize cap → dead letter, token/payload log hygiene at trace level in both log formats, 401 remedy without flooding. |
| `tests/l3_resilience.rs` (4) | K7 hub stop/start drill with a bounded log-volume window; W1 SIGTERM mid-delivery and SIGINT; AR1 panic → supervisor respawn → message survives. |
| `tests/l4_extras.rs` (6) | K6 dead-letter event → warning webhook (sweeper-emitted, the only kind the hub emits); W3 applied + hub-refused policy path; W4 healthz; AR16 from-now half; K1 two routes through one outage without cross-route blocking. |

Every test name carries its feature/milestone IDs (checked by the
gap audit; the two CLI-surface tests are tagged W2).

## What each test double cannot express (standing rule 9)

- **FakeHa (the webhook server)** is not Home Assistant. It cannot
  express: (1) **HA's 200-for-unknown-webhook-ids behaviour** — the
  reason dead letters catch transport failure but never misrouting;
  the runbook's per-route smoke test is the control, and measuring the
  real behaviour is queued (Q9); (2) HA's webhook trigger processing
  (does the automation see the `kyu-*` headers? — Q9); (3) HA's
  restart timing. Everything transport-side (status codes, redirects,
  slow responses, connection refusal, binary bodies) it expresses
  faithfully and is covered.
- **The hub is never mocked** — every E2E runs a real kyu process.
  Consequence, accepted: hub behaviours the real hub will not produce
  on demand are untestable here: `HubError::Protocol` (a 200 without
  a valid `kyu-id`) and hub 5xx answers have no test, and the
  malicious-hub scenarios (forged headers, streaming oversize bodies
  beyond `KYU_MAX_BODY_BYTES`) are covered by code paths (cap,
  sanitisation, charset checks) whose hostile halves are argued, not
  executed. Recorded as the price of rule 9.

## Not covered, by decision (pending Kenny's ratification, Q11)

1. **Hub-restart drill under docker (CI):**
   `l3_k7_a_hub_outage_is_one_log_line_and_recovery_is_automatic` returns early
   when only the docker image is available (a removed container keeps
   no state) — CI green does not include the K7 drill; the drill's
   evidence is the dev-machine run. A cached hub binary in CI would
   close this later.
2. **S3 kill point:** the kill -9 drill fires at one instant
   (post-POST, pre-ack — the worst one), not at arbitrary instants.
3. **Second-signal immediate exit and grace-exceeded abort** (AR9
   fine print) are unexercised; single SIGTERM and SIGINT are.
4. **Ack/nack settle failures** (hub dying between POST and settle):
   the log arms exist, the duplicate is legal by contract, no test
   forces the timing.
5. **401-recovery without restart** (token re-minted mid-run): the
   denial and no-flood are tested; the resumption is not.
6. **W2 door tested with the master token**, not a dashboard-minted
   app token (the `/apps` mint flow is browser/CSRF-bound); both are
   bearers on the same header path.
7. **Healthz negative space:** no test asserts the absent-key case
   opens no socket (the code simply never binds) or reads `/healthz`
   during failure states.
8. **musl artifact on the real LXC:** the release workflow E2Es the
   musl binary against the docker hub; first run on LXC 109 itself is
   deploy step 1 (Q9).

## Reasoned vs measured (Phase 7 sweep)

Measured this build: docker harness path + pinned public image (full
suite run with `KYU_BIN=""`), the restore-from-zero drill with the
musl artifact (DRILL-OK), `kyu.events` pre-exists on a fresh hub,
poison-pill nacks emit **no** event (engine-read + measured — the K6
test uses the sweeper path deliberately), hub `wait` is in seconds,
policy PUT fails on a fresh subscription. Still argued, queued as Q9:
everything under "FakeHa cannot express", and the static-pie binary
running on LXC 109's kernel.
