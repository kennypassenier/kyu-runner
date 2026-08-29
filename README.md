# hub-bridge

A stateless daemon that lets Home Assistant consume from the
[mailbox](https://github.com/kennypassenier/mailbox) hub. Per
configured route it long-polls a topic subscription, POSTs each
message byte-for-byte to a Home Assistant webhook, and acknowledges
**only** when HA answers 2xx — so the hub's lease/retry/dead-letter
machinery works *for* the HA delivery. The bridge holds no state: the
hub owns every cursor, and killing the bridge at any moment loses
nothing.

Built and hardened under the dev procedure; not yet released or
deployed (the release tag and the LXC 109 rollout wait for Kenny's
explicit go — see `docs/PENDING_MINI_ROUNDS.md` for everything queued
from the AFK build).

## Documentation

| Doc | What it answers |
|---|---|
| [docs/USER_GUIDE.md](docs/USER_GUIDE.md) | what each feature does, config examples, every claim with its proving test |
| [docs/OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | numbered install / update / restore / monitoring procedures for LXC 109 |
| [docs/DEBUGGING_GUIDE.md](docs/DEBUGGING_GUIDE.md) | log lines, symptom→cause table, /healthz states |
| [docs/ARCHITECTURE_REFERENCE.md](docs/ARCHITECTURE_REFERENCE.md) | the system as built: route loop, timing model, supervision |
| [docs/TEST_PLAN.md](docs/TEST_PLAN.md) | what the 57 tests prove, what the doubles cannot express, gaps accepted by decision |

## Development setup (one-time per clone)

The commit gates are git-native and must be activated once:

```
git config core.hooksPath .githooks
```

Every commit then runs format check, clippy (warnings are errors) and
the full test suite, and requires feature IDs in the message
(`[K3, AR2]` or `[meta]`). CI re-runs the same gates on every push.

Tests spawn a local scratch hub: set `MAILBOX_BIN` to a mailbox binary
(default: `~/Projects/mailbox/target/release/mailbox` if present) or
have docker available (`MAILBOX_IMAGE`, default
`ghcr.io/kennypassenier/mailbox:1.0.0`). The real hub is never
touched by tests.
