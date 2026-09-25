#!/usr/bin/env bash
# Soak runner — the gate for the hardware claims CI cannot make.
# Protocol: docs/soak-protocol.md. Read it before changing anything here.
#
# Exit codes are the protocol's three outcomes, and the distinction matters more
# than the pass/fail bit:
#   0  PASS  — preconditions held, every claim asserted, zero skips
#   1  FAIL  — preconditions held and a claim did not. A real finding.
#   2  VOID  — a precondition did not hold. Proves nothing in either direction.
#
# Conflating FAIL and VOID is how "it passed on rerun" becomes a habit, so a
# void run refuses to write a pass and says which precondition failed.
set -uo pipefail

ITERATIONS="${SOAK_ITERATIONS:-3}"
LOAD_CEILING="${SOAK_LOAD_CEILING:-3.0}"
MIN_FREE_GIB="${SOAK_MIN_FREE_GIB:-8}"
TARGET_CEILING_GB="${SOAK_TARGET_CEILING_GB:-40}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${SOAK_OUT:-$REPO/target/soak/$(date -u +%Y%m%dT%H%M%SZ)}"
cd "$REPO" || { echo "VOID: cannot cd to $REPO" >&2; exit 2; }

mkdir -p "$OUT"
void() { echo "VOID: $*" | tee -a "$OUT/soak.log" >&2; exit 2; }
say()  { echo "$*" | tee -a "$OUT/soak.log"; }

say "=== soak $(date -u +%Y-%m-%dT%H:%M:%SZ)  head $(git rev-parse --short HEAD)"
say "=== artifacts: $OUT"

# ---------------------------------------------------------------------------
# Preconditions (docs/soak-protocol.md §2). Each one is a VOID, not a FAIL:
# a threshold measured under the wrong conditions produces a number someone
# will later cite.
# ---------------------------------------------------------------------------

# 1. The normative machine. Not a VM, not the arm64 runner.
BASELINE="$REPO/docs/hardware-baseline.txt"
[ -f "$BASELINE" ] || void "docs/hardware-baseline.txt missing — cannot verify the host"
HOST_CPU="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
HOST_ARCH="$(uname -m)"
HOST_CORES="$(sysctl -n hw.physicalcpu 2>/dev/null || echo 0)"
say "host: $HOST_CPU / $HOST_ARCH / ${HOST_CORES}p"
case "$HOST_CPU" in
  *"i7-9750H"*) : ;;
  *) void "not the normative machine (docs/hardware-baseline.txt): got '$HOST_CPU'" ;;
esac
[ "$HOST_ARCH" = "x86_64" ] || void "normative machine is x86_64, host is $HOST_ARCH"

# 2. Quiescence. Measured, not assumed: the rehearsal returned 13/15 under
#    concurrent build load and 15/15 four times idle (2026-09-17). The
#    thresholds this script owns are exactly what load perturbs.
LOAD="$(sysctl -n vm.loadavg | awk '{print $2}')"
say "load average (1m): $LOAD  (ceiling $LOAD_CEILING)"
awk -v l="$LOAD" -v c="$LOAD_CEILING" 'BEGIN { exit !(l <= c) }' \
  || void "machine is not quiescent: load $LOAD exceeds $LOAD_CEILING — wait, do not measure"
# Build/test contenders only. `cargo` and `rustc` exist only while building, so
# their presence by name is signal. `node` is NOT: this machine runs long-lived
# node MCP servers and editor tooling that never touch the repo, and a blanket
# `pgrep -x node` made this precondition unsatisfiable — a precondition nobody
# can satisfy is the same defect as a gate that is always red. So node and tsc
# count only when their command line references THIS repo, which is what a
# concurrent `npm test` or rehearsal looks like.
for proc in cargo rustc; do
  if pgrep -x "$proc" >/dev/null 2>&1; then
    void "concurrent '$proc' running — quiescence precondition failed"
  fi
done
SELF=$$
CONTENDERS="$(pgrep -f "$REPO" 2>/dev/null | grep -vx "$SELF" || true)"
for pid in $CONTENDERS; do
  CMD="$(ps -o command= -p "$pid" 2>/dev/null || true)"
  case "$CMD" in
    *node*test*|*npm*test*|*tsc*|*dress-rehearsal*)
      void "concurrent repo test process ($pid): ${CMD:0:80} — quiescence precondition failed" ;;
  esac
done

