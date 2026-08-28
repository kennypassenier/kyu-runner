# hub-bridge

A stateless daemon that lets Home Assistant consume from the
[mailbox](https://github.com/kennypassenier/mailbox) hub. Per
configured route it long-polls a topic subscription, POSTs each
message byte-for-byte to a Home Assistant webhook, and acknowledges
**only** when HA answers 2xx — so the hub's lease/retry/dead-letter
machinery works *for* the HA delivery. The bridge holds no state: the
hub owns every cursor, and killing the bridge at any moment loses
nothing.

Status: in development under the dev procedure. Documentation is
written in Phase 8; until then `docs/` holds the phase documents.

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
