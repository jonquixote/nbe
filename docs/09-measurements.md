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

---

# ZERO-COPY Phase 1 — the spike, and what it measured (2026-09-19)

## Outcome: the chain works. Every link, on the normative machine.

wgpu View texture → IOSurface-backed `MTLTexture` → `CVPixelBuffer` →
VideoToolbox, with no CPU readback at any point. Run on the reference machine
per `docs/hardware-baseline.txt` — Intel i7-9750H, and wgpu's
`HighPerformance` preference selected the **AMD Radeon Pro 555X**, which matters:
the IOSurface-backed texture is created on the *same* `MTLDevice` wgpu renders
with, so the dual-GPU hazard this machine presents (Intel UHD 630 alongside the
Radeon) never arises.

```
== link 1: IOSurfaceCreate 1920x1080 BGRA
   ok: IOSurface created, id 117
== link 2: wgpu Metal device -> MTLTexture backed by that IOSurface
   adapter: AMD Radeon Pro 555X
   ok: MTLTexture from IOSurface
== link 3: import that MTLTexture into wgpu
   ok: wgpu::Texture imported
== link 4: render into it through wgpu
   ok: rendered a clear into the IOSurface-backed texture
== link 5: same IOSurface -> CVPixelBuffer -> VideoToolbox encode
   ok: CVPixelBuffer wraps the IOSurface (no copy)
   ok: encoded 30 IOSurface frames with NO readback -> 24 units (30 total, 2143 bytes)
== CHAIN COMPLETE
```

**What made it possible, recorded because it was the open question.** wgpu 30.0.1
exposes `Device::as_hal::<Metal>()` and `create_texture_from_hal`, and
`wgpu_hal::metal::Device` has a public `raw_device()` returning the
`MTLDevice` — without that accessor the texture would have to be created on
`MTLCreateSystemDefaultDevice()`, which on this dual-GPU machine is not
necessarily the device wgpu chose. `objc2-metal 0.3.2` is already in the lock via
wgpu-hal, the same 0.3.x line as `nbe-decode`'s existing objc2 dependencies, so
the `Retained<ProtocolObject<dyn MTLTexture>>` types unify rather than colliding
across versions.

**One architectural fact the spike settles.** The workspace denies `unsafe_code`
with a single exemption — `crates/nbe-decode`. Every link above is `unsafe` FFI,
so a production zero-copy tap lives in `nbe-decode` (which today has no wgpu
dependency) or in a new crate carrying the same exemption. It cannot live in
`nbe-engine` without changing that lint, which is a portability decision, not an
implementation detail.

## Measurements — 300 frames, render + encode submit, quiescent

The span timed is the whole tap: render one frame into the shared surface, then
hand the *same* surface to the encoder. No readback anywhere inside it.

| Path | Geometry | Load (1m) | mean | min | p50 | p95 | max | over 33.333 ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **Zero-copy** | 1080p30 | 2.60 | **4.033** | 1.977 | 3.574 | **6.044** | 26.052 | **0 / 300** |
| **Zero-copy** | 4K (3840×2160) | 2.32 | **13.092** | 6.000 | 13.095 | **17.582** | 51.720 | 1 / 300 |
| CPU readback (published, §0b above) | 1080p30 | — | 12.1 | — | 13.2 | 19.3 | 24.5 | 0 / 600 |

**At 1080p30 zero-copy costs about a third of readback** — mean 4.0 against 12.1,
p95 6.0 against 19.3. Its `max` is marginally worse (26.1 against 24.5), a single
outlier rather than a trend, and it is stated rather than smoothed.

**At 4K the comparison is not close, and this is the trip-wire answer.** The fork
decision listed four conditions that would force zero-copy; the third was "any 4K
target requires zero-copy, no re-measure needed", because readback alone scales
linearly to ≈48 ms and exceeds the 33.333 ms budget by itself. Zero-copy renders
*and encodes* 4K at p95 **17.6 ms** — inside the budget with room. That condition
is now answered with a measurement rather than an extrapolation.

