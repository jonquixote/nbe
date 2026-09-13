# Agent Prompt 09 — Recording Output (crates/nbe-engine)

> **Upgrade pass, 2026-09-12.** Re-measured against the tree after 07, 07b and
> 08 merged. The recording mission is unchanged: the composited View plus the
> master audio bus, hardware-encoded and written crash-safe. What changed is
> everything this document assumed about the engine, and three of its spec
> citations, which were wrong.
>
> The scope decisions stand: hardware encode only (§9.2 forbids CPU x264 in the
> live path), fragmented MP4 as the crash-safe default (§0.1 assumption 14), no
> ISO tracks in v1, no streaming (that is 10).

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — §9.2 (hardware encoding, line 1443), §9.3 (recording, line 1460), §9.7 (multi-output unification, line 1646), §16.14 (output commands, line 2733), §16.11 (marker commands, line 2692), §16.1 (`show.stop` quiescence), §10.1 (telemetry, `recordSpaceMib` at line 1677), §0.1 assumptions 14 and 24, AC-6 (crash-safe recording). Prerequisites: Agent Prompts 01–08 merged — 08 landed with `main` @ `387ecdf`.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt
builds the recording output. Every show becomes a permanent record, and a
recording that is not playable after a crash is not a record.

Read these first:

- `docs/spec.v0.4.md` — §9.3 and §9.7 are the contract; AC-6 is the test you are measured against.
- `crates/nbe-engine/src/render.rs` — the compositor whose frames you tap.
- `crates/nbe-decode/src/lib.rs` — the decode side of the IOSurface/Metal interop, and the only VideoToolbox code in the tree.
- `docs/prompt-map-07-13.md` — 09's queue note, and `[RI-8]`'s pinned residency policy.
- `VOCABULARY.md` — term ledger.

## Quality bar

Complies with the NBE Implementation Standards (`docs/implementation-standards.md`):

- **Schema-driven typed models (§1):** the recording-output typed model (container enum, fragment policy) is round-trip tested and enum-audited against the `OutputDefaults.record` schema definition.
- **Strict CI contracts (§2):** every observable behaviour gets an exact gate. AC-6's SIGKILL invariant is a gate, not a claim.
- **Falsification (§2a):** every claimed behaviour needs a test that fails without it. Commit before mutating; `git reset --hard HEAD` is the restore; rebuild after restoring; evidence pastes complete (rule 5); commit messages name what they carry (rule 6).
- **Prompt structure (§3):** Forbidden changes, new tests and CI changes are listed explicitly below.

---

## Step 0 — The As-Built Ledger

Read from the tree, not from a report.

| Concern | Where | State for 09 |
|---|---|---|
| **The frame seam** | `render.rs:627` `readback_view()`, `:672` `readback_preview()`, `gpu.rs:117` `readback_rgba()` | **Present, and it is a CPU readback.** This is the *only* way a composited frame leaves the GPU today. See Step 0b — it is also the thing §0.1 assumption 24 says outputs must not do. |
| **The composited View** | `render.rs` `render_frame()` → `targets.view` | **Present.** One composite per frame already; §9.7's "one composite, one GPU frame" is half-satisfied — what does not exist is a second consumer. |
| **Master audio bus** | `audio.rs:63` `BusId::Master`, `:78` `feeds_master()`, `:525` `render()` | **Present.** Allocation-free `render()` on a dedicated OS thread (work order DRESS). §9.3 requires master audio in the file; the mix exists, the tap does not. |
| **Command surface** | `dispatch.ts:140-141` `record.start`/`record.stop`, `:135` `marker.add`; handlers `commands/output.ts:8,19` and `commands/state.ts:91` | **Present — and hollow.** `record.start` checks `showState`, flips `state.recordState = "recording"`, forwards a directive. That is all it does. |
| **Engine-side record handling** | — | **Does not exist.** `directive.rs` has no `record.*` arm. The forwarded directive lands nowhere. |
| **`show.stop` internal stop** | `commands/show.ts:136` emits `{ command: "record.stop" }` | **Present.** §16.1's quiescence path already sends it; nothing acts on it. |
| **Telemetry fields** | `recordState`, `recordSpaceMib` in the §10.1 tick | **Present as fields.** `recordSpaceMib` is not measured against a real volume. |
| **Audit path** | `audit.ts` — `kind`, `outcome`, `errorCode`, and `intentSource` (v0.4.1) | **Present.** Record commands are audited like any other; 09 adds no audit surface. |
| **`E_NO_HARDWARE_ENCODER`** | `nbe-protocol/src/lib.rs:76`, `:111`, `:135` | **Present as an error code only.** Nothing can raise it, because nothing tries to open an encoder. |
| **VideoToolbox** | `nbe-decode/src/lib.rs` — `DecodeSession`, `probe_asset`, `decode_audio` | **Decode only.** `grep -riE "VTCompressionSession\|encodeSession"` over `crates/` returns **nothing**. |
| **The encoder** | — | **Does not exist.** No compression session, no bitrate control, no keyframe policy. |
| **The file writer** | — | **Does not exist.** No container muxer, no fragment policy, no `outputs.record.directory` handling, no file naming. |
| **Crash safety** | — | **Does not exist, and is untested.** AC-6 has no test anywhere in the tree. |
| **Marker → chapter** | `marker.add` accepted; nothing writes a chapter or a sidecar | **Does not exist.** See the `[RI-5]` block below. |
| **Residency policy** | `[RI-8]` decided it; nothing implements it | **Does not exist.** See the `[RI-8]` block below. |
| **Rehearsal record step** | `dress-rehearsal.test.ts` — 12 steps, none of them record | **Does not exist.** The `record` identifier in that file is a stdout-capture closure, not recording. |

