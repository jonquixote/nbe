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
| **Stream reconnect is automatic** — transport loss reads `Reconnecting` on the publisher, which redials with no operator action and re-announces its codecs at the stream's current media time; across a kill on the production loop `droppedFramesTotal` is unchanged | `prompt10_rtmp::reconnect_kill_midstream_then_live_again_without_operator_action` (runner-independent — every CI run) and `prompt10_rtmp::transport_death_leaves_the_loop_untouched` (the production loop with a live encoder — here). **Live:** `scripts/soak.sh` records their `RECONNECT:` and `SURVIVAL:` lines per iteration into `stream-evidence.txt` and `soak.json`. **Where the state is:** `Reconnecting` is the publisher's (`PublisherState`), and **since v0.4.6 it is on the wire**: the §10.1 tick's engine-owned `streamTransportState` reads `live` / `reconnecting` / `closed` (stub `none` before any stream), beside the control plane's commanded `streamState`, which stays `live` through a redial by design (§9.5). `transport_death_leaves_the_loop_untouched` asserts the tick reads `live` → `reconnecting` → `live` → `closed` across the kill, the return and the stop. **Live:** `scripts/soak.sh` records the distinct `streamTransportState` values with counts per iteration into `stream-transport-state.txt` and `soak.json` (`stream.transport_states_last_iteration`) — the record-tap shape — and a clean streaming iteration whose ticks never read `live` FAILS the soak. ~~The §10.1 wire carries the control plane's commanded `streamState`, which stays `live` through a redial by design, so no telemetry consumer sees the redial today; an engine-owned transport-state field is drafted UNRATIFIED in `docs/v0.5-outline.md` §7~~ (~~§4~~ — the candidates live in §7; corrected after the second-key pass) — struck in v0.4.6, which ratified that field and landed it (§2c). ~~"transport loss surfaces `Reconnecting`"~~ — PR #30's wording, which read as a wire claim it was not (§2c) |
| **Stream drops are counted, never silent** — a stalled stream sheds only its own frames (`skipped_stream_frames`, engine-internal), never record skips, never View drops; `streamBufferMs` is buffered bytes through the stream's envelope bitrate, nonzero under stall, zero drained | `zerocopy_g1::a_stalled_stream_costs_record_nothing` on real GPU surfaces through record's production seams (every CI run — the runner has a chain), `prompt10_rtmp::both_live_stream_shares_the_record_composite` on the production loop (here), and the transport's buffer/tick tests (every CI run). **Live:** `soak.sh` records the `G1 guard:` and `BOTH LIVE:` lines per iteration. ~~"stream drops counted two independent ways"~~ — PR #30's guard counted a model's drops (`SharedPool<()>`), not the pool's (§2c) |
| **The zero-copy pool never hands out a surface VideoToolbox still reads** — record and stream alike: after `encode_pixel_buffer` returns, the buffer stays retained by VideoToolbox (1.6–17.5 ms measured) and the pool withholds it until the retain drops, read after an `Acquire` fence | `zerocopy_g1::the_pool_never_hands_out_a_surface_videotoolbox_still_reads` — needs a hardware encoder, so it **skips on CI and runs only here**. **Live:** `soak.sh` runs `zerocopy_g1` with zero skips and REQUIRES its `VT retain guard:` line each iteration (missing = FAIL), recorded in `soak.json` as `stream.vt_retain_last_iteration`. The guard asserts VideoToolbox was observed holding the buffer (else the run proves nothing) and that the pool handed it out 0 times. The fence's arm64 half cannot be exercised on the x86 normative machine; its proof is the construction argument in `SurfacePool::is_free` |
| **The loop never encodes for the stream** — the stream's share of a tick is a surface loan and a bounded `try_send`; the encoder opens and runs on the stream thread | `docs/09-measurements.md`, Prompt 10 section (the loop's timed region, 300 frames per output config, before and after). **Live:** `soak.sh` records the `LIVE LOOP:` line (worst stream share of a tick) per iteration; a soak whose worst share reads like an encode (milliseconds) is a finding |
| **The engine binary streams during the rehearsal** — `stream.start` on a running show, VideoToolbox H.264 and the show's AAC onto an RTMP socket, `stream.stop` inside the grace window | rehearsal step 9 (`[P10]`). **Live:** `soak.sh` records the rehearsal's `STREAM:` line (video/keyframe/audio message counts, stop time, span drops and underruns); a clean iteration without it FAILS the soak. Interop with a real ingest (MediaMTX) is recorded as `stream.mediamtx` — proved, or skipped because the out-of-band binary is absent — and is **not** a row: nothing here claims it |
| **Automation fires within one frame — AC-25 #1 and #2 on the normative machine.** Every trigger kind's action is dispatched within one frame of the control plane observing its trigger (design note §6 defines "observed" per kind); a hold cancels every pending action within one frame of its acceptance; and an `audioLevel` rule's action is queued within one frame of the engine's crossing, end to end through the release binary and the real socket (B1's acceptance evidence) | `packages/control-plane/src/automation-latency.measure.ts` (Prompt 11 WU7; `npm run measure:automation`). **Not in `npm test`**: a timing claim from a shared runner is noise (R9). The functional claims — which kinds fire, the limiter, the hold, one queue — are asserted every CI run by `automation.test.ts` on a frozen clock; this row owns only the numbers. **Live:** `scripts/soak.sh` runs it every iteration against `target/release/nbe-engine`, keeps the whole output in `automation-latency.log` and its `AUTOMATION:` line in `soak.json` (`automation.latency_last_iteration`). Counts that disagree (issued, audit rows and state, per phase), a span over one frame, or a missing line FAILS the soak; a measurement VOIDed by a mid-run load spike fails the iteration as VOID-ish — a VOID record is no record. `streamHealth`'s engine→tick hop (up to 1 s, design note §5) is outside the span, and says so |
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
6. **Power** (the user's word of 2026-10-06; why, with the evidence, is §2a).
   - **On AC, awake.** The run goes under `caffeinate -i -s`, and the power log
     for its window is checked (`pmset -g log`). **Any `Sleep`, or any
     `Using Batt`, voids the run.**
   - **Recorded with the run:**
     - the adapter's wattage (`ioreg -rn AppleSmartBattery`,
       `AdapterDetails.Watts`) against the machine's rated 87 W;
     - Low Power Mode (`pmset -g custom`, `lowpowermode`);
     - the battery's current through the run (`InstantAmperage`; negative
       means it is discharging while plugged in) and the gauge's `SystemLoad`.
   - **Power-limited, not void.** A run on an adapter below the rating, under
     Low Power Mode, or with any discharge on AC is recorded as
     **power-limited**. It is not normative, but it is not void either: it is a
     datum about the very condition a show machine with its devices attached
     meets. The adapter in use today is 45 W, so until an 87 W adapter is on
     hand, every run on this machine is power-limited, and is recorded so. (The
     lesson of the disk and node preconditions: a precondition the machine
     cannot meet must not void every run.)

