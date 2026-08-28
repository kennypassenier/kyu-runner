# Pending gates & ratifications — AFK build queue

Kenny went AFK right after the Phase 0 gate (2026-08-28: "doe alles
wat je kan zonder mijn input"). Per the AFK rule (procedure L2), work
continued on documented recommendations; every skipped gate is queued
here as a ratification round and this file is the FIRST thing
presented on Kenny's return. Nothing here is frozen until ratified.

| # | Gate | What was decided provisionally | Where recorded |
|---|---|---|---|
| Q1 | Phase 1 · build-vs-buy | Build (study §4 option A re-validated); alternatives rejected with reasons | SCOPE.md "Build-vs-buy" |
| Q2 | Phase 2 · feature ratings | K1-K11 Essential; W1 Essential; W2-W4 Desired; W5-W6 Later; W7 Don't do (B1); M1-M3 answered | FEATURES.md |
| Q3 | Phase 3 · tech choices | Rust 1.97 pinned, tokio, reqwest (no TLS), serde+toml, tracing, cargo-deny, musl-static release artifact, MIT/Apache-2.0 | ARCHITECTURE_DECISIONS.md T1-T9 |
| Q4 | Phase 3 · **platform (OPEN question)** | Assumption: LXC 109 only, x86_64. The procedure forbids guessing this — Kenny states which machines actually run it | ARCHITECTURE_DECISIONS.md T8 |
| Q5 | Phase 4 · architecture freeze | AR1-AR14 incl. critic amendments | ARCHITECTURE_DECISIONS.md |
| Q6 | Phase 5 · milestones + standing rules + hook config | L0-L5 plan; enforcement installed before L0 | REALIZATION_PLAN.md |
| Q7 | Phase 6 · milestone reports | Combined report per AFK rule, evidence per exit criterion | REALIZATION_PLAN.md status table |
| Q8 | GitHub remote + branch protection | **Not done** — outward-facing (standing rule 13). Claude can do it on return with the gh token (rule 21): create private repo, push, enable protection, read back the result | — |
| Q9 | Real-HA verification step | HA webhook response semantics (does HA 200 unknown webhook ids?) must be MEASURED against real HA, not argued (rule: reasoned vs measured); scheduled as an explicit agreed step with the LXC 109 rollout | TEST_PLAN.md (Phase 7) |

Deviations from frozen decisions during the build: **none yet** — this
line is updated the moment one occurs (quarantine + queue, per L2).