**Discipline notes, because the numbers are only as good as their conditions.**
The first 4K run was taken at load 4.66 and is **VOID**, not data — the soak
protocol's word, applied to itself. It is recorded here because a discarded run
that goes unmentioned is indistinguishable from one that never happened: it read
mean 13.104 / p95 15.155, within noise of the quiescent re-run at 13.092 / 17.582,
so nothing turned on it. The 1080p run at load 2.60 and the 4K re-run at 2.32 are
both under the 3.0 ceiling.

## Status: Phase 1 only

The spike is deleted, as its phase required — it lived in
`crates/nbe-decode/examples/` with temporary dev-dependencies and one temporary
`encode_pixel_buffer_spike` method, all reverted. **Nothing in this revision
changes the product.** The record path still runs CPU readback; §0.1 assumption
24's scoped allowance still stands unretired; Phases 2 and 3 are not done.

What the next phase now has that it did not: a proven chain, the accessor that
makes it reachable, the crate boundary it must respect, and both numbers the
selection table needs.


---

# ZERO-COPY Phase 2 — the tap, the table, the candidate (2026-09-19)

## The crate boundary, decided and costed

**The tap lives in `crates/nbe-decode`, with wgpu as an OPTIONAL dependency
behind a `gpu-tap` feature.**

It was not a free choice. The workspace denies `unsafe_code` with exactly one
exemption, and a CI gate hard-codes it: `grep -rn "allow(unsafe_code)" crates
… | grep -v "^crates/nbe-decode/"` fails the job on any hit. Every link of the
chain is Objective-C FFI. So the options were this crate, or a new crate that
would need the gate amended — and amending the exemption policy is the kind of
portability decision this work order reserves for the user. `nbe-engine` was
never available without a lint change, for the same reason.

**The cost, and why it is paid with a feature flag.** `nbe-preflight` also
depends on `nbe-decode` and never touches a GPU. An unconditional wgpu
dependency would pull the entire GPU stack into a 13.5 MB CLI binary that CI
already size-gates. Measured after the change:

```
distinct wgpu-family packages in nbe-preflight: 0
distinct wgpu-family packages in nbe-engine:    8
```

~~*wgpu nodes in nbe-engine's dependency tree: 12*~~ **CORRECTED 2026-09-19
(§2c).** Two numbers were reported for the same quantity — 12, then 13 — and
neither was the quantity. `cargo tree | grep -c wgpu` counts **lines**, and a
package appears on as many lines as it has paths into the graph; the count also
moved when a `pollster` dev-dependency landed between the two readings. Derived
three ways at the fix round:

| Method | nbe-engine | nbe-preflight |
|---|---:|---:|
| `cargo tree \| grep -c wgpu` (lines) | 13 | 0 |
| `cargo tree --prefix none`, unique first field | 13 | 0 |
| `cargo metadata`, **distinct packages** | **8** | **0** |

Eight is the number that means what the sentence claims. The discrepancy stays
recorded rather than quietly replaced: the measurement discipline says a count
derived one way is a count nobody checked, and this is the counter-example that
earned the rule its second instance — the first was an awk field separator that
made a failure count read zero forever.

The load-bearing half was never in doubt: **preflight is 0 by every method.**

Preflight's surface is unchanged. `nbe-engine` turns the feature on; nothing
else does.

## The published selection table

Capability × resolution × consumer → path. **This table is the product
decision**; `record::tap_path::select` is only its evaluator. A runtime dial
was rejected: it would make every deployment's frame path an operational
accident, and the one thing the §0.1 assumption 24 allowance cannot survive is
a path nobody can predict.

| zero-copy capable | height | consumer | path | reason |
|---|---|---|---|---|
| no | any | record | `CpuReadback` | `ProbeUnavailable` |
| no | any | **stream** *(future, unbuilt)* | **none — refused** | no lawful path |
| yes | ≤ 1080 | record | `ZeroCopy` | `Table` |
| yes | > 1080 | record | `ZeroCopy` | `Table` |
| yes | any | **stream** *(future, unbuilt)* | `ZeroCopy` | `Table` |

