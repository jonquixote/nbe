#!/bin/bash
# clean-stale: prune stale build artifacts and report reclaimed space.
# Weekly habit (see README Quickstart). Uses cargo-sweep when installed
# (`cargo install cargo-sweep`; invoked as the cargo subcommand below);
# otherwise falls back to removing the incremental cache. Note: with
# workspace incremental=false the incremental dir stays empty and the
# fallback is a no-op — the sweep path is the one that does work there.
set -euo pipefail
cd "$(dirname "$0")/.."

before=$(df -k . | awk 'NR==2 {print $4}')
if cargo sweep --version >/dev/null 2>&1; then
  cargo sweep --time 14
else
  rm -rf target/debug/incremental
fi
after=$(df -k . | awk 'NR==2 {print $4}')
echo "reclaimed: $(((after - before) / 1024)) MiB (avail before: $((before / 1024)) MiB, after: $((after / 1024)) MiB)"
