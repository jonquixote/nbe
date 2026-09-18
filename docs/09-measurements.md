# P9 Step 0 measurements (normative machine)

Machine: MacBook Pro 15,1, i7-9750H, 16 GB, Intel UHD 630 + AMD Radeon Pro 555X
(`docs/hardware-baseline.txt`). Adapter selected by harness: AMD Radeon Pro 555X.
Branch: `P9-recording`. Date: 2026-09-14 (UTC).

## Disk baseline (start of session)

```
$ df -h / && df -k /
Filesystem        Size    Used   Avail Capacity iused ifree %iused  Mounted on
/dev/disk1s5s1   233Gi    10Gi   9.7Gi    52%    427k  102M    0%   /
Filesystem     1024-blocks      Used Available Capacity iused     ifree %iused  Mounted on
/dev/disk1s5s1   244810132  11009668  10166500    52%  426864 101665000    0%   /
```

9.7 Gi free (task brief said 9.8 Gi; 9.7 Gi measured).

## 0a — preflight reload cost on tests/fixtures/dress_show

Binary prebuilt (`cargo build --release -p nbe-preflight`, 2.41 s, excluded
from timings). Wall clock via `time`, release x3 then debug x1. Complete outputs:

```
$ time ./target/release/nbe-preflight --package-path tests/fixtures/dress_show; echo "EXIT=$?"
preflight OK: air-ready. report at tests/fixtures/dress_show/preflight_report.json
./target/release/nbe-preflight --package-path tests/fixtures/dress_show  0.18s user 0.18s system 21% cpu 1.685 total
EXIT=0
```

```
$ time ./target/release/nbe-preflight --package-path tests/fixtures/dress_show; echo "EXIT=$?"
preflight OK: air-ready. report at tests/fixtures/dress_show/preflight_report.json
./target/release/nbe-preflight --package-path tests/fixtures/dress_show  0.18s user 0.17s system 37% cpu 0.928 total
EXIT=0
```

```
$ time ./target/release/nbe-preflight --package-path tests/fixtures/dress_show; echo "EXIT=$?"
preflight OK: air-ready. report at tests/fixtures/dress_show/preflight_report.json
./target/release/nbe-preflight --package-path tests/fixtures/dress_show  0.16s user 0.14s system 37% cpu 0.798 total
EXIT=0
```

```
$ time ./target/debug/nbe-preflight --package-path tests/fixtures/dress_show; echo "EXIT=$?"
preflight OK: air-ready. report at tests/fixtures/dress_show/preflight_report.json
./target/debug/nbe-preflight --package-path tests/fixtures/dress_show  0.58s user 0.15s system 32% cpu 2.186 total
EXIT=0
```

Summary: release 1.685 s / 0.928 s / 0.798 s (run 1 cold page cache, runs 2-3
warm); debug 2.186 s. The stale 46 s figure (debug build under contention) is
superseded by ~2 orders of magnitude; the rewrite is to cite release ~0.8-1.7 s
for the dress package. Exit 0, air-ready, all runs. Tree clean after
(`preflight_report.json` is git-ignored; `git status --short` empty).

## 0b — frame-path fork at 1080p30

Scratch harness `crates/nbe-engine/tests/scratch_p9_measure.rs` (deleted after;
pattern copied from `prompt07_overlay.rs:450 render_engine()`), 300 frames each
phase, 1920x1080 View, deadline `Some(33.333 ms)` per `main.rs:54-66` budget
logic (`frame_budget = 1/house_rate`). Harness ran in debug/test profile.
Package minimal (solid fills + slate) — composite cost is a floor, readback
cost is resolution-bound. 5 warmup frames before each phase, excluded.

(A) `render_frame()` then blocking `readback_view()` per frame; internal
deadline accounting covers render only (readback sits outside the loop today).
(B) `render_frame()` then `copy_texture_to_texture` view -> second texture +
submit + poll-Wait per frame, as encoder-feed proxy (blocking upper bound; a
real share path would not stall).