The two capable record rows are identical in outcome and listed separately on
purpose: their *reasons differ in strength*. At ≤ 1080 either path fits the
budget and zero-copy is chosen because it is three times cheaper (4.033 ms mean
against 12.1). Above 1080 readback does not fit at all — 4K readback is ~48 ms
against a 33.333 ms budget — so that row is a requirement, not a preference.
Collapsing them would hide which is which.

**The stream rows are the point of the consumer axis.** Streaming is Prompt 10,
next in the queue, and it is not built. Its incapable row is **refused**, not
`CpuReadback`: §0.1 assumption 24 forbids readback for outputs, and v0.4.2's
allowance is recording-only in as many words — *"Streaming (Prompt 10) inherits
no allowance from this row."* `select_stream` returns `None` there. Prompt 10
arrives to a decision already made rather than finding the tap defaulted to the
path it is forbidden from.

**The override is an escape hatch, not a feature.** It exists for the day the
probe is wrong — a machine where the chain links but produces garbage. It can
*restrict* (force CPU) but never *conjure*: an override to `ZeroCopy` on a
machine whose probe failed is refused and reports `ProbeUnavailable`. An
override that becomes routine means the table is wrong, and the fix is the
table.

## The choice is telemetry-visible

`record_tap_path` and `record_tap_reason` join the §10.1 tick, additive.
Which path is live is **operational state**: an operator who cannot see it
cannot tell a machine that chose zero-copy from one that silently fell back to
the allowance.

**They are always emitted, stubbed `"none"` before any take has selected a
path.** Superseded text kept per §2c:

> Absent until a take selects a path — absence means "no take yet", not a
> defaulted guess, which is why the fields are `Option` rather than defaulted
> strings.

That was a §10.1.1 violation — *"an absent field and a stubbed field are
different failures and only one of them is diagnosable"* — caught by the Phase
3a design memo (Q1′) and fixed in Phase 3b before the migration, since it is a
defect in merged code independent of it. The distinction the old shape wanted is
kept in full: `"none"` is not `"cpuReadback"`, so a machine that never recorded
is still distinguishable from one that fell back.

**Recorded as a §10.1 wire-addition candidate, unratified** — the same shape
`intentSource` took before v0.4.1 ratified it.

*Status, 2026-09-21: RATIFIED as v0.4.4. The fields are §10.1's, normative, with
a note in that section and ownership assigned to the render node in §10.1.1.
The sentence above is Phase 2's, kept as written.*

## Falsifications

| Mutation | Result |
|---|---|
| Suppress the telemetry report of the chosen path | `a_failed_probe_selects_cpu_readback_and_the_fallback_reaches_telemetry` FAILED — *"the fallback path must be visible on the wire"* |
| Omit the field from the wire (`#[serde(skip_serializing)]`, Phase 3b) | `the_tap_fields_are_always_on_the_wire_and_stub_before_any_take_selects` FAILED — *"§10.1.1: `recordTapPath` must be present on every tick"* |
| Swallow the probe's geometry refusal | Metal aborts the process: `MTLTextureDescriptor has width of zero`, SIGABRT. The guard converts a hard abort into a typed `E_NO_ZEROCOPY` refusal |
| Remove the override guard so a pin beats the table | `the_table_drives_the_choice_and_an_override_cannot_conjure_a_capability` FAILED — *"an override to ZeroCopy on an incapable machine must not be honoured"* |

Every test drives a real entry point — `zerocopy::probe`, `tap_path::select*`,
`telemetry::build_tick` — never a hand-written state effect (§2a rule 7).

## One defect found in the building, recorded because it was mine

The first telemetry wiring read `state.record_tap_selection.lock()` **twice
inside one struct literal**. Struct-literal temporaries live until the end of
the enclosing statement, so the first `MutexGuard` was still held when the
second `lock()` ran — a deadlock on a non-reentrant `Mutex`. It surfaced as
`pump_tick_wires_the_loaded_record_dir` **hanging** rather than failing, which
is the worse failure mode: a hang has no assertion message. The lock is hoisted
and read once, with the reason at the site.

A second process note: `git checkout --` restored neither `tap_path.rs` nor
`zerocopy.rs` after their falsifications, because both were **untracked**. It
did revert the tracked telemetry work alongside. §2a rule 3's trap in a shape
the rule does not yet name — new files need a manual copy, not a checkout.

