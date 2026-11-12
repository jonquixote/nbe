#!/usr/bin/env bash
# The check of scripts/soak-r42.sh's classification, with the stub harness
# (scripts/soak-r42-stub.mjs) in place of the measure harness: no engine, no
# GPU, no ffprobe. Re-run it after any change to soak-r42.sh:
#
#   scripts/soak-r42-check.sh
#
# Each case runs soak-r42.sh once (with --stub, but for the case that runs it
# without) under a kill timer and checks its exit code,
# one line its output must hold, one it must not, and that no nbe-engine is
# left when it ends. It proves how the script classifies what a harness hands
# it: not the real harness, the engine or ffprobe (the rehearsal shows those).
#
# The load gate is opened (SOAK_ADMIT and SOAK_KEEP far above any load), so the
# check gives the same answer on a loaded machine or a CI runner: nothing here
# is timed. The admission case closes it instead (SOAK_ADMIT=0, so no load
# admits) with a short bound (SOAK_ADMIT_WAIT_S=15) in place of the soak's 30
# minutes. The hang case uses a 10 s show kill timer (SOAK_ALARM_S) in place of
# the soak's 40 minutes. The indexer cases hand the script a process list
# (SOAK_STUB_PS); the admission-after-a-finding case hands it the loads it reads
# (SOAK_STUB_LOADS): the first show admitted and kept, the second never admitted.
#
# macOS (the soak script reads vm.loadavg and the power log); needs node, cc
# and perl. It refuses to start while an nbe-engine runs, since the soak
# script's stray check would kill it.
#
# Exit: 0 every case as expected, 1 a case not as expected, 2 could not run.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$REPO/scripts/soak-r42.sh"
STUB="$REPO/scripts/soak-r42-stub.mjs"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/soak-r42-check.XXXXXX")"
engines() { pgrep -x nbe-engine | paste -sd, -; }

for t in node cc perl pgrep; do
  command -v "$t" >/dev/null || { echo "soak-r42-check: $t is missing" >&2; exit 2; }
done
[ -z "$(engines)" ] || { echo "soak-r42-check: an nbe-engine is running (pids $(engines)); the check would kill it" >&2; exit 2; }

ok=0; bad=0
# case name | stub case | extra environment | expected exit | a line the output must hold | a line it must not ("" for none)
# | the script's arguments (--stub when left out)
check() {
  local name=$1 kase=$2 extra=$3 want=$4 must=$5 mustnt=$6 args=${7---stub} out="$WORK/$1"
  local log="$out.log" rc left
  # shellcheck disable=SC2086 # $extra is a list of NAME=value words; $args is empty or --stub
  env SOAK_HARNESS="$STUB" SOAK_STUB_CASE="$kase" SOAK_OUT="$out" SOAK_ADMIT=1000 SOAK_KEEP=1000 $extra \
    perl -e 'alarm 180; exec @ARGV' bash "$SCRIPT" $args > "$log" 2>&1
  rc=$?
  left="$(engines)"
  # shellcheck disable=SC2086 # one word per pid
  [ -z "$left" ] || kill -KILL ${left//,/ } 2>/dev/null
  local why=""
  [ "$rc" = "$want" ] || why="exit $rc, expected $want"
  grep -qF -- "$must" "$log" || why="${why:+$why; }no line \"$must\""
  [ -z "$mustnt" ] || ! grep -qF -- "$mustnt" "$log" || why="${why:+$why; }a line \"$mustnt\""
  [ -z "$left" ] || why="${why:+$why; }an nbe-engine left (pids $left)"
  if [ -z "$why" ]; then
    ok=$((ok + 1)); echo "ok   $name: exit $rc, \"$must\""
  else
    bad=$((bad + 1)); echo "FAIL $name: $why (output: $log)"
    tail -n 5 "$log" | sed 's/^/       /'
  fi
}

check pass     pass     "SOAK_SHOWS=20"                    0 "STUB PASS"                                           "FINDING"
check short    short    "SOAK_SHOWS=1"                     1 "record video 1798 s against a 1800 s span"          ""
check null     null     "SOAK_SHOWS=20"                    1 "stream video: no duration (ffprobe read none)"      ""
check crash    crash    "SOAK_SHOWS=2"                     1 "FINDING at show 1: no result in a kept window"      "void twice"
check noresult noresult "SOAK_SHOWS=2"                     1 "FINDING at show 1: no result in a kept window"      "void twice"
check hang     hang     "SOAK_SHOWS=1 SOAK_ALARM_S=10"     1 "rc=142 NO RESULT (KEPT)"                            "outlived"
check escape   escape   "SOAK_SHOWS=20"                    2 "STUB VOID: an nbe-engine outlived a show"           ""
check admit    pass     "SOAK_SHOWS=1 SOAK_ADMIT=0 SOAK_ADMIT_WAIT_S=15" 2 "STUB VOID: show-1 was not admitted within 15 s" "STUB PASS"
# #97's review: an admission timeout after a finding is a FAIL, not a VOID.
check admit-after-finding short "SOAK_SHOWS=2 SOAK_ADMIT_WAIT_S=15 SOAK_STUB_LOADS=0,0,5000" 1 "STUB FAIL" "STUB VOID"
# #97's review: SOAK_HARNESS without --stub voids before anything runs.
check harness-env pass  "SOAK_SHOWS=1"                     2 "VOID: SOAK_HARNESS is set" "STUB" ""
# The indexer check (the user's ruling of 2026-11-10, 18:20): each knowledge
# daemon, and one indexer, at 6% voids; all eight at 0.5% (4% summed) beside a
# busy process of another name passes, and the sum names each.
ps_line() { printf '%5s /System/Library/PrivateFrameworks/Stub.framework/%s\n' "$2" "$1"; }
for n in knowledgeconstructiond intelligenceplatformd siriknowledged knowledge-agent mds_stores; do
  ps_line "$n" 6.0 > "$WORK/ps-$n"
  check "idx-$n" pass "SOAK_SHOWS=1 SOAK_STUB_PS=$WORK/ps-$n" 2 "STUB VOID: an indexer is working (6% CPU: $n 6.0%)" "STUB PASS"
done
{ for n in mds_stores mdworker mediaanalysisd photoanalysisd knowledgeconstructiond intelligenceplatformd siriknowledged knowledge-agent; do
    ps_line "$n" 0.5; done; ps_line yes 90.0; } > "$WORK/ps-quiet"
check idx-under-sum pass "SOAK_SHOWS=20 SOAK_STUB_PS=$WORK/ps-quiet" 0 "indexers' CPU: 4% (mds_stores 0.5%, mdworker 0.5%, mediaanalysisd 0.5%, photoanalysisd 0.5%, knowledgeconstructiond 0.5%, intelligenceplatformd 0.5%, siriknowledged 0.5%, knowledge-agent 0.5%)" "VOID"

echo "soak-r42-check: $ok of $((ok + bad)) as expected"
if [ "$bad" = 0 ]; then rm -rf "$WORK"; exit 0; fi
echo "soak-r42-check: outputs kept in $WORK"
exit 1
