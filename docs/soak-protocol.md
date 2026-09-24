# Soak protocol — the gate for hardware claims

Status: normative for the hardware claims it names. Ratified 2026-09-17 as part
of the gate split (see `docs/prompt-map-07-13.md`, and the comment block on the
`dress-rehearsal` job in `.github/workflows/ci.yml`).

## 0. Why this exists

CI cannot gate recording. The `macos-14` runner has no hardware H.264 encoder,
so 22 of the 60 Prompt 09 tests skip there and the rehearsal's record, sync and
kill steps skip with them — which means **AC-6 has never run in CI**. The same
runner is 3 arm64 cores against a reference machine that is 6-core Intel with
discrete AMD graphics (§0.3), so it cannot meet the zero-drop and zero-underrun
thresholds either: work order DRESS measured 83 then 120 underruns there against
0 on the reference machine.

Two failure modes were available and both are worse than this document:

- **A gate red on purpose.** Work order DRESS settled it: a job that is red
  every run teaches reviewers to ignore red.
- **A gate green about what it cannot see.** The Prompt 09 floors did this until
  PR #18's fix round — they counted `passed`, a capability-gated skip reports
  `ok`, and forcing the encoder absent satisfied all ten floors while exercising
  nothing.

So the split is: **structure gates in CI, thresholds gate here.** CI asserts
what is machine-independent and says out loud what it did not cover. This
protocol owns the rest, on the machine where the numbers mean something.

## 1. What this protocol owns

| Claim | Where it is asserted today |
|---|---|
| Zero View drops across a show | rehearsal gate step (`droppedFramesTotal == 0`) |
| Zero audio underruns across a show | rehearsal gate step (`audioUnderrunsTotal == 0`) |
| No fallback, real quality profile | rehearsal gate step |
| Recording produces bytes that parse | rehearsal step 11 |
| A/V sync inside the file (≤ 20 ms) | rehearsal step 12 |
| **AC-6 — crash-safe recording under `SIGKILL`** | rehearsal step 13 |
| The encoder, fMP4 writer, fragment cadence, AAC tap, on-disk sidecar | `prompt09_*` suites (22 of which skip on CI) |
| **Audio-tap push latency** — worst single `AudioTap::push` over 10k pushes under 1 ms | Nowhere else. Rebound out of the default suite 2026-09-18 (R9): it measured the machine, passing at load 2.58 and failing at load ~5 and ~30 on the same binary. The SPSC contract is now asserted by work in `prompt09_record_file`; this THRESHOLD lives here, where quiescence is checked and a violation is VOID |
| **The record tap's path choice** — `record_tap_path` / `record_tap_reason` from the §10.1 tick | **Live.** `record.start` selects the path and publishes it (ZERO-COPY Phase 3b), the rehearsal asserts the take names one, and `scripts/soak.sh` records the distinct values per iteration into `record-tap-path.txt` and `soak.json`. A silent fallback from `zeroCopy` to `cpuReadback` is the event this catches: the machine still records, the file is still correct, and the only visible difference is a telemetry field nobody was reading. Recording it every soak makes a capability regression a dated event rather than a discovery. **Not a threshold** — which path a machine gets is a property of that machine — but a clean recording iteration whose ticks never name a path FAILS the soak, because that is the field lost, not the capability |
| Flake-register watch list (R7 and successors) | §5 below |
| The v0.5 failover drill | when it exists; this protocol is its home |
| **Stream reconnect is automatic** — transport loss reads `Reconnecting` on the publisher, which redials with no operator action and re-announces its codecs at the stream's current media time; across a kill on the production loop `droppedFramesTotal` is unchanged | `prompt10_rtmp::reconnect_kill_midstream_then_live_again_without_operator_action` (runner-independent — every CI run) and `prompt10_rtmp::transport_death_leaves_the_loop_untouched` (the production loop with a live encoder — here). **Live:** `scripts/soak.sh` records their `RECONNECT:` and `SURVIVAL:` lines per iteration into `stream-evidence.txt` and `soak.json`. **Where the state is:** `Reconnecting` is the publisher's (`PublisherState`, engine-internal). The §10.1 wire carries the control plane's commanded `streamState`, which stays `live` through a redial by design, so **no telemetry consumer sees the redial today**; an engine-owned transport-state field is drafted UNRATIFIED in `docs/v0.5-outline.md` §4. ~~"transport loss surfaces `Reconnecting`"~~ — PR #30's wording, which read as a wire claim it was not (§2c) |
| **Stream drops are counted, never silent** — a stalled stream sheds only its own frames (`skipped_stream_frames`, engine-internal), never record skips, never View drops; `streamBufferMs` is buffered bytes through the stream's envelope bitrate, nonzero under stall, zero drained | `zerocopy_g1::a_stalled_stream_costs_record_nothing` on real GPU surfaces through record's production seams (every CI run — the runner has a chain), `prompt10_rtmp::both_live_stream_shares_the_record_composite` on the production loop (here), and the transport's buffer/tick tests (every CI run). **Live:** `soak.sh` records the `G1 guard:` and `BOTH LIVE:` lines per iteration. ~~"stream drops counted two independent ways"~~ — PR #30's guard counted a model's drops (`SharedPool<()>`), not the pool's (§2c) |
| **The zero-copy pool never hands out a surface VideoToolbox still reads** — record and stream alike: after `encode_pixel_buffer` returns, the buffer stays retained by VideoToolbox (1.6–17.5 ms measured) and the pool withholds it until the retain drops, read after an `Acquire` fence | `zerocopy_g1::the_pool_never_hands_out_a_surface_videotoolbox_still_reads` — needs a hardware encoder, so it **skips on CI and runs only here**. **Live:** `soak.sh` runs `zerocopy_g1` with zero skips and REQUIRES its `VT retain guard:` line each iteration (missing = FAIL), recorded in `soak.json` as `stream.vt_retain_last_iteration`. The guard asserts VideoToolbox was observed holding the buffer (else the run proves nothing) and that the pool handed it out 0 times. The fence's arm64 half cannot be exercised on the x86 normative machine; its proof is the construction argument in `SurfacePool::is_free` |
| **The loop never encodes for the stream** — the stream's share of a tick is a surface loan and a bounded `try_send`; the encoder opens and runs on the stream thread | `docs/09-measurements.md`, Prompt 10 section (the loop's timed region, 300 frames per output config, before and after). **Live:** `soak.sh` records the `LIVE LOOP:` line (worst stream share of a tick) per iteration; a soak whose worst share reads like an encode (milliseconds) is a finding |
| **The engine binary streams during the rehearsal** — `stream.start` on a running show, VideoToolbox H.264 and the show's AAC onto an RTMP socket, `stream.stop` inside the grace window | rehearsal step 9 (`[P10]`). **Live:** `soak.sh` records the rehearsal's `STREAM:` line (video/keyframe/audio message counts, stop time, span drops and underruns); a clean iteration without it FAILS the soak. Interop with a real ingest (MediaMTX) is recorded as `stream.mediamtx` — proved, or skipped because the out-of-band binary is absent — and is **not** a row: nothing here claims it |
Nothing in that table is gated by a green CI run. A reviewer who wants these
claims looks for a soak record, not a checkmark.

