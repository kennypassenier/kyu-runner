# Architecture decisions — kyu-runner

Phases 3-4 output. T = tech choice, AR = architecture.

> **FROZEN 2026-08-29** by Kenny (ratification form 2): AR1-AR17
> confirmed; changes go through mini-rounds only. One dated amendment
> was recorded at the freeze itself: AR11 (see there). The platform
> question was answered at the same gate: the runner will run on **an
> LXC on the Proxmox host, which one is deliberately not yet chosen**
> (x86_64 Linux; the earlier LXC-109 assumption is thereby replaced),
> and Kenny authorized deployment testing on a **scratch LXC**.

> **AFK note (2026-08-28).** Decisions below were taken on documented
> recommendations during the AFK build and are PROVISIONAL until the
> queued ratification forms in `docs/PENDING_MINI_ROUNDS.md` are
> answered. The architecture-critic pass (mandatory: network + auth)
> ran before the build; surviving objections are recorded per item as
> "⚔" and are embedded in the queued forms.

## Tech (Phase 3)

- **T1 · Language & toolchain.** Rust, edition 2024, toolchain pinned
  to **1.97** via `rust-toolchain.toml` (standing rule 7: the gate must
  predict the build; matches kyu and this machine). `rust-version`
  in Cargo.toml stays in lockstep.
- **T2 · Runtime: tokio** (`rt-multi-thread`, `macros`, `signal`,
  `time`). House standard (kyu, homelab); gives clean per-route
  concurrency, timeouts and cancellation. The alternative (ureq +
  threads) was weighed: fewer deps, but blocking threads parked in
  30-second long-polls make graceful shutdown (W1) and per-route
  backoff clumsier, and the house has no ureq idiom to match.
- **T3 · HTTP client: reqwest** with `default-features = false` — no
  TLS stack at all: the hub and HA are plain-HTTP LAN peers (hub N3),
  and a TLS stack that exists is a TLS stack to patch. Same choice
  kyu's own test suite makes.
- **T4 · Config: serde + toml**, `deny_unknown_fields` everywhere — a
  typo'd key is a startup error with a remedy, never silently ignored
  (standing rule 12: no silent fallbacks).
- **T5 · Logging: tracing + tracing-subscriber** (`env-filter`,
  `json`). `KYU_RUNNER_LOG` filter, `KYU_RUNNER_LOG_FORMAT=json` for
  Loki — the hub's exact convention with the runner's prefix.
- **T6 · Errors: thiserror** for typed config/validation errors,
  **anyhow** context at the binary edge. Every message carries a
  remedy (standing rule 11).
- **T7 · Dependency policy: reluctant, policed.** Each direct
  dependency justified in the commit that adds it; **cargo-deny** in CI
  (advisories, licenses, duplicates) — kyu's regime.
- **T8 · Platform & targets.** Dev + CI: `x86_64-unknown-linux-gnu`.
  Release artifact: **`x86_64-unknown-linux-musl`, statically linked** —
  LXC 109's libc is not this Arch machine's libc, and a static binary
  removes the whole class (the runner needs no C dependencies; there is
  no sqlite here). **Platform answered at ratification (2026-08-29):**
  an LXC on the Proxmox host, which one deliberately TBD — the static
  musl artifact keeps every Debian-ish LXC in reach; scratch-LXC
  deployment testing authorized by Kenny.
- **T9 · License:** MIT OR Apache-2.0, like kyu.

## Architecture (Phase 4)

- **AR1 · Process model.** One process. A supervisor spawns one tokio
  task per route; a route task never takes the process down — internal
  errors log and the loop restarts with backoff. Startup errors
  (config, token file) exit non-zero before any network call
  (fail-closed). systemd `Restart=on-failure` is the outer layer.
- **AR2 · One in-flight message per route, held through transient
  failures.** Receive → deliver → settle, strictly sequential per
  route; no pipelining, no prefetch. A failing delivery is **retried
  in-process on the same claim** (AR3) instead of nacked per attempt,
  so the head of the line stays the head of the line. ⚔ *Critic
  (2026-08-29, adopted):* naive nack-per-failure let the very next
  poll deliver msg2 before the backed-off msg1 — ordering inverted
  exactly in the failure scenarios S2 describes. Fine print, recorded
  honestly: after a lease-budget exhaustion nack (AR3), the nacked
  message re-slots behind its hub backoff, so a bounded inversion
  around that boundary is possible; the accumulate-then-drain scenario
  of S2 itself is preserved by the circuit breaker (AR15), which stops
  claiming entirely while the webhook target is down.
