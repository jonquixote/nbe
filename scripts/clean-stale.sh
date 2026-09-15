#!/bin/bash
# clean-stale: prune stale build artifacts and report reclaimed space.
# Weekly habit (see README Quickstart). Uses cargo-sweep when available;
# otherwise falls back to removing the incremental cache, which churns
# hardest under commit-mutate-restore falsification batteries.
set -euo pipefail
cd "$(dirname "$0")/.."

before=$(df -k . | awk 'NR==2 {print $4}')
if command -v cargo-sweep >/dev/null 2>&1; then
  cargo-sweep -t 14
else
  rm -rf target/debug/incremental
fi
after=$(df -k . | awk 'NR==2 {print $4}')
echo "reclaimed: $(((after - before) / 1024)) MiB (avail before: $((before / 1024)) MiB, after: $((after / 1024)) MiB)"