## 2. Preconditions — a soak that runs without them is void

A soak is not "run the rehearsal again". These are checked and recorded first,
because a threshold measured under the wrong conditions is worse than no
measurement: it produces a number someone will later cite.

1. **The normative machine.** `docs/hardware-baseline.txt` matches the host —
   6-core Intel i7-9750H, 16 GiB, discrete AMD. Not a VM, not the arm64 runner.
2. **Quiescence.** No concurrent build, test, or index. This is not
   fastidiousness: on 2026-09-17 the rehearsal returned **13/15** when launched
   immediately after a full `cargo test --workspace` plus `clippy`, then
   **15/15 four consecutive times** once the machine was idle. The thresholds
   this protocol owns are exactly the assertions that load perturbs, so the
   protocol measures an idle machine or it measures nothing. The script enforces
   a load-average ceiling and refuses above it.

   "Quiescent" means **no build or test contending for this repo**, not an
   empty process table. The first version of the script refused on any running
   `node`, which on this machine is unsatisfiable — long-lived MCP servers and
   editor tooling hold node processes that never touch the tree, and a
   precondition nobody can satisfy is the same defect as a gate that is always
   red. It now refuses on `cargo`/`rustc` by name, and on node/tsc processes
   only when their command line references this repository.
3. **Disk headroom.** At least **8 GiB** free, and `target/` under the 40 GB
   ceiling (`docs/implementation-standards.md` §4). A full disk truncates
   artifacts, which is how a soak produces a lie rather than a failure — the
   2026-09-14 disk-pressure episode hit ~600 MB free with a 32 GB `target/`.

   The 8 GiB figure is a **calibration, not a measurement**, and it was 20 GiB
   until this machine reported 14 GiB free and voided every run. Same lesson as
   the node check: a precondition the normative machine cannot meet is the same
   defect as a gate that is always red. Artifacts are MB-scale and a release
   rebuild is a few GB, so 8 GiB guards the real failure while staying
   satisfiable. Raise it with evidence if a soak is ever truncated above it.
