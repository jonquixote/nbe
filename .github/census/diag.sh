#!/usr/bin/env bash
# diag.sh PGID: what a hung solo run's process group and the media daemons
# are doing, into $DIAG_OUT, before pgtimeout.pl kills the group. Nothing
# here may fail the run.
pg=$1
out=${DIAG_OUT:-$RUNNER_TEMP/diag}
mkdir -p "$out" || exit 0
ps -axo pid,ppid,pgid,stat,etime,%cpu,rss,command > "$out/ps.txt" 2>&1 || true
# Every process in the group at once, so none finishes while another is
# being sampled.
for p in $(ps -axo pid=,pgid= | awk -v g="$pg" '$2 == g {print $1}'); do
  sample "$p" 3 -file "$out/sample-$p.txt" > /dev/null 2>&1 &
done
wait
grep -iE 'VTEncoder|VTDecoder|videotoolbox|coremedia|mediaserverd|coreaudiod|AudioComponent|audioanalyticsd|replayd|WindowServer' "$out/ps.txt" > "$out/media-procs.txt" || true
sudo -n log show --last 6m --style compact \
  --predicate 'subsystem BEGINSWITH "com.apple.videotoolbox" OR subsystem BEGINSWITH "com.apple.coremedia" OR process CONTAINS "VTEncoder" OR process == "coreaudiod"' \
  > "$out/log.txt" 2>&1 || true
exit 0
