#!/usr/bin/env bash
# M1: build the deployable artifact — a static musl binary (T8: LXC
# 109's libc is not the build machine's libc; static removes the class)
# plus its checksum. The release workflow runs the same steps.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

cargo build --release --target x86_64-unknown-linux-musl

out=target/x86_64-unknown-linux-musl/release/hub-bridge
cp "$out" hub-bridge-x86_64-linux-musl
sha256sum hub-bridge-x86_64-linux-musl > SHA256SUMS
echo "artifact: hub-bridge-x86_64-linux-musl"
cat SHA256SUMS