# 3. Disk headroom. A full disk truncates artifacts, which turns a failure
#    into a lie — the disk-pressure episode of 2026-09-14 hit ~600 MB free with
#    a 32 GB target/, and produced exactly that.
#
#    8 GiB is a CALIBRATION, not a measurement, and it was 20 GiB until this
#    machine reported 14 GiB free and voided every run. A precondition the
#    normative machine cannot meet is the same defect as a gate that is always
#    red, so the number is set where it guards the real failure (artifacts are
#    MB-scale; a release rebuild is a few GB) while staying satisfiable. Raise
#    it with evidence if a soak is ever truncated above this line.
FREE_GIB="$(df -g "$REPO" | awk 'NR==2{print $4}')"
say "free space: ${FREE_GIB} GiB (minimum $MIN_FREE_GIB)"
[ "${FREE_GIB:-0}" -ge "$MIN_FREE_GIB" ] || void "only ${FREE_GIB} GiB free, need $MIN_FREE_GIB"
if [ -d "$REPO/target" ]; then
  TARGET_GB="$(du -sg "$REPO/target" 2>/dev/null | awk '{print $1}')"
  say "target/: ${TARGET_GB} GB (ceiling $TARGET_CEILING_GB, standards §4)"
  if [ "${TARGET_GB:-0}" -gt "$TARGET_CEILING_GB" ]; then
    void "target/ is ${TARGET_GB} GB, above the ${TARGET_CEILING_GB} GB ceiling — run scripts/clean-stale.sh"
  fi
fi

# 4. Release binaries built and size-verified (standards §4 artifact hygiene:
#    a truncated binary fails as a code defect and costs a whole session).
say "--- building release binaries"
cargo build --release -p nbe-preflight -p nbe-engine >>"$OUT/soak.log" 2>&1 \
  || void "release build failed — build the tree before soaking it"
for b in nbe-preflight nbe-engine; do
  P="$REPO/target/release/$b"
  [ -f "$P" ] || void "release binary missing: $P"
  SZ="$(wc -c <"$P" | tr -d ' ')"
  say "  $b: $SZ bytes"
  [ "$SZ" -ge 1000000 ] || void "$b is $SZ bytes — truncated write, not a real binary"
done

# 5. Hardware present. THE INVERSE OF THE CI RULE: in CI a skip is honest and
#    reported; here a skip means the run did not test what it exists to test.
say "--- capability probes"
cargo build -p nbe-engine >>"$OUT/soak.log" 2>&1 || void "debug build failed"
PROBE="$(cargo test -p nbe-engine --test prompt09_encode -- --nocapture 2>&1)"
echo "$PROBE" >>"$OUT/soak.log"
if echo "$PROBE" | grep -q '^SKIP'; then
  void "no hardware H.264 encoder on this host — a soak that skips is void, not green"
fi
say "  H.264 encoder: present"

# ---------------------------------------------------------------------------
# The run. Preconditions held, so from here a failure is a FINDING.
# ---------------------------------------------------------------------------
say "=== preconditions held; $ITERATIONS iterations"
FAILED=0
PASSES=0
R7_HITS=0
# The last iteration's record-tap capture, for soak.json. Empty until an
# iteration records; `set -u` is on, so they are declared rather than assumed.
LAST_TAP_PATHS=""
LAST_TAP_REASONS=""
# The last iteration's stream evidence (Prompt 10), for soak.json.
LAST_STREAM_LINE=""
LAST_SURVIVAL_LINE=""
LAST_RECONNECT_LINE=""
LAST_G1_LINE=""
LAST_VT_LINE=""
LAST_MEDIAMTX=""