## Clean feed: not built, not precluded

`docs/v0.5-outline.md` §3a's clean-feed row wants
`outputs.record.source: "program" | "clean"`, where `clean` composites the scene
level only. The selection design does not preclude it: `Consumer` is an axis, not
a boolean, and a clean feed is a *second consumer of a second surface* — another
`SharedSurface` built by the same `probe`, selected by the same table with a new
consumer row. Nothing in the path selection assumes one surface per engine. That
is the whole claim; no clean-feed code exists.

## Status: Phase 2 ships no migration

The record path still runs CPU readback. `probe` is reachable and the table is
evaluable, but **nothing in the production record path calls them yet** — the
work order places migration in Phase 3 and says this PR ships none. What exists
now: the tap, the table, the telemetry, the falsifications, and the crate
boundary with its cost measured.


## PR #24's two-key pass — three findings, closed (2026-09-19)

**F1 (HIGH) — the mirror agreement was vacuous, and the field would have broken
the control plane.** `rust_and_typescript_agree_on_the_engine_telemetry_fields`
collects the **serialized key set** and checks each key against the TypeScript
schema. The new fields are `skip_serializing_if = "Option::is_none"`, and the
fixture sampled them as `None` — so they serialized to nothing, the loop never
checked them, and the suite read 16/16 while TypeScript knew nothing about them.

That was not merely a coverage gap. The TS schema is `.strict()`:

```
without the new fields: ACCEPTED
WITH recordTapPath:     REJECTED — unrecognized_keys ["recordTapPath","recordTapReason"]
```

The first tick Phase 3 populated would have been rejected **whole** — not the
field dropped, the entire §10.1 tick refused, every tick.

Closed three ways, because parsing is not visibility: the TS schema gained both
fields as `.optional()` (never `.default()` — absent means "no take yet", and a
default would make a machine that never recorded look like one that fell back);
the agreement fixture samples them with values so the key-set check actually
checks them; and `buildTick` now **forwards** them to the operator tick, which
the first version did not — the frame parsed at the boundary and the field died
there. Guarded by `an engineTelemetry tick carrying recordTapPath parses and the
field is readable`.

Falsified both directions: remove the fields from the TS schema →
`TypeScript engineTelemetry has no 'recordTapPath' field`; remove
`rename_all = "camelCase"` from the Rust struct →
`TypeScript engineTelemetry has no 'audio_drift_ms' field`.

**This is the fourth instance of one shape** — a floor green about what it
cannot see. The 09 floors counted `passed` while capability-gated tests skipped;
two overlay guards named a take path they never entered; a wall-clock bound
measured the machine instead of the code; and now an agreement audit checked a
key set that omitted the keys. The lineage is worth naming because the fix is
always the same: make the check see the thing it claims to check.

**F2 (MED) — the suite shipped with no gate.** `zerocopy_tap` appeared nowhere
in `ci.yml`, so all six tests reported `ok` on CI and nothing said whether the
two GPU-dependent ones had run or skipped. It now carries the same two-floor
shape as the 09 suites (`ran >= 6`, `exercised >= 4`, where four tests run on any
runner) plus an observational block that greps `^SKIP` and prints what went
unexercised. Locally: `ran=6 skipped=0 exercised=6`.

**F3 (LOW-MED) — the soak row described the plan in the present tense.** It said
the path choice is "recorded every soak", but `scripts/soak.sh` is unchanged,
nothing in production calls `select()`, and the field is absent from every tick
by design until Phase 3. The row now says so, and names the migration as what
backs it.


---

# ZERO-COPY Phase 3b — the migration, measured (2026-09-20)

## Outcome: recording runs the zero-copy path, and the allowance is a fact

`record.start` probes the chain at the take's geometry, asks the published
table, publishes the selection to the §10.1 tick, and builds the take's surface
pool. The frame path and telemetry's claim are now the same statement.

## The numbers

