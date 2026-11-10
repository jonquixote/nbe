#!/usr/bin/env bash
# The check of scripts/soak-r42.sh's classification, with the stub harness
# (scripts/soak-r42-stub.mjs) in place of the measure harness: no engine, no
# GPU, no ffprobe. Re-run it after any change to soak-r42.sh:
#
#   scripts/soak-r42-check.sh
#
# Each case runs soak-r42.sh once under a kill timer and checks its exit code,
# one line its output must hold, one it must not, and that no nbe-engine is
# left when it ends. It proves how the script classifies what a harness hands
# it: not the real harness, the engine or ffprobe (the rehearsal shows those).
#
# The load gate is opened (SOAK_ADMIT and SOAK_KEEP far above any load), so the
# check gives the same answer on a loaded machine or a CI runner: nothing here
# is timed. The admission case closes it instead (SOAK_ADMIT=0, so no load
# admits) with a short bound (SOAK_ADMIT_WAIT_S=15) in place of the soak's 30
# minutes. The hang case uses a 10 s show kill timer (SOAK_ALARM_S) in place of
# the soak's 40 minutes.
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
check() {
  local name=$1 kase=$2 extra=$3 want=$4 must=$5 mustnt=$6 out="$WORK/$1"
  local log="$out.log" rc left
  # shellcheck disable=SC2086 # $extra is a list of NAME=value words
  env SOAK_HARNESS="$STUB" SOAK_STUB_CASE="$kase" SOAK_OUT="$out" SOAK_ADMIT=1000 SOAK_KEEP=1000 $extra \
    perl -e 'alarm 180; exec @ARGV' bash "$SCRIPT" > "$log" 2>&1
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

echo "soak-r42-check: $ok of $((ok + bad)) as expected"
if [ "$bad" = 0 ]; then rm -rf "$WORK"; exit 0; fi
echo "soak-r42-check: outputs kept in $WORK"
exit 1
