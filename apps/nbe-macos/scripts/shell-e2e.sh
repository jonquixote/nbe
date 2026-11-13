#!/usr/bin/env bash
# The shell's client against a real control plane: starts packages/control-plane
# on a spare port with fresh tokens, runs the live tests, and stops it. Every
# step has a kill timer.
#
# With NBE_ENGINE_BIN set (and NBE_PREFLIGHT_BIN, or a built target/), it also
# starts the engine as the control plane's render node and stages a package
# (the dress fixture plus a lower-third and a breaking template, a record
# directory and a stream URL), and AC16LiveTests drives AC-16's ten actions
# through the shell's own commands. Without it, only LiveControlPlaneTests run.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
cp_dir="$repo/packages/control-plane"
port="${NBE_SHELL_E2E_PORT:-18462}"
work="$(mktemp -d)"
op="op-$(uuidgen)"
admin="admin-$(uuidgen)"
render="render-$(uuidgen)"
export NBE_TOKENS="{\"$op\":\"operator\",\"$admin\":\"admin\",\"$render\":\"render\"}"
export NBE_AUDIT_LOG="$work/audit.jsonl" NBE_STATE_DIR="$work/state" NBE_PORT="$port"
(cd "$cp_dir" && exec perl -e 'alarm 900; exec @ARGV' npx tsx src/index.ts) > "$work/cp.log" 2>&1 &
cp_pid=$!
engine_pid=""
trap 'kill "$cp_pid" 2>/dev/null || true; [ -n "$engine_pid" ] && kill "$engine_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
  grep -q "listening on" "$work/cp.log" && break
  kill -0 "$cp_pid" 2>/dev/null || { cat "$work/cp.log"; exit 1; }
  sleep 0.1
done
grep -q "listening on" "$work/cp.log" || { echo "the control plane did not come up"; cat "$work/cp.log"; exit 1; }
export NBE_SHELL_E2E_URL="ws://127.0.0.1:$port/nbe/v0.3"
export NBE_SHELL_E2E_OPERATOR_TOKEN="$op" NBE_SHELL_E2E_ADMIN_TOKEN="$admin"

filter="LiveControlPlaneTests"
if [ -n "${NBE_ENGINE_BIN:-}" ]; then
  pkg="$work/ac16_show"
  mkdir -p "$pkg/media" "$work/record-out"
  cp -R "$repo/tests/fixtures/dress_show/." "$pkg/"
  cp "$repo/tests/fixtures/overlay_show/media/Amiri-Regular.ttf" "$pkg/media/"
  python3 - "$pkg/manifest.json" "$work/record-out" <<'PY'
import json, sys
path, record = sys.argv[1], sys.argv[2]
m = json.load(open(path))
m["assets"].append({"id": "amiri", "kind": "font", "source": "media/Amiri-Regular.ttf"})
m.setdefault("templates", []).extend([
    {"id": "tpl_lt", "kind": "lowerThirdHeadline", "fontAssetIds": ["amiri"],
     "fields": [{"name": "headline", "label": "Headline"}, {"name": "subhead", "label": "Subhead"}]},
    {"id": "tpl_banner", "kind": "breakingBanner", "fontAssetIds": ["amiri"],
     "fields": [{"name": "headline", "label": "Headline"}]},
])
m["show"]["outputs"] = {"record": {"directory": record},
                        "stream": {"protocol": "rtmp", "url": "rtmp://127.0.0.1:1935/live/program"}}
json.dump(m, open(path, "w"))
PY
  NBE_CP_URL="$NBE_SHELL_E2E_URL" NBE_RENDER_TOKEN="$render" NBE_AUDIO_SINK="${NBE_AUDIO_SINK:-null}" \
    perl -e 'alarm 900; exec @ARGV' "$NBE_ENGINE_BIN" > "$work/engine.log" 2>&1 &
  engine_pid=$!
  sleep 3
  kill -0 "$engine_pid" 2>/dev/null || { cat "$work/engine.log"; exit 1; }
  export NBE_SHELL_E2E_PACKAGE="$pkg"
  filter="LiveControlPlaneTests|AC16LiveTests"
fi
cd "$here" && perl -e 'alarm 900; exec @ARGV' swift test --filter "$filter"