Reference machine per `docs/hardware-baseline.txt`; wgpu's `HighPerformance`
preference selected the **AMD Radeon Pro 555X**, as in Phase 1. Quiescent —
load(1m) **2.23 at start, 2.79 at end**, both under the soak protocol's 3.0
ceiling, no `cargo`/`rustc` running at launch. 300 frames per row, unpaced
(pacing to the frame boundary measures the pacing). Harness:
`crates/nbe-engine/tests/zerocopy_bench.rs`, `#[ignore]`d so it is run
deliberately rather than by `cargo test --workspace`.

**Two spans, reported separately because they answer different questions.**
`tap` is what the record path costs and what lands on `record_tap_ms`: the
readback plus handoff on the CPU path, the acquire plus retarget plus handoff on
the zero-copy path. `frame` is render + tap, the whole per-frame cost against
the 33.333 ms budget, and is the span comparable to Phase 1's table.

### 1080p30, before and after

| span | n | mean | min | p50 | p95 | max | over 33.333 ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| cpuReadback tap | 300 | 14.832 | 13.733 | 14.873 | 15.709 | 25.158 | 0 / 300 |
| **zeroCopy tap** | 300 | **0.008** | 0.007 | 0.008 | 0.009 | 0.029 | 0 / 300 |
| cpuReadback frame (render+tap) | 300 | 15.866 | 14.635 | 15.856 | 16.787 | 28.224 | 0 / 300 |
| **zeroCopy frame (render+tap)** | 300 | **1.376** | 0.972 | 1.362 | 1.575 | 1.954 | 0 / 300 |

**Read the tap row carefully, because the obvious reading is wrong.** 0.008 ms
is not "encoding became free". It is the cost of a `try_send` and nothing else:
the draw already happened inside `render_frame` (into the surface rather than
into the View target — the retarget replaces a texture reference, so it adds no
pass and no copy, **at about +0.3 ms per frame** against the native RGBA
target), and the encode happens on the record thread, where it always did. What
the 14.8 ms was, and no longer is, is a **readback the loop awaited** — 8.3 MiB
copied out of VRAM per frame, on the record counter, every frame of every take.

**The +0.3 ms, stated rather than smoothed.** Superseded wording kept per §2c:
the passage above read *"at no extra cost"*, which the numbers do not support.
Subtracting the tap span from the frame span leaves the render-only residual,
and it is larger on the zero-copy side on both independent runs:

| run | cpuReadback render | zeroCopy render | delta |
|---|---:|---:|---:|
| this revision | 1.034 | 1.368 | **+0.334** |
| two-key pass (independent, quiescent) | 1.048 | 1.291 | **+0.243** |

Drawing into a BGRA IOSurface-backed texture costs ~0.25–0.33 ms per frame more
than drawing into the engine's own RGBA target. Structurally the claim holds —
no additional pass, no copy — but "no extra cost" was a structural statement
wearing a measurement's clothes. It is swamped by the ~15 ms the readback cost,
and saying so is not the same as saying it is zero.

The honest headline is the frame row: **15.866 ms → 1.376 ms** for render plus
tap, a factor of 11.5.

### 4K, the trip-wire row

| span | n | mean | min | p50 | p95 | max | over 33.333 ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| zeroCopy render+encode | 300 | 12.433 | 6.021 | 11.087 | 19.525 | 59.303 | 2 / 300 |

Against Phase 1's 13.092 / p95 17.582 / 1 over 300 — the same measurement,
within its own spread. **This row does not go through `RenderLoop`**, and that
is stated rather than implied: the engine's View is fixed at `VIEW_W x VIEW_H`,
so the 4K run renders into a 4K surface with wgpu directly and hands the same
surface to a 4K encoder. Same chain, one link short of the production loop.

**The over-budget count here is a TAIL STATISTIC, not a fixed fact**, and the
table above reads as though it were one. Three independent runs of the same
measurement on the same machine:

| run | mean | p50 | p95 | max | over 33.333 ms |
|---|---:|---:|---:|---:|---:|
| Phase 1 | 13.092 | 13.095 | 17.582 | 51.720 | **1 / 300** |
| this revision | 12.433 | 11.087 | 19.525 | 59.303 | **2 / 300** |
| two-key pass | 12.550 | 11.306 | 19.785 | 61.257 | **4 / 300** |

