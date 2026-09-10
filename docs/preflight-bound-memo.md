# Work order PREFLIGHT-BOUND — diagnosis and decision memo

Status: measurement record and decision memo. **No production code changed by this work order.** The redesign is ratified by a later prompt.

Branch `preflight-bound` off `origin/main` @ `f626c9a`. **Prompt defect, stated per instruction:** the order says "off origin/main (post-#11)". PR #11 is **open, not merged** — `origin/main` is at `f626c9a` (PR #10). This branch is therefore pre-#11 and does not contain #11's docs. Proceeded on the tree.

Machine: Intel Core i7-9750H @ 2.60 GHz, 6 physical / 12 logical, 16 GiB — matches `docs/hardware-baseline.txt` (`MacBookPro15,1`, 6-Core Intel Core i7, 2.6 GHz, 16 GB). This is the §0.3 normative reference target.

---

## 0. Reproduction, and the machinery under test

### 0.1 Seven release runs, observed order, as-found disk

Disk as found: `1.1Gi avail, 100% capacity` (233Gi volume). Unchanged across the set.

```
  run 1: 38.8 s   exit 0
  run 2: 35.6 s   exit 0
  run 3: 67.1 s   exit 0
  run 4: 43.2 s   exit 0
  run 5: 36.1 s   exit 0
  run 6: 33.5 s   exit 0
  run 7: 59.0 s   exit 0
```

Range 33.5–67.1 s, mean 44.8 s. Every run `exit 0`, `"airReady": true`.

**The applied bound is 67,500 ms. Run 3 finished 0.4 s under it — 0.6% margin.** No run was killed today, so the audit's "roughly two in seven" is **not reproduced at that rate**; what is reproduced is the near-miss regime. The audit's two worst samples (81.7, 106.3 s) were its *first two*, taken immediately after a heavy `cargo test`/`cargo build` session; today's set followed no concurrent work. That difference is itself evidence about the mechanism — see §2.

### 0.2 The derivation, quoted

`packages/control-plane/src/package.ts`:

```ts
const MS_PER_FRAME_RELEASE = 25;
const MS_PER_FRAME_DEBUG   = 200;
const PREFLIGHT_SAFETY_FACTOR = 3;
export const PREFLIGHT_FLOOR_MS   = 60_000;
export const PREFLIGHT_CEILING_MS = 3_600_000;

const framesMs = frames * msPerFrame * PREFLIGHT_SAFETY_FACTOR;
const bytesMs  = (bytes / (1024 * 1024)) * msPerMb * PREFLIGHT_SAFETY_FACTOR;
const derivedMs = Math.round(Math.max(PREFLIGHT_FLOOR_MS, framesMs, bytesMs));
```

For `tests/fixtures/valid_show_v0.3`:

```
  frames=900  bytes=87064 (0.08 MiB)
  basis=frames msPerFrame=25 msPerMb=80000 derivedMs=67500 appliedMs=67500
  framesMs=67500  bytesMs=19927
```

So the bound is **900 declared frames × 25 ms × 3 = 67,500 ms**, and the package it bounds is **one 85 KiB `A1.mp4`** plus a 57-byte placeholder PNG. A 67.5-second budget to inspect 85 kilobytes.

### 0.3 The enforcement path, quoted

```ts
const bound = preflightBound(packagePath);
const { stdout, stderr } = await execFileP(preflightBin(), args, {
  cwd: process.cwd(),
  timeout: bound.ms,
  // SIGTERM can be ignored; a wedged process must not survive its bound.
  killSignal: "SIGKILL",
});
```
On the timeout branch: `exitCode: 124`, `timedOut: true`, and `report: timedOut ? null : readReport(...)` — no verdict. `show.load` then fails.

### 0.4 The hook the bound serves

The ceiling's own doc comment states it: the control plane executes a connection's commands strictly in arrival order, so the bound is *"also how long one load may leave that channel answering nothing."* Without a ceiling, a 100 MB package with no declared durations derived 20.8 h on debug, 6.7 h on release; a 1.8 GB bulletin put it in days. **The bound exists to protect the command channel from an unbounded stall, not to police decoder throughput.** Hold that distinction — §4 turns on it.

---

## 1. Where the time goes

Scratch instrumentation at the phase boundaries of `run()` plus a timer around the single decode call. Reverted before this memo; tree confirmed clean and rebuilt (§2a rule 3), gates re-run green.