Complete output of the canonical run (`cargo test -p nbe-engine
--test scratch_p9_measure -- --nocapture --test-threads=1`, exit 0):

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.04s
     Running tests/scratch_p9_measure.rs (target/debug/deps/scratch_p9_measure-a89105719bc70a8d)

running 2 tests
test scratch_p9_gpu_copy_cost ... adapter: AMD Radeon Pro 555X
B copy_texture_to_texture+submit+poll ms: n=300 mean=1.578ms min=0.655ms p50=0.876ms p95=4.541ms max=20.594ms
B copy_texture_to_texture+submit+poll ms per-frame ms (all 300):
  9.45 3.49 0.82 0.82 4.76 0.90 0.94 0.90 0.84 0.71 2.83 0.89 0.92 0.91 0.89 0.80 0.93 0.93 3.23 0.87 0.71 0.84 3.25 0.85 0.73 0.78 0.84 0.78 0.86 4.95
  3.00 0.87 0.83 0.92 0.86 0.76 0.73 0.85 0.81 0.84 0.94 0.81 3.00 0.91 0.87 0.92 0.93 4.05 2.70 0.80 0.79 2.91 0.88 0.70 0.79 0.82 0.81 0.88 0.69 3.72
  2.51 0.75 0.84 0.82 0.86 0.84 0.79 0.93 0.88 0.83 0.86 0.96 0.83 0.80 0.80 0.77 0.80 2.42 0.88 0.70 0.93 0.87 0.82 0.81 1.03 3.28 2.69 0.81 0.78 1.87
  3.06 2.54 0.86 0.99 0.77 0.85 0.77 0.85 0.77 0.81 0.94 0.76 0.81 0.74 1.72 0.93 0.73 1.02 0.90 3.59 2.85 0.81 0.78 1.95 0.80 0.78 0.81 0.83 0.85 0.78
  5.08 2.91 0.95 0.74 1.95 0.76 0.82 0.79 0.79 0.76 0.76 0.88 3.74 2.73 1.25 0.85 0.85 3.59 2.83 0.81 0.78 2.36 0.88 0.88 0.74 0.80 0.70 0.66 0.89 4.07
  2.58 0.77 0.71 0.85 3.27 2.90 0.84 0.90 0.72 0.86 0.76 0.79 0.74 0.81 2.31 0.95 3.45 2.60 0.83 0.88 0.87 2.35 0.75 0.80 0.84 0.86 0.79 4.62 2.99 0.83
  0.95 0.85 1.76 0.87 0.73 0.92 0.92 0.98 4.11 2.89 0.92 0.90 0.81 0.91 0.84 0.77 0.94 0.85 0.74 0.74 0.77 0.77 0.76 0.76 1.74 1.08 3.33 2.73 0.93 0.77
  0.90 0.73 0.96 2.60 0.79 0.78 1.06 3.31 2.67 0.88 0.77 0.89 0.87 0.86 0.90 0.93 0.95 0.94 0.85 0.94 0.93 0.85 0.97 0.79 0.81 0.93 0.99 1.40 5.56 9.60
  2.54 1.34 6.12 1.40 1.34 8.37 6.79 6.76 1.41 5.40 1.30 1.27 1.74 1.82 20.59 8.53 3.95 0.85 1.04 0.90 0.77 0.87 2.83 3.26 0.78 0.90 0.76 1.47 0.80 0.90
  0.88 0.88 0.67 4.54 2.86 0.75 0.84 0.85 1.36 0.78 0.85 0.77 0.76 0.84 0.77 1.01 3.47 0.75 3.40 0.87 1.98 0.84 0.74 0.78 0.79 0.89 0.79 3.85 2.84 0.74
