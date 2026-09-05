# kyu-runner — user guide

Everything the runner does, one feature at a time. Written in Phase 8
from the code and tests, not from intent: every claim names where it
is proven.

**If you have three minutes:** the runner is a stateless pump. You
give it routes — `{topic, subscription, webhook_url}` — in one TOML
file. Per route it long-polls the kyu hub, POSTs each message
byte-for-byte to the Home Assistant webhook, and acks **only** when HA
answers 2xx. Everything else (retries, backoff, dead letters) is the
hub's machinery, kept working *for* the HA delivery. Kill the runner
whenever you like; the hub owns every cursor.

---

## The pump

### K1 · Routes

```toml
hub_url = "http://127.0.0.1:8080"

[[routes]]
name = "kyu-events"          # the log/health handle, unique
topic = "kyu.events"
subscription = "ha-runner"
webhook_url = "http://homeassistant.lan:8123/api/webhook/hub_kyu_events"
```

Routes run concurrently and independently: one route's dead webhook
never stalls another. **Within** a route there is exactly one message
in flight — that is what keeps a backlog draining in publish order.

**Proven by:** `l4_k1_two_routes_run_independently_through_one_outage`,
`l2_s2_ar15_an_outage_accumulates_unclaimed_and_drains_in_order`.

### K2 · Byte-for-byte forward

The payload reaches HA exactly as it was published — bytes, not a
string round-trip — with the original `content-type` and the
`kyu-id`, `kyu-topic`, `kyu-attempt` and
`kyu-published-at` headers passed through. The runner never parses
a payload (scope NG2): the envelope schema is somebody else's contract.

**Proven by:** `l2_k1_k2_k3_ar16_the_pump_delivers_byte_for_byte_and_acks`,
`l2_k2_ar4_a_binary_payload_survives_byte_for_byte`.

### K3 · Ack only on 2xx

"Delivered" means *HA answered 2xx*, not *sent*. A 3xx is a failure
too — redirects are never followed, because a followed 302 becomes a
body-less GET that HA might answer 200, acking a payload that never
arrived (AR17).

**Proven by:** `l2_k1_k2_k3_ar16_the_pump_delivers_byte_for_byte_and_acks`,
`l2_ar17_a_redirect_is_a_failure_never_followed`.

### K4 · Failure → the hub's machinery

A failing delivery is first retried in-process on the same claim
(1/2/4/8 s pauses) while the lease budget allows; only then does the
runner nack, handing the message to the hub's backoff/retry. When the
attempts run out, the message dead-letters **visibly** on the hub
dashboard, with a Requeue button. The runner never poison-pills — it
cannot judge a payload it refuses to read.

**Proven by:** `l2_s1_a_500_from_ha_is_not_acked_and_the_message_returns`,
`l2_s4_k4_a_permanently_failing_webhook_dead_letters_visibly`.

### K5 · Stateless

No cursors, no queue, no disk beyond the config file. `kill -9` at
the worst possible moment — after the POST, before the ack — costs at
most a duplicate delivery, never a loss.

**Proven by:** `l2_s3_kill_nine_mid_delivery_loses_nothing`.

---

## When things go wrong

### AR15 · The circuit breaker

When the webhook's *machine* is down (connection refused/timed out),
the route stops claiming entirely: the backlog accumulates unclaimed
on the hub, no attempts burn, and a bare TCP probe (no HTTP request —
nothing can trigger a webhook) watches for the machine's return. On
recovery the backlog drains in publish order. An up-but-erroring HA
(5xx) does **not** open the circuit — there the next real message is
the only side-effect-free probe, and attempts burn slowly and visibly.

**Proven by:** `l2_s2_ar15_an_outage_accumulates_unclaimed_and_drains_in_order`,
the 5xx boundary inside `l2_s1_a_500_from_ha_is_not_acked_and_the_message_returns`.

### K7 · Hub down

The runner idles and reconnects with backoff (0.5 s doubling to 30 s).
One log line when the hub goes away, one when it returns — never a
line per failed poll.

**Proven by:** `l3_k7_a_hub_outage_is_one_log_line_and_recovery_is_automatic`.

### AR16 · New topics

A topic that does not exist yet is a quiet wait state: the runner
polls every 5 s, and when the topic is born its **first** messages are
replayed from the beginning — nothing falls in the gap between birth
and subscription. A topic that already existed before the route's
first poll starts **from now**, deliberately: backfilling months of
history into HA would be the worse surprise (the runbook §5 has the
manual replay for when you do want history).

**Proven by:** `l2_k1_k2_k3_ar16_the_pump_delivers_byte_for_byte_and_acks`,
`l4_ar16_a_pre_existing_topic_starts_from_now_not_from_history`.

### AR1 · A crashing route

