# Features — kyu-runner

Phase 2 output. Feature IDs are permanent: they appear in commits,
test names, docs and forms forever.

> **Ratified by Kenny, 2026-08-29** (ratification form 1): every K/M/T
> item and the build-vs-buy record confirmed as recommended; W2 and W4
> raised Desired → **Essential** (already built), W6 raised Later →
> **Desired** (to build — registered as milestone L6), W5's
> supersession and W7's out-of-scope confirmed. The scale is the
> canonical one: Essential · Desired · Later · Don't do.

## Core (from the approved scope)

| ID | Rating | Feature |
|---|---|---|
| K1 | Essential | **Route engine.** TOML config defines routes `{topic, subscription, webhook_url}`. One pump per route, routes run concurrently; **within a route deliveries are strictly sequential** (one in-flight message, no pipelining) so a backlog drains in publish order (scope S2). |
| K2 | Essential | **Byte-for-byte forward.** POST the payload unmodified to the webhook, with the original `content-type` and the kyu metadata headers (`kyu-id`, `kyu-topic`, `kyu-attempt`, `kyu-published-at`) passed through. No parsing, ever (scope NG2). |
| K3 | Essential | **Ack only on 2xx.** The hub ack happens only after the webhook answered 2xx. "Delivered" means processed by HA, not sent. |
| K4 | Essential | **Nack on failure.** Non-2xx, timeout or connect error → nack *without* `dead=true`, so the hub's backoff/retry runs and exhausted attempts dead-letter visibly. The runner never poison-pills: payload-agnostic code cannot judge a payload (scope NG2). |
| K5 | Essential | **Stateless.** No disk state, no cursors, no dedup store. `kill -9` at any moment loses nothing; a restart resumes from the hub's cursors (scope NG1/S3). |
| K6 | Essential | **`kyu.events` default route (P8).** Shipped example config routes the hub's own events to an HA warning webhook; the HA-side automation (warning notification with `click_url` to the hub dashboard) is a documented deliverable. ⚔ Critic: HA answers 200 even for unknown webhook ids (anti-enumeration), so a typo'd `webhook_url` acks messages into the void with healthy metrics — dead letters catch transport failure, never misrouting. Therefore the runbook orders "HA automation first, then route", closing with a per-route test-publish smoke check, and mandates `local_only: true` on every HA webhook trigger. |
| K7 | Essential | **Hub-down resilience.** Hub unreachable → keep running, reconnect with capped backoff, log state *transitions* (down/up) rather than every attempt, resume where it was (scope S5). |
| K8 | Essential | **Config validation, fail-closed.** Startup refuses an invalid config with a remedy in the message (standing rule 11); unknown keys are refused (no silent typo-tolerance); duplicate `{topic, subscription}` pairs and duplicate route names are errors. |
| K9 | Essential | **Hub token.** Bearer token read from the environment (systemd `EnvironmentFile`), sent on every hub request. Never logged, never in git (standing rule 10, asserted by test). A tokenless hub (dev/scratch) works without it. |
| K10 | Essential | **systemd deployment.** Unit file + numbered install procedure for LXC 109, native binary next to the hub — the same pattern the hub itself uses. |
| K11 | Essential | **P8 wiring documentation.** `/healthz` → Uptime Kuma and `kyu_sweeper_age_ms` → Grafana alert, as copy-pasteable steps. Docs deliverable; no code. |

## Proposals (Claude's round-2 additions)