**The one-line summary:** 09's commands are on the surface and everything behind
them is absent. A `record.start` today returns `ok`, sets a string, and produces
no bytes.

### Step 0b — The blocking contradiction, to resolve before Step 1

The current document forbids "GPU readback of frames" and says "never read back
to CPU". **The only frame seam in the engine is a CPU readback**
(`readback_view()` → `gpu.rs:117` `readback_rgba()`), and it is async, staging-buffer
based, and used by every golden-frame test.

So 09 cannot both tap frames and honour its own constraint using what exists.
Two honest paths, and this prompt must pick one **in writing before Step 1**:

1. **Land zero-copy first** (IOSurface-backed `CVPixelBuffer` → `MTLTexture` →
   `wgpu::hal` import), making the encoder a second consumer of one surface as
   §0.1 assumption 24 and §9.7 describe. This is the spec-faithful path and the
   larger one.
2. **Readback for the first cut, with the cost measured and recorded**, then
   close it. Permissible only if the measurement is taken and published — see
   the zero-copy block below, which requires numbers either way.

Do not resolve this by quietly reading back while the constraint still says you
do not. A constraint the code violates is worse than no constraint.

---

## Mandatory inclusions from the queue note

Each verified against the tree, with its citation.

### `[RI-5]` — 09 owns `marker.add` → recording chapter

§16.11 (line 2692) defines `marker.add` as *"marker added; recording chapter
written if container supports it"*, and §9.3 says markers *"SHOULD be written as
recording chapters where the container supports them."* The command is
registered (`dispatch.ts:135`) and handled (`commands/state.ts:91`); no chapter
and no sidecar is written anywhere.

09 builds: Matroska chapters where the container carries them, and **a sidecar
JSON beside the file always** — including for fragmented MP4, which does not
carry chapters cleanly. Always-sidecar means the markers pipeline never depends
on container support, and the operator's marker list survives regardless of the
container the show chose.

### §0.1 assumption 14 — fragmented MP4 is the crash-safe default

Verbatim (line 74): *"Recording container default is fragmented MP4. Matroska is
allowed, but fragmented MP4 is the default crash-safe container."*

§9.3's fragment policy is the contract, and it is a table, not prose: fragment
interval **≤ 1 second**; moov placement fragmented/init-segment safe; audio
interleaving **yes**; finalization required **no**. "Finalization required: no"
is the whole crash-safety story — a file that needs a clean close to be playable
fails AC-6 by construction.

### `[RI-8]` — unload-at-next-load, in 09's resource accounting

The pinned policy, verbatim from `docs/review-midpoint-report.md` §8:

> **Policy (pinned): unload-at-next-load.** `show.stop` releases decode sessions
> but retains package residency — video rings, image textures, audio assets —
> until the next `show.load` replaces it.