A route loop that dies unexpectedly is respawned by its supervisor
(after `route_respawn_ms`, 5 s by default); the process never exits over one route, and an unacked
claim redelivers on its own.

**Proven by:** `l3_ar1_a_panicking_route_is_respawned_and_the_message_survives`.

### Oversize messages

Bodies above `max_body_bytes` (default 16 MiB) are never buffered or
forwarded: the runner nacks them unread and they dead-letter visibly.

**Proven by:** `l2_f1_an_oversize_message_is_nacked_and_dead_letters_without_oom`.

---

## Configuration

### K8 · Fail-closed

An invalid config never half-starts the runner: unknown keys, missing
routes, duplicate names or `{topic, subscription}` pairs, unusable
URLs (no host, credentials, non-http), out-of-range timings and
unencodable policy values are all startup errors — each with a remedy
in the message.

```toml
[defaults]
poll_wait_ms = 25000        # long-poll window, 1000-300000
webhook_timeout_ms = 10000  # per POST; overridable per route
max_body_bytes = 16777216   # ≥ 1024
```

**Proven by:** the `l1_k8_*` unit tests in `src/config.rs` (thirteen
rules, each asserting its remedy) and
`l1_k8_a_missing_config_file_fails_with_a_remedy` at the process
boundary. The shipped `deploy/config.toml` is itself parsed by
`l1_k6_k10_the_shipped_example_config_is_valid`.

### AR5 · The lease budget

In-process retries stop at 70% of the route's lease (its
`policy.lease_ms`, else the hub default 30 s), keeping 5 s for the
settle call — a claim is always settled by its owner, never by
expiry. A `webhook_timeout_ms` that cannot fit is refused at startup
with a remedy pointing at `policy.lease_ms`.

**Proven by:** `l1_ar5_a_webhook_timeout_that_outgrows_the_lease_budget_is_refused`,
`l1_ar5_a_raised_policy_lease_widens_the_budget`.

### W3 · Declarative route policy

```toml
[routes.policy]
max_attempts = 25
```

The block is forwarded verbatim to the hub after the route's first
successful poll (the poll is what creates the subscription); the hub
validates it and the runner logs the hub's "values in force" answer.
**A policy write replaces every field** — the config block IS the
whole policy, and a runner restart reverts dashboard tweaks. A policy
the hub refuses is warned once; the route keeps running under the
hub's defaults.

**Proven by:** `l4_w3_the_configured_policy_lands_on_the_hub_and_is_logged`,
`l4_w3_a_hub_refused_policy_warns_once_and_the_route_keeps_delivering`,
`l1_w3_policy_json_renders_integers_strings_and_booleans`.

### W4 · Health endpoint

Since 0.2.0 the observation socket is the kit's (chassis-rs) and always
listens — default `127.0.0.1:8082`, set with `KYU_RUNNER_LISTEN` (or
`--listen`) in the environment file, e.g. `10.10.10.9:8082` for Uptime
Kuma. `GET /healthz` answers

```json
{"status":"ok","version":"0.2.0","subsystems":{"kyu-events":{"ok":true,"detail":"idle"}}}
```

— one subsystem per route, its loop state as the detail, never a payload,
never the token. While a route is `hub-down`, `auth-denied` or
`circuit-open` the status is `degraded` and the code 503 (the kit's rule;
`kyu-runner --healthcheck` still counts that as alive). `GET /metrics`
keeps `kyu_runner_delivered_total{route}` and `kyu_runner_nacked_total{route}`
and adds the kit's build-info, uptime and request counters.

**Proven by:** `l4_w4_healthz_reports_route_states_when_opted_in`,
`l4_w4_the_config_key_is_what_opens_the_socket` (the address comes from
the listen knob; nothing listens where you did not point it) and
`l4_w4_healthz_reports_the_failure_state_not_a_frozen_ok` (the state
really moves to `hub-down` during an outage, so a monitor cannot watch
a light that never changes colour).

### Tuning (MR1)

Every operational timing has a default that matches what used to be
hardcoded, so `[tuning]` is optional:

```toml
[tuning]
hub_backoff_max_ms = 60000     # be lazier about a hub that is down
settle_timeout_ms = 8000       # a slower hub; tightens the lease budget too
healthz_max_connections = 8
```

Keys: `hub_backoff_ms`, `hub_backoff_max_ms`, `circuit_probe_ms`,
`circuit_probe_max_ms`, `circuit_probe_timeout_ms`,
`topic_unborn_poll_ms`, `route_respawn_ms`, `settle_timeout_ms`,
`healthz_max_connections`, `healthz_timeout_ms`. Raising
`settle_timeout_ms` also raises AR5's budget margin — the config check
will tell you if a route's webhook timeout no longer fits.