The count wanders — 1, 2, 4 — while mean, p50 and p95 barely move. It counts
the far tail of an unpaced run, where a handful of samples out of 300 decide
the number. **Anyone watching this row should watch p95, not the count.** A
count that moved while p95 moved with it would be a finding; a count that moves
alone is the tail breathing.

### Reproduced independently, by the two-key pass

The pass over PR #26 re-ran the same `#[ignore]`d harness on the same machine,
quiescent — load(1m) **2.32 at start, 2.90 at end**, 0 `cargo`/`rustc`, 9 GiB
free, `target/` 7.9G, adapter **AMD Radeon Pro 555X**. Recorded here because a
ratification changelog may cite only what a reader can chase.

| span | PR mean | pass mean | PR p95 | pass p95 | PR over | pass over |
|---|---:|---:|---:|---:|---:|---:|
| cpuReadback tap | 14.832 | **15.191** | 15.709 | **15.763** | 0 / 300 | **0 / 300** |
| zeroCopy tap | 0.008 | **0.008** | 0.009 | **0.010** | 0 / 300 | **0 / 300** |
| cpuReadback frame (render+tap) | 15.866 | **16.239** | 16.787 | **16.862** | 0 / 300 | **0 / 300** |
| zeroCopy frame (render+tap) | 1.376 | **1.299** | 1.575 | **1.497** | 0 / 300 | **0 / 300** |
| 4K zeroCopy render+encode | 12.433 | **12.550** | 19.525 | **19.785** | 2 / 300 | **4 / 300** |

**Means within ~2.5%, p95s within 1.3%.** The one row that moved is the 4K
over-budget count, 2 → 4, and that is the tail statistic the section above
describes rather than a change in the measurement: p50 went 11.087 → 11.306 and
p95 19.525 → 19.785 while the count doubled.