for i in $(seq 1 "$ITERATIONS"); do
  say "--- iteration $i/$ITERATIONS"
  ITER="$OUT/iter-$i"; mkdir -p "$ITER"

  # The recording suites, in full. Zero skips is a precondition, not a result:
  # any skip here means the host lost a capability mid-run.
  SKIPS=0
  for t in prompt09_record prompt09_record_file prompt09_session prompt09_markers \
           prompt09_telemetry prompt09_thread prompt09_feed prompt09_encode \
           prompt09_residency prompt09_review; do
    O="$(cargo test -p nbe-engine --test "$t" -- --nocapture 2>&1)"
    echo "$O" >"$ITER/$t.log"
    K="$(echo "$O" | grep -c '^SKIP')"
    SKIPS=$((SKIPS + K))
    if echo "$O" | grep -q "^test result: FAILED"; then
      say "  FAIL $t (see $ITER/$t.log)"; FAILED=1
    fi
  done
  say "  09 suites: $SKIPS skips"
  [ "$SKIPS" -eq 0 ] || { say "  VOID-ish: $SKIPS capability skips during the run"; FAILED=1; }

  # The stream suites (Prompt 10), in full, zero skips — with one exception:
  # the MediaMTX interop proof needs an out-of-band binary in /tmp, and its
  # absence is RECORDED (soak.json stream.mediamtx), not failed, because no
  # soak row claims interop. Every other skip means the host lost a
  # capability mid-run. The evidence lines each suite prints are the rows'
  # record (docs/soak-protocol.md §1).
  SSKIPS=0
  : >"$ITER/stream-evidence.txt"
  for t in prompt10_rtmp prompt10_stream_cmds prompt10_telemetry zerocopy_g1; do
    O="$(cargo test -p nbe-engine --test "$t" -- --nocapture 2>&1)"
    echo "$O" >"$ITER/$t.log"
    K="$(echo "$O" | grep '^SKIP' | grep -vc 'MediaMTX')"
    SSKIPS=$((SSKIPS + K))
    if echo "$O" | grep -q "^test result: FAILED"; then
      say "  FAIL $t (see $ITER/$t.log)"; FAILED=1
    fi
    echo "$O" | grep -E '^(LIVE LOOP|SURVIVAL|RECONNECT|BOTH LIVE|BACKPRESSURE|BOUNDED STOP|G1 guard|VT retain guard|MEDIAMTX-PROOF)' \
      >>"$ITER/stream-evidence.txt"
  done
  say "  stream suites: $SSKIPS capability skips (MediaMTX proof excluded)"
  [ "$SSKIPS" -eq 0 ] || { say "  VOID-ish: $SSKIPS capability skips in the stream suites"; FAILED=1; }
  sed 's/^/    /' "$ITER/stream-evidence.txt" | tee -a "$OUT/soak.log"
  LAST_SURVIVAL_LINE="$(grep -h '^SURVIVAL:' "$ITER/stream-evidence.txt" | head -1)"
  LAST_RECONNECT_LINE="$(grep -h '^RECONNECT:' "$ITER/stream-evidence.txt" | head -1)"
  LAST_G1_LINE="$(grep -h '^G1 guard:' "$ITER/stream-evidence.txt" | head -1)"
  LAST_VT_LINE="$(grep -h '^VT retain guard:' "$ITER/stream-evidence.txt" | head -1)"
  if grep -q '^MEDIAMTX-PROOF .*2 tracks (H264, MPEG-4 Audio)' "$ITER/stream-evidence.txt"; then
    LAST_MEDIAMTX="proved"
  else
    LAST_MEDIAMTX="skipped (binary absent) or not proved — see $ITER/prompt10_rtmp.log"
  fi
  # 'VT retain guard' is required, not merely grepped: the guard skips on CI
  # (no encoder), so this is its only home, and a row whose capture is
  # optional is the tense defect PR #24's pass named.
  for need in SURVIVAL RECONNECT 'G1 guard' 'LIVE LOOP' 'BOTH LIVE' 'VT retain guard'; do
    grep -q "^$need" "$ITER/stream-evidence.txt" \
      || { say "  stream evidence missing: '$need' — the row it backs has no record this iteration"; FAILED=1; }
  done

  # The rehearsal: thresholds, recording end-to-end, AC-6.
  ( cd packages/control-plane && npm run test:rehearsal ) >"$ITER/rehearsal.log" 2>&1
  RP="$(grep -E '^# pass' "$ITER/rehearsal.log" | awk '{print $3}')"
  RF="$(grep -E '^# fail' "$ITER/rehearsal.log" | awk '{print $3}')"
  RS="$(grep -c 'SKIP' "$ITER/rehearsal.log")"
  say "  rehearsal: pass=${RP:-?} fail=${RF:-?} skip_lines=$RS"
  if [ "${RF:-1}" -ne 0 ] || [ "$RS" -ne 0 ]; then
    grep -E '^not ok ' "$ITER/rehearsal.log" | sed 's/^/    /' | tee -a "$OUT/soak.log"
    FAILED=1
  else
    PASSES=$((PASSES + 1))
  fi

  # Collect the artifact set the protocol names (§4).
  for f in engine.log telemetry.jsonl pushes.jsonl show-states.json timings.json; do
    SRC="$REPO/target/dress-rehearsal/$f"
    [ -f "$SRC" ] && cp "$SRC" "$ITER/$f"
  done

  # The record tap's path choice (§1), live since ZERO-COPY Phase 3b.
  #
  # Read out of the rehearsal's own telemetry rather than re-derived: the
  # rehearsal already asserts the take names a path, and this records WHICH,
  # every soak. A silent fall from zeroCopy to cpuReadback is the event this
  # catches — the machine still records, the file is still correct, and the
  # only visible difference is this field. Recording it weekly makes a
  # capability regression a dated event rather than a discovery.
  #
  # Distinct values across the run, so a take that changed path mid-soak shows
  # as two rows and not as whichever tick happened to be last.
  TAP_PATHS="$(grep -ho '"recordTapPath":"[^"]*"' "$ITER/telemetry.jsonl" 2>/dev/null \
    | sed 's/.*:"//; s/"$//' | sort | uniq -c | awk '{printf "%s:%s ", $2, $1}')"
  TAP_REASONS="$(grep -ho '"recordTapReason":"[^"]*"' "$ITER/telemetry.jsonl" 2>/dev/null \
    | sed 's/.*:"//; s/"$//' | sort -u | tr '\n' ' ')"
  # The rehearsal prints it too, which is the cross-check: two independent
  # derivations of the same fact, from the ticks and from the step.
  TAP_LINE="$(grep -h '^RECORD PATH:' "$ITER/rehearsal.log" 2>/dev/null | head -1)"
  # The rehearsal's stream step (Prompt 10): the binary streamed beside the
  # show. A clean iteration that never printed it did not stream.
  STREAM_LINE="$(grep -h 'STREAM:' "$ITER/rehearsal.log" 2>/dev/null | head -1 | sed 's/^# //')"
  say "  rehearsal stream: ${STREAM_LINE:-<none>}"
  if [ "$RS" -eq 0 ] && [ "${RF:-1}" -eq 0 ] && [ -z "$STREAM_LINE" ]; then
    say "  rehearsal stream: NO STREAM LINE across a clean iteration"; FAILED=1
  fi
  LAST_STREAM_LINE="$STREAM_LINE"
  say "  record tap: ${TAP_PATHS:-<none captured>} | reasons: ${TAP_REASONS:-<none>}"
  [ -n "$TAP_LINE" ] && say "  rehearsal said: $TAP_LINE"
  echo "${TAP_PATHS:-}" >"$ITER/record-tap-path.txt"
  # A soak whose rehearsal recorded (no skips, which is already required above)
  # and whose ticks never named a path has lost the field, not the capability.
  if [ "$RS" -eq 0 ] && [ "${RF:-1}" -eq 0 ]; then
    case "${TAP_PATHS:-}" in
      *zeroCopy*|*cpuReadback*) : ;;
      *) say "  record tap: NO PATH ON THE WIRE across a clean recording iteration"; FAILED=1 ;;
    esac
  fi
  LAST_TAP_PATHS="$TAP_PATHS"
  LAST_TAP_REASONS="$TAP_REASONS"

  # Flake-register watch list (§5): R7's signature, with a COUNT, because a
  # quiet return is what the list exists to catch.
  RUN7="$(cd packages/control-plane && npm test 2>&1)"
  echo "$RUN7" >"$ITER/control-plane.log"
  if echo "$RUN7" | grep -qE "expected 3 directives, got 4"; then
    R7_HITS=$((R7_HITS + 1))
    say "  R7 SIGHTING (iteration $i) — signature 'expected 3 directives, got 4'"
    echo "$RUN7" | grep -B 4 -A 20 "expected 3 directives, got 4" >"$ITER/r7-sighting.txt"
  fi
  if echo "$RUN7" | grep -qE "^# fail [1-9]"; then
    say "  control-plane suite failed (see $ITER/control-plane.log)"; FAILED=1
  fi
