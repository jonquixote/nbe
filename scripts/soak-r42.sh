#!/usr/bin/env bash
# R42's closure soak: 600 show-minutes on merged main (docs/soak-protocol.md
# section 8; the user's rulings of 2026-11-08, 01:55 and 03:45). Prepared, not
# started: it runs only on the user's word, after the pre-flight checklist in
# section 8 is ticked by hand.
#
# Twenty AC-5-style shows of 30 minutes (SOAK_SHOWS, SOAK_MINUTES), one after
# another, each its own load-gated window: admitted when the 1-minute load is
# under 2.6, kept only if it ends under 3.0 (the standing rule). A void window
# runs that show again, once. A drop or a panic is a finding and is never rerun.
#
# Exit codes, as scripts/soak.sh's:
#   0  PASS  every show kept, zero panics, zero drops, each output as long as its span
#   1  FAIL  a show kept with a panic, a drop, or a short output: a finding
#   2  VOID  a precondition did not hold, or too few shows were kept
set -uo pipefail

SHOWS="${SOAK_SHOWS:-20}"
MINUTES="${SOAK_MINUTES:-30}"
ADMIT="${SOAK_ADMIT:-2.6}"
KEEP="${SOAK_KEEP:-3.0}"
MIN_FREE_GIB="${SOAK_MIN_FREE_GIB:-15}"   # the user's "keep >= 15"; section 2's floor is 8
SHORT_MS="${SOAK_SHORT_MS:-1000}"          # an output this much shorter than its span is a finding
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Spotlight skips a folder whose name ends in .noindex, so nothing the soak
# writes is indexed while it runs (the user's "Spotlight excluded").
OUT="${SOAK_OUT:-$REPO/target/soak-r42.noindex/$(date -u +%Y%m%dT%H%M%SZ)}"
BIN="$REPO/target/release/nbe-engine"
PREFLIGHT_BIN="$REPO/target/release/nbe-preflight"
cd "$REPO" || { echo "VOID: cannot cd to $REPO" >&2; exit 2; }
mkdir -p "$OUT"
LOG="$OUT/soak.log"
void() { echo "VOID: $*" | tee -a "$LOG" >&2; exit 2; }
say()  { echo "$*" | tee -a "$LOG"; }
load1() { sysctl -n vm.loadavg | awk '{print $2}'; }

HEAD="$(git rev-parse HEAD)"
say "=== R42 soak $(date -u +%Y-%m-%dT%H:%M:%SZ): $SHOWS shows x $MINUTES min; head $HEAD"
say "=== artifacts: $OUT"

# --- Pre-flight (section 8's checklist; each failure is a VOID) ---------------
# The tree: main, clean, at the commit the release build was made from.
[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || void "not on main"
[ -z "$(git status --porcelain --untracked-files=no)" ] || void "the tree has changes"
git fetch -q origin main && [ "$HEAD" = "$(git rev-parse origin/main)" ] || void "HEAD is not origin/main"
# The normative machine (section 2, item 1).
case "$(sysctl -n machdep.cpu.brand_string)" in *"i7-9750H"*) : ;; *) void "not the normative machine" ;; esac
# A release build of main at this commit, newer than every source file.
for b in "$BIN" "$PREFLIGHT_BIN"; do
  [ -x "$b" ] || void "missing release binary $b (cargo build --release -p nbe-engine -p nbe-preflight)"
  [ "$(stat -f %z "$b")" -gt 1000000 ] || void "$b looks truncated"
done
newer="$(find crates vendor Cargo.lock -newer "$BIN" -type f \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) 2>/dev/null | head -1)"
[ -z "$newer" ] || void "the release engine is older than $newer: rebuild at $HEAD"
# Disk (section 2, item 3, held to the user's 15 GiB).
free_gib="$(df -g "$REPO" | awk 'NR==2 {print $4}')"
say "disk free: ${free_gib} GiB (keep >= $MIN_FREE_GIB)"
[ "$free_gib" -ge "$MIN_FREE_GIB" ] || void "only ${free_gib} GiB free"
# Quiet: no build or test of this repo, and no indexer at work (section 2, item 2).
pgrep -x cargo >/dev/null && void "cargo is running"
pgrep -x rustc >/dev/null && void "rustc is running"
idx="$(ps -Ao pcpu=,comm= | awk '/mds_stores|mdworker|mediaanalysisd|photoanalysisd/ {s+=$1} END {print s+0}')"
say "indexers' CPU: ${idx}%"
awk -v s="$idx" 'BEGIN {exit !(s < 5)}' || void "an indexer is working (${idx}% CPU): wait"
say "spotlight: $(mdutil -s / 2>&1 | tr '\n' ' ')"
# Power (section 2, item 6): on AC; the adapter and Low Power Mode recorded.
pmset -g batt | grep -q "AC Power" || void "not on AC power"
say "adapter: $(ioreg -rn AppleSmartBattery | awk -F'= ' '/"AdapterDetails"/ {print $2}' | grep -o '"Watts"=[0-9]*')"
say "low power mode: $(pmset -g | awk '/lowpowermode/ {print $2}')"
# ffprobe, for the outputs' durations.
command -v ffprobe >/dev/null || [ -x /opt/homebrew/bin/ffprobe ] || [ -x /usr/local/bin/ffprobe ] || void "no ffprobe"
say "binaries: nbe-engine $(shasum -a 256 "$BIN" | cut -c1-16), nbe-preflight $(shasum -a 256 "$PREFLIGHT_BIN" | cut -c1-16)"

