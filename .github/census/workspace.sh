#!/usr/bin/env bash
# workspace.sh OUT: ci.yml's workspace step (`cargo test --workspace
# --no-fail-fast`) RUNS passes per arm, a b on odd passes and b a on even
# ones, each under a 20 min process-group kill timer (about 5x its CI time).
set -u
out=$1
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out/logs"
runs="$out/runs.csv"
echo "pass,arm,rc,secs,load1_before" > "$runs"
for i in $(seq 1 "$RUNS"); do
  if [ $((i % 2)) -eq 1 ]; then order="a b"; else order="b a"; fi
  for arm in $order; do
    log="$out/logs/ws-$i-$arm.log"
    load1=$(sysctl -n vm.loadavg | awk '{print $2}')
    start=$(date +%s)
    ( cd "$RUNNER_TEMP/wt-$arm" && perl "$here/pgtimeout.pl" 1200 \
        cargo test --workspace --no-fail-fast --color never ) < /dev/null > "$log" 2>&1 && rc=0 || rc=$?
    secs=$(( $(date +%s) - start ))
    echo "$i,$arm,$rc,$secs,$load1" | tee -a "$runs"
    grep '^test result' "$log" | awk -v i="$i" -v a="$arm" '{p+=$4; f+=$6; g+=$8} END {printf "pass %s arm %s: %d passed, %d failed, %d ignored\n", i, a, p, f, g}'
  done
done
