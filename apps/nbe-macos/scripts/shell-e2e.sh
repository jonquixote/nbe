#!/usr/bin/env bash
# The shell's client against a real control plane, no engine attached:
# starts packages/control-plane on a spare port with two fresh tokens, runs
# LiveControlPlaneTests, and stops the server. Every step has a kill timer.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
cp_dir="$here/../../packages/control-plane"
port="${NBE_SHELL_E2E_PORT:-18462}"
work="$(mktemp -d)"
op="op-$(uuidgen)"
admin="admin-$(uuidgen)"
export NBE_TOKENS="{\"$op\":\"operator\",\"$admin\":\"admin\"}"
export NBE_AUDIT_LOG="$work/audit.jsonl" NBE_STATE_DIR="$work/state" NBE_PORT="$port"
(cd "$cp_dir" && exec perl -e 'alarm 600; exec @ARGV' npx tsx src/index.ts) > "$work/cp.log" 2>&1 &
cp_pid=$!
trap 'kill "$cp_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
  grep -q "listening on" "$work/cp.log" && break
  kill -0 "$cp_pid" 2>/dev/null || { cat "$work/cp.log"; exit 1; }
  sleep 0.1
done
grep -q "listening on" "$work/cp.log" || { echo "the control plane did not come up"; cat "$work/cp.log"; exit 1; }
export NBE_SHELL_E2E_URL="ws://127.0.0.1:$port/nbe/v0.3"
export NBE_SHELL_E2E_OPERATOR_TOKEN="$op" NBE_SHELL_E2E_ADMIN_TOKEN="$admin"
cd "$here" && perl -e 'alarm 900; exec @ARGV' swift test --filter LiveControlPlaneTests