| ID | Rating | Feature |
|---|---|---|
| W1 | Essential | **Graceful shutdown.** SIGTERM/SIGINT → stop claiming new messages, let the in-flight delivery finish within a bounded grace period, then exit. An interrupted delivery is safe anyway (K5); this just avoids a gratuitous duplicate on every deploy. |
| W2 | Essential | **`--check-config`.** Parse + validate the config and exit; wired as `ExecStartPre=` in the unit and used in the runbook before restarts. |
| W3 | Essential | **Declarative route policy.** Optional `[routes.policy]` block (lease_ms, max_attempts, ttl_ms, …) that the runner PUTs to the hub per subscription — config in git as the source of truth for the study's "TTS-ish routes get short TTL, ops routes long". ⚔ Promoted Desired → Essential by the critic pass: AR15's long-outage story leans on raising `max_attempts`, and AR5's lease budget leans on `lease_ms`. Order matters (the PUT fails on a subscription that does not exist yet): first successful poll creates the subscription, then PUT with retry until in force, logging the hub's "values in force" answer per route. The hub's write replaces every field (kyu K7): a runner restart reverts dashboard tweaks — documented loudly in the runbook. |
| W4 | Essential | **Runner `/healthz`.** Minimal HTTP listener reporting the route loops' liveness, for Uptime Kuma. The hub-side idle-subscription flag (kyu K11) already catches a dead runner; this is the direct probe. |
| W5 | — | **Superseded by AR16** (critic pass): topic-birth replay is built-in behaviour, not a config knob — after a 404 the next successful poll carries `from=beginning`, so a brand-new topic's first messages are never lost. Pre-existing topics start from now; manual replay is a runbook procedure. |
| W6 | Desired | **Runner `/metrics`.** Prometheus counters (delivered, nacked, per route). Raised Later → Desired at ratification (Kenny, 2026-08-29): to build as milestone L6, on the same opt-in listener as W4's `/healthz`. Test bar: E2E — counters visible and moving after a delivery and after a nack. |
| W7 | Don't do | **Reverse direction (HA → hub via the runner).** Decided at the Phase 0 gate (B1): HA produces via `rest_command` directly to the hub. |

## Mandatory items (procedure Phase 2)

| ID | Decision |
|---|---|
| M1 | **Update & distribution.** GitHub release workflow: tag → build static `x86_64-unknown-linux-musl` binary → checksum manifest → GitHub Release. Installing/updating on LXC 109 is a numbered runbook procedure (download, verify checksum, replace binary, restart unit). **No self-update, by decision** — a LAN daemon on one machine, updated deliberately. |
| M2 | **Ecosystem integration.** Consumes **kyu** (the point). Token delivery: systemd `EnvironmentFile` (0600, root) now; **latch** recorded as the migration candidate for a mini-round once latch manages LXC 109. Deployment: native systemd like the hub, NOT the homelab preset — deliberate, mirrors Kenny's hub choice. Monitoring rides **Uptime Kuma** + **Grafana** (K11). |
| M3 | **Backup & restore.** The runner's full state = config file + unit file (both in git; deployed copies are just copies) + the app token (re-mintable on the hub's `/apps` page in seconds). Therefore: **state-in-git + re-mint, no backup jobs, by decision.** Restore-from-zero is a numbered runbook procedure and is drilled against the scratch hub in Phase 7. The backup is automatic by construction (git); the restore is exercised, not assumed. |

## Test expectations (the concrete bar, fixed now)

| ID(s) | Bar |
|---|---|
| K1-K5 | E2E against a **real hub binary** (scratch) + a fake HA webhook server: happy path (publish → forward → ack), 500-from-HA does not ack and the message returns (S1), HA-down backlog drains in publish order on recovery (S2), `kill -9` mid-delivery loses nothing and at worst duplicates (S3). |
| K4 | E2E: webhook permanently failing → message dead-letters on the hub after max attempts (S4). |
| K6 | E2E: a dead-letter event on `kyu.events` reaches the configured warning webhook. |
| K7 | E2E: stop the scratch hub under a running runner, restart it, delivery resumes untouched; log output during the outage is bounded (assert on line count). |
| K8 | Unit tests per validation rule, each asserting the remedy text is present. |
| K9 | Plaintext-scan assertion: the token appears in no log line and no error message (standing rule 10). |
| W1 | E2E: SIGTERM mid-delivery → in-flight completes, ack recorded, clean exit. |
| AR15 | E2E: webhook target down (connection refused) while messages publish → attempts do not burn (circuit open, unclaimed backlog); on target return, drain completes in publish order. |
| AR16 | E2E: route configured before its topic exists → 404 quiet-wait; first publish births the topic; the pre-subscription message is delivered anyway (birth replay). |
| AR17 | E2E: webhook answers 302 → treated as failure, no ack, no redirect followed. |
| AR5 | Unit (K8): `webhook_timeout_ms` too large for the effective lease is refused with a remedy naming W3. |
| W2 | E2E: invalid config → non-zero exit + remedy; valid → zero exit, no network calls. |
| W3 | E2E: after startup the hub's policy endpoint reports the configured values. |
| W4 | E2E: `/healthz` answers 200 while routes run. |
| M1 | Workflow exists and is exercised at the first tag (Phase 9); checksum verified in the release drill. |
| M3 | Restore-from-zero drill against the scratch hub (Phase 7), following the runbook literally. |