| Run | manifest read+parse | schema validate | **probe_asset** | 17.5 + 12.4 + 12.11 + 7.10 | wall |
|---|---:|---:|---:|---:|---:|
| 1 | 1 ms | 25 ms | **57,982 ms** | 29 ms | 60.9 s |
| 2 | 1 ms | 26 ms | **60,332 ms** | 30 ms | 60.8 s |
| 3 | 4 ms | 19 ms | **40,136 ms** | 25 ms | 40.6 s |
| 4 | 0 ms | 24 ms | **61,678 ms** | 26 ms | 62.3 s |

Complete marker output, run 4:

```
PHASE          manifest read+parse       0 ms
PHASE              schema validate      24 ms
PHASE                  probe_asset   61702 ms  (asset A1_clip)
PHASE    asset loop (incl. decode)   61728 ms
PHASE     17.5 contradictory items   61728 ms
PHASE               12.4 frame cap   61728 ms
PHASE        12.11 resource demand   61728 ms
TOTAL wall: 62.3 s
```

**One call is 99.9% of the runtime, and it carries all of the variance.** Every other phase totals under 60 ms and is stable to a few milliseconds. Schema validation, the §17.5 contradiction check, the §12.4 cap, the §12.11 resource arithmetic and the §7.10 overlay checks are collectively free.

### 1.1 Why one call on 85 KiB costs 40–62 s

`nbe_decode::probe_asset` calls `session.decode_all(limit)`, which is:

```rust
pub fn decode_all(&mut self, limit: usize) -> Result<Vec<DecodedFrame>, DecodeError> {
    let mut out = Vec::new();
    while out.len() < limit {
        match self.next_frame()? { Some(f) => out.push(f), None => break }
    }
    Ok(out)
}
```

and `DecodedFrame` carries `pub rgba: Vec<u8>` — "RGBA8, tightly packed". The asset is **1920×1080, 900 frames**. So:

```
900 frames × 1920 × 1080 × 4 B = 7,464,960,000 B = 6.95 GiB of RGBA retained
```

What `probe_asset` then computes from those 900 frames: width, height, `has_alpha` (**from `frames[0]` only**), a PTS-delta series for the CFR test, and `frames.len()`. **Only the first frame's pixels are ever read.** The other 899 framebuffers exist to supply a timestamp and be counted.

Measured, one run, `/usr/bin/time -l`:

```
        66.56 real        25.51 user        13.24 sys
   4024733696  maximum resident set size
      1844054  page reclaims
            6  page faults
            0  swaps
```

Peak RSS **3.75 GiB**; 1.84 million page reclaims; and `real 66.56` against `user + sys = 38.75` — **~28 seconds of that run were not CPU at all.**

### 1.2 Pricing each layer

Two scratch experiments, each reverted:

1. **Stream instead of accumulate** — keep `frames[0]`, keep the PTS series, drop every other framebuffer.
2. **Stream, and skip the pixel copy** — the `rgba` swizzle loop bypassed, to price it.

| Configuration | probe_asset | wall | peak RSS | user | sys |
|---|---:|---:|---:|---:|---:|
| **as shipped** (accumulate 900 + swizzle) | 40,136–61,678 ms | 40.6–66.6 s | **3.75 GiB** | 25.5 s | 13.2 s |
| streamed (retain frame 0) | 22,215–25,618 ms | 22.3–28.0 s | **82.7 MiB** | 17.1 s | 1.6 s |
| streamed, swizzle skipped | 5,281–6,772 ms | 5.4–8.6 s | **19.5 MiB** | 2.1 s | 1.1 s |

Streaming alone: **46× less memory**, wall roughly halved, `sys` time from 13.2 s to 1.6 s, and the run-to-run spread from 27 s to 5.7 s.

So the ~48 s mean decomposes as:

| Layer | Cost | Necessary? |
|---|---:|---|
| VideoToolbox demux + decode | **~5.5 s** | yes — this is the real work |
| BGRA→RGBA swizzle, 900 frames | **~17 s** | **no** — only frame 0's pixels are read |
| Memory-pressure stall from retaining 6.95 GiB | **~25 s** | **no** — nothing reads those frames |

**Roughly 88% of the runtime is avoidable overhead, and it is also where the variance lives.**

### 1.3 The swizzle, and why it is 17 seconds

`next_frame` copies each frame with a scalar per-pixel loop:

