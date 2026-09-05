# Operations runbook — kyu-runner

Numbered procedures. Written at L5, re-validated in Phase 8. The
deployment target is **an LXC on the Proxmox host — which one is
Kenny's still-open call** (`<target-lxc>` below). The kyu hub runs on
LXC 109 (`10.10.10.9`) as a native binary under systemd; the runner
follows the same pattern, wherever it lands.

Reality checks baked into these procedures (from the Phase 4 critic):

- **HA answers 200 even for unknown webhook ids** (anti-enumeration).
  A route whose HA automation does not exist yet acks messages into
  the void with healthy metrics. Order is therefore always:
  *automation first, route second, smoke test third.*
- **A policy write replaces every field** (kyu K7). The
  `[routes.policy]` block in git is the whole policy: any tweak made
  on the hub dashboard is reverted the next time the runner starts.
  Change policies in `deploy/config.toml`, not on the dashboard.
- **The release binary is glibc for Debian trixie since 0.2.0** (built by the
  chassis release workflow; was static musl): it resolves names via DNS
  only — no mDNS/Avahi. Use router-DNS names or IPs in
  `webhook_url`, never a `.local` name.

## 1 · Install on the target LXC

> Which LXC is deliberately still Kenny's call (ratification form 2);
> the procedure below is LXC-agnostic and was drilled end-to-end on a
> scratch LXC (191) on 2026-08-30. Drill finding: the debian-13
> template ships **without curl** — the read-back steps below show the
> python3 alternative that is always present:
> `python3 -c 'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:8080/healthz",timeout=3).read().decode())'`

1. Build or download the artifact:
   - from a release: download `kyu-runner` +
     `SHA256SUMS` from the GitHub release, then `sha256sum -c SHA256SUMS`;
   - or locally: `scripts/drill-release.sh` of the kit builds a drill
     release; `scripts/sign-release.sh` signs a CI-built one.
2. Copy it in place:
   `scp kyu-runner root@<target-lxc>:/opt/kyu-runner/bin/kyu-runner`
   and `chmod 755 /opt/kyu-runner/bin/kyu-runner`.
3. **HA side first (K6):** create the webhook automation(s) in HA for
   every route you are about to enable — see §6. Every webhook trigger
   sets `local_only: true`.
4. Mint the app token: hub dashboard → `/apps` → register
   `kyu-runner` → copy the token. On the target LXC (the `read -rs` keeps the
   token out of the shell history — standing rule 10):
   ```
   install -m 600 /dev/null /etc/kyu-runner/kyu-runner.env
   read -rs TOKEN && printf 'KYU_RUNNER_HUB_TOKEN=%s\n' "$TOKEN" > /etc/kyu-runner/kyu-runner.env && unset TOKEN
   ```
5. Config: `mkdir -p /etc/kyu-runner` and copy `deploy/config.toml`
   to `/etc/kyu-runner/config.toml`. Verify:
   `/opt/kyu-runner/bin/kyu-runner --config /etc/kyu-runner/config.toml --check`
6. Unit: copy `deploy/kyu-runner.service` to
   `/etc/systemd/system/kyu-runner.service`, then
   `systemctl daemon-reload && systemctl enable --now kyu-runner`.
7. Read it back (standing rule 13a): `systemctl status kyu-runner`
   and `journalctl -u kyu-runner -n 20` — expect the "kyu-runner
   started" line with the route count, and per route either polling
   silence or a quiet "topic not born yet" line.
8. **Smoke test per route (mandatory):** publish a test message on the
   route's topic (the hub dashboard's test-publish box, or `curl`),
   and confirm the HA automation fired (HA → Settings → Automations →
   Traces). A route without a green smoke test is not live — HA's 200
   proves nothing about the automation existing (see above).

## 2 · Update

1. Download + verify the new artifact (install step 1).
2. `systemctl stop kyu-runner` — in-flight deliveries finish within
   the grace; unacked messages redeliver (K5), so nothing is lost.
3. Replace `/opt/kyu-runner/bin/kyu-runner`, keep the old binary as
   `/opt/kyu-runner/bin/kyu-runner.prev` until the new one is seen running.
4. `systemctl start kyu-runner`; read back per install step 7.

## 3 · Restore from zero (M3 — drilled in Phase 7)

The runner's full state is: the binary (releases), the config + unit
(this repo), and the token (re-mintable). Nothing else exists.

1. Install the binary (install steps 1-2).
2. Copy `deploy/config.toml` and `deploy/kyu-runner.service` from this
   repo (they ARE the backup — check drift first if the old machine
   still answers: `diff deploy/config.toml /etc/kyu-runner/config.toml`).
3. Re-mint the token on the hub's `/apps` page (revoke the old
   `kyu-runner` app if it is still listed), write `token.env`
   (install step 4).
4. Enable + start + read back (install steps 6-7); smoke test one
   route (install step 8).
5. Backlog: nothing to do — the hub owns all cursors; the new runner
   resumes every subscription where the old one stopped.

## 4 · P8 wiring (K11)

1. **Uptime Kuma → hub:** add an HTTP(s) monitor on
   `http://10.10.10.9:8080/healthz`, expect 200. The endpoint stays
   open on a token-protected hub.
2. **Uptime Kuma → runner (optional, W4):** uncomment
   `healthz_listen` in the config — bind the address Kuma actually
   probes (`<target-lxc>:8081`), or leave it commented out when unused:
   any LAN device can reach an open listener, and less surface is less
   surface. Restart, add a monitor on `http://<target-lxc>:8081/healthz`.
   Without it, a dead runner still surfaces via the hub's
   idle-subscription flag and the `kyu.events` route.
3. **Grafana → sweeper alert:** the hub's `/metrics` exposes
   `kyu_sweeper_age_ms` — the one series that catches the hub
   *hanging* rather than dying. Alert when it exceeds 60 000 for
   5 minutes.
4. **Dead letters → HA warnings:** that is the shipped
   `kyu-events` route (K6) — smoke test it like any route.

## 5 · Adding, renaming or removing a route

1. Edit `deploy/config.toml` in the repo (git is the truth), copy to
   `/etc/kyu-runner/config.toml`, `--check`, restart.
2. **Adding:** HA automation first, then the route, then the smoke
   test (§1 steps 3/8). If producers were already publishing to the
   topic before the route existed, the subscription starts *from now*;
   pull the retained backlog once, deliberately, with:
   `curl "http://10.10.10.9:8080/t/<topic>/next?as=<sub>&wait=0&from=beginning"`
   — repeated until 204, or leave history behind on purpose.
   (A topic that does not exist yet is the easy case: the runner waits
   quietly and replays its birth messages automatically, AR16.)
3. **Renaming/removing:** the old subscription stays behind on the
   hub, pins retention for up to 30 days, then archives as `lapsed`
   (kyu K11). Archive it yourself on the topic's dashboard page
   the same day instead — a deliberate goodbye beats a 30-day flag.

## 6 · The HA-side automation (K6 reference shape)

One automation per webhook route. For the shipped `kyu-events`
route:

```yaml
alias: "Hub: dead letter warning"
triggers:
  - trigger: webhook
    webhook_id: hub_kyu_events   # matches deploy/config.toml
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
      title: "Kyu: dode brief"
      message: >-
        {{ trigger.json.event }} op topic {{ trigger.json.topic }}
      priority: warning
      click_url: "http://10.10.10.9:8080/t/{{ trigger.json.topic }}"
```

Field names under `trigger.json.*` follow the hub's event payloads
(kyu W11); adjust to the dispatcher's real parameters when wiring
(the notification system's own documentation is the authority for the
`script.notification_dispatch` contract).