The pass's two-way accounting held on its own run: `frames handed to the drain:
600 (requested 600, pool skips 0)`; the harness's per-frame wall-vs-`record_tap_ms`
assertion (≤ 2 ms) never fired across 300 CPU frames; 4K `295 streamed / 300 at
finish`.

### Counts, derived two independent ways

| quantity | derivation A | derivation B | agree |
|---|---|---|---|
| 1080p frames handed off | 600 requested (2 paths × 300) | 600 received by the drain thread | yes |
| 1080p tap cost per frame | wall clock around the seam | the seam's own `record_tap_ms` return (asserted within 2 ms per frame, in the harness) | yes |
| 4K access units | 295 streamed during encode | 300 reported at `finish()` | yes — finish includes the streamed set plus the in-flight tail |

### A number the migration found on the way

Paced at 30 fps through the production seams, a **cpuReadback take sheds 20 of
40 frames** (`zerocopy_migration::a_machine_with_no_chain_records_by_readback_and_says_so`,
`record_tap_ms` 583.7 for 40 frames ≈ 14.6 ms each — consistent with the table
above). The zero-copy take sheds 0. `prompt09_feed`'s live take never saw this
because it feeds synthetic RGBA and never reads back. Per the gate split the
number belongs to the soak, not to a test threshold.

## Falsifications

| Mutation | Result |
|---|---|
| Omit `recordTapPath` from the wire | `the_tap_fields_are_always_on_the_wire_and_stub_before_any_take_selects` FAILED — *"§10.1.1: `recordTapPath` must be present on every tick"* |
| Drop `set_render_device` from `RenderLoop::new` | `the_render_loop_publishes_its_device_so_the_directive_path_can_probe` FAILED — `left: ProbeUnavailable, right: Table` |
| `SurfacePool::acquire` returns `surfaces.first()` (hand out a busy surface) | `the_pool_never_hands_out_a_surface_that_is_still_in_flight` FAILED — *"acquire kept yielding past the pool's size: it is reusing surfaces"* |
| Remove the geometry check from `encode_pixel_buffer` | `the_encoder_refuses_a_surface_of_the_wrong_shape...` FAILED — and note the failure: VideoToolbox **accepted** a 1280×720 buffer as 1920×1080 and returned no error |
| `render_bus` ignores the retarget | `the_compositor_draws_into_the_loaned_surface...` FAILED — *"2073600 of 2073600 pixels still carry the stain"* |
| One composite pipeline (no BGRA sibling) | wgpu validation error: *"the RenderPass uses textures with formats [Some(Bgra8Unorm)] but the RenderPipeline ... [Some(Rgba8Unorm)]"* |
| Drop the `readback_view` swizzle | FAILED — `left: [0, 128, 255, 255], right: [255, 128, 0, 255]` |
| `record.start` selects but never attaches the pool | **At head:** `a_zero_copy_take_reports_the_table_and_never_reads_back` FAILED — *"retarget: E_NO_ZEROCOPY: the take's surface pool is gone mid-take; the chain was available at record.start and is not now"*. See the note below — the signature changed after the row was first written |
| Never publish the selection | both migration takes FAILED — `left: ("none","none")` |
| Swallow a mid-take chain loss and keep feeding | `losing_the_chain_mid_take...` FAILED — *"losing the chain mid-take must end the take"* |
| Leave the take running after the loss | FAILED — `left: Recording, right: Idle` |
| Clear `record_tap_selection` on the loss | FAILED — `left: ("none","none"), right: ("zeroCopy","Table")` |

**One row's signature changed between step 5 and head, and the change was an
improvement.** Superseded text kept per §2c — the row above originally read:

> `record.start` selects but never attaches the pool | `a_zero_copy_take_reports_the_table_and_never_reads_back` FAILED — telemetry said `zeroCopy` while the frames went through readback

That is what the mutation produced **at step 5** (`4588b41`), verified by
checking that commit out and re-running it there: the take ran 40 frames through
the readback and the test caught it afterwards, on *"THE MIGRATION'S WHOLE
POINT: no frame on this path may await a View readback"*. Step 6 then added
`claims_zero_copy`, which treats "the selection says `zeroCopy` and there is no
pool" as chain loss — so at head the same mutation is refused at **frame 0**,
before a single readback happens, with the `E_NO_ZEROCOPY` token.

The step-5 commit message is accurate at its own commit and is left as written.
The table is written for a reader at head, so it names the signature a reader at
head will see. A guard that grew stronger after its evidence was recorded is a
good problem; leaving the old signature in place so a reader hunts for a failure
that no longer occurs is not.

## The rehearsal names its path, three consecutive times

`[RI-1]` step 11 now waits for a tick whose `recordTapPath` is not `"none"` and
asserts it is one of the published table's paths with a reason attached. Not a
threshold — which path a machine gets is a property of that machine — but the
field must stop reading `"none"` during a take, because `"none"` there means the
selection never reached telemetry and the soak's weekly capture would record
nothing.

| run | load(1m) at start | tests | pass | fail | skipped | path |
|---|---:|---:|---:|---:|---:|---|
| 1 | 3.90 | 15 | 15 | 0 | 0 | zeroCopy (Table) |
| 2 | 2.73 | 15 | 15 | 0 | 0 | zeroCopy (Table) |
| 3 | 2.57 | 15 | 15 | 0 | 0 | zeroCopy (Table) |

Four earlier consecutive green runs are not tabled above because the first was
taken at load 7.46, immediately after a release build; it passed 15/15 and named
the same path, and is stated rather than smoothed. The three above follow the
falsification below, so they are runs of the restored tree.

**Falsification of the step:** remove `record.start`'s publication of the
selection and rebuild the engine the rehearsal actually spawns
(`target/debug/nbe-engine` — the rehearsal uses the debug binary):

```
not ok 13 - [RI-1] step 11: record the running show, mark it, stop cleanly
  error: 'waited 3000 ms for the take names its frame path; last telemetry was
  {... "recordTapPath":"none","recordTapReason":"none", ...
       "recordState":"recording", ...}'