- **AR3 · Outcome mapping.** Webhook 2xx → ack. Ack gets a 5 s
  timeout and one retry; a failed or conflicted ack (409 `NotClaimed`
  after a lease expiry) is logged and the loop moves on — the
  redelivered duplicate is S3-legal. Anything else — non-2xx status
  (3xx included, AR17), timeout, connect error — is retried
  **in-process on the same claim** with a short backoff (1/2/4/8 s)
  while the lease budget allows (AR5); only when the budget is
  exhausted does the runner nack **without** `dead=true`, handing the
  message back to the hub's backoff/retry/DLQ machinery (K4). A
  connect-class failure additionally opens the route's circuit
  breaker (AR15). If the nack itself fails (hub vanished mid-settle),
  do nothing: the lease expiry redelivers (kyu K5). The runner
  never sends a poison pill — payload-agnostic code cannot judge
  payloads. ⚔ *Critic (adopted):* with hub defaults (5 attempts,
  linear 1 s backoff) the draft's nack-per-failure dead-lettered every
  message ~10-15 s into an HA outage — a routine HA update would have
  killed the entire backlog on every route.
- **AR4 · Raw mode, not `envelope=json`.** The payload arrives as the
  raw body with metadata in `kyu-*` response headers; the runner
  forwards body + `content-type` byte-for-byte and passes the
  `kyu-id`, `kyu-topic`, `kyu-attempt`,
  `kyu-published-at` headers through on the webhook POST. No
  re-encoding, no double-JSON.
- **AR5 · Timeouts and the lease budget.** Long-poll `wait` = 25 s
  (config in ms, sent to the hub in whole seconds — the wire unit;
  hub default 30, max 300); the poll request timeout = wait + 10 s;
  settle calls (ack/nack/policy) get their own 5 s timeout. Webhook
  POST timeout 10 s default, `webhook_timeout_ms` overridable per
  route. **Lease budget:** in-process retries (AR3) stop at 70% of
  the effective lease (the route's `policy.lease_ms` if set, else the
  hub default 30 000 ms), so the claim is always settled by its owner,
  never by expiry. K8 validates `webhook_timeout_ms + 5 s margin ≤
  budget` and refuses the config otherwise, with a remedy naming a
  W3 lease raise. ⚔ *Critic (adopted):* an unvalidated 60 s webhook
  timeout against a 30 s lease made the POST succeed after expiry,
  the ack fail 409, and HA receive a duplicate — per delivery.
- **AR6 · Hub-down backoff and transition-only logging.** Exponential
  per route: 500 ms doubling to a 30 s cap, with jitter. Log **state
  transitions only** — never a line per failed attempt: S5's "no log
  flood" is a design property, not a hope. ⚔ *Critic (adopted):* this
  applies to **all three** route states, not just hub-down: hub
  unreachable, webhook target down (AR15 circuit), and auth denied
  (401 — the hub is up but the token is missing/revoked; one line
  with the `/apps` remedy, retried with backoff, not a flood every
  25 s). A poll answering 404 (`UnknownTopic` — the topic has not
  been born yet; kyu creates topics on first publish) is a
  **quiet wait state**: one transition line, a 5 s re-poll, no error
  spam.
- **AR7 · Token.** `KYU_RUNNER_TOKEN` from the environment, injected
  via a root-owned 0600 systemd `EnvironmentFile`
  (`/etc/kyu-runner/token.env`). Sent as `authorization: Bearer` on
  every hub request. Never logged, never in argv, never in the config
  file (which lives in git — standing rule 10); a redaction test
  asserts it. A hub without a door (dev/scratch) works with the
  variable absent; a 401 from the hub logs a remedy naming the `/apps`
  page.
- **AR8 · Config schema** (`/etc/kyu-runner/config.toml`, in git as
  `deploy/config.toml`):

  ```toml
  hub_url = "http://127.0.0.1:8080"
  # healthz_listen = "0.0.0.0:8081"   # W4: absent = no listener

  [defaults]
  poll_wait_ms = 25000
  webhook_timeout_ms = 10000

  [[routes]]
  name = "kyu-events"             # required, unique; the log/health handle
  topic = "kyu.events"
  subscription = "ha-runner"
  webhook_url = "http://homeassistant.lan:8123/api/webhook/hub_kyu_events"
  # webhook_timeout_ms = 5000         # per-route override
  # [routes.policy]                   # W3: applied to the hub at startup
  # max_attempts = 10
  ```

  Validation: unique route names, unique `{topic, subscription}`
  pairs, absolute http URLs, non-empty fields — each violation a
  startup error with a remedy (K8).
- **AR9 · Shutdown (W1).** SIGTERM/SIGINT → stop claiming, in-flight
  delivery gets a bounded grace, settle, exit 0. The grace is
  **derived**, not configured: max over routes of `webhook_timeout` +
  5 s settle margin. ⚔ *Critic (adopted):* a fixed 10 s grace equal to
  the webhook timeout produced on every deploy exactly the duplicate
  W1 exists to avoid. A second signal exits immediately — an
  interrupted delivery is safe by K5.
- **AR10 · `/healthz` (W4).** Opt-in via `healthz_listen`; absent
  means no socket is opened (fail-closed default). Minimal hand-rolled
  HTTP over a tokio listener — no web framework for one endpoint.
  Reports `{status, routes: [{name, state}]}`; route names only, never
  payloads, never the token.
