# Debugging guide — hub-bridge

The evidence trail: what the bridge tells you, what it means, and
where to look next. Written in Phase 8 from the code as built.

## Where to look

1. **The journal:** `journalctl -u hub-bridge -f` (systemd), or the
   process stderr. Default format is plain text; `HUB_BRIDGE_LOG_FORMAT=json`
   gives Loki-ready lines. `HUB_BRIDGE_LOG=info,hub_bridge=debug`
   turns on per-delivery debug lines without hyper's connect spam.
2. **The hub dashboard** (`http://10.10.10.9:8080/`): per topic, the
   subscriptions with their backlogs, dead letters (payload +
   Requeue), and policy values. The hub knows more state than the
   bridge does — the bridge deliberately holds none.
3. **`/healthz`** (if `healthz_listen` is configured): per route one
   of `starting · idle · delivering · topic-unborn · hub-down ·
   auth-denied · circuit-open · respawning · stopped`.

## The log lines (transition-only, on purpose)

| Line (phrase) | Level | Meaning | What now |
|---|---|---|---|
| `hub-bridge started` | info | config valid, routes spawned | — |
| `hub unreachable — reconnecting with backoff` | warn | poll failed; ONE line per outage, not per attempt | check the hub / LXC; nothing is lost, the backlog waits on the hub |
| `hub answering normally again — resuming` | info | outage over | — |
| `topic not born yet` | info | the topic has never been published to; quiet 5 s re-poll; birth messages will be replayed | expected for a new route; if permanent, the producer never published — check its side |
| `hub denied the request (401)` | warn | token missing/wrong/revoked; ONE line, retried with backoff | mint/set the token per the remedy in the line (`/apps` page → `token.env`) |
| `delivered and acked` | debug | one message completed | — |
| `delivery failed — retrying in-process on the same claim` | debug | transient failure inside the lease budget | only interesting in volume |
| `delivery kept failing within the lease budget — nacking` | warn | HA's server answers but refuses (5xx/4xx/3xx); the hub's retry takes over | check the HA automation / URL; watch the topic's dead letters |
| `webhook target unreachable — circuit open` | warn | HA's machine is down; claiming paused, backlog accumulates unclaimed | check HA/VM; the TCP probe closes the circuit by itself |
| `webhook target reachable again — circuit closed` | info | drain resumes in publish order | — |
| `message larger than max_body_bytes` | warn | oversize body nacked unread | raise `max_body_bytes` if legitimate; otherwise find the runaway producer; the message dead-letters visibly |
| `policy in force (W3)` | info | the route's policy landed; the logged JSON is the hub's authoritative answer | remember: a policy write replaces every field |
| `policy PUT failed — retrying on later polls` | warn | hub refused or subscription not there yet; ONE warn, then quiet | fix the `[routes.policy]` values; until then the hub's defaults govern |
| `route loop died unexpectedly — respawning in 5 s` | error | a route panicked; the supervisor restarts it; an unacked claim redelivers | this is a bug worth reporting — grab the panic message above it |
| `ack failed after a retry — the message will redeliver` | warn | hub blinked between POST and ack; the duplicate is legal | only worrying in volume |
| `shutdown signal — letting in-flight deliveries finish` / `hub-bridge stopped` | info | orderly stop | — |
| `second signal — exiting immediately` | warn | double Ctrl-C / stop; safe by design | — |

## Symptom → cause

| Symptom | Likely cause, in order |
|---|---|
| Nothing arrives in HA, no errors anywhere | (1) The HA automation does not exist or listens to another `webhook_id` — **HA answers 200 regardless**, so the bridge acks into the void: run the runbook §1.8 smoke test. (2) The route was added after traffic started: the subscription starts from now (runbook §5.2 replays history). (3) The producer publishes to a different topic name. |
| Everything arrives twice (occasionally) | At-least-once doing its job after a kill/blink between POST and ack. Structural duplicates: check for two routes or two bridge instances on the same `{topic, subscription}` — the config refuses the first, not the second machine. |
| Dead letters piling up | HA up but refusing this webhook (see `delivery kept failing` lines). Payload visible on the dashboard; fix the HA side, press Requeue. |
| Route stuck in `circuit-open` | The webhook host:port is genuinely unreachable from the LXC. If HA is fine in the browser: wrong host/port in `webhook_url`, or a `.local` name — the static musl binary resolves via DNS only, use the router name or IP. |
| Policy on the dashboard differs from the config | The bridge restarted after someone tweaked the dashboard: the config block replaced every field (W3, by design). Change policy in git. |
| `/healthz` not answering | The key is commented out (opt-in, fail-closed), the bind address is not the probed interface, or startup failed on a bind error (the journal has the remedy). |
| Bridge exits immediately at startup | Config invalid — the first journal line is a K8 error with its remedy. `--check-config` reproduces it without side effects. |
| One 401 line, then silence | Deliberate (no flood). The route retries with backoff and resumes by itself once the token is fixed. |
| Backlog grows while HA is up | Check `/healthz`: `delivering` + `delivery kept failing` lines = HA refusing; `idle` = the producer stopped, not the bridge. |

## Proving it to yourself

The scenarios above are not hypothetical — each is an E2E test you can
run against a scratch hub (`cargo test`, see TEST_PLAN.md for the
suite map and the accepted blind spots). The fastest live check of a
route remains: publish a test message from the hub dashboard's
test-publish box and watch the HA automation's trace.