# pass 14  # fail 1  # skipped 0
```

`recordState: "recording"` beside `recordTapPath: "none"` is exactly the
diagnosable pair §10.1.1 asks for: a take is running and the field says no take
has chosen a path. An absent key would have said nothing at all.

## Three things the Phase 3a memo did not reach

Recorded in `docs/zero-copy-p3-design.md` under "Corrections found in
execution", and all three found by a test failing rather than by review: the
retarget's **format** obligation (and the `readback_view` swizzle it implies),
the probe texture's missing `COPY_SRC | COPY_DST`, and that Q2's "the take's
surface for the take's lifetime" is superseded by Q3's pool — the retarget is
per frame.

## On the ledger: the override is built and wired to nothing

`select_with_override` exists in `crates/nbe-engine/src/record/tap_path.rs`, is
covered by its own tests, and honours the rule that matters — an override can
*restrict* (force CPU) but never *conjure* a capability the probe denied.
**Nothing calls it.** There is no config surface carrying an override from a
manifest, an environment variable, or a command, so today the published table is
the only voice: an operator on a machine where the chain links but produces
garbage has no lawful way to say "use readback on this one".

That is a gap in the escape hatch, not in the rule. The table is the product
decision and it is working as designed; what is missing is the documented way
out of it for the day the probe is right about capability and wrong about
quality. Wiring it needs somewhere for the value to live, and inventing a config
surface was not the migration's scope.

**It lands wherever a config surface next appears** — Prompt 10's streaming work
is the likely place, since a second consumer needs per-output settings anyway,
but earlier is fine. This sentence is where that decision is owed; an override
that stays unwired through the next config surface is a choice, and should be
recorded as one rather than left to drift.

**The debt list is one shorter than it was.** The hatch's `"Override"` reason is
a normative §10.1 token as of v0.4.4 and was, until PR #27's corrections,
asserted as a string nowhere — only as an enum variant, while the wire spelling
came from `format!("{:?}")`, so a variant rename would have changed a normative
token silently. `reason_tokens_are_stable` (beside `path_tokens_are_stable` in
`tap_path.rs`) now pins all three reason tokens and checks `"Override"` against
a real `build_tick`, which is the one place the spec's third token meets a tick.
What remains owed is the wiring, not the token.

*Status, 2026-09-21 (second update, after SPEC-REV): **the field now has a
home; the wiring remains owed, to Prompt 10.** SPEC v0.4.5 landed
`outputs.{record,stream}.tapPath: { enum: ["auto", "cpuReadback"], default
"auto" }` — `auto` meaning the published table decides, which is the default
precisely because the table is the product decision. The enum has no `zeroCopy`
value, so "an override may restrict but never conjure" is unrepresentable rather
than merely refused. **Nothing reads the field yet.** `select_with_override`
still has no caller; closing that is a work item of Prompt 10's execution, no
longer a precondition of it.*

*Superseded status, kept per §2c: the Prompt 10 upgrade went looking for the
config surface this sentence points at, and found that **the wiring could not
land in Prompt 10 as the tree then stood.** `OutputDefaults.record` and `OutputDefaults.stream` are both
`additionalProperties: false`, nothing in §9 or §16 mentions a tap-path
override, and the standards make `schemas/*.json` a spec revision rather than
prompt work. So the debt needs a spec word first — a field such as
`outputs.{record,stream}.tapPath: { enum: ["auto", "cpuReadback"] }`, where
`auto` is the table and `cpuReadback` restricts, matching
`select_with_override`'s existing behaviour and its tests. Recorded in
`agents/prompts/10-streaming.md` §3 as blocker B2 and in the prompt map. The
sentence above stands as written; what changed is that we now know which word
unblocks it.* — **that word was given on 2026-09-21 and the field landed; see
the status note above.**

## Status

The record path runs zero-copy where the probe allows it and CPU readback where
it does not, and says which.

**RATIFIED as SPEC v0.4.4 on 2026-09-21**, at merge commit `84dc9c8`, with each
of the five guards run immediately before its marker was flipped. §0.1
assumption 24's rescope and the two §10.1 fields are law together — the rescope
requires the engine to report which path is live, and ratifying that while its
reporting mechanism stayed a draft would have made law of a sentence with no
observable.

Superseded text kept per §2c:

> §0.1 assumption 24's **rescoped candidate (b) remains UNRATIFIED** — this
> revision makes its mechanism a fact in the tree, not law. Ratification is a
> separate word.

The word was given.
