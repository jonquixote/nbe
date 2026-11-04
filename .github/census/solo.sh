#!/usr/bin/env bash
# solo.sh SHARD OUT: each test of SHARD in tests.txt alone, RUNS passes. A
# 'both' test runs on arm a and arm b, in the order a b on odd passes and b a
# on even ones; a 'b' test runs on arm b. Each run is
# `cargo test -p nbe-engine --test BIN -- --exact TEST --nocapture` under a
# process-group kill timer (SOLO_KILL_S, 300 s by default). On a timeout,
# diag.sh samples the hung group first. After WEDGE_AFTER timeouts in a row
# (3 by default) the loop stops: the runs after a wedge are one event, not
# samples, and stopping keeps the job's budget for the evidence.
set -u
shard=$1; out=$2
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out/logs"
csv="$out/solo.csv"
echo "pass,binary,test,arm,verdict,rc,secs,load1_before,normative_skips,detail" > "$csv"
kill_s=${SOLO_KILL_S:-300}
mkdir -p "$out/diag"
streak=0
while read -r s bin t heads _gates; do echo "$s $bin $t $heads"; done < <(grep -v '^#' "$here/tests.txt") > "$out/list.txt"
for i in $(seq 1 "$RUNS"); do
  while read -r s bin t heads; do
    [ "$s" = "$shard" ] || continue
    if [ "$heads" = both ]; then
      if [ $((i % 2)) -eq 1 ]; then order="a b"; else order="b a"; fi
    else
      order="b"
    fi
    for arm in $order; do
      log="$out/logs/$i-$bin-$t-$arm.log"
      load1=$(sysctl -n vm.loadavg | awk '{print $2}')
      start=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
      ( cd "$RUNNER_TEMP/wt-$arm" && DIAG_OUT="$out/diag/$i-$bin-$t-$arm" PGTIMEOUT_HOOK="$here/diag.sh" \
          perl "$here/pgtimeout.pl" "$kill_s" \
          cargo test -p nbe-engine --test "$bin" --color never -- --exact "$t" --nocapture --color never ) \
          < /dev/null > "$log" 2>&1 && rc=0 || rc=$?
      end=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
      secs=$(echo "$end - $start" | bc)
      norm=$(grep -c '^SKIP normative:' "$log" || true)
      # A run that ran anything but this one test is INVALID. A SKIP line
      # other than `SKIP normative:` (a bound the runner does not assert,
      # not a skip: docs/soak-protocol.md §5) means the test did not run its
      # case: SKIPPED, neither a pass nor a failure.
      if [ "$rc" -eq 124 ]; then
        verdict=TIMEOUT
      elif ! grep -q '^running 1 test$' "$log"; then
        verdict=INVALID
      elif grep -q '^test result: FAILED\. 0 passed; 1 failed' "$log"; then
        verdict=FAIL
      elif [ "$rc" -eq 0 ] && grep -q '^test result: ok\. 1 passed; 0 failed' "$log"; then
        if grep -E '^SKIP' "$log" | grep -vq '^SKIP normative:'; then verdict=SKIPPED; else verdict=PASS; fi
      else
        verdict=ERROR
      fi
      detail=$( { grep -E '^(BOTH LIVE|SURVIVAL):' "$log"; grep -A1 -E "panicked at" "$log" | grep -vE 'panicked at|^--$' | head -2; grep -E '^SKIP' "$log" | grep -v '^SKIP normative:' | head -1; } \
        | tr ',"' ';'"'" | tr '\n' ' ' | cut -c1-300)
      echo "$i,$bin,$t,$arm,$verdict,$rc,$secs,$load1,$norm,\"$detail\"" | tee -a "$csv"
      if [ "$verdict" = TIMEOUT ]; then streak=$((streak + 1)); else streak=0; fi
      if [ "$streak" -ge "${WEDGE_AFTER:-3}" ]; then
        echo "WEDGED: $streak timeouts in a row, the last pass $i $bin $t arm $arm; the loop stops here" | tee "$out/WEDGED"
        exit 0
      fi
    done
  done < "$out/list.txt"
done