done

# ---------------------------------------------------------------------------
# The record.
# ---------------------------------------------------------------------------
cat >"$OUT/soak.json" <<JSON
{
  "utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "head": "$(git rev-parse HEAD)",
  "host": { "cpu": "$HOST_CPU", "arch": "$HOST_ARCH", "physical_cores": $HOST_CORES },
  "preconditions": {
    "normative_machine": true,
    "load_1m": $LOAD,
    "free_gib": ${FREE_GIB:-0},
    "target_gb": ${TARGET_GB:-0},
    "hardware_encoder": true
  },
  "iterations": $ITERATIONS,
  "rehearsal_clean_iterations": $PASSES,
  "flake_register": { "R7_sightings": $R7_HITS, "R7_iterations": $ITERATIONS },
  "record_tap": {
    "paths_last_iteration": "${LAST_TAP_PATHS:-}",
    "reasons_last_iteration": "${LAST_TAP_REASONS:-}"
  },
  "stream": {
    "rehearsal_last_iteration": "${LAST_STREAM_LINE:-}",
    "survival_last_iteration": "${LAST_SURVIVAL_LINE:-}",
    "reconnect_last_iteration": "${LAST_RECONNECT_LINE:-}",
    "g1_last_iteration": "${LAST_G1_LINE:-}",
    "vt_retain_last_iteration": "${LAST_VT_LINE:-}",
    "mediamtx": "${LAST_MEDIAMTX:-}"
  },
  "outcome": "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
}
JSON
say "=== soak.json written"
say "=== record tap: ${LAST_TAP_PATHS:-<none>} (${LAST_TAP_REASONS:-<none>})"
say "=== R7 watch list: $R7_HITS sighting(s) in $ITERATIONS iterations"

if [ "$FAILED" -eq 0 ]; then
  say "=== PASS — $PASSES/$ITERATIONS clean, zero skips, thresholds met"
  exit 0
fi
say "=== FAIL — a precondition held and a claim did not. This is a finding;"
say "    do not rerun until green. Artifacts: $OUT"
exit 1
