#!/usr/bin/env bash
# R42's closure soak: 600 show-minutes on merged main (docs/soak-protocol.md
# section 8; the user's rulings of 2026-11-08, 01:55 and 03:45). Prepared, not
# started: it runs only on the user's word, after the pre-flight checklist in
# section 8 is ticked by hand.
#
# Twenty AC-5-style shows of 30 minutes (SOAK_SHOWS, SOAK_MINUTES), one after
# another, each its own load-gated window: admitted when the 1-minute load is
# under 2.6, kept only if it ends under 3.0 (the standing rule). A void window
# runs that show again, once. A drop, a panic, a short or unmeasured output, or
# a show that leaves no result in a kept window (a crash or a hang) is a finding
# and is never rerun. A load that does not admit within SOAK_ADMIT_WAIT_S
# (30 minutes) voids the run, with the load's history. Each show runs in a
# process group of its own, stopped whole at its kill timer, and an nbe-engine
# left after a show stops the run before the next one.
#
# Exit codes, as scripts/soak.sh's:
#   0  PASS  every show kept, zero panics, zero drops, each output measured and as long as its span
#   1  FAIL  a finding: a show kept with a panic, a drop, a short or unmeasured output, or no result
#   2  VOID  a precondition did not hold, or too few shows were kept
#
# SOAK_HARNESS=<program> runs that program in place of the measure harness,
# with the same arguments (--out <dir> --minutes <n>), so each of the paths
# above can be tested with a stub (scripts/soak-r42-stub.mjs). The pre-flight
# is then skipped and every line says STUB: a stub run is never a soak.
set -uo pipefail

SHOWS="${SOAK_SHOWS:-20}"
MINUTES="${SOAK_MINUTES:-30}"
ADMIT="${SOAK_ADMIT:-2.6}"
KEEP="${SOAK_KEEP:-3.0}"
ADMIT_WAIT_S="${SOAK_ADMIT_WAIT_S:-1800}" # the admission wait's bound; then VOID, with the load's history
ALARM_S="${SOAK_ALARM_S:-$(( (MINUTES + 10) * 60 ))}" # a show's kill timer
MIN_FREE_GIB="${SOAK_MIN_FREE_GIB:-15}"   # the user's "keep >= 15"; section 2's floor is 8
SHORT_MS="${SOAK_SHORT_MS:-1000}"          # an output this much shorter than its span is a finding
HARNESS="${SOAK_HARNESS:-}"
if [ -n "$HARNESS" ]; then HARNESS="$(cd "$(dirname "$HARNESS")" && pwd)/$(basename "$HARNESS")"; STUB="STUB "; else STUB=""; fi
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Spotlight skips a folder whose name ends in .noindex, so nothing the soak
# writes is indexed while it runs (the user's "Spotlight excluded").
OUT="${SOAK_OUT:-$REPO/target/soak-r42.noindex/$(date -u +%Y%m%dT%H%M%SZ)}"
BIN="$REPO/target/release/nbe-engine"
PREFLIGHT_BIN="$REPO/target/release/nbe-preflight"
cd "$REPO" || { echo "VOID: cannot cd to $REPO" >&2; exit 2; }
mkdir -p "$OUT"
LOG="$OUT/soak.log"
void() { echo "${STUB}VOID: $*" | tee -a "$LOG" >&2; exit 2; }
say()  { echo "${STUB}$*" | tee -a "$LOG"; }
load1() { sysctl -n vm.loadavg | awk '{print $2}'; }
engines() { pgrep -x nbe-engine | paste -sd, -; }

HEAD="$(git rev-parse HEAD)"
say "=== R42 soak $(date -u +%Y-%m-%dT%H:%M:%SZ): $SHOWS shows x $MINUTES min; head $HEAD"
say "=== artifacts: $OUT"

# --- Pre-flight (section 8's checklist; each failure is a VOID) ---------------
# No engine is running: a show's engine must be the only one.
[ -z "$(engines)" ] || void "an nbe-engine is already running (pids $(engines))"
if [ -n "$HARNESS" ]; then
  say "pre-flight skipped: SOAK_HARNESS=$HARNESS (a stub run, never a soak)"