```rust
for row in 0..height as usize {
    for col in 0..width as usize {
        let s = src.add(row * stride + col * 4);
        let d = (row * width as usize + col) * 4;
        rgba[d] = *s.add(2); rgba[d+1] = *s.add(1); rgba[d+2] = *s; rgba[d+3] = *s.add(3);
    }
}
```

1920 × 1080 = 2,073,600 iterations per frame; **× 900 frames = 1.87 billion iterations**, each with four bounds-checked `Vec` index writes. For a probe that reads one frame's pixels.

### 1.4 A limit that does not limit

`DECODE_FRAME_LIMIT = 100_000` reads as a safety valve. In memory terms it is not one: 100,000 × 8.29 MB = **829 GB**. A one-hour 1080p asset is 108,000 frames, so the cap engages — and the capped run still attempts 829 GB of retained RGBA. The limit bounds the frame *count* and says nothing about the resource that actually runs out.

---

## 2. Where the variance comes from

Protocol: as-found disk first, then the controlled comparisons, with the memory covariates captured on every run rather than inferred.

### 2.1 As-found disk, back-to-back

`1.4Gi avail, 100%` (a little was freed by the scratch rebuilds).

| Run | real | user | sys | user+sys | **non-CPU wall** | peak RSS | page reclaims |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | 58.64 s | 25.91 | 13.22 | 39.13 | **19.5 s** | 3.53 GiB | 1,841,222 |
| 2 | 37.65 s | 21.11 | 11.73 | 32.84 | **4.8 s** | 3.94 GiB | 1,841,754 |
| 3 | 48.17 s | 21.77 | 11.84 | 33.61 | **14.6 s** | 4.35 GiB | 1,841,759 |
| 4 | 38.33 s | 20.79 | 11.36 | 32.15 | **6.2 s** | 4.70 GiB | 1,841,756 |

CPU cost is stable within ±10% (32.2–39.1 s). Page reclaims are effectively constant. **The variance is entirely in non-CPU wall time — a 4× spread, 4.8 s to 19.5 s.** Peak RSS climbs monotonically across back-to-back runs (3.53 → 3.94 → 4.35 → 4.70 GiB): each run meets a different memory landscape left by its predecessor, and pays a different stall for the same work.

### 2.2 Disk headroom — a limitation, and a negative result

**I could not test ≥20% headroom.** The volume is 233 GiB with 1.1–1.4 GiB free; reaching 20% would require freeing ~47 GiB of the user's data, which is not mine to delete. Reporting the constraint rather than a substitute measurement.

The evidence nonetheless says **disk is not the mechanism**: every run reports `0 swaps`, and pressure appears as page reclaims and rising RSS — the macOS memory compressor, not swap-out to disk. The audit's 99%-full disk was a co-occurring condition, not a cause. This also revises the audit's framing that "freeing disk is a diagnostic": there is nothing to diagnose there.

### 2.3 File cache

Not a candidate, and no experiment needed to exclude it: the inputs are an 85 KiB video, a 57-byte PNG and a 13.5 MB binary. Nothing in that set can produce a 20-second swing, and cold-vs-warm cannot explain a spread that persists across four consecutive runs of identical inputs. What *does* carry between runs is resident memory state (§2.1), which is the opposite of a cold-cache effect.

### 2.4 Conclusion

The variance is **memory-pressure stall against a ~4 GiB allocation burst on a 16 GiB machine**, and it is a property of the retention defect, not of the machine's disk or its caches. Remove the retention and the spread collapses from 27 s to 5.7 s (§1.2). **A bound cannot be tuned around this; the allocation has to stop.**

---

## 3. The debug "hang" — root cause, and a correction to my own finding

### 3.1 It is not a hang

The debug binary reaches `schema validate` at 198 ms and never emits the asset-loop marker. Watched across 220 seconds:

```
  t(s)  stat  %cpu   rss(MiB)
    10s  RN    66.7   389
    40s  RN    76.7   742
    70s  RN    98.3   1507
   100s  RN    98.8   2492
   130s  RN    98.6   3160
   160s  RN    98.3   3145
   190s  RN    99.8   3172
   220s  RN    98.9   3254
```

**Running at 66–99% CPU throughout, RSS climbing to 3.25 GiB.** It never sleeps.

`sample(1)`, 3 seconds, 226 samples — all in one stack:

```
main.rs:753 → main.rs:273 → nbe_decode::probe_asset → DecodeSession::decode_all
  → DecodeSession::next_frame
      core::iter::range::RangeIteratorImpl::spec_next
      alloc::vec::Vec<u8>::index_mut → core::slice::index::SliceIndex::index_mut
      core::num::unchecked_add::precondition_check
```

