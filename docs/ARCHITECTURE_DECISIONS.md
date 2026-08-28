# Architecture decisions — hub-bridge

Phases 3-4 output. T = tech choice, AR = architecture.

> **AFK note (2026-08-28).** Decisions below were taken on documented
> recommendations during the AFK build and are PROVISIONAL until the
> queued ratification forms in `docs/PENDING_MINI_ROUNDS.md` are
> answered. The architecture-critic pass (mandatory: network + auth)
> ran before the build; surviving objections are recorded per item as
> "⚔" and are embedded in the queued forms.

## Tech (Phase 3)

- **T1 · Language & toolchain.** Rust, edition 2024, toolchain pinned
  to **1.97** via `rust-toolchain.toml` (standing rule 7: the gate must
  predict the build; matches mailbox and this machine). `rust-version`
  in Cargo.toml stays in lockstep.
- **T2 · Runtime: tokio** (`rt-multi-thread`, `macros`, `signal`,
  `time`). House standard (mailbox, homelab); gives clean per-route
  concurrency, timeouts and cancellation. The alternative (ureq +
  threads) was weighed: fewer deps, but blocking threads parked in
  30-second long-polls make graceful shutdown (W1) and per-route
  backoff clumsier, and the house has no ureq idiom to match.
- **T3 · HTTP client: reqwest** with `default-features = false` — no
  TLS stack at all: the hub and HA are plain-HTTP LAN peers (hub N3),
  and a TLS stack that exists is a TLS stack to patch. Same choice
  mailbox's own test suite makes.
- **T4 · Config: serde + toml**, `deny_unknown_fields` everywhere — a
  typo'd key is a startup error with a remedy, never silently ignored
  (standing rule 12: no silent fallbacks).
- **T5 · Logging: tracing + tracing-subscriber** (`env-filter`,
  `json`). `HUB_BRIDGE_LOG` filter, `HUB_BRIDGE_LOG_FORMAT=json` for
  Loki — the hub's exact convention with the bridge's prefix.
- **T6 · Errors: thiserror** for typed config/validation errors,
  **anyhow** context at the binary edge. Every message carries a
  remedy (standing rule 11).
- **T7 · Dependency policy: reluctant, policed.** Each direct
  dependency justified in the commit that adds it; **cargo-deny** in CI
  (advisories, licenses, duplicates) — mailbox's regime.
- **T8 · Platform & targets.** Dev + CI: `x86_64-unknown-linux-gnu`.
  Release artifact: **`x86_64-unknown-linux-musl`, statically linked** —
  LXC 109's libc is not this Arch machine's libc, and a static binary
  removes the whole class (the bridge needs no C dependencies; there is
  no sqlite here). ⚠ The procedure requires the platform question to be
  put to Kenny as an OPEN question — queued; the working assumption is
  "LXC 109 only".
- **T9 · License:** MIT OR Apache-2.0, like mailbox.

## Architecture (Phase 4)

- **AR1 · Process model.** One process. A supervisor spawns one tokio
  task per route; a route task never takes the process down — internal
  errors log and the loop restarts with backoff. Startup errors
  (config, token file) exit non-zero before any network call
  (fail-closed). systemd `Restart=on-failure` is the outer layer.
- **AR2 · One in-flight message per route.** Receive → deliver →
  settle, strictly sequential per route; no pipelining, no prefetch.
  This is what makes "backlog drains in publish order" (S2) true by
  construction. Parallelism across routes only.
- **AR3 · Outcome mapping.** Webhook 2xx → ack. Everything else —
  non-2xx status, timeout, connect error — → nack **without**
  `dead=true`; the hub's backoff/retry/DLQ machinery is the retry
  brain (K4). If the nack itself fails (hub vanished mid-settle), do
  nothing: the lease expiry redelivers (mailbox K5). The bridge never
  sends a poison pill — payload-agnostic code cannot judge payloads.