- **AR11 · No TLS, LAN only** *(amended at ratification, 2026-08-29)*.
  Mirrors the hub's N3. The runner is never exposed beyond the LAN;
  documented loudly in the runbook. **Amendment (Kenny, form 2,
  "Aanpassen"):** https traffic via Traefik may appear in the future.
  Recorded consequences: (1) *inbound* TLS (e.g. `/healthz` or
  `/metrics` behind Traefik) needs **no runner change** — Traefik
  terminates TLS and forwards plain HTTP; (2) *outbound* https (a
  `webhook_url` or `hub_url` behind a TLS-terminating Traefik) is the
  real trigger: the moment such a concrete URL exists, a **mini-round**
  adds `rustls` + `webpki-roots` to reqwest and relaxes the K8 scheme
  check — queued as AR11-TLS in PENDING_MINI_ROUNDS.md. Until then the
  no-TLS build stands and the K8 remedy names this path.
- **AR12 · Module layout.** `config.rs` (pure parse/validate — unit
  tested), `route.rs` (the pump state machine over client traits),
  `hub.rs` + `webhook.rs` (reqwest adapters), `health.rs`, `main.rs`
  (wiring, signals). Mock transports exist for state-machine unit
  tests, but correctness is proven E2E against the **real hub binary**
  (standing rule 9); each mock's inexpressible behaviours are named in
  TEST_PLAN.md.
- **AR13 · No dedup, by decision.** At-least-once reaches HA
  (kyu N4); the `kyu-id` header gives HA automations a
  correlation key if a consumer ever needs to suppress duplicates.
  Idempotency lives consumer-side (study §7).
- **AR14 · Payload log hygiene.** Payloads never reach a log line —
  ids, sizes and statuses only (they may carry personal data; mirrors
  the hub's own G9 guarantee). Asserted E2E with a sentinel payload.
- **AR15 · Circuit breaker per route** *(added from the critic pass,
  2026-08-29)*. When a delivery fails connect-class (connection
  refused/timed out — the target machine is down, nothing
  payload-specific) and the lease budget runs out, the route nacks
  once and **opens its circuit: it stops claiming entirely.** A probe
  — a bare TCP connect to the webhook's host:port, no HTTP request,
  so no webhook can ever be triggered by probing — runs with backoff
  (1 s → 60 s cap); on success the circuit closes and claiming
  resumes. While the circuit is open the backlog accumulates
  **unclaimed** on the hub: no attempts burn, and recovery drains in
  publish order — this is what makes S2 literally true across an HA
  outage. HTTP-status failures (HA's server up but answering 5xx) do
  not open the circuit — there the next real message is the only
  side-effect-free probe, and the attempt burn is bounded and
  documented (≈1 attempt per message per lease budget; W3 raises
  `max_attempts` on routes that must survive long partial outages).
- **AR16 · Topic-birth replay** *(added from the critic pass)*.
  kyu creates a topic on first publish and a subscription on its
  first poll, and a subscription only sees what follows its creation —
  so the first message on a brand-new topic would fall between the
  runner's 404 and its next poll, permanently. Therefore: after a
  route has seen 404 (`UnknownTopic`), its next successful poll
  carries `from=beginning`, which backfills the just-born topic's
  retained messages (idempotent and bounded — the topic is seconds
  old). A topic that already exists when the route first polls starts
  **from now**, on purpose: backfilling months of retained history
  into HA would be the worse surprise. The runbook documents the
  manual `from=beginning` replay for the rare "route added after
  traffic started" case.
- **AR18 · Operational timings are configuration, coupled ones are
  constants** *(mini-round MR1, ratified by Kenny 2026-08-30 — the
  standing rule "operational knobs are configurable" landed after the
  Phase 4 freeze)*. A `[tuning]` block carries the ten timings an
  operator might change on a running machine: hub backoff (base/max),
  circuit probe backoff (base/max) and probe timeout, topic-unborn poll
  interval, route respawn pause, settle timeout, and the two health
  socket limits. Every default equals the value that was hardcoded
  before, so an absent block is a no-op. **Pinned, with the reason
  beside each:** the 1/2/4/8 s delivery retry ladder and the single
  settle retry (both exist to fit inside the lease budget — separate
  knobs invite a combination that silently outlives the claim, the very
  failure AR5 prevents), and the health socket's 100 ms accept pause (a
  busy-loop guard, not a tuning knob). The settle timeout is the
  coupling that had to be handled rather than avoided: it now **drives**
  AR5's budget margin, so raising it tightens the budget instead of
  letting the two drift apart.
- **AR17 · No redirects.** The webhook client uses
  `redirect::Policy::none()`; any 3xx is a delivery failure. ⚔
  *Critic (adopted):* reqwest's default policy follows a 302 by
  converting POST to a body-less GET, which HA can answer 200 — the
  runner would then ack a message whose payload never arrived. K8
  already refuses `https://` URLs with a remedy naming the no-TLS
  build (T3/AR11).