B internal late reports (render only vs deadline): 0/300
B dropped_frames_total delta: 0 (before=0 after=0)
B rung after load: Nominal
ok
test scratch_p9_readback_cost ... adapter: AMD Radeon Pro 555X
view target: 1920x1080
A render_frame ms: n=300 mean=1.878ms min=1.370ms p50=1.753ms p95=2.637ms max=3.142ms
A render_frame ms per-frame ms (all 300):
  1.55 1.82 1.53 1.49 1.55 1.37 2.31 1.59 1.68 1.54 1.64 2.02 1.56 1.57 1.48 1.68 1.64 2.08 1.81 2.13 1.63 2.14 2.20 1.56 1.49 1.54 1.87 1.50 1.77 2.52
  2.02 1.73 1.50 1.83 2.85 1.88 1.55 2.39 1.74 1.57 1.43 2.53 1.82 1.51 2.45 1.80 1.56 1.81 2.33 1.62 1.50 2.00 2.14 1.67 1.75 2.58 1.73 1.58 1.45 2.40
  2.64 1.61 1.68 2.01 2.51 1.79 2.07 1.70 1.82 1.96 2.38 1.91 1.59 2.62 1.92 1.55 1.64 3.14 1.94 1.97 2.71 1.64 1.51 2.12 2.32 1.68 1.52 2.27 2.01 2.90
  1.52 2.36 1.61 1.71 1.87 2.80 1.58 1.57 2.05 1.65 1.96 1.40 2.14 1.70 1.71 1.92 2.14 1.72 1.75 2.36 2.71 1.85 2.74 1.52 1.62 2.23 1.80 1.68 1.55 2.53
  1.81 1.47 1.52 2.19 1.60 1.69 1.82 2.12 2.05 2.14 1.75 1.71 2.07 1.49 2.12 2.50 1.58 2.20 1.60 1.70 1.66 2.04 1.56 1.97 1.70 3.14 1.80 2.81 2.25 1.87
  1.75 2.28 1.63 1.77 2.35 2.45 2.22 1.77 2.30 1.68 1.88 1.88 2.08 1.89 1.72 2.00 1.97 1.56 1.72 2.05 1.66 1.67 1.75 2.54 1.51 1.57 1.47 1.66 2.37 1.74
  3.01 1.51 1.73 1.56 2.31 1.59 1.45 2.36 2.06 1.53 1.59 1.53 2.35 1.71 1.52 1.54 2.63 1.60 1.85 1.66 1.64 2.20 1.64 1.68 1.93 1.91 1.51 1.42 1.72 2.09
  1.59 1.53 1.99 1.43 1.41 1.70 1.62 2.01 1.45 2.08 1.62 1.62 1.56 2.03 1.50 1.76 1.62 1.77 1.50 1.40 1.60 2.52 1.69 1.56 1.55 2.24 1.80 2.05 1.93 1.68
  1.67 1.69 1.86 1.76 1.55 1.61 2.04 2.20 1.51 1.86 1.67 1.56 1.65 2.29 1.85 1.60 2.83 1.72 2.26 1.93 1.86 1.53 1.54 2.07 1.68 2.23 1.85 1.68 1.58 1.86
  1.88 1.91 2.75 1.53 1.49 1.65 2.20 1.95 2.39 1.59 1.53 2.09 2.63 2.03 2.07 1.69 1.65 2.62 2.68 1.85 1.50 2.01 2.05 2.92 1.58 2.14 1.54 1.56 1.59 2.09
