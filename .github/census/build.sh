#!/usr/bin/env bash
# build.sh OUT ARM... -- CARGO_ARGS...: one cold worktree and target dir per
# arm, built with `cargo test CARGO_ARGS --no-run`. Each arm's worktree is
# checked against its SHA, and where prompt10_rtmp is built, #81's past-cap
# warn line must be compiled into arm b's binary and absent from arm a's.
set -euo pipefail
out=$1; shift
arms=()
while [ "$1" != "--" ]; do arms+=("$1"); shift; done
shift
mkdir -p "$out"
for arm in "${arms[@]}"; do
  if [ "$arm" = a ]; then sha="$ARM_A"; else sha="$ARM_B"; fi
  wt="$RUNNER_TEMP/wt-$arm"
  git worktree add --detach "$wt" "$sha"
  got=$(git -C "$wt" rev-parse HEAD)
  if [ "$got" != "$sha" ]; then echo "FAIL: arm $arm is $got, not $sha"; exit 1; fi
  echo "arm $arm: $(git -C "$wt" log -1 --format='%H %s')"
  start=$(date +%s)
  ( cd "$wt" && cargo test "$@" --no-run --color never 2>&1 ) | tee "$out/build-$arm.log"
  echo "arm $arm: built in $(( $(date +%s) - start )) s"
done
marker="the rest of the gap is silence"
for arm in "${arms[@]}"; do
  exe=$(sed -nE 's/.*Executable tests\/prompt10_rtmp\.rs \((.*)\).*/\1/p' "$out/build-$arm.log")
  [ -n "$exe" ] || continue
  n=$(grep -a -c "$marker" "$RUNNER_TEMP/wt-$arm/$exe" || true)
  echo "arm $arm: #81's marker in prompt10_rtmp: $n"
  if [ "$arm" = a ] && [ "$n" -ne 0 ]; then echo "FAIL: arm a carries #81's code"; exit 1; fi
  if [ "$arm" = b ] && [ "$n" -lt 1 ]; then echo "FAIL: arm b lacks #81's code"; exit 1; fi
done
