# Operations runbook — hub-bridge

Numbered procedures. Written at L5, re-validated in Phase 8. The
deployment target is LXC 109 (`10.10.10.9`), where the mailbox hub
already runs as a native binary under systemd; the bridge follows the
same pattern.

Reality checks baked into these procedures (from the Phase 4 critic):

- **HA answers 200 even for unknown webhook ids** (anti-enumeration).
  A route whose HA automation does not exist yet acks messages into
  the void with healthy metrics. Order is therefore always:
  *automation first, route second, smoke test third.*
- **A policy write replaces every field** (mailbox K7). The
  `[routes.policy]` block in git is the whole policy: any tweak made
  on the hub dashboard is reverted the next time the bridge starts.
  Change policies in `deploy/config.toml`, not on the dashboard.
- **The release binary is static musl**: it resolves names via DNS
  only — no mDNS/Avahi. Use router-DNS names or IPs in
  `webhook_url`, never a `.local` name.

## 1 · Install on LXC 109

1. Build or download the artifact:
   - from a release: download `hub-bridge-x86_64-linux-musl` +
     `SHA256SUMS` from the GitHub release, then `sha256sum -c SHA256SUMS`;
   - or locally: `scripts/build-release.sh` (same artifact + manifest).
2. Copy it in place:
   `scp hub-bridge-x86_64-linux-musl root@10.10.10.9:/usr/local/bin/hub-bridge`
   and `chmod 755 /usr/local/bin/hub-bridge`.
3. **HA side first (K6):** create the webhook automation(s) in HA for
   every route you are about to enable — see §6. Every webhook trigger
   sets `local_only: true`.
4. Mint the app token: hub dashboard → `/apps` → register
   `hub-bridge` → copy the token. On LXC 109 (the `read -rs` keeps the
   token out of the shell history — standing rule 10):
   ```
   install -m 600 /dev/null /etc/hub-bridge/token.env
   read -rs TOKEN && printf 'HUB_BRIDGE_TOKEN=%s\n' "$TOKEN" > /etc/hub-bridge/token.env && unset TOKEN
   ```
5. Config: `mkdir -p /etc/hub-bridge` and copy `deploy/config.toml`
   to `/etc/hub-bridge/config.toml`. Verify:
   `/usr/local/bin/hub-bridge --config /etc/hub-bridge/config.toml --check-config`
6. Unit: copy `deploy/hub-bridge.service` to
   `/etc/systemd/system/hub-bridge.service`, then
   `systemctl daemon-reload && systemctl enable --now hub-bridge`.
7. Read it back (standing rule 13a): `systemctl status hub-bridge`
   and `journalctl -u hub-bridge -n 20` — expect the "hub-bridge
   started" line with the route count, and per route either polling
   silence or a quiet "topic not born yet" line.
8. **Smoke test per route (mandatory):** publish a test message on the
   route's topic (the hub dashboard's test-publish box, or `curl`),
   and confirm the HA automation fired (HA → Settings → Automations →
   Traces). A route without a green smoke test is not live — HA's 200
   proves nothing about the automation existing (see above).

## 2 · Update

1. Download + verify the new artifact (install step 1).
2. `systemctl stop hub-bridge` — in-flight deliveries finish within
   the grace; unacked messages redeliver (K5), so nothing is lost.
3. Replace `/usr/local/bin/hub-bridge`, keep the old binary as
   `/usr/local/bin/hub-bridge.prev` until the new one is seen running.
4. `systemctl start hub-bridge`; read back per install step 7.

## 3 · Restore from zero (M3 — drilled in Phase 7)

The bridge's full state is: the binary (releases), the config + unit
(this repo), and the token (re-mintable). Nothing else exists.

1. Install the binary (install steps 1-2).
2. Copy `deploy/config.toml` and `deploy/hub-bridge.service` from this
   repo (they ARE the backup — check drift first if the old machine
   still answers: `diff deploy/config.toml /etc/hub-bridge/config.toml`).
3. Re-mint the token on the hub's `/apps` page (revoke the old
   `hub-bridge` app if it is still listed), write `token.env`
   (install step 4).
4. Enable + start + read back (install steps 6-7); smoke test one
   route (install step 8).
5. Backlog: nothing to do — the hub owns all cursors; the new bridge
   resumes every subscription where the old one stopped.

## 4 · P8 wiring (K11)

1. **Uptime Kuma → hub:** add an HTTP(s) monitor on
   `http://10.10.10.9:8080/healthz`, expect 200. The endpoint stays
   open on a token-protected hub.
2. **Uptime Kuma → bridge (optional, W4):** uncomment
   `healthz_listen` in the config — bind the address Kuma actually
   probes (`10.10.10.9:8081`), or leave it commented out when unused:
   any LAN device can reach an open listener, and less surface is less
   surface. Restart, add a monitor on `http://10.10.10.9:8081/healthz`.
   Without it, a dead bridge still surfaces via the hub's
   idle-subscription flag and the `mailbox.events` route.
3. **Grafana → sweeper alert:** the hub's `/metrics` exposes
   `mailbox_sweeper_age_ms` — the one series that catches the hub
   *hanging* rather than dying. Alert when it exceeds 60 000 for
   5 minutes.
4. **Dead letters → HA warnings:** that is the shipped
   `mailbox-events` route (K6) — smoke test it like any route.

## 5 · Adding, renaming or removing a route

1. Edit `deploy/config.toml` in the repo (git is the truth), copy to
   `/etc/hub-bridge/config.toml`, `--check-config`, restart.
2. **Adding:** HA automation first, then the route, then the smoke
   test (§1 steps 3/8). If producers were already publishing to the
   topic before the route existed, the subscription starts *from now*;
   pull the retained backlog once, deliberately, with:
   `curl "http://10.10.10.9:8080/t/<topic>/next?as=<sub>&wait=0&from=beginning"`
   — repeated until 204, or leave history behind on purpose.
   (A topic that does not exist yet is the easy case: the bridge waits
   quietly and replays its birth messages automatically, AR16.)
3. **Renaming/removing:** the old subscription stays behind on the
   hub, pins retention for up to 30 days, then archives as `lapsed`
   (mailbox K11). Archive it yourself on the topic's dashboard page
   the same day instead — a deliberate goodbye beats a 30-day flag.

## 6 · The HA-side automation (K6 reference shape)

One automation per webhook route. For the shipped `mailbox-events`
route:

```yaml
alias: "Hub: dead letter warning"
triggers:
  - trigger: webhook
    webhook_id: hub_mailbox_events   # matches deploy/config.toml
    allowed_methods: [POST]
    local_only: true                 # capability token stays LAN-only
conditions:
  - condition: template
    value_template: >-
      {{ trigger.json.event in ['message.dead_lettered',
                                'subscription.archived'] }}
actions:
  - action: script.notification_dispatch
    data:
      title: "Mailbox: dode brief"
      message: >-
        {{ trigger.json.event }} op topic {{ trigger.json.topic }}
      priority: warning
      click_url: "http://10.10.10.9:8080/t/{{ trigger.json.topic }}"
```

Field names under `trigger.json.*` follow the hub's event payloads
(mailbox W11); adjust to the dispatcher's real parameters when wiring
(the notification system's own documentation is the authority for the
`script.notification_dispatch` contract).