A readback_view ms: n=300 mean=12.127ms min=9.179ms p50=11.407ms p95=17.001ms max=23.062ms
A readback_view ms per-frame ms (all 300):
  9.79 10.61 14.69 9.40 9.31 9.81 10.59 11.52 9.67 11.66 11.31 12.55 9.34 9.44 9.55 10.87 11.04 9.81 16.89 14.45 10.86 10.74 11.60 9.23 13.71 9.86 11.39 10.93 10.01 16.92
  11.27 12.09 9.56 13.19 11.18 14.40 13.48 12.69 10.70 9.57 11.70 11.11 16.80 12.04 11.46 11.40 11.17 13.41 13.75 9.39 10.29 11.78 11.34 11.98 12.64 11.64 10.75 9.29 16.58 10.78
  10.58 9.18 10.93 11.02 14.11 10.65 11.39 10.88 12.21 17.83 11.02 14.27 11.96 11.38 12.62 11.07 12.96 10.30 13.81 17.65 10.51 12.60 9.57 11.64 10.87 10.95 10.37 13.29 10.23 11.85
  11.10 10.49 11.25 11.41 11.97 14.49 14.17 10.66 12.84 12.05 9.95 13.74 9.43 11.10 12.64 11.69 13.22 11.32 19.21 10.71 17.22 13.33 14.48 10.35 13.21 11.57 12.24 11.70 12.33 12.96
  10.72 9.44 10.91 9.77 12.23 10.31 13.57 10.65 11.69 12.72 12.75 11.76 9.90 15.49 13.49 13.26 11.12 10.87 11.62 11.81 13.71 9.75 10.08 11.32 12.85 14.17 10.06 15.81 10.87 11.73
  12.47 13.39 12.54 10.48 12.96 14.74 10.91 13.86 10.95 10.74 11.00 16.74 10.47 11.00 12.09 11.23 11.03 9.29 9.91 13.70 12.26 14.41 11.23 15.15 9.79 9.29 12.28 11.55 16.94 10.42
  9.83 9.19 9.48 11.16 11.44 11.06 11.74 11.82 11.53 9.49 9.33 11.15 10.81 10.79 10.96 11.79 10.07 10.21 11.23 13.31 11.14 11.23 11.57 10.77 10.50 10.41 9.18 10.71 10.23 10.52
  9.31 11.35 19.46 14.28 22.19 16.05 22.71 13.46 23.06 13.32 11.43 9.37 16.85 9.80 9.39 10.79 13.89 9.72 9.55 9.55 15.27 10.63 10.78 9.41 11.29 15.05 10.96 10.09 13.03 11.50
  11.15 11.41 10.67 10.92 11.45 17.96 11.99 13.68 10.53 10.40 10.98 11.39 18.20 12.00 15.39 12.70 14.19 10.30 10.62 11.37 11.82 16.47 11.03 21.91 13.10 16.44 17.00 10.80 17.42 12.75
  14.63 13.97 14.15 11.87 18.38 11.48 16.97 13.60 22.10 9.64 13.13 11.66 9.70 12.00 14.33 11.23 11.53 13.51 11.23 11.57 9.61 13.03 10.58 12.35 10.99 10.72 9.60 13.12 13.55 12.80
A render+readback ms: n=300 mean=14.005ms min=10.690ms p50=13.246ms p95=19.311ms max=24.514ms
A render+readback ms per-frame ms (all 300):
  11.33 12.43 16.22 10.88 10.86 11.18 12.90 13.12 11.35 13.20 12.95 14.57 10.90 11.01 11.02 12.55 12.68 11.90 18.71 16.58 12.48 12.87 13.80 10.79 15.20 11.40 13.25 12.43 11.78 19.44
  13.29 13.82 11.05 15.01 14.03 16.28 15.04 15.07 12.44 11.15 13.13 13.64 18.61 13.54 13.90 13.19 12.73 15.22 16.08 11.01 11.78 13.78 13.49 13.66 14.40 14.22 12.48 10.87 18.03 13.18
  13.22 10.79 12.61 13.03 16.61 12.44 13.47 12.58 14.04 19.79 13.40 16.18 13.55 14.00 14.53 12.62 14.60 13.44 15.75 19.62 13.23 14.24 11.09 13.76 13.20 12.63 11.89 15.55 12.24 14.75
  12.61 12.85 12.86 13.11 13.84 17.29 15.75 12.22 14.89 13.70 11.91 15.14 11.57 12.79 14.35 13.60 15.36 13.04 20.96 13.07 19.93 15.18 17.22 11.87 14.83 13.79 14.04 13.39 13.88 15.50
  12.54 10.91 12.43 11.96 13.83 12.00 15.39 12.78 13.74 14.86 14.51 13.48 11.97 16.98 15.61 15.76 12.70 13.07 13.22 13.51 15.36 11.79 11.64 13.29 14.55 17.32 11.85 18.62 13.12 13.60
  14.22 15.67 14.17 12.25 15.31 17.19 13.13 15.63 13.25 12.43 12.88 18.62 12.55 12.89 13.81 13.23 13.00 10.85 11.63 15.75 13.92 16.08 12.98 17.69 11.30 10.86 13.75 13.21 19.31 12.16
  12.85 10.70 11.21 12.72 13.75 12.65 13.19 14.18 13.59 11.03 10.92 12.67 13.16 12.50 12.48 13.33 12.70 11.80 13.08 14.97 12.77 13.43 13.21 12.45 12.43 12.32 10.69 12.12 11.95 12.61
  10.90 12.88 21.46 15.71 23.61 17.75 24.34 15.46 24.51 15.39 13.05 10.98 18.41 11.83 10.89 12.56 15.51 11.49 11.05 10.95 16.87 13.15 12.47 10.96 12.83 17.29 12.76 12.13 14.96 13.18
  12.82 13.09 12.53 12.68 13.00 19.57 14.03 15.88 12.05 12.26 12.65 12.95 19.85 14.28 17.24 14.30 17.02 12.03 12.87 13.30 13.68 18.00 12.56 23.97 14.78 18.67 18.85 12.49 19.00 14.61
  16.52 15.88 16.89 13.41 19.87 13.13 19.17 15.54 24.48 11.24 14.67 13.75 12.33 14.03 16.40 12.92 13.18 16.13 13.91 13.42 11.11 15.03 12.62 15.28 12.56 12.86 11.14 14.69 15.14 14.90