Deliberately not configurable, each pinned with its reason in
`src/config.rs`: the 1/2/4/8 s delivery retry ladder and the single
settle retry (both fit inside the lease budget by construction), and
the health socket's accept pause (a busy-loop guard).

**Proven by:** `l7_mr1_the_tuning_defaults_match_the_previously_hardcoded_values`,
`l7_mr1_a_tuning_block_overrides_only_what_it_names`,
`l7_mr1_a_raised_settle_timeout_tightens_the_lease_budget`,
`l7_mr1_an_inverted_backoff_pair_is_refused_with_a_remedy`,
`l7_mr1_out_of_range_tuning_values_are_refused`,
`l7_mr1_an_unknown_tuning_key_is_refused`.

### W6 · Metrics

The same socket serves `/metrics`: Prometheus counters
`kyu_runner_delivered_total{route=…}` (delivered to the webhook and
settled) and `kyu_runner_nacked_total{route=…}` (handed back to the
hub). Route labels are the K8-restricted route names, so nothing needs
escaping. Note the house currently has no Prometheus backend to scrape
it — the endpoint is ready for the day one exists.

**Proven by:** `l6_w6_counters_move_on_delivery_and_on_nack`.

---

## Running it

### The CLI (W2)

```
kyu-runner [--config <path>] [--state-dir <dir>] [--listen <host:port>] [--check]
kyu-runner --version | --help | --print-config | --healthcheck | update | gen-secret | rekey
```

The command line is the kit's (chassis-rs): every knob is also an
environment variable (`KYU_RUNNER_CONFIG`, `KYU_RUNNER_STATE_DIR`,
`KYU_RUNNER_LISTEN`, `KYU_RUNNER_LOG`, `KYU_RUNNER_LOG_FORMAT`,
`KYU_RUNNER_SHUTDOWN_TIMEOUT_MS`, the `KYU_RUNNER_UPDATE_*` family) and a
key in the same config file; precedence flag > env > file > default.
`--check` validates the pump's config AND the kit's knobs, probes the state
directory and exits — zero network calls, which is why it is safe as
`ExecStartPre=` on a cold-booting LXC where the hub is not up yet.
`--print-config` shows every knob with its source, secrets masked.

**Proven by:** `l1_w2_check_config_exits_zero_on_a_valid_config`,
`l1_w2_check_config_makes_no_network_calls`,
`l1_w2_an_unknown_argument_fails_with_a_remedy`,
`l1_w2_version_prints_the_crate_version`.

### K9 · The token

```
KYU_RUNNER_HUB_TOKEN   app token, minted on the hub's /apps page (was KYU_RUNNER_TOKEN before 0.2.0)
KYU_RUNNER_LOG         log filter (default: info)
KYU_RUNNER_LOG_FORMAT  "json" for one JSON object per line (Loki)
```

The token lives in the environment (the unit's `EnvironmentFile`),
never in the config file, never in a URL, never in a log line — at
any log level, in either format. Without it against a doored hub you
get one 401 line with the `/apps` remedy, not a flood.

**Proven by:** `l2_k9_ar7_the_token_reaches_the_hub_but_never_the_logs`,
`l2_k9_the_token_stays_out_of_failure_path_and_json_logs`,
`l2_k9_a_missing_token_logs_the_apps_remedy_once_without_flooding`,
and `l3_k9_a_route_resumes_once_the_hub_accepts_its_token_again` — a
denied route picks up by itself when the door reopens, without a
restart.

### W1 · Stopping

SIGTERM or SIGINT lets the in-flight delivery finish (the grace is
derived: largest webhook timeout + 5 s) and exits 0. A second signal
exits immediately — safe, because unacked messages redeliver.

**Proven by:** `l3_w1_sigterm_mid_delivery_finishes_the_message_and_exits_zero`,
`l3_w1_sigint_also_stops_cleanly`,
`l1_ar9_the_shutdown_grace_is_derived_from_the_largest_timeout`,
`l3_w1_a_second_signal_exits_immediately` (exit code 130) and
`l3_ar9_shutdown_never_outlasts_the_derived_grace` (a delivery stuck
past its timeout still cannot make systemd wait).

### K6 · The shipped kyu.events route · K10/K11 · Deployment

`deploy/config.toml` ships the P8 route (hub events → HA warning
webhook) and `deploy/kyu-runner.service` the hardened unit;
`docs/OPERATIONS_RUNBOOK.md` holds the numbered install, update,
restore and monitoring procedures — including the one rule that
matters most: **HA automation first, then the route, then the smoke
test**, because HA answers 200 even for unknown webhook ids.

**Proven by:** `l4_k6_a_dead_letter_event_reaches_the_warning_webhook`
(a real sweeper-emitted event through the runner into the webhook);
deployment procedures are drilled, not unit-tested — see
TEST_PLAN.md and the DRILL-OK record in REALIZATION_PLAN.md.
