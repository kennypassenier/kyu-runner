# Scope — hub-bridge

Phase 0 output. Every statement below was approved item-by-item by Kenny
in the Phase 0 gate form (2026-08-28); the form IDs (G/NG/S/R/B) are
kept for traceability back to that gate.

## What this is

A small stateless Rust daemon that makes Home Assistant able to
**consume** from the mailbox hub. HA can already produce trivially
(`rest_command` → `POST /t/<topic>`), but nothing in HA long-polls;
the bridge is the missing pump. It implements proposals **P1**
(hub-bridge) and **P8** (hub self-monitoring) of the Mailbox
Integration Study (`~/ObsidianVault/Home Assistant/Documentation/
Mailbox Integration Study.md`), both rated Essential by Kenny on
2026-08-28.

The mailbox interface authority is `~/Projects/mailbox/docs/USER_GUIDE.md`
(v1.0.x, the three-verb HTTP contract).

## Goals

- **G1 · The stateless pump.** A config file defines routes
  `{topic, subscription, ha_webhook_url}`. Per route the bridge
  long-polls the hub (`GET /t/<topic>/next?as=<subscription>`), POSTs
  the payload byte-for-byte to the HA webhook, and acks **only** on a
  2xx answer from HA. On failure it nacks (or lets the lease expire),
  so the hub's retry → dead-letter machinery works *for* the HA
  delivery.
- **G2 · P8 built in.** A shipped default route `mailbox.events` → HA
  warning webhook (dead letters and archived subscriptions surface as
  house warnings, with a `click_url` to the hub dashboard), plus
  documented wiring for `/healthz` → Uptime Kuma and
  `mailbox_sweeper_age_ms` → Grafana. The HA-side webhook automation is
  delivered as a documented step of this project.

## Non-goals

- **NG1 · No state of its own.** No cursors, no queue, no disk beyond
  the config file. The hub owns every position. `kill -9` at any
  moment followed by a restart loses nothing by construction.
- **NG2 · Payload-agnostic.** Bytes in, bytes out: no parsing, no
  validation, no transformation, no content-based routing (mailbox N5
  inherited). The envelope v1 schema is a DRAFT owned by the
  pipeline-v2 project; schema changes there do not touch the bridge.
- **NG3 · Never in a critical path.** Study §7 degradation doctrine:
  hub or bridge down returns the house to its 2026-08-28 behaviour,
  never worse. The freezer chain keeps zero new dependencies. This
  sentence survives into the Phase 4 freeze.
- **B1 · No reverse direction** (decided at the gate): the bridge does
  not expose a publish endpoint for HA. HA produces via `rest_command`
  directly to the hub. Revisit via mini-round if a real need appears.

## Success criteria

- **S1** Every message on a configured route reaches the HA webhook
  at least once; ack happens only after HA answered 2xx. (E2E against
  a scratch hub + a fake HA webhook server; a 500 from "HA" must not
  ack and the message must return.)
- **S2** HA down → the backlog accumulates on the hub; on HA's return
  the bridge drains it **in publish order**, nothing lost.
- **S3** `kill -9` on the bridge at any moment — including between the
  webhook POST and the ack — causes at worst a duplicate delivery
  (at-least-once), never a loss. Restart resumes from the hub's
  cursors.
- **S4** A message HA persistently refuses dead-letters visibly on the
  hub dashboard; with the G2 route active, that dead letter also
  becomes an HA warning.
- **S5** Hub unreachable → the bridge keeps running: log, reconnect
  with backoff, resume where it was. No crash-loop, no log flood.

## Hard constraints

- **R1 · Scratch hub only.** Development and tests run against a local
  hub (the `~/Projects/mailbox` binary or its docker image). The real
  hub on LXC 109 (`http://10.10.10.9:8080`, native binary under
  systemd) is touched only as an explicitly agreed step.
- **R2 · Rust, new repo, full dev procedure** (all 11 phases,
  git-native hooks + CI from Phase 5, feature IDs everywhere).
- **R3 · Deployment direction:** second systemd service on LXC 109
  next to the hub, as a native binary — the same pattern Kenny chose
  for the hub itself. Final decision falls in Phase 2/3 with the
  trade-offs on the table.
- **R4 · Own app token** from the hub's `/apps` page (name:
  `hub-bridge`), independently revocable, never in git. How the token
  reaches the bridge is the Phase 2 ecosystem item (latch is the
  ecosystem candidate).

## Build-vs-buy (Phase 1 record)

The study's §4 examined the alternatives before this project started;
re-validated at Phase 1:

| Option | Verdict |
|---|---|
| **Build `hub-bridge` (this project)** | **Chosen.** ~200 lines of logic; keeps all queue semantics (lease, retry, DLQ) working for HA delivery; ecosystem component. |
| AppDaemon/pyscript consumer inside HA | Rejected: a second runtime bolted into HA (the house deliberately removed NetDaemon 2026-08-09); harder to test than a Rust binary under the procedure. |
| Per-use-case mini-consumers | Right for PC endpoint consumers (desk-courier, vault-courier); wrong as the generic HA path — N daemons instead of one. |
| Generic pipeline tools (Node-RED, Benthos/Redpanda Connect, n8n) | Rejected: none speak mailbox's lease/ack/nack protocol natively; ack-on-200 semantics would need custom code inside a much larger runtime than the thing being built. |
| No consuming (HA produce-only) | Legitimate phase-1 posture, loses half the value; superseded by this project. |

Ecosystem findings (Phase 1): consumes **mailbox** (the point of the
project); **latch** is the candidate for token delivery (Phase 2 item);
**homelab** deployment preset deliberately not used — the hub itself
runs native under systemd on LXC 109 and the bridge follows that
pattern (R3); monitoring rides the existing **Uptime Kuma** and
**Grafana** (G2).