**Root cause: the same 1.87-billion-iteration swizzle of §1.3, unoptimized.** In debug, each `rgba[d] = …` is a real bounds-checked `IndexMut` call and the range iterator is not inlined, so the loop runs orders of magnitude slower — on top of the same 6.95 GiB retention. No lock, no channel, no fd, no EOF frame-wait, no undrained pipe. It is work, and there is far too much of it.

Release shares the identical code path. It does not "hang" there because optimisation collapses the swizzle to roughly `17 s` and inlines the bounds checks away — the same defect, 8× cheaper, which is why release lands at 40–67 s instead of past 220.

### 3.2 The correction

The backlog record on `main` (`docs/prompt-map-07-13.md`) states the debug binary "**blocks**", citing "state `SN` at 0.0% CPU having accumulated 0:00.02 of CPU time", and concludes "**That is a wait, not work**". **That conclusion is wrong, and the measurement behind it was mine.**

What happened: the probe used `pgrep -f "target/debug/nbe-preflight" | head -1`. That pattern matches **two** processes, and `head -1` takes the wrapper:

```
88387 timeout 25 target/debug/nbe-preflight --package-path tests/fixtures/valid_show_v0.3
88389 target/debug/nbe-preflight --package-path tests/fixtures/valid_show_v0.3

88387 SN     0.0   0:00.01 timeout
88389 RN    98.6   0:11.82 target/debug/nbe-preflight
```

`SN`, `0.0%`, `0:00.01` is the **`timeout` process**, sleeping as designed. The worker beside it was at 98.6% CPU. I sampled the wrapper and reported the wrapper's idleness as the subject's. The 180-second `exit 124` was real; the inference about *why* was not.

The record is corrected in the same commit as this memo, with the original claim left visible per §2c.

---

## 4. Decision memo

### 4.1 What the kill prevents

`show.load` without a bound. The command channel is strictly ordered, so an unbounded preflight is an unbounded stall on every subsequent command from that connection. The measured worst cases the ceiling comment records — 6.7 h release, 20.8 h debug, days for a 1.8 GB bulletin — are not hypothetical: they are what a package with no declared durations produced. **Removing the bound is not on the table.**

### 4.2 What the kill causes

`show.load` refusing a valid, air-ready package non-deterministically, with `report: null` — no verdict, so the operator cannot even see why. The margin on the repository's own reference fixture was **0.4 s in seven runs today**, and the audit measured two overruns in its seven under concurrent load. CI cannot see any of it: the fixture gates run `cargo run -p nbe-preflight` directly, where no bound exists.

### 4.3 Is 67.5 s even the right derivation?

**No — it is derived from the wrong quantity.** The derivation models cost as decode throughput: `frames × ms_per_frame`. The measured cost is dominated by *retention*: `frames × width × height × 4` bytes of RGBA held for no reason (§1.1–1.2). Those scale differently, and the fixture shows how differently — 85 KiB of input, 6.95 GiB of retention.

`MS_PER_FRAME_RELEASE = 25` is not a bad measurement of decode; §1.2 puts genuine decode at ~6 ms/frame and the swizzle at ~19 ms/frame, so 25 ms/frame describes the shipped `next_frame` accurately. What the constant cannot describe is the stall, because the stall is not per-frame — it is a function of total resident bytes against available RAM. A 900-frame 4K package would retain 27.8 GiB and stall enormously while deriving only the same 67.5 s. **The derivation is dimensionally wrong, and it is coincidence that it lands near the observed range for this one fixture.**

### 4.4 Options

**Option A — Fix the measured pathology; then re-derive.**
Stream the probe (retain frame 0 and the PTS series), and skip the pixel copy for frames the probe never reads.
- *Protective property:* unchanged — the bound stays exactly as it is, and every existing refusal path is untouched.
- *Flakiness exposure:* measured. Streaming alone takes the fixture to 22.3–28.0 s against a 67,500 ms bound — from 0.6% margin to roughly 2.4×, with the spread down from 27 s to 5.7 s. Skipping the swizzle takes it to 5.4–8.6 s, an 8× margin.
- *Cost:* localized to `nbe-decode`. `decode_all` has other callers (short-loop load), so the change is a new streaming probe path rather than an edit to `decode_all` itself.
- *Risk:* the CFR test and `has_alpha` must produce byte-identical verdicts. The scratch experiment preserved both by keeping the PTS series and frame 0; `cargo test --workspace` stayed at 190/0 throughout.