That review also assigned its two tests to a rehearsal extension that has not
been written: `decodeSessions == 0` within the grace window, and a second
`show.load` after stop accumulating nothing. **09 carries both**, because a
stop→start recovery that re-decodes is a recording operator's worst minute.

**The rationale's number is stale, and 09 must re-measure rather than repeat
it.** The policy was justified by "the 46 s measured in §3.2". That 46 s was a
debug-build figure under contention; since then the preflight probe was
re-architected to stream (PR #12, 163× less memory) and `show.load`'s decode was
measured at **10.7 s** for the dress package on the normative machine during
work order DRESS. The policy stands on its own merits; its cited cost does not.
Re-measure on the normative machine and cite the new figure — the prompt map
already records that this line "can now be rewritten to cite the release binary
rather than an outstanding fix".

### Zero-copy IOSurface → Metal — re-defer **with numbers**, or land it

This is 09's benchmark trigger, and the queue note is explicit that prose is not
an acceptable deferral. Today a decoded `CVPixelBuffer` is copied to CPU memory
and uploaded as RGBA8; §7.13's rule still holds because the copy happens at
load/read-ahead time and never in the render loop.

**09 is where it starts to cost**, because the encoder is the second consumer of
the same frames, and per-consumer readback-plus-upload is exactly what §0.1
assumption 24 forbids: *"Outputs share rendered frames with hardware encoders via
GPU texture sharing (Metal `IOSurface` / Vulkan external memory) without CPU
readback."*

The decision rule, and it is a measurement, not a judgement:

- Measure encode + decode + composite against the frame budget (§7.13) at the
  target profile **on the normative machine** (`docs/hardware-baseline.txt`).
- If it fits **with** readback: defer again, publish the numbers, and say which
  measurement would change the answer.
- If it does not: land zero-copy in 09.

Either way the deferral is discharged with a table, not a paragraph.

### The display-surface deferral, inherited

Carried 04 → 09 in practice. 09 does not build it. Re-defer explicitly and say
what would trigger it, so it does not silently become 13's surprise.

### The dress rehearsal gains a record step

The rehearsal is 12 steps and none of them record. 09 extends it, and the step
proves the three things a "the command returned ok" assertion cannot:

1. **Bytes on disk.** A file exists, is non-trivially sized, and `ffprobe`
   parses it — the file, not the return value.
2. **Crash-mid-record recoverability (AC-6).** `SIGKILL` the engine while
   recording; the file that remains is still parseable and plays. This is the
   only test in the repository that would catch a container that needs a clean
   close.
3. **Audio-video sync in the file.** Not "audio is present" — that a known
   event lands at the expected timecode *inside the recording*, so a drifting
   mux fails rather than passing quietly.

Note the rehearsal's own constraint, learned the hard way in work order DRESS:
its gate asserts zero drops and zero underruns, which are **reference-hardware**
claims, and the CI runner is 3 arm64 cores. A record step that asserts timing
thresholds will fail on CI for reasons that are not defects. Assert *structure*
(bytes, parseability, sync-within-tolerance) everywhere; assert *performance*
only where it means something.

---

## Work items

1. **Encoder session** (`crates/nbe-engine/src/encode.rs`, new): VideoToolbox H.264, hardware only. `E_NO_HARDWARE_ENCODER` when none is available — refuse, never fall back to CPU (§9.2). Keyframe interval 1 s; bitrate from `outputs.record`.
2. **The frame tap**: per Step 0b's recorded decision. Whichever path, the render loop must not block on encode — the frame budget is §7.13's and the encoder is not in it.
3. **Audio tap**: the master bus mix into the encoder's audio input, interleaved per §9.3. The audio thread is real-time and dedicated; do not do file I/O on it.
4. **Writer**: fragmented MP4 (default) or Matroska, per `outputs.record.container`. Fragment policy from §9.3's table. Files in `outputs.record.directory`, named by show/episode + start timestamp. Timecode metadata where available.
5. **Commands, made real**: `record.start`/`record.stop` reaching the engine (`directive.rs` has no arm today). `show.stop` quiescence per §16.1 — and **move the `appliedStateVersion` ack behind the encoder's actual shutdown**: acknowledging before the output stops makes the control plane's 2-second window a lie. (This note is carried from the Prompt 03 review and is now actionable, because the outputs are becoming real.)
6. **Markers**: chapters + always-sidecar, per `[RI-5]` above.
7. **Telemetry**: `recordSpaceMib` measured against the real target volume; `E_DISK` on an unwritable target, never touching the render loop.
8. **Residency**: `[RI-8]`'s policy implemented and both its assigned tests written.

## Tests & CI

1. **AC-6, the kill test**: SIGKILL mid-record; `ffprobe` parses the file and it plays.
2. **Fragment policy**: fragments ≤ 1 s, init-segment safe, audio interleaved, no finalization needed.
3. **Quiescence**: `show.stop` leaves a playable file; the force path logs its warning and the file still plays; the ack lands *after* encoder shutdown.
4. **Markers**: Matroska chapters readable; the sidecar carries the marker list with timecodes, for both containers.
5. **§9.7 multi-output**: a headless record run with the compositor live drops zero View frames and does not recomposite.
6. **Telemetry**: `recordSpaceMib` reports real free space; `E_DISK` fires against an unwritable target.
7. **`[RI-8]`**: `decodeSessions == 0` within the grace window; a second `show.load` after stop accumulates nothing.
8. **Rehearsal**: the record step above.
9. **Falsification battery (§2a, complete pastes)**: remove the fragment policy → AC-6's kill test fails; drop the sidecar → the marker test fails for fMP4; revert the ack ordering → the quiescence test fails; disable the hardware-encoder check → the `E_NO_HARDWARE_ENCODER` test fails.

**CI:** the `rust` job on macos-14 covers this (Metal and VideoToolbox available). Add an anchored floor gate for the new test target, in the shape `prompt04`/`prompt07_overlay`/`prompt07b_graphics` use — a suite that silently runs zero tests is a green build that proves nothing. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, control-plane `tsc` + `npm test` all pass.

## Constraints

**Forbidden:** RTMP/SRT streaming (that is 10); ISO/isolated tracks (the `isolation` hook is reserved, master-only in v1); CPU x264 anywhere in the live path (§9.2); recomposition for a second output (§9.7); blocking the render loop on encode or file I/O; schema edits.

**Required:** `anyhow` for binaries, `thiserror` for library errors. Vocabulary: `View`, `Element`, `Sequence`, `Item`, `Marker`.

## Corrections this upgrade pass made to the previous document

Recorded rather than silently applied, because a reader comparing versions
deserves to know which citations moved and why.

| Was | Is |
|---|---|
| "SPEC §5.8's no-GPU-readback rule" | **§5.8 is "Operator topology"** (line 528). The no-readback rule is **§0.1 assumption 24** (line 84). Same class of mis-citation the 08–15 retarget pass found in prompts 11, 13 and 14 |
| "Prerequisites: Agent Prompts 01–08 merged" | Now true — 08 merged as `387ecdf`. It was a forward reference when written |
| Step 0's inventory ("Allowed now… Forbidden…") | Kept, but it described an engine that did not exist. Replaced by the As-Built Ledger above; the scope decisions inside it survive |
| Markers as a Step 4 detail | Promoted: `[RI-5]` **assigns** ownership, and the previous document did not mention the assignment |
| The zero-copy deferral | Kept, with the decision turned into a measurement rule. "Defer with numbers" is the queue note's explicit instruction |

## One thing 09 will hit that the spec does not cover

§9.2's hardware-encoder table lists **Apple Silicon** (VideoToolbox) and
**Linux/NVIDIA** (NVENC). The normative reference machine (§0.3) is an **Intel**
MacBook Pro with discrete AMD graphics, and it is not in the table. VideoToolbox
does encode there, but the spec does not say so, and 09 is the first prompt that
has to care. Raise it as a v0.5 candidate rather than inventing a row: the table
is normative and this prompt does not amend the spec.

## Definition of done

Per Standards §5, plus: Step 0b's contradiction resolved in writing before any
encoder code; the zero-copy deferral discharged with a table of measurements;
`[RI-8]`'s rationale re-measured rather than repeated; the rehearsal record step
green on the normative machine; the falsification table in the report with
complete pastes; CI summary lines quoted verbatim per §2b.
