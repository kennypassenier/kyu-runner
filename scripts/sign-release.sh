#!/usr/bin/env bash
# Sign a release's manifest with the ecosystem key and upload the two
# assets the self-updater needs (J2, K19). `chassis release` calls this;
# it also works by hand:
#
#   scripts/sign-release.sh v1.2.0
#
# What it does: downloads SHA256SUMS from the GitHub release the CI made,
# signs it with minisign (one password prompt — the key never leaves this
# machine), writes VERSION, and uploads SHA256SUMS.minisig BEFORE VERSION
# (critic #15: an updater that sees VERSION first would count a missing
# signature as a failure).
set -euo pipefail

tag="${1:?usage: sign-release.sh vX.Y.Z}"
repo="${REPO:-kennypassenier/kyu-runner}"
key="${MINISIGN_KEY:-$HOME/.minisign/minisign.key}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for tool in gh minisign; do
  command -v "$tool" >/dev/null || { echo "$tool is not installed. What now: install it, then rerun." >&2; exit 1; }
done
[ -f "$key" ] || { echo "no minisign secret key at $key. What now: set MINISIGN_KEY to the key file." >&2; exit 1; }

echo "downloading SHA256SUMS of $tag from $repo"
gh release download "$tag" --repo "$repo" -p SHA256SUMS -D "$work"
[ -s "$work/SHA256SUMS" ] || { echo "the release has no SHA256SUMS yet. What now: wait for the Release workflow to finish." >&2; exit 1; }

echo "signing (minisign will ask for the key password)"
minisign -S -s "$key" -m "$work/SHA256SUMS" -x "$work/SHA256SUMS.minisig" -t "$repo $tag"
printf '%s\n' "${tag#v}" > "$work/VERSION"

echo "verifying the signature with the key baked into chassis (rule R3)"
minisign -V -P "RWQWCzzUBquIHGkS3YERMkuqEm4C3vBArnlb9rySbr8z5ytgVYuji3bS" -m "$work/SHA256SUMS" -x "$work/SHA256SUMS.minisig" >/dev/null

echo "uploading SHA256SUMS.minisig, then VERSION"
gh release upload "$tag" --repo "$repo" --clobber "$work/SHA256SUMS.minisig"
gh release upload "$tag" --repo "$repo" --clobber "$work/VERSION"
echo "done: $repo $tag is now installable by the self-updater"
