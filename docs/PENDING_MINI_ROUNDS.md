# Pending gates & ratifications — AFK build queue

Kenny went AFK right after the Phase 0 gate (2026-08-28: "doe alles
wat je kan zonder mijn input"). Per the AFK rule (procedure L2), work
continued on documented recommendations; every skipped gate is queued
here as a ratification round and this file is the FIRST thing
presented on Kenny's return. Nothing here is frozen until ratified.

| # | Gate | What was decided provisionally | Where recorded |
|---|---|---|---|
| Q1 | Phase 1 · build-vs-buy | **RATIFIED 2026-08-29** (form 1): Klopt | SCOPE.md "Build-vs-buy" |
| Q2 | Phase 2 · feature ratings | **RATIFIED 2026-08-29** (form 1): all K/M items as recommended; W2 + W4 raised to Essential (already built); W6 raised to Desired → **new milestone L6** (bridge /metrics); W5 supersession + W7 out-of-scope confirmed | FEATURES.md |
| Q3 | Phase 3 · tech choices | **RATIFIED 2026-08-29** (form 1): T1-T9 Klopt | ARCHITECTURE_DECISIONS.md T1-T9 |
| Q4 | Phase 3 · **platform (OPEN question)** | **ANSWERED 2026-08-29** (form 2): an LXC on the Proxmox host, which one deliberately TBD; scratch-LXC deployment testing authorized. Docs updated away from the LXC-109 assumption | ARCHITECTURE_DECISIONS.md T8 |
| AR11-TLS | Outbound https via Traefik | Kenny amended AR11 at ratification (2026-08-29): https may appear via Traefik. Inbound TLS needs no bridge change (Traefik terminates); outbound https triggers this mini-round: add rustls + webpki-roots, relax the K8 scheme check, test against a TLS fake. Fires the moment a concrete https `webhook_url`/`hub_url` exists | ARCHITECTURE_DECISIONS.md AR11 |
| Q5 | Phase 4 · architecture freeze | **RATIFIED + FROZEN 2026-08-29** (form 2): AR1-AR17 confirmed except AR11 ("Aanpassen") → dated amendment + AR11-TLS mini-round queued. Original queue text for the record: AR1-AR17. The mandatory critic pass ran 2026-08-29 (network + auth) and found 4 blocking + 4 serious objections; all fixes adopted before the build and marked "⚔ Critic (adopted)" in the doc. Highlights: AR15 circuit breaker (naive nack would dead-letter the whole backlog ~10-15 s into an HA outage), AR2/AR3 hold-and-retry (ordering), AR16 topic-birth replay (first messages on a new topic were silently lost), AR17 no-redirects (a 302 could ack a payload HA never received), W3 promoted to Essential (load-bearing). Minor items landed in the runbook: `After=kyu.service`, git-vs-/etc drift check, archive-on-route-removal, musl-resolver/mDNS note, release workflow E2E on the musl artifact, `local_only: true` on HA webhook triggers | ARCHITECTURE_DECISIONS.md |
| Q6 | Phase 5 · milestones + standing rules + hook config | L0-L5 plan; enforcement installed before L0 | REALIZATION_PLAN.md |
| Q7 | Phase 6 · milestone reports | Combined report per AFK rule, evidence per exit criterion | REALIZATION_PLAN.md status table |
| Q8 | GitHub remote + branch protection | **Not done** — outward-facing (standing rule 13). Claude can do it on return with the gh token (rule 21): create private repo, push, enable protection, read back the result | — |
| Q9 | Real-HA verification step | HA webhook response semantics (does HA 200 unknown webhook ids? does the automation see the kyu-* headers?) must be MEASURED against real HA, not argued (rule: reasoned vs measured); The artifact-on-LXC half was MEASURED 2026-08-30 in the scratch drill (LXC 191, gate log); what remains needs the real HA and stays an explicit agreed step with the rollout | TEST_PLAN.md (Phase 7) |
| Q10 | /security-review constraint | The repo-scoped `/security-review` skill needs a git remote (`origin/HEAD`) and the repo deliberately has none until Q8. Interim: a full manual security pass ran during the AFK build (findings + fixes in the gate log); the real `/security-review` runs in the same move as Q8 (remote + branch protection) | REALIZATION_PLAN.md gate log |
| Q11 | Phase 7 · gap decision form | The test-gap audit found 20 gaps; 14 were CLOSED during the AFK build (see the gate log), the remaining 6 are provisionally **Accepted as known limitation** and listed verbatim in TEST_PLAN.md "not covered, by decision" — Kenny re-decides each (Dichten · Accepteren · Later) in the ratification round | TEST_PLAN.md |
| Q12 | Phase 7 · security findings | The interim security pass found 12 findings (0 critical/high). All code-level fixes landed (body cap F1, healthz hardening F2, SHA-pinned workflows F3, log sanitisation F4, id validation F5, real URL parsing F6, policy JSON constraints F7, runbook token hygiene F8, systemd hardening F9, bind guidance F12). Ratification covers the chosen limits (16 MiB default cap, 16 healthz connections, MemoryMax=128M) | gate log + commit history |

| Q13 | Phase 8 · document approval | USER_GUIDE, DEBUGGING_GUIDE, ARCHITECTURE_REFERENCE written from code/tests; OPERATIONS_RUNBOOK and TEST_PLAN from Phases 5-7; README honesty pass done. Every "Proven by" test name mechanically diffed against the real test functions (rule 11a): zero missing. Kenny approves per document (Approve · Adjust · Rewrite) with spot-checks | docs/ |

Deviations from frozen decisions during the build: **none** — nothing
was frozen beyond the Phase 0 scope, which was followed as approved;
every other decision is provisional-by-design and queued above.

**Phase 9/10 are deliberately NOT started:** "Tag & release?" is
always Kenny's explicit go (procedure), the GitHub remote is Q8, and
the retrospective is a two-way form.