A internal late reports (render only vs deadline): 0/300
A dropped_frames_total delta: 0 (before=0 after=0)
A frames where render+readback > 33.333ms budget: 0/300
A rung after load: Nominal
ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.68s
```

An earlier harness run (same code, terminal-truncated B section) corroborates A:
render mean 1.915 / p50 1.795 / p95 2.628 / max 3.457 ms;
readback mean 11.780 / min 8.871 / p50 11.440 / p95 15.752 / max 21.502 ms;
total mean 13.694 / max 24.156 ms; over-budget 0/300; drops delta 0.

Readback throughput: 1920x1080x4 = 8,294,400 B/frame / 12.127 ms ≈ 684 MB/s
effective (staging copy + map + CPU repack, `gpu.rs:117-174`).

## Decision table (1080p30 budget = 33.333 ms)

| Path | Cost (300 frames, normative) | Budget headroom | Drops (render-loop accounting) | Verdict |
|---|---|---|---|---|
| A. CPU readback in-loop (render + `readback_view`) | mean 14.0 ms, p50 13.2 ms, p95 19.3 ms, max 24.5 ms; 0/300 over budget | ~14 ms at p95, ~8.8 ms at max | `dropped_frames_total` +0; rung Nominal (note: internal accounting sees render-only ~1.9 ms, so it reports 0 late by construction — the over-budget count above is the honest signal) | FITS at 1080p30 on this scene |
| B. GPU-side copy (copy + submit + poll-Wait, blocking upper bound) | mean 1.58 ms, p50 0.88 ms, p95 4.54 ms, max 20.6 ms | ~29 ms at p95 | +0; rung Nominal | FITS with large margin; true share path costs less (no stall) |

## Recommendation: defer zero-copy, with numbers

Fits-with-readback, so per the document rule: **defer zero-copy, land the
readback first cut**. Numbers: synchronous readback consumes mean 12.1 ms of
the 33.3 ms frame budget at 1080p30 on the normative machine, worst observed
24.5 ms end-to-end, zero over-budget frames in 600 measured.

What would change the answer (measure in 09 before building on this):

1. Dress-content composite: harness scene is solid fills (~1.9 ms composite).
   Re-run phase A against dress_show frames (video rings + overlays + ticker);
   if composite + readback p95 crosses ~28 ms, land zero-copy.
2. Encoder + mux + audio-tap load attached: unmeasured here. If the record-step
   rehearsal shows drops or p95 total over budget with the encoder fed, land
   zero-copy.
3. Resolution bump: bytes scale linearly; 4K readback ≈ 4x ≈ 48 ms mean, which
   exceeds the budget alone — any 4K target requires zero-copy, no re-measure
   needed.
4. Sustained variance: readback max 23 ms and copy max 20.6 ms show system
   jitter; if long-run (10k+ frames) p99 exceeds budget, move the tap off the
   render thread or land zero-copy.

Caveats: harness ran in debug/test profile (GPU-bound portion unaffected;
CPU-side floor only); readback measured outside `render_frame`, so current
`dropped_frames_total` accounting is blind to it — 09 must put the tap inside
the deadline or account it separately, or the telemetry will report healthy
while the loop is late.

No encoder, writer, or zero-copy code was implemented. No `src/` changes;
scratch harness deleted; nothing committed.

## Appendix A — dress-content composite, release profile (2026-09-15, fork condition #1)

Scratch harness `crates/nbe-engine/tests/scratch_p9_dress_measure.rs`
(deleted after; pattern from `prompt07_overlay.rs:450 render_engine()`):
dress_show loaded, show started, A1 (real video + AAC rings) taken on air at
1920x1080, `cargo test --release`, 30 warmup frames excluded, 300 measured.
Complete output:

```
dress render_frame ms: n=300 mean=0.217 min=0.164 p50=0.205 p95=0.318 max=0.449
dress readback_view ms: n=300 mean=12.893 min=8.206 p50=13.606 p95=19.023 max=23.705
dress render+readback ms: n=300 mean=13.110 min=8.382 p50=13.819 p95=19.221 max=23.907
test result: ok. 1 passed
```

dress_show declares no overlays, so no ticker ran here; the ticker path needs
no per-frame layout (glyphs rasterize once, scroll is a texture offset —
measured under 07b), and readback cost is resolution-bound, hence
content-independent. Composite with real rings costs 0.2 ms in release
against readback's ~13 ms: content moves the total by under 2 ms.

## Appendix B — record-span pressure on the normative machine (fork condition #2)

Step 11 now snapshots the wire-visible counters into `timings.json`
(`recordSpan`) and asserts no View-drop delta across the span where budgets
mean something (skipped on CI). `record_tap_ms` / `skipped_record_frames`
live in `EngineState` only — NOT on the §10.1 tick — so record-path pressure
is invisible to operators today (recorded finding; v0.5 candidate).

Three consecutive rehearsal runs, 2026-09-15, normative machine:

```
run 1: # tests 15 / # pass 15 / # fail 0 — recordSpan {"droppedFramesTotal": 0, "audioUnderrunsTotal": 0, "endDroppedFramesTotal": 0, "endAudioUnderrunsTotal": 0}
run 2: # tests 15 / # pass 15 / # fail 0 — recordSpan {"droppedFramesTotal": 0, "audioUnderrunsTotal": 0, "endDroppedFramesTotal": 0, "endAudioUnderrunsTotal": 0}
run 3: # tests 15 / # pass 15 / # fail 0 — recordSpan {"droppedFramesTotal": 0, "audioUnderrunsTotal": 0, "endDroppedFramesTotal": 0, "endAudioUnderrunsTotal": 0}
```

## Fork status, stated

CONFIRMED as measured. Release composite+record p95 19.2 ms / max 23.9 ms
against the ~28 ms criterion with zero View drops across three recorded
takes: the readback first cut stands on its own numbers, and zero-copy stays
deferred with the four trip-wires from the decision table above still armed.

## Appendix C — mid-mix composite on dress content, release (2026-09-18, Step 2)

Heaviest normal frame under Step-1 semantics: a MIX take landing mid-mix, so
the underlay path composites 3 layers (2 frozen + 1 fresh) instead of the
plain blend. Scratch harness
`crates/nbe-engine/tests/scratch_step2_midmix.rs` (deleted after; helpers
copied from `prompt04_midmix.rs`): dress_show loaded via the real `show.load`
path, clock started, A1 on air, real `view.take` directives drive both mixes
through `DirectiveHandler::on_take` (no hand-built Transition state — the
interrupt arms exactly what production arms): A1→A2 over 600 frames,
interrupted ~5 frames in by →A3 over 600, so all measured frames sit inside
the 3-layer window. A3 (cadence clip) chosen as the interrupting target so
all three layers are video — the honest heaviest on this package.
`cargo test --release`, 30 warmup frames excluded, 300 measured, deadline
`None` (same blind-accounting caveat as 0b/Appendix A: the honest
over-budget signal is render+readback vs 33.333 ms). Complete output:

```
underlay layers: A1@1.000 A2@0.007 + fresh A3 ramping; S1=1 S2=6
midmix render_frame ms: n=300 mean=0.418ms min=0.285ms p50=0.403ms p95=0.555ms max=0.703ms
midmix readback_view ms: n=300 mean=11.703ms min=8.282ms p50=10.010ms p95=19.229ms max=23.156ms
midmix render+readback ms: n=300 mean=12.121ms min=8.626ms p50=10.433ms p95=19.634ms max=23.593ms
midmix layers S2/S2+299/end: 3/3/3
midmix dropped_frames_total delta: 0 (deadline None — blind by construction)
midmix frames where render+readback > 33.333ms budget: 0/300
test result: ok. 1 passed
```

Against Appendix A (plain on-air composite, release): render mean
0.217→0.418 ms / p95 0.318→0.555 ms — two extra fullscreen layers cost
~0.2 ms mean; the total stays readback-dominated (p95 19.221→19.634 ms,
max 23.907→23.593 ms). verdict: mid-mix composite p95 19.6 ms sits
~8.4 ms inside the ~28 ms criterion, 0/300 over budget — no fork-reopening,
the readback first cut still stands.

## Appendix D — record-through-mix span on the normative machine (Step 2)

Step 11 now drives a real 15-frame A1→A2 mix mid-recording (both video:
clip + loop), waits it out (1.5 s > 0.5 s blend + a tick), then cuts back
to A1 so step 13's on-air assumption is undisturbed — the mix and the cut
both sit inside the spanStart→spanEnd window. `timings.json` `recordSpan`
gains the mix-local wire deltas (`mixDroppedBefore/After`,
`mixUnderrunsBefore/After`); the new no-View-drop assert mirrors the
existing whole-span assert's normative-only gating (CI skips). Re-verified
Step 2 against `telemetry.rs`'s `build_tick_for_dir`: `record_tap_ms` /
`skipped_record_frames` still EngineState-only, still absent from the §10.1
tick — the reachability gap stands (v0.5 candidate); no wire fields added,
no schema edits, no engine behavior change.

Machine is the normative one (i7-9750H, same silicon as the §0 baseline).
Three consecutive rehearsal runs, 2026-09-18 (extension adds no new test —
the file still holds 15):

```
run 1: # tests 15 / # pass 15 / # fail 0 — recordSpan {"droppedFramesTotal": 0, "audioUnderrunsTotal": 0, "endDroppedFramesTotal": 0, "endAudioUnderrunsTotal": 0, "mixDroppedBefore": 0, "mixDroppedAfter": 0, "mixUnderrunsBefore": 0, "mixUnderrunsAfter": 0}
run 2: # tests 15 / # pass 15 / # fail 0 — recordSpan {"droppedFramesTotal": 0, "audioUnderrunsTotal": 0, "endDroppedFramesTotal": 0, "endAudioUnderrunsTotal": 0, "mixDroppedBefore": 0, "mixDroppedAfter": 0, "mixUnderrunsBefore": 0, "mixUnderrunsAfter": 0}
run 3: # tests 15 / # pass 15 / # fail 0 — recordSpan {"droppedFramesTotal": 0, "audioUnderrunsTotal": 0, "endDroppedFramesTotal": 0, "endAudioUnderrunsTotal": 0, "mixDroppedBefore": 0, "mixDroppedAfter": 0, "mixUnderrunsBefore": 0, "mixUnderrunsAfter": 0}
```

Zero View drops across the whole span AND across the blend itself, zero
audio underruns throughout, on all three runs: the OBS scar (transitions
skipping while recording) does not reproduce on the normative machine for
a 15-frame mix under record load. Fork stays closed on this leg.