4. **Release binaries built and verified.** Size-checked per §4's artifact
   hygiene rule — a truncated binary fails as a code defect and costs a session.
5. **Hardware present.** The H.264 and AAC probes both answer yes.
   **A soak where any capability gate fires is VOID, not green.** This is the
   inverse of the CI rule: there, a skip is honest and reported; here, a skip
   means the run did not test the thing it exists to test.

## 3. Cadence

- **Weekly**, unattended or operator-launched.
- **REQUIRED before anything called a release.** A release without a green soak
  on the normative machine has no evidence for any threshold claim in §1.
- Scheduling mechanics — cron, a launchd job, a manual trigger — are the
  operator's choice and deliberately not specified here. What must exist in the
  tree is this protocol and `scripts/soak.sh`.

## 4. Artifacts

The existing rehearsal shape, extended with the pressure counters:

| File | Contents |
|---|---|
| `engine.log` | Engine stdout/stderr for the whole run |
| `telemetry.jsonl` | Every §10.1 tick, one JSON object per line |
| `pushes.jsonl` | Every server-push frame |
| `show-states.json` | The observed `showState` sequence |
| `timings.json` | `timings`, `clockMovedAtMs`, `clipAudibleAtMs`, `recordSpan` |
| `soak.json` | **New.** Preconditions as checked, per-iteration pass/fail, the counter set below, and the head SHA |

Counter set recorded per iteration, all from the §10.1 tick except where noted:

- `droppedFramesTotal`, `audioUnderrunsTotal`, `audioDriftMs`, `masterClockDriftMs`
- `recordSpaceMib`, `decodeSessions`, `vramUsedMib`, `textureCacheUsedMib`
- `degradationRung`, `fallbackActive`
- **pressure counters** (Prompt 09): `record_tap_ms` and `skipped_record_frames`
  deltas across the record span — the record-yields-first degradation order is
  only observable as a nonzero skip count with drops still at zero.
- **the record tap's path** (ZERO-COPY Phase 3b): the distinct `recordTapPath`
  and `recordTapReason` values seen across the iteration's ticks, with counts.
  Distinct values rather than the last one, so a take that changed path mid-soak
  shows as two rows instead of whichever tick happened to be last. Cross-checked
  against the rehearsal's own `RECORD PATH:` line — two independent derivations
  of the same fact, from the ticks and from the step.

Artifacts are diagnostics and never a reason to fail — but a soak with no
artifacts is void, because an unreproducible green is not evidence.

## 5. The flake-register watch list

A flake that stops appearing has not necessarily been fixed; it may have moved
to a machine nobody watches. So every entry in the flake register is asserted
during a soak with its **count** recorded, not merely its pass/fail:

| Entry | Signature | What the soak records |
|---|---|---|
| ~~**R7**~~ **CLOSED 2026-09-18 (second attempt — the first mechanism was wrong)** | `expected 3 directives, got 4` in the control-plane suite — **4** sightings across unrelated changes (2026-09-08; PR #17 run `34745014794`; PR #19 run `35314193753`) — two of them on branches whose diff was docs+CI only, load-sensitive, always green on rerun | Iterations run, iterations where the signature appeared, and the full stderr of any appearance. **Closed:** neither — the test read `directives.at(-1)` for "the seq this command produced", which aliased the previous command's directive whenever the response outran its own directive, so two of three waits could be no-ops and the count fell on a fixed 30 ms sleep. Waits are now by command name and the count is asserted twice around settle windows. Kept on this list for one quarter of soaks as a watch, then removed. The payloads are no longer uncaught: as of the fourth sighting the assertion dumps each directive's `command`, `seq` and `stateVersion`, so the next appearance — in CI or in a soak — names the duplicate |

A quiet return is the thing this list exists to catch. An entry leaves the list
when its root cause is found and falsified, never because it went quiet.

## 6. Pass, fail, void

- **Pass** — every precondition held, every §1 claim asserted, zero skips, all
  thresholds met, artifacts written.
- **Fail** — a precondition held and a claim did not. This is a real finding:
  record it, do not rerun until green.
- **Void** — a precondition did not hold (wrong machine, machine under load,
  disk short, a capability gate fired). A void run proves nothing in either
  direction and must not be recorded as either.

The distinction between *fail* and *void* is the whole point. Conflating them is
how "it passed on rerun" becomes a habit.

## 7. What the split does NOT do

- **No self-hosted runner.** The normative machine is a daily driver; making it
  a CI runner is a separate decision and stays open.
- **No CI gate that pretends to cover hardware it lacks.** CI's observational
  step names what went unexercised on every run, and its exit 0 is not evidence
  about recording.
- **No new thresholds.** This protocol moves where the existing §1 claims are
  gated. It does not invent numbers.
