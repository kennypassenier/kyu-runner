# Changelog

## 0.2.0 — 2026-09-05 (unreleased; branch `chassis-migration`)

Built on [chassis-rs](https://github.com/kennypassenier/chassis-rs) v1.1.0:
the kit now owns the command line, configuration layers, logging,
`/healthz`, `/metrics`, the graceful shutdown and signed self-update. The
pump — routes, hub client, webhook client, circuit breaker, policies — is
unchanged.

### Migration

- **Command line.** `--check-config` is `--check`; `--config <path>` stays
  (or `KYU_RUNNER_CONFIG`); every knob also has an environment variable
  (`KYU_RUNNER_LISTEN`, `KYU_RUNNER_STATE_DIR`, `KYU_RUNNER_LOG`,
  `KYU_RUNNER_LOG_FORMAT`, `KYU_RUNNER_SHUTDOWN_TIMEOUT_MS`, …); an unknown
  argument exits 1 (was 2).
- **The hub token is `KYU_RUNNER_HUB_TOKEN`** (was `KYU_RUNNER_TOKEN`, which
  the kit reserves for its dashboard login token). Rename the line in the
  environment file.
- **The observation socket always listens** (default `127.0.0.1:8082`; set
  `KYU_RUNNER_LISTEN`). `healthz_listen`, `healthz_max_connections` and
  `healthz_timeout_ms` are gone from the config file and refused as unknown
  keys. `/healthz` answers `{"status","version","subsystems":{<route>:
  {"ok","detail":<state>}}}` and 503 while a route is `hub-down`,
  `auth-denied` or `circuit-open`; `/metrics` keeps
  `kyu_runner_delivered_total{route}` and `kyu_runner_nacked_total{route}`
  and gains the kit's `kyu_runner_build_info`/`_uptime_seconds`/
  `_http_requests_total`.
- **A state directory is required** (default `/var/lib/kyu-runner`, knob
  `KYU_RUNNER_STATE_DIR`): `--check` refuses a missing or unwritable one;
  it holds the self-update state only.
- **Shutdown.** The bound is explicit (`KYU_RUNNER_SHUTDOWN_TIMEOUT_MS`,
  default 10 s; set it above the largest `webhook_timeout_ms` + settle) and
  a second signal is ignored by design (systemd's `TimeoutStopSec` is the
  backstop) — it no longer exits 130.
- **Install path** `/opt/kyu-runner/bin/kyu-runner` with the unit in
  `deploy/kyu-runner.service` (Type=notify, fixed user `kyu-runner`,
  hardening set); environment file `/etc/kyu-runner/kyu-runner.env`; the
  homelab stack file is `deploy/service.yml`.
- **Self-update is on** (A7): releases are glibc/trixie binaries named
  `kyu-runner` with `SHA256SUMS`, `SHA256SUMS.minisig` (trusted comment
  `kennypassenier/kyu-runner v<version>`) and `VERSION`, produced by
  `.github/workflows/release.yml` + `scripts/sign-release.sh`; the musl
  build and `scripts/build-release.sh` are retired.


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