- **AR4 · Raw mode, not `envelope=json`.** The payload arrives as the
  raw body with metadata in `mailbox-*` response headers; the bridge
  forwards body + `content-type` byte-for-byte and passes the
  `mailbox-id`, `mailbox-topic`, `mailbox-attempt`,
  `mailbox-published-at` headers through on the webhook POST. No
  re-encoding, no double-JSON.
- **AR5 · Timeouts.** Long-poll `wait` = 25 s (hub default is 30, max
  300); hub request timeout = wait + 10 s; webhook POST timeout 10 s
  default. `[defaults]` config section, `webhook_timeout_ms`
  overridable per route.
- **AR6 · Hub-down backoff.** Exponential per route: 500 ms doubling
  to a 30 s cap, with jitter. Log **state transitions only** (one line
  when the hub goes unreachable — with remedy — one when it returns),
  never a line per failed attempt: S5's "no log flood" is a design
  property, not a hope.
- **AR7 · Token.** `HUB_BRIDGE_TOKEN` from the environment, injected
  via a root-owned 0600 systemd `EnvironmentFile`
  (`/etc/hub-bridge/token.env`). Sent as `authorization: Bearer` on
  every hub request. Never logged, never in argv, never in the config
  file (which lives in git — standing rule 10); a redaction test
  asserts it. A hub without a door (dev/scratch) works with the
  variable absent; a 401 from the hub logs a remedy naming the `/apps`
  page.
- **AR8 · Config schema** (`/etc/hub-bridge/config.toml`, in git as
  `deploy/config.toml`):

  ```toml
  hub_url = "http://127.0.0.1:8080"
  # healthz_listen = "0.0.0.0:8081"   # W4: absent = no listener

  [defaults]
  poll_wait_ms = 25000
  webhook_timeout_ms = 10000

  [[routes]]
  name = "mailbox-events"             # required, unique; the log/health handle
  topic = "mailbox.events"
  subscription = "ha-bridge"
  webhook_url = "http://homeassistant.lan:8123/api/webhook/hub_mailbox_events"
  # webhook_timeout_ms = 5000         # per-route override
  # [routes.policy]                   # W3: applied to the hub at startup
  # max_attempts = 10
  ```

  Validation: unique route names, unique `{topic, subscription}`
  pairs, absolute http URLs, non-empty fields — each violation a
  startup error with a remedy (K8).
- **AR9 · Shutdown (W1).** SIGTERM/SIGINT → stop claiming, in-flight
  delivery gets a bounded grace (10 s), settle, exit 0. A second
  signal exits immediately — an interrupted delivery is safe by K5.
- **AR10 · `/healthz` (W4).** Opt-in via `healthz_listen`; absent
  means no socket is opened (fail-closed default). Minimal hand-rolled
  HTTP over a tokio listener — no web framework for one endpoint.
  Reports `{status, routes: [{name, state}]}`; route names only, never
  payloads, never the token.
- **AR11 · No TLS, LAN only.** Mirrors the hub's N3. The bridge is
  never exposed beyond the LAN; documented loudly in the runbook.
- **AR12 · Module layout.** `config.rs` (pure parse/validate — unit
  tested), `route.rs` (the pump state machine over client traits),
  `hub.rs` + `webhook.rs` (reqwest adapters), `health.rs`, `main.rs`
  (wiring, signals). Mock transports exist for state-machine unit
  tests, but correctness is proven E2E against the **real hub binary**
  (standing rule 9); each mock's inexpressible behaviours are named in
  TEST_PLAN.md.
- **AR13 · No dedup, by decision.** At-least-once reaches HA
  (mailbox N4); the `mailbox-id` header gives HA automations a
  correlation key if a consumer ever needs to suppress duplicates.
  Idempotency lives consumer-side (study §7).
- **AR14 · Payload log hygiene.** Payloads never reach a log line —
  ids, sizes and statuses only (they may carry personal data; mirrors
  the hub's own G9 guarantee). Asserted E2E with a sentinel payload.