**Option B — Machine-calibrated bound.**
Calibrate on first run and store: decode a known fixture, measure ms/frame and available RAM, derive the bound from the local numbers.
- *Calibration protocol:* on install or first `show.load`, run the probe against a committed calibration asset of known frame count and resolution; record observed ms/frame and peak RSS per frame; derive `bound = frames × observed_ms_per_frame × safety`, and refuse *before spawning* when `frames × w × h × 4` exceeds a fraction of free RAM.
- *Protective property:* the channel stays bounded, and the bound tracks the actual machine rather than one reference machine.
- *Flakiness exposure:* better than today but not solved — calibration measures a moment, and §2.1 shows the same machine varying 4× in stall depending on what ran before. A calibrated bound still has to carry a large safety factor to survive a loaded machine.
- *Verdict:* worth having **after** A, and cheap once A removes the memory term. On its own it calibrates against a pathology.

**Option C — Stall detector: kill on no-progress, with a generous wall cap.**
Preflight emits progress (frames probed, asset index) on a pipe; the control plane kills only when progress stops for N seconds, with a wall cap far above any legitimate run.
- *Protective property:* strictly stronger than a wall bound for the failure the bound actually exists to prevent. A wedged process is detected in N seconds regardless of package size; a slow-but-working run is never killed.
- *Flakiness exposure:* the lowest of the four, because it keys on "no verdict is coming" — the ceiling comment's own words — rather than on elapsed time.
- *Cost:* the highest. Needs a progress protocol, a reader on the control-plane side, and care that the pipe cannot itself wedge. It also changes the preflight binary's output contract, which §19.2 governs.

**Option D — Bound as warning.**
Let preflight run; log and surface a warning when it exceeds the derived estimate; never kill.
- *Protective property:* **none for the failure that matters.** It reintroduces the unbounded channel stall the ceiling was written to prevent, and the ceiling comment already records what that cost. Listed for completeness; recommend against.

### 4.5 Should CI fixture gates exercise the bound path?

**Yes — on its own merits, independent of everything above.**

The argument for: a defect that fails the product's primary entry point on the repository's own reference fixture survived four independent review passes and reached `main`, because every gate that touches preflight invokes the binary directly. The gates prove the *binary* works. Nothing proves `show.load` works. That is a hole in the shape of the finding.

The argument against, honestly stated: `runPreflight` in CI would import a real timeout into the test suite, and a gate that can time out is a gate that can flake. On a GitHub runner — slower and noisier than this machine — a fixture at 40–67 s against 67.5 s would flake immediately.

Which is why the sequencing matters: **land Option A first, then add the gate.** After A the fixture runs in 5–9 s against a 67,500 ms bound, and a gate asserting "`show.load` on `valid_show_v0.3` returns a verdict, `timedOut: false`" is both meaningful and stable. Adding the gate before A would encode the flake instead of catching it.

### 4.6 Recommendation

**Option A, then B, with C recorded as the eventual shape and D rejected.**

1. **A first, and A alone is likely sufficient.** It is the only option that addresses what the measurements actually found. It needs no spec change, no protocol change and no new contract; it removes ~88% of the runtime and ~79% of the variance; and it leaves every refusal path exactly as reviewed. The bound question largely dissolves: the fixture goes from 0.6% margin to 8×.
2. **Then re-derive the constants against the fixed probe** — `MS_PER_FRAME_RELEASE = 25` describes the shipped `next_frame`, not the streaming one, so it will overstate by roughly 4× once A lands. Re-measure rather than scale it.
3. **Then B**, cheaply, because after A the bound is a decode-throughput estimate and decode throughput is what calibration can actually measure.
4. **Then the CI gate through `runPreflight`.**
5. **C when the progress protocol is worth its cost** — most likely when §19.2's report schema is next revised, since that is when preflight's output contract is open anyway.

**The evidence that carries it:** the bound is not too tight for the work; the work is roughly nine times larger than it needs to be, and the excess is where every millisecond of variance lives. `probe_asset` retains 6.95 GiB of RGBA to read one frame's pixels, spends 1.87 billion scalar iterations converting frames nothing reads, and stalls 4.8–19.5 s per run against the resulting memory pressure. Fix that and the bound is comfortable, the variance collapses, the debug binary stops appearing to hang, and CI can finally gate the path operators use.

