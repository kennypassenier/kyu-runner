# Features — hub-bridge

Phase 2 output. Feature IDs are permanent: they appear in commits,
test names, docs and forms forever.

> **AFK note (2026-08-28).** Ratings below are Claude's recommended
> ratings, taken during the AFK build on Kenny's instruction ("doe
> alles wat je kan zonder mijn input"). They are PROVISIONAL until the
> queued rating form in `docs/PENDING_MINI_ROUNDS.md` is answered.
> The scale is the canonical one: Essential · Desired · Later · Don't do.

## Core (from the approved scope)

| ID | Rating | Feature |
|---|---|---|
| K1 | Essential | **Route engine.** TOML config defines routes `{topic, subscription, webhook_url}`. One pump per route, routes run concurrently; **within a route deliveries are strictly sequential** (one in-flight message, no pipelining) so a backlog drains in publish order (scope S2). |
| K2 | Essential | **Byte-for-byte forward.** POST the payload unmodified to the webhook, with the original `content-type` and the mailbox metadata headers (`mailbox-id`, `mailbox-topic`, `mailbox-attempt`, `mailbox-published-at`) passed through. No parsing, ever (scope NG2). |
| K3 | Essential | **Ack only on 2xx.** The hub ack happens only after the webhook answered 2xx. "Delivered" means processed by HA, not sent. |
| K4 | Essential | **Nack on failure.** Non-2xx, timeout or connect error → nack *without* `dead=true`, so the hub's backoff/retry runs and exhausted attempts dead-letter visibly. The bridge never poison-pills: payload-agnostic code cannot judge a payload (scope NG2). |
| K5 | Essential | **Stateless.** No disk state, no cursors, no dedup store. `kill -9` at any moment loses nothing; a restart resumes from the hub's cursors (scope NG1/S3). |
| K6 | Essential | **`mailbox.events` default route (P8).** Shipped example config routes the hub's own events to an HA warning webhook; the HA-side automation (warning notification with `click_url` to the hub dashboard) is a documented deliverable. |
| K7 | Essential | **Hub-down resilience.** Hub unreachable → keep running, reconnect with capped backoff, log state *transitions* (down/up) rather than every attempt, resume where it was (scope S5). |
| K8 | Essential | **Config validation, fail-closed.** Startup refuses an invalid config with a remedy in the message (standing rule 11); unknown keys are refused (no silent typo-tolerance); duplicate `{topic, subscription}` pairs and duplicate route names are errors. |
| K9 | Essential | **Hub token.** Bearer token read from the environment (systemd `EnvironmentFile`), sent on every hub request. Never logged, never in git (standing rule 10, asserted by test). A tokenless hub (dev/scratch) works without it. |
| K10 | Essential | **systemd deployment.** Unit file + numbered install procedure for LXC 109, native binary next to the hub — the same pattern the hub itself uses. |
| K11 | Essential | **P8 wiring documentation.** `/healthz` → Uptime Kuma and `mailbox_sweeper_age_ms` → Grafana alert, as copy-pasteable steps. Docs deliverable; no code. |

## Proposals (Claude's round-2 additions)

| ID | Rating | Feature |
|---|---|---|
| W1 | Essential | **Graceful shutdown.** SIGTERM/SIGINT → stop claiming new messages, let the in-flight delivery finish within a bounded grace period, then exit. An interrupted delivery is safe anyway (K5); this just avoids a gratuitous duplicate on every deploy. |
| W2 | Desired | **`--check-config`.** Parse + validate the config and exit; wired as `ExecStartPre=` in the unit and used in the runbook before restarts. |
| W3 | Desired | **Declarative route policy.** Optional `[routes.policy]` block (lease_ms, max_attempts, ttl_ms, idle thresholds) that the bridge PUTs to the hub per subscription at startup — config in git as the source of truth for the study's "TTS-ish routes get short TTL, ops routes long". Note: the hub's policy write replaces every field (mailbox K7), so the bridge always sends the complete block. |
| W4 | Desired | **Bridge `/healthz`.** Minimal HTTP listener reporting the route loops' liveness, for Uptime Kuma. The hub-side idle-subscription flag (mailbox K11) already catches a dead bridge; this is the direct probe. |
| W5 | Later | **`from=beginning` per route.** Opt-in backlog pull when a brand-new subscription should start from retained history instead of from now. |
| W6 | Later | **Bridge `/metrics`.** Prometheus counters (delivered, nacked, per route). The hub's metrics already expose queue state; revisit when Grafana wants bridge-side series. |
| W7 | Don't do | **Reverse direction (HA → hub via the bridge).** Decided at the Phase 0 gate (B1): HA produces via `rest_command` directly to the hub. |

## Mandatory items (procedure Phase 2)

| ID | Decision |
|---|---|
| M1 | **Update & distribution.** GitHub release workflow: tag → build static `x86_64-unknown-linux-musl` binary → checksum manifest → GitHub Release. Installing/updating on LXC 109 is a numbered runbook procedure (download, verify checksum, replace binary, restart unit). **No self-update, by decision** — a LAN daemon on one machine, updated deliberately. |
| M2 | **Ecosystem integration.** Consumes **mailbox** (the point). Token delivery: systemd `EnvironmentFile` (0600, root) now; **latch** recorded as the migration candidate for a mini-round once latch manages LXC 109. Deployment: native systemd like the hub, NOT the homelab preset — deliberate, mirrors Kenny's hub choice. Monitoring rides **Uptime Kuma** + **Grafana** (K11). |
| M3 | **Backup & restore.** The bridge's full state = config file + unit file (both in git; deployed copies are just copies) + the app token (re-mintable on the hub's `/apps` page in seconds). Therefore: **state-in-git + re-mint, no backup jobs, by decision.** Restore-from-zero is a numbered runbook procedure and is drilled against the scratch hub in Phase 7. The backup is automatic by construction (git); the restore is exercised, not assumed. |

## Test expectations (the concrete bar, fixed now)

| ID(s) | Bar |
|---|---|
| K1-K5 | E2E against a **real hub binary** (scratch) + a fake HA webhook server: happy path (publish → forward → ack), 500-from-HA does not ack and the message returns (S1), HA-down backlog drains in publish order on recovery (S2), `kill -9` mid-delivery loses nothing and at worst duplicates (S3). |
| K4 | E2E: webhook permanently failing → message dead-letters on the hub after max attempts (S4). |
| K6 | E2E: a dead-letter event on `mailbox.events` reaches the configured warning webhook. |
| K7 | E2E: stop the scratch hub under a running bridge, restart it, delivery resumes untouched; log output during the outage is bounded (assert on line count). |
| K8 | Unit tests per validation rule, each asserting the remedy text is present. |
| K9 | Plaintext-scan assertion: the token appears in no log line and no error message (standing rule 10). |
| W1 | E2E: SIGTERM mid-delivery → in-flight completes, ack recorded, clean exit. |
| W2 | E2E: invalid config → non-zero exit + remedy; valid → zero exit, no network calls. |
| W3 | E2E: after startup the hub's policy endpoint reports the configured values. |
| W4 | E2E: `/healthz` answers 200 while routes run. |
| M1 | Workflow exists and is exercised at the first tag (Phase 9); checksum verified in the release drill. |
| M3 | Restore-from-zero drill against the scratch hub (Phase 7), following the runbook literally. |