## 2a. Power: the budget the show runs on

> **⚠️ Power availability changes how this machine runs, and nbe does not yet
> account for it** (the user's word of 2026-10-06).
>
> A show machine carries audio interfaces, Stream Decks, capture cards, and a
> monitor or teleprompter. Every one of them draws from the same budget: the
> adapter's wattage minus what the machine and its devices consume. When the
> budget runs short, the battery covers the deficit until it can't, and macOS
> throttles to fit. The user met exactly this with OBS: with every device
> plugged in and ready to go, frame-rate and buffering problems appeared.
>
> **Power is a show precondition, not only a test precondition.** Every
> threshold this protocol owns was measured on whatever power the machine
> happened to have: zero drops, zero underruns, the rise times, the dress
> counts. So power is accounted for here in the documents, and it is owed in
> the application: the work order is queued in `docs/prompt-map-07-13.md`,
> Finding R14.

**What this machine showed on 2026-10-06 (measured, not inferred):**
- **On battery, the show slept.** The charger came out during PR #37's
  fix-forward battery. On battery this machine idle-sleeps **1 minute** after
  the last input (`pmset`: `sleep 1`; on AC, `sleep 0`), and nothing in nbe
  holds a sleep assertion.
  - At 02:27:39 the machine slept for 491 s with a show running.
  - The slate-release end-to-end's test 4 ran 486 s and failed its
    precondition: the engine's report read `fallbackActive` false.
  - That run is void. Re-run on AC under `caffeinate -i -s`, it was 4 of 4,
    with no sleep in the window.
- **On AC, the adapter fell short.** The adapter in use is **45 W**
  (`AdapterDetails.Watts`: 20 V × 2.25 A). Apple's adapter for this
  MacBookPro15,1 is **87 W**.
  - In the re-run's final CPU-bound step, the clippy build, the gauge read
    `InstantAmperage` **−1092 mA** at 12.387 V, with `SystemLoad` at 43387.
  - So the battery was discharging, about 13.5 W, while plugged in.
  - That is one reading, from a gauge that refreshes about every 20 s. The
    dress rehearsal's own draw is not yet measured.
- **Low Power Mode is on, on AC as well as on battery** (`lowpowermode 1` in
  both). When it was set is not recorded, so which past measurements ran under
  it is unknown.
- **The battery is worn:** 1002 cycles, `Condition: Service Recommended`, and
  4682 of 7336 mAh design capacity (64%). A worn battery covers a deficit for
  less time.

**What was not shown:** a frame-rate or underrun difference caused by power.
The suites that ran on battery passed, and the void run failed by sleeping,
not by slowing. The deficit is real and measurable. Its effect on the show's
thresholds is the open question that R14's work order measures.

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
| `automation-latency.log` | Per iteration: the automation latency harness's whole output — both tiers' tables, loads at each tier's start and end, the clock-alignment bound, the `AUTOMATION:` line |

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
- **the stream transport's state** (SPEC v0.4.6): the distinct
  `streamTransportState` values seen across the iteration's ticks, with counts
  — the same shape, for the same reason. A healthy rehearsal reads `none`,
  `live`, `closed` (first measured 2026-09-25: `closed:26 live:5 none:19`); a
  `reconnecting` count on an iteration that killed no ingest is a finding.

Artifacts are diagnostics and never a reason to fail — but a soak with no
artifacts is void, because an unreproducible green is not evidence.

## 5. The flake-register watch list

A flake that stops appearing has not necessarily been fixed; it may have moved
to a machine nobody watches. So every entry in the flake register is asserted
during a soak with its **count** recorded, not merely its pass/fail:

| Entry | Signature | What the soak records |
|---|---|---|
| ~~**R7**~~ **CLOSED 2026-09-18 (second attempt — the first mechanism was wrong)** | `expected 3 directives, got 4` in the control-plane suite — **4** sightings across unrelated changes (2026-09-08; PR #17 run `34745014794`; PR #19 run `35314193753`) — two of them on branches whose diff was docs+CI only, load-sensitive, always green on rerun | Iterations run, iterations where the signature appeared, and the full stderr of any appearance. **Closed:** neither — the test read `directives.at(-1)` for "the seq this command produced", which aliased the previous command's directive whenever the response outran its own directive, so two of three waits could be no-ops and the count fell on a fixed 30 ms sleep. Waits are now by command name and the count is asserted twice around settle windows. Kept on this list for one quarter of soaks as a watch, then removed. The payloads are no longer uncaught: as of the fourth sighting the assertion dumps each directive's `command`, `seq` and `stateVersion`, so the next appearance — in CI or in a soak — names the duplicate |
| **R10** (open, watch) | `drained reads 0 on the tick` in `prompt10_rtmp::telemetry_tick_wires_the_live_session_counter` — **1** sighting (CI run `36120910087` attempt 1, `000159f`, runner load unrecorded; green on rerun; 10/10 locally at 0.37–0.46 s). A 5 s drain bound on a single-threaded test server, R9's class | `prompt10_rtmp` runs whole every iteration (`scripts/soak.sh`'s stream-suite loop); a sighting FAILS the soak, and the full output is kept in `$ITER/prompt10_rtmp.log`, where the signature is greppable. **A soak sighting refutes the load reading** — the soak runs only when quiescent — and reopens R10 as a defect (`docs/prompt-map-07-13.md`, Finding R10). No dedicated count like R7's `R7_sightings` is wired yet: owed with the CI load capture, not made in the records-only commit that filed this row |
| ~~**R11** (open)~~ **R11** (resolved by `bb7d4a2`; watch) | *Since `bb7d4a2` the wait hangs off the thread's exit event with an 1100 ms deadlock backstop, and a sighting's message carries the capture (`phase=…, encoder_ready=…, encoder_open_us=…, aac_ready=…`). The post-fix signature is `did not exit within 1100 ms (phase=…)`, and ~~any such sighting reopens it with the capture in hand~~ **(reopen condition amended 2026-10-03, the user's word, §2c):** a sighting in **any phase other than `exiting`** reopens it, with the capture in hand. An **`exiting`-phase expiry under extreme load** is the backstop doing its job — R9's class, a wall-clock bound measuring the machine — and is recorded as a sighting, not reopened. An `exiting`-phase expiry **under the ceiling** ~~is not covered by that word; it is recorded and put to the user, not dismissed~~ is the user's call: it is recorded and put to the user, neither reopened nor dismissed (the final clause, the user's word of 2026-10-03). **Sharpened 2026-10-06 (the user's word, on sightings 3 and 4, both under the ceiling): R11 stays resolved, and a THIRD `exiting`-phase sighting under the ceiling reopens it.** The row's three cases: **a non-`exiting` phase reopens; an `exiting` expiry under extreme load is a sighting; an `exiting` expiry under the ceiling is ~~the user's call~~ counted, and the third reopens (§2c, 2026-10-06).*** *Post-fix sighting 1 (2026-09-27, on the executor's machine during WU8, at `4c6e176`'s parent tree: `record::stream::tests::a_slow_exit_inside_the_backstop_is_a_clean_stop` panicked at `crates/nbe-engine/src/record/stream.rs:1029:14`. That is the `.expect` on a clean stop of an exit delayed 800 ms, under the 1100 ms backstop. **The message was not kept**: the executor's own filter dropped the line after `panicked at`. Inside the lib binary the close-error seam is serialized (`STOP_SERIAL`), so the error was one of two: the backstop, which is R11's post-fix signature `did not exit within 1100 ms (phase=…)`, or the transport's shutdown timeout. The message would have said which. Load was 4.90 when read a minute later, over the ceiling. It went 0 for 6 in reproduction on the full lib suite at load 4.60–4.68. **R11 is not reopened on an unconfirmed signature. The next sighting, with its message, decides.** A test authored in this PR, with 300 ms of slack by design. **Process, recorded 2026-09-28:** the message was lost to a grep pipeline that dropped the line after `panicked at` — **capture raw output first, filter second.** The sighting stands as unconfirmed; R11 stays resolved; a recurrence WITH its message reopens.)* *Post-fix sighting 2 (2026-10-03, PR #35's two-key pass, on the normative machine): the first post-fix sighting that kept its capture. It ran at `main` `592444a`; the test is unchanged at PR #35's head. `record::stream::tests::a_slow_exit_inside_the_backstop_is_a_clean_stop` failed with `Teardown("stream thread did not exit within 1100 ms (phase=exiting, encoder_ready=true, encoder_open_us=539120, aac_ready=true)")`. Load was **37.5** (the one-minute average, straight after a full workspace rebuild), far over the ceiling. The same suite passed at load 2.38 in the re-run that followed (471/0/3 at `592444a`). The thread was in its exit path, where this test injects an 800 ms delay against the 1100 ms backstop, leaving 300 ms of slack by design. The encoder open took 539 ms. This is the sighting sighting 1 waited for, and it decides as the amended condition says: an `exiting`-phase expiry under extreme load. R11 stays resolved.* *Post-fix sightings 3 and 4 (2026-10-05, PR #37's two-key pass, on the normative machine): both under the ceiling, both with the capture kept; `stream.rs` is unchanged since `592444a`. **Sighting 3**, in a full workspace run at `9473da3` (load 2.40 at the start, 2.32 at the end): `Teardown("stream thread did not exit within 1100 ms (phase=exiting, encoder_ready=true, encoder_open_us=286487, aac_ready=true)")`. The quiescent re-run that followed was 488/0/3. **Sighting 4**, run 19 of 20 of the test alone, at load 2.15: the same signature, with `encoder_open_us=340090`. The other 19 runs passed. Both were put to the user. The user's word of 2026-10-06 keeps R11 resolved, with the sharper tripwire above. **Queued: measure the exit path's true duration.** In `run_stream_thread` the phase turns `exiting` once the run loop returns; then come the test's 800 ms sleep and the drop of the thread's encoders, where the VideoToolbox session is invalidated (the suspect), and only then `done`. An `exiting` expiry therefore says the run loop's return after the stop plus that drop took more than 300 ms. The 1100 ms margin against reality is now a measured question, not a guess.* Original row: `released seam must close: Teardown("stream thread did not exit within 500 ms")` in `record::stream::tests::close_error_seam_fails_loudly_with_the_network_token` — **2** sightings: PR #33's two-key pass at load 27.3; 2026-09-25 at load **2.59, under the ceiling**, in a full workspace run (unreproduced 0-for-10 at load 5.6–6.3 in the lib suite, 20/20 alone). Load does NOT predict it; cause unknown | **Not yet asserted by the soak**: the test is an `nbe-engine` lib unit test, and `scripts/soak.sh` runs named integration suites, not `--lib`. Asserting it here — a run of the lib suite per iteration with the signature counted, and on a sighting the `StreamStats` phase capture R11 requires — is owed with R11's queued work order (`docs/prompt-map-07-13.md`, Finding R11). Until then its sightings come from CI and local runs, and each one is recorded in the register |
| **R12** (~~open~~ **resolved; watch**, 2026-10-05. The user's word of 2026-10-04 is executed: main's PR #36 merge run, `37188708420` at `dc312a6`, is **green per test** (§2b): dress 27 of 27 (`ran=27 passed=27 failed=0 skips=9 exercised=18`), all 138 rust `test result:` lines `ok.`, control-plane 0 `not ok`. The trend rule below keeps its home here and stays live. Disposition decided 2026-10-03, ~~implementation queued~~ **implemented in PR #36**, in `76f092f`, `83bfb60` and `2582b2a`. The condition it was held open for is met: the first honest CI run under the new gate, **`37181092309` at `83bfb60`, is green per test**. Closing is the user's word, after the two-key pass. **After the merge (the user's word of 2026-10-04):** R12 becomes *resolved; watch* once main's merge run reads green per test, and the trend rule below keeps its home in this row) | `not ok 12 - [RI-1] gate: no drops, no underruns, no fallback, and the profile is real` in the CI dress rehearsal (`dress-rehearsal.test.ts`, line 849 onward). It was red in **every run sampled under the old gate, ~~4 of 4~~ ~~5 of 5~~ 7 of 7**, failing on whichever conjunct its ordered asserts reached first. At the gate, from each run's `telemetry.jsonl` (drops / underruns / fallback ticks / watchdog trips): `36214380771` (`619846c`, main, v0.4.6) failed zero-drop `2 !== 0`, with 2 / 211 / 16 / —. `36567491896` (`de90bbe`, PR #34 head) failed underruns `179 !== 0`, with 0 / 179 / 0 / 0. `36658852056` (`592444a`, main, PR #34 merge) failed underruns `160 !== 0`, with 0 / 160 / 0 / 0. `37171465949` (`8265292`, PR #35 head) failed zero-drop `1 !== 0`, with 1 / 164 / 1 / 1. **Sighting 5:** `37177561864` (`66f9f94`, PR #35 head after the ride-along) failed zero-drop `3 !== 0`, with 3 / 174 / 2 / 1; its whole-run maxima are drops 4 and underruns 267. Those counts are inside every backstop, but **3 drops at the gate is above the earlier 0–2 pattern**: the trend the work order will log starts being watched here. **Sighting 6:** `37178415083` (`f1a8279`, PR #35's final head) failed zero-drop `2 !== 0`, with 2 / 152 / 1 / 1; its whole-run maxima are drops 3, underruns 235, trips 2. **Sighting 7:** `37179591329` (`d7b96f7`, main, PR #35's merge) failed underruns `170 !== 0`, with 0 / 170 / 0 / 0; its whole-run maxima are drops 1, underruns 259, trips 1. **Under the new gate, the counts are logged, not failed:** `37181092309` (`83bfb60`, PR #36) gave 0 / 176 / 0 / 0, green per test. `37181886837` (`4c61016`, PR #36) gave 2 / 230 / 2 / 1, green per test. That 230 is the highest at-gate underrun count yet: inside the backstop, and the first reading above the previous peak (211). **Logged since** (the PR #37 two-key pass, 2026-10-05, from each run's `DRESS-COUNTS` line), each green per test: `37186357367` (`2b8efc6`, PR #36's final head) gave 3 / 208 / 1 / 1; `37188708420` (`dc312a6`, main, PR #36's merge) gave 2 / 216 / 1 / 1; `37417955175` (`b8af9ff`, PR #37) gave 0 / 208 / 0 / 0. The normative machine at `8265292`, quiescent, gives 0 / 0 / 0 / 0, 16 of 16. The class is a hardware-bound gate asserting on non-reference hardware: R9's family at CI scale. The leg ran advisory and unread (`continue-on-error` plus a "failed ≤ 3" band) while two bookends recorded it green (`docs/prompt-map-07-13.md`, Finding R12) | The soak's local rehearsal asserts the zero conjuncts and records, at the gate, four counts: drops, underruns, fallback ticks and watchdog trips. CI ~~is to log~~ logs the same four counts on every run against backstops (drops ≤ 20, underruns ≤ 2000, trips ≤ 10), with the underrun trend. ~~**Owed:** that is the queued dress-leg work order, Finding R12's disposition. Today the gate stops at its first failing conjunct, so a CI run shows exactly one count~~ **Implemented (PR #36):** there is one test per conjunct, so no count hides another, and one machine-readable line per run, `DRESS-COUNTS {...}`. The line carries the machine, run, head commit, drops, underruns, fallback ticks (with those outside a watchdog episode), watchdog trips, both rise times and the backstops, and it is echoed to the step summary. A bookend extends this row's trend from that line alone. **The trend rule (the user's word of 2026-10-04): an investigation opens** on (a) three consecutive bookends, each above the previous at-gate underrun peak; or (b) a single at-gate reading above **2× the running median of the last eight**. At `4c61016` the last eight at-gate readings ran 152–230, median about 172, so the threshold is about 344. **Read at `b8af9ff` (2026-10-05), neither leg fires.** (a) The peak is still 230, and no reading since exceeds it. (b) Each new reading is well under twice the median of the eight before it: 208 against about 344 (median 172), 216 against about 344 (median 172), and 208 against about 350 (median 175). The backstop of 2000 catches only gross regressions; this rule is what makes the trend line acted on rather than merely logged |
| **R13** (~~open~~ **closed**, 2026-10-05. The user's word of 2026-10-04 is executed: main's PR #36 merge run, `37188708420` at `dc312a6`, is **green per test** (§2b), and the rises are logged on every run. Disposition ~~rides R12's work order~~ **implemented with it in PR #36**. ~~It is held open, as R12 is, until the user's word after the two-key pass.~~ **After the merge (the user's word of 2026-10-04):** R13 closes outright once main's merge run reads green per test, because its rises are logged on every run) | The late-rise class in the CI dress rehearsal: a step waits 3000 ms (`RISE_MS + TICK_MS`) for a bus to rise, and the rise arrives after the wait has expired. ~~**3**~~ **4** sightings under the old gate, in the steps the band used to absorb: `not ok 5 - [RI-1] step 4: a take with audio follow raises the clip bus on the wire` at `36567491896` (`de90bbe`) and at `37177561864` (`66f9f94`), and `not ok 7 - [RI-1] step 6: a soundboard stab raises the sfx bus and drops nothing` at `36658852056` (`592444a`). At `37177561864` the wait expired with the clip at −120 dBFS: the take was at 11044 ms and the clip became audible at 15011 ms, about 4 s later. Across all five runs' artifacts, the time from take or play to the first audible tick is 1971–3967 ms for step 4 and about 1927–3986 ms for step 6, landing in steps of about 1 s (the telemetry tick plus the meter window). The rise always comes; the behaviour is correct, and the rise is late on a loaded runner. The class is R9's: a wall bound measuring the runner. The normative machine is 16 of 16 (`8265292`). **Sighting 4:** `not ok 7` (step 6) at `37179591329` (`d7b96f7`, main, PR #35's merge); the sfx rise took about 3986 ms. **Under the new gate:** `37181092309` (`83bfb60`) logged step 6's rise at **4971 ms**, the latest yet, inside the 10 s backstop: green. **Logged since** (2026-10-05), step 4 / step 6 in ms, every one inside the backstop and green: 1999 / 1971 (`4c61016`), 3020 / 1997 (`2b8efc6`), 3025 / 2985 (`dc312a6`, main's merge), 3000 / 2039 (`b8af9ff`). The step-4 rises of 3020 and 3025 ms are past the old 3000 ms wait: the late rise still happens, and now it is logged, not failed | The local rehearsal (in the soak) keeps the 3000 ms bound, asserted. ~~**Owed with R12's work order:**~~ **Implemented (PR #36):** CI logs each step's rise time on every run, with its trend, and asserts the rise **present within a 10 s backstop**. That is sized from the sightings: 2.5× the worst (3986 ms), ten telemetry ticks, and the same window `recall.e2e.ts` uses for the same witness (`302c0e3`). The 3000 ms bound is asserted on the normative machine only (`docs/prompt-map-07-13.md`, Finding R13) |
| **R14** (open, 2026-10-06, the user's word): power | Two readings, one cause class. **On battery, a running show slept:** `Entering Sleep state due to 'Idle Sleep' … Using Batt (Charge:98%) 491 secs` at 02:27:39, and the slate-release end-to-end's test 4 ran 486 s and failed `precondition: the engine reports the slate … false !== true`. That run is void; the AC re-run under `caffeinate` was 4 of 4. **On AC, the 45 W adapter fell short** of a CPU-bound build: the gauge's `InstantAmperage` read −1092 mA at 12.387 V (`SystemLoad` 43387), so the battery discharged while plugged in. The class is environmental, and it exposes an application gap: nbe holds no sleep assertion while a show runs, and reports no power state. Evidence and context: §2a | Every soak and battery records precondition 6's power facts with the run. A sleep or a battery event in the window voids the run, and a discharge on AC marks it power-limited, so the count kept here is the power-limited runs and what they measured. **Closes when the application work order lands** (queued in `docs/prompt-map-07-13.md`, Finding R14): the engine holds a sleep assertion while a show runs, and the power state is reported at preflight and on the tick. It does not close because the adapter is swapped |

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