---

## 5. CORRECTION ENTRY — the redesign landed (work order PREFLIGHT-BOUND-2, 2026-09-10)

Everything above is left as written. §4.3's finding — that the derivation is
**dimensionally wrong**, modelling `frames × ms_per_frame` while the measured
cost was dominated by `frames × w × h × 4` bytes of retention — is the reason
this section exists, and it stays visible per Standards §2c rather than being
edited into hindsight.

**Option A landed.** `probe_asset` now retains only the one frame whose pixels a
check reads and decodes the rest for their timestamps alone; `next_frame_meta`
runs every validation and skips only the copy.

Measured on the same machine, same fixture:

| | before | after |
|---|---:|---:|
| `probe_asset` | 23,137–29,377 ms | **4,059–5,052 ms** |
| wall | 23.2–31.2 s | **4.10–5.10 s** |
| peak RSS | 4.43–4.69 GiB | **28.5–28.7 MB** |
| user / sys | 14.5–18.4 / 7.6–9.4 s | **0.75–0.94 / 0.73–0.93 s** |
| run-to-run spread | 8.0 s | **1.00 s** |

**The constants were re-derived, and the old ones were not wrong — they were
measurements of the defect.** 25 ms/frame release and 200 ms/frame debug
described the retaining path accurately. Post-fix: release 4.51–5.61 ms/frame
(12 runs), debug 4.04–15.0 ms/frame (11 runs). `MS_PER_FRAME_RELEASE` 25 → 8,
`MS_PER_FRAME_DEBUG` 200 → 25.

The reference fixture is now **floor-bound**: `max(60,000, 900×8×3 = 21,600,
19,927)` = 60,000 ms, giving **11.9×** headroom against its worst run. Neither
per-frame term reaches the floor for a package this small, which is the honest
outcome — the floor's own rationale (spawn, schema validation, asset hashing) is
what a 900-frame package actually spends its bound on.

**§3's root cause is confirmed by the fix.** The debug binary no longer appears
to hang: 4.9–13.5 s `exit 0`, peak RSS 32.9 MB, against `exit 124` after 180 s
with RSS climbing past 3.2 GiB before. It was the swizzle and the retention, as
§3 concluded — and *not* the wait that §3.2 corrected the record for claiming.

**§4.5's CI recommendation was followed, and its precondition was measured
rather than assumed.** A diagnostic step timed the streaming probe on the
runner: `macos-14` is **arm64, 3 cpus** — Apple Silicon, not the Intel machine
§0.3 makes normative — at **2,933–3,390 ms**, spread 457 ms, **17.7× under** the
bound. That cleared the memo's own test ("5–9 s territory, not 0.6% margins"), so
one fixture gate now runs through `runPreflight`, failing if the bound kills a
valid package, if no verdict returns, or if a verdict arrives having used more
than half its bound. The diagnostic step stays, so the margin remains visible if
a runner change erodes it.

**What §4.6 recommended and this work order did not do:** Option B
(machine-calibrated bound) and Option C (stall detector) are untouched and
remain queued in that order. Neither is urgent now — the fixture's margin went
from 0.6% to 1,090%.

**One new observation for the record.** The CI runner being arm64 while §0.3
makes an Intel MacBook Pro normative means every constant in this repository
measured "on CI" describes different hardware from the reference target. Nothing
currently depends on that, but the s/MB and ms/frame families were all measured
on the Intel machine, and a future contributor timing something on a runner will
get a systematically different number. Worth a line in the deferral ledger.

---

## Appendix — measurement hygiene

- Every number above traces to a run pasted in this document or in the report accompanying it; no figure is scaled, averaged from memory, or carried from a prior session.
- All scratch instrumentation (phase markers in `nbe-preflight/src/main.rs`; streaming and swizzle-skip experiments in `nbe-decode/src/lib.rs`) was reverted from file backups taken before each edit. Tree confirmed clean (`git status --porcelain` empty), rebuilt from the restored tree, and re-verified: `cargo fmt --all -- --check` exit 0, `clippy --workspace --all-targets -D warnings` clean, `cargo test --workspace` **190 passed, 0 failed**.
- Background processes from the hang investigation were killed and confirmed gone (`pgrep -c -f nbe-preflight` → 0) before proceeding.
- One negative result recorded rather than dropped: ≥20% disk headroom could not be tested, and the reason it does not matter is given in §2.2 rather than assumed.