else
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
fi

# --- The shows ----------------------------------------------------------------
TABLE="$OUT/table.tsv"
printf 'show\tverdict\tadmitted\tended\tminutes\tdropped\tpanics\trecord_s\trecord_wall_s\tstream_s\tstream_wall_s\n' > "$TABLE"
kept=0; findings=0; stray=0
# A show's engine must not outlive it: the harness runs as the leader of a
# process group of its own, so the engine it spawns is in that group; at the
# kill timer the whole group is stopped (TERM, then KILL 5 s later) and the
# exit is 142, as perl's alarm. Whatever is left in the group when the harness
# exits is stopped the same way.
run_group() { # seconds program [args...]
  perl -MPOSIX=setpgid -e '
    my $t = shift; my $pid = fork; die "fork: $!" unless defined $pid;
    if ($pid == 0) { setpgid(0, 0); exec @ARGV or die "exec $ARGV[0]: $!" }
    setpgid($pid, $pid);
    my $timed_out = 0;
    $SIG{ALRM} = sub { $timed_out = 1; kill "TERM", -$pid; sleep 5; kill "KILL", -$pid };
    alarm $t; waitpid($pid, 0); my $s = $?; alarm 0;
    if (kill 0, -$pid) { kill "TERM", -$pid; sleep 3; kill "KILL", -$pid }
    exit 142 if $timed_out;
    exit($s & 127 ? 128 + ($s & 127) : $s >> 8);' "$@"
}
run_show() { # tag
  local tag=$1 o="$OUT/$1"
  mkdir -p "$o"
  # Admission, bounded: a load that does not come under ADMIT within
  # ADMIT_WAIT_S voids the run, with the history of the load it waited on.
  local a waited=0 hist="$o/admission-load.txt"
  while :; do
    a="$(load1)"; echo "$(date +%H:%M:%S) $a" >> "$hist"
    awk -v l="$a" -v c="$ADMIT" 'BEGIN {exit !(l < c)}' && break
    if [ "$waited" -ge "$ADMIT_WAIT_S" ]; then
      say "$tag: the load did not come under $ADMIT within ${ADMIT_WAIT_S} s; its history (time, 1-minute load):"
      tee -a "$LOG" < "$hist"
      ps -Ao pcpu,comm -r | sed -n 2,7p | tee -a "$LOG"
      void "$tag was not admitted within ${ADMIT_WAIT_S} s"
    fi
    sleep 15; waited=$((waited + 15))
  done
  local s; s="$(date +%H:%M:%S)"
  if [ -n "$HARNESS" ]; then
    run_group "$ALARM_S" "$HARNESS" --out "$o" --minutes "$MINUTES" > "$o/run.out" 2>&1
  else
    (cd packages/control-plane && export NBE_ENGINE_BIN="$BIN" NBE_PREFLIGHT_BIN="$PREFLIGHT_BIN" &&
      run_group "$ALARM_S" node --import tsx src/r42-soak.measure.ts --out "$o" --minutes "$MINUTES" > "$o/run.out" 2>&1)
  fi
  local rc=$? e; e="$(load1)"
  local v=KEPT; awk -v a="$a" -v e="$e" -v c="$ADMIT" -v k="$KEEP" 'BEGIN {exit !(a < c && e < k)}' || v=VOID
  if [ ! -f "$o/result.json" ]; then
    printf '%s\t%s\t%s\t%s\tno result (rc %s)\n' "$tag" "$v" "$a" "$e" "$rc" >> "$TABLE"
    say "$tag $s-$(date +%H:%M:%S) admitted $a ended $e rc=$rc NO RESULT ($v)"
    [ "$v" = KEPT ] && return 3; return 2
  fi
  local row; row="$(node -e '
    const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const f = (x) => (x === null || x === undefined ? "-" : x);
    console.log([r.minutes.toFixed(1), f(r.droppedFramesTotal), r.panics,
      f(r.record?.video?.seconds), f(r.record?.wallSeconds), f(r.stream?.video?.seconds), f(r.stream?.wallSeconds)].join("\t"));' "$o/result.json")"
  printf '%s\t%s\t%s\t%s\t%s\n' "$tag" "$v" "$a" "$e" "$row" >> "$TABLE"
  say "$tag $s-$(date +%H:%M:%S) admitted $a ended $e rc=$rc $v: $row"
  [ "$v" = VOID ] && { ps -Ao pcpu,comm -r | sed -n 2,7p | tee -a "$LOG"; return 2; }
  # A finding: a panic, a drop, an output shorter than its span by SHORT_MS or
  # more, or an output ffprobe could not measure (no duration for the
  # recording's video or audio, or the stream's video, or no wall span).
  local why; why="$(node -e '
    const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); const short = Number(process.argv[2]) / 1000;
    const why = [];
    if (r.panics !== 0) why.push(`panics ${r.panics}`);
    if (r.droppedFramesTotal !== 0) why.push(`droppedFramesTotal ${r.droppedFramesTotal}`);
    const num = (x) => typeof x === "number" && Number.isFinite(x);
    for (const [name, o, tracks] of [["record", r.record, ["video", "audio"]], ["stream", r.stream, ["video"]]]) {
      if (!o) { why.push(`no ${name} output`); continue; }
      for (const t of tracks) if (!num(o[t]?.seconds)) why.push(`${name} ${t}: no duration (ffprobe read none)`);
      if (!num(o.wallSeconds)) why.push(`${name}: no wall span`);
      else if (num(o.video?.seconds) && o.wallSeconds - o.video.seconds >= short) why.push(`${name} video ${o.video.seconds} s against a ${o.wallSeconds} s span`);
    }
    console.log(why.join("; "));
    process.exit(why.length ? 1 : 0);' "$o/result.json" "$SHORT_MS" 2>>"$LOG")" || { say "$tag: $why"; return 1; }
  return 0
}
# Before the next show: no nbe-engine left. One left stops the run here.
strays() { # tag
  local p; p="$(engines)"
  [ -z "$p" ] && return 0
  say "$1: an nbe-engine outlived its show (pids $p); the run stops here"
  ps -o pid=,pgid=,etime=,command= -p "$p" | tee -a "$LOG"
  kill -KILL ${p//,/ } 2>/dev/null
  stray=1
}
PM_START="$(pmset -g log | wc -l | tr -d ' ')"
caffeinate -i -s -w $$ &
for i in $(seq 1 "$SHOWS"); do
  run_show "show-$i"; r=$?; strays "show-$i"
  if [ "$r" = 2 ] && [ "$stray" = 0 ]; then run_show "show-$i-again"; r=$?; strays "show-$i-again"; fi
  case "$r" in
    0) kept=$((kept + 1)) ;;
    1) kept=$((kept + 1)); findings=$((findings + 1)); say "FINDING at show $i: never rerun" ;;
    3) findings=$((findings + 1)); say "FINDING at show $i: no result in a kept window (a crash or a hang), never rerun" ;;
    *) say "show $i void twice" ;;
  esac
  [ "$stray" = 1 ] && break
done
# The power log's lines since the first show: a Sleep, or a line on battery.
pmset -g log | tail -n +"$((PM_START + 1))" | awk '$4 == "Sleep" || tolower($0) ~ /using batt/' > "$OUT/power-events.txt" || true
say "power events in the window: $(wc -l < "$OUT/power-events.txt" | tr -d ' ') (any Sleep or Using Batt voids the run)"
say "=== kept $kept of $SHOWS shows, $((kept * MINUTES)) show-minutes; findings $findings"
[ -s "$OUT/power-events.txt" ] && void "a sleep or battery event in the window"
[ "$findings" -gt 0 ] && { say "FAIL"; exit 1; }
[ "$stray" = 1 ] && void "an nbe-engine outlived a show"
[ $((kept * MINUTES)) -ge 600 ] || void "only $((kept * MINUTES)) show-minutes kept"
say "PASS"; exit 0