# --- The shows ----------------------------------------------------------------
TABLE="$OUT/table.tsv"
printf 'show\tverdict\tadmitted\tended\tminutes\tdropped\tpanics\trecord_s\trecord_wall_s\tstream_s\tstream_wall_s\n' > "$TABLE"
kept=0; findings=0
run_show() { # tag
  local tag=$1 o="$OUT/$1"
  mkdir -p "$o"
  local a; while :; do a="$(load1)"; awk -v l="$a" -v c="$ADMIT" 'BEGIN {exit !(l < c)}' && break; sleep 15; done
  local s; s="$(date +%H:%M:%S)"
  (cd packages/control-plane && NBE_ENGINE_BIN="$BIN" NBE_PREFLIGHT_BIN="$PREFLIGHT_BIN" \
    perl -e 'alarm shift; exec @ARGV' $(( (MINUTES + 10) * 60 )) \
    node --import tsx src/r42-soak.measure.ts --out "$o" --minutes "$MINUTES" > "$o/run.out" 2>&1)
  local rc=$? e; e="$(load1)"
  local v=KEPT; awk -v a="$a" -v e="$e" -v c="$ADMIT" -v k="$KEEP" 'BEGIN {exit !(a < c && e < k)}' || v=VOID
  [ -f "$o/result.json" ] || { say "$tag $s-$(date +%H:%M:%S) admitted $a ended $e rc=$rc NO RESULT ($v)"; [ "$v" = KEPT ] && return 3; return 2; }
  local row; row="$(node -e '
    const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const f = (x) => (x === null || x === undefined ? "-" : x);
    console.log([r.minutes.toFixed(1), f(r.droppedFramesTotal), r.panics,
      f(r.record?.video?.seconds), f(r.record?.wallSeconds), f(r.stream?.video?.seconds), f(r.stream?.wallSeconds)].join("\t"));' "$o/result.json")"
  printf '%s\t%s\t%s\t%s\t%s\n' "$tag" "$v" "$a" "$e" "$row" >> "$TABLE"
  say "$tag $s-$(date +%H:%M:%S) admitted $a ended $e rc=$rc $v: $row"
  [ "$v" = VOID ] && { ps -Ao pcpu,comm -r | sed -n 2,7p | tee -a "$LOG"; return 2; }
  # A finding: a panic, a drop, or an output shorter than its span by SHORT_MS or more.
  node -e '
    const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); const short = Number(process.argv[2]) / 1000;
    const shortOf = (o) => o && o.video && o.video.seconds !== null && o.wallSeconds - o.video.seconds >= short;
    process.exit(r.panics > 0 || r.droppedFramesTotal !== 0 || !r.record || !r.stream || shortOf(r.record) || shortOf(r.stream) ? 1 : 0);' \
    "$o/result.json" "$SHORT_MS" || return 1
  return 0
}
PM_START="$(pmset -g log | wc -l | tr -d ' ')"
caffeinate -i -s -w $$ &
for i in $(seq 1 "$SHOWS"); do
  run_show "show-$i"; r=$?
  if [ "$r" = 2 ]; then run_show "show-$i-again"; r=$?; fi
  case "$r" in
    0) kept=$((kept + 1)) ;;
    1) kept=$((kept + 1)); findings=$((findings + 1)); say "FINDING at show $i: never rerun" ;;
    *) say "show $i void twice" ;;
  esac
done
# The power log's lines since the first show: a Sleep, or a line on battery.
pmset -g log | tail -n +"$((PM_START + 1))" | awk '$4 == "Sleep" || tolower($0) ~ /using batt/' > "$OUT/power-events.txt" || true
say "power events in the window: $(wc -l < "$OUT/power-events.txt" | tr -d ' ') (any Sleep or Using Batt voids the run)"
say "=== kept $kept of $SHOWS shows, $((kept * MINUTES)) show-minutes; findings $findings"
[ -s "$OUT/power-events.txt" ] && void "a sleep or battery event in the window"
[ "$findings" -gt 0 ] && { say "FAIL"; exit 1; }
[ $((kept * MINUTES)) -ge 600 ] || void "only $((kept * MINUTES)) show-minutes kept"
say "PASS"; exit 0
