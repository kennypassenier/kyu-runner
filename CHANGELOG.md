# Changelog

## 0.1.0 — 2026-08-30

First release. Deliberately 0.x: everything below is built, tested and
drilled, but the runner has not yet met the real Home Assistant. That
measurement happens at the rollout, and 1.0.0 follows it.

### What it does

Long-polls configured kyu topics, POSTs each message byte-for-byte to a
Home Assistant webhook, and acknowledges **only** when HA answers 2xx —
so kyu's lease, retry and dead-letter machinery works *for* the HA
delivery. It holds no state: kill it whenever, nothing is lost.

### Delivery semantics

- Ack on 2xx only; a 3xx counts as failure and redirects are never
  followed, in either direction.
- A failing delivery is retried in-process on the same claim while the
  lease budget allows, so the head of the queue stays the head of the
  queue; only then is it handed back to the hub.
- **Circuit breaker:** when the webhook's machine is unreachable the
  route stops claiming entirely, so the backlog waits unclaimed and no
  attempts burn during an HA outage. A bare TCP probe (never an HTTP
  request) watches for its return, and the backlog then drains in
  publish order.
- **Topic-birth replay:** a topic that does not exist yet is a quiet
  wait state, and its first messages are replayed once it appears. A
  topic that already existed starts from now, on purpose.
- Payloads are never parsed, never logged, and capped at
  `max_body_bytes` (16 MiB by default) before they can be buffered.

### Operations

- One TOML config: routes, defaults, an optional `[tuning]` block for
  the operational timings, and an optional observation socket serving
  `/healthz` and `/metrics`.
- Fail-closed startup: unknown keys, duplicate routes, unusable URLs
  and timings that do not fit the lease budget are refused with a
  remedy in the message. `--check-config` validates without touching
  the network, which makes it safe as `ExecStartPre=`.
- The app token comes from the environment only — never git, never a
  URL, never a log line, at any log level.
- Graceful shutdown within a derived grace; a second signal leaves at
  once. A panicking route is respawned by its supervisor.
- Ships a hardened systemd unit, a static musl binary, and a numbered
  runbook whose install procedure was drilled end-to-end on a real
  container.

### Known limitations, by decision

Recorded in `docs/TEST_PLAN.md`: the kill drill fires at the single
worst instant rather than at arbitrary ones; the token door is
exercised with the hub's master token rather than an `/apps`-minted
one; and the eventual target machine is still open, so the first
install there is a deployment step rather than a test. The settle-
failure timing is parked until real running time teaches more than a
contrived test would.
