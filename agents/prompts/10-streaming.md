# Agent Prompt 10 — Streaming Output (crates/nbe-engine)

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`, patch level v0.4.4) — §9.1 (outputs
ceiling), §9.2 (hardware encode), §9.4 (streaming), §9.5 (local network
survivability), §9.7 (multi-output unification — now load-bearing), §10.1 /
§10.1.1 (telemetry and field ownership), §16.1 (`show.stop` quiescence), §16.14
(output commands), AC-10 (internet-loss survivability). Prerequisites: Agent
Prompts 01–09 merged, plus the ZERO-COPY arc (Phases 1, 2, 3a, 3b) and SPEC
v0.4.4.**

**UPGRADED 2026-09-21.** The previous draft was written 2026-09-10, before the
ZERO-COPY arc existed. It asked for streaming "fed by the shared GPU frames per
Section 9.7 … never read back to CPU" — a mechanism that did not exist when it
was written and now does. Everything that changed is in §0 below, and §12
records what this upgrade superseded (§2c). **Read §0 before anything else: an
executor starting from the old draft would re-derive or contradict roughly half
of it.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt
builds the streaming output: the show goes out over RTMP or SRT,
hardware-encoded from the same shared frames the recorder already uses, with a
reconnect loop that never takes the local show down with it.

---

## 0. What is already true

Build on these. Do not re-derive them, and do not contradict them without
saying so out loud and stopping.

### 0.1 The rule about readback is LAW, and streaming inherits no exemption

§0.1 assumption 24's rescope was **ratified as v0.4.4 on 2026-09-21**:

> the recording output MAY use CPU readback only where the zero-copy probe
> reports the chain unavailable, and the engine MUST report which path is live

Read the scope carefully: the allowance is **recording's**. Streaming has none,
and never did — v0.4.2's original allowance said so explicitly ("Streaming
(Prompt 10) inherits no allowance from this row"), and v0.4.4 narrowed rather
than widened it. **A streaming path that reads back is a spec violation, not a
fallback.**

### 0.2 The selection table already has a stream row, and its incapable cell REFUSES

`crates/nbe-engine/src/record/tap_path.rs`:

```rust
pub fn select_stream(zero_copy_capable: bool) -> Option<Selection> {
    zero_copy_capable.then_some(Selection {
        path: TapPath::ZeroCopy,
        reason: Reason::Table,
    })
}
```

`None` is not "fall back" — it is "this machine has no lawful streaming path".
Its guard is `streaming_has_no_lawful_readback_row`
(`crates/nbe-engine/tests/zerocopy_tap.rs`) and its unit sibling
`streaming_has_no_lawful_path_without_zero_copy`. The function exists, is
tested, and **is called by nothing**: wiring it is your job.

**Decide, and state, what a `stream.start` on an incapable machine does.** §9.2
gives the shape for the encoder case (`E_NO_HARDWARE_ENCODER` refuses the
start); §16.14 lists `E_NO_HARDWARE_ENCODER` and `E_NETWORK` as `stream.start`'s
failure modes and no third code. A refusal with no matching error code is a
gap — see §3, blocker B4.

### 0.3 The frames are shareable, the pool exists, and the encode seam speaks CVPixelBuffer

- `crates/nbe-decode/src/zerocopy.rs` — `SharedSurface` (one IOSurface seen as
  a `wgpu::Texture` and a `CVPixelBuffer`), `probe`, and `SurfacePool` with
  `new` / `acquire` / `free` / `len` / `surface_ids`. `Send + Sync` are
  `unsafe impl`'d there with an argued safety note, and that argument depends on
  the pool's one-writer discipline — read it before you add a second consumer.
- `EncodeSession::encode_pixel_buffer` (`crates/nbe-decode/src/encode.rs`) —
  no pool fetch, no copy, geometry and format checked against the session.
  `submit_frame` is shared with the RGBA path so both paths cannot disagree
  about PTS or the forced IDR on frame 0.
- `RenderLoop::set_view_surface` / `view_target` (`crates/nbe-engine/src/render.rs`)
  — the checked retarget, dimensions asserted at the swap, plus a
  `Bgra8Unorm` sibling pipeline because a record surface is BGRA and the
  composite pipeline was RGBA. `readback_view` swizzles to keep its RGBA
  contract.
- `record::feed::{begin_tap_frame, restore_view, end_tap_frame}` and
  `acquire_record_surface` — the loop's per-frame seams, and the
  shed-before-draw discipline.
- `record::zerocopy_pool` sizes the pool `RECORD_CHANNEL_BOUND + 1`. **That
  constant is record's, not a general answer** — see §2, gate G1.

### 0.4 The measurements exist; spend them, do not re-derive them

`docs/09-measurements.md` carries all four ZERO-COPY sections. On the reference
machine (`docs/hardware-baseline.txt`), 1080p30, quiescent, 300 frames:

| span | mean | p95 |
|---|---:|---:|
| cpuReadback tap | 14.832 | 15.709 |
| zeroCopy tap | 0.008 | 0.009 |
| cpuReadback frame (render+tap) | 15.866 | 16.787 |
| zeroCopy frame (render+tap) | 1.376 | 1.575 |

4K zero-copy render+encode: mean 12.433, p95 19.525. Independently reproduced
by PR #26's two-key pass (means within ~2.5%, p95s within 1.3%).

Two facts from those sections you will need and should not rediscover:

- **The retarget is not free**, at about **+0.3 ms per frame** against the
  native RGBA target — no extra pass and no copy, but not zero.
- **The 4K over-budget count is a tail statistic** (1, 2, 4 per 300 across three
  runs) while p95 barely moves. Watch p95, not the count.

### 0.5 The telemetry precedent is set, and part of streaming's is already law

`streamState` and `streamBufferMs` are **already normative**: both appear in
§10.1's field block, and §10.1.1 assigns `streamState` to the control plane ("as
commanded") and `streamBufferMs` to the render node. They are not candidates —
they are law you must satisfy.

What is *not* law is any **new** field. `recordTapPath` / `recordTapReason` are
the pattern to copy, ratified in v0.4.4 with §10.1's note. The obligations that
came with them are in §5 below.

### 0.6 The disciplines tightened

`docs/implementation-standards.md` §2a now carries **eight** rules. Two are new
since the old draft and both bite here:

- **Rule 7** — a test that claims a path it never enters is not a guard. Three
  instances closed across three suites.
- **Rule 8** — a floor that is green about what it cannot see is not a floor.
  Five instances cited, including a CI gate that counted `passed` while a
  capability-gated skip reported `ok`.

`docs/soak-protocol.md` is the home for every threshold CI cannot measure, with
PASS / FAIL / **VOID** and a load ceiling of 3.0.

### 0.7 One decision is OWED to this work

From `docs/09-measurements.md`, "On the ledger: the override is built and wired
to nothing":

> `select_with_override` exists in `crates/nbe-engine/src/record/tap_path.rs`,
> is covered by its own tests, and honours the rule that matters — an override
> can *restrict* (force CPU) but never *conjure* a capability the probe denied.
> **Nothing calls it.** […] **It lands wherever a config surface next appears**
> — Prompt 10's streaming work is the likely place, since a second consumer
> needs per-output settings anyway, but earlier is fine. This sentence is where
> that decision is owed.

You are closing a recorded debt, not inventing a feature. **But read §3 blocker
B2 first: as the tree stands you cannot close it without a spec revision, and
that word is not yours.**

---

## 1. The protocol question, answered from the spec — and the part the spec leaves open

### What §9 mandates

§9.1, verbatim:

> The engine MUST support: […] 4. One live streaming output in MVP: RTMP or
> SRT. 5. WHIP output as future/contribution output.
>
> MVP hard ceiling: 1 local display output / 1 preview output / 1 recording
> output / 1 RTMP or SRT output

§9.4's default MVP stream, verbatim:

| Property | Value |
|---|---|
| Protocol | RTMP or SRT |
| Video | H.264 High |
| Resolution | 1920x1080 |
| Frame rate | 30 fps |
| Video bitrate | 6–12 Mbps recommended |
| Audio | AAC 48 kHz |
| Audio bitrate | 192 kbps |
| Keyframe interval | 1 second |

> Stream failure MUST NOT stop local playout.
>
> Stream reconnect MUST be automatic.

**So the spec settles: exactly one live stream output, H.264 High / 1080p30 /
AAC 48 kHz / 1 s keyframes, and the protocol is "RTMP or SRT". It does not
choose between them, and it defers WHIP.**

### The part it leaves open — an explicit user choice, not the executor's

**USER CHOICE C1 — which transport does Prompt 10 build first?**

| Option | For | Against |
|---|---|---|
| **(a) RTMP first** *(recommended)* | §9.1 and §9.4 both name it first; it is the platform path (YouTube/Twitch/Facebook) most operators need on day one; pure-Rust implementations exist, so no FFI; the test double is inspectable — an RTMP handshake plus FLV tag parsing needs no third-party server | FLV muxing is work that SRT (which carries MPEG-TS or raw) does not need |
| (b) SRT first | The contribution-link protocol, caller mode against a MediaMTX-class server; better loss behaviour on a bad uplink, which is closer to AC-10's subject | Most SRT stacks are libsrt bindings, and **FFI collides with the workspace's `unsafe_code` policy** — exactly one crate is exempt (`crates/nbe-decode`) and a CI gate hard-codes it. A pure-Rust SRT is possible but is a bigger dependency bet |
| (c) Both in one prompt | The spec permits both, so the schema enum is already satisfied | Doubles the transport surface in one prompt, against §6's scope discipline. The record path shipped one encoder and one container first for the same reason |

**DECIDED 2026-09-21: (a) RTMP first. Landed as SPEC v0.4.5 §9.1.** ~~Recommendation:
(a) RTMP first, with SRT as the immediate follow-on prompt.~~ §9.1 now names
RTMP as the v1 streaming transport and defers SRT explicitly — *pending a policy
decision about the workspace's single `unsafe_code` exemption*, because most SRT
stacks are libsrt bindings. That policy question is not this prompt's to answer;
if you want SRT, ask for the word.
Note carefully that this is *sequencing*, not deferral: the spec permits both
and the schema's enum already accepts both, so refusing `srt` at runtime needs
a stated reason and an error path (§3 blocker B3).

**The FFI point generalises and is worth stating once.** The workspace denies
`unsafe_code` with a single exemption and CI enforces it:

```
grep -rn "allow(unsafe_code)" crates --include=*.rs | grep "/src/" | grep -v "^crates/nbe-decode/"
```

Any transport crate that needs FFI is therefore a **policy decision**, not an
implementation detail — the same finding ZERO-COPY Phase 1 recorded about the
tap. Choose a pure-Rust transport, or bring the policy question to the user
before writing code.

---

## 2. The first design gate — and it is not the one the old draft implies

### Gate G1: with record and stream both live, who owns the surfaces?

The old draft says "fed by the shared GPU frames per Section 9.7" and stops
there. The ZERO-COPY memo's Q3
(`docs/zero-copy-p3-design.md`) is the analysis that makes that sentence
buildable, and **its reasoning transfers while its answer does not.**

Q3's finding, quoted:

> `Frame { rgba: Vec<u8> }` carries an *owned copy per frame*, so shedding is
> free […] A shared surface is **one mutable allocation**. If the encoder is
> still reading frame N when the compositor starts frame N+1, "shed" is not
> available — the pixels have already been overwritten. Shedding a surface you
> have already drawn into is not a skip, it is a corrupted frame.

For recording, the answer was a pool of `RECORD_CHANNEL_BOUND + 1` and a
pre-check moved *before the draw*: no free surface → skip before rendering.
**Three things make streaming different**, and they change the answer:

1. **A stream outlives a take.** A record take is bounded and operator-ended; a
   stream runs for the show.
2. **Its backpressure is the network's, not a channel's.** A record thread
   drains at encoder speed, which is bounded and measured. A publisher on a bad
   uplink can stall for *seconds*. "No free surface → skip before the draw" is
   wrong here, because the View must be drawn regardless — it is the program.
3. **§9.7 forbids the obvious workaround.** Verbatim:

   > One composite produces one GPU frame. Display, recording, streaming, and
   > preview outputs are hardware-encoder sessions sharing those rendered
   > frames via GPU texture sharing (Metal `IOSurface`, Vulkan external
   > memory).
   >
   > Running record + stream concurrently MUST NOT add CPU load beyond
   > encoder-session overhead, and MUST NOT recomposite.

**So "give the stream its own pool" is not obviously available.** Two pools
means either two draws (a recomposite, forbidden) or a copy between surfaces
(CPU load beyond encoder-session overhead, forbidden). The shape §9.7 actually
describes is **one composite into one surface per frame, with N consumers
holding references to it.**

That reframes the gate. The question is not "one pool or two" but:

> **When one consumer of a shared surface falls indefinitely behind, how does it
> give up its frame without holding the allocation hostage?**

**A recommendation, for you to falsify rather than inherit.** The pool's free
list is already `Arc::strong_count == 1` — a surface is free exactly when
nobody outside the pool holds it. That generalises to N consumers at no cost: a
slow consumer *drops its `Arc`* and the surface returns. So:

- **One pool, sized for the sum of its consumers' in-flight bounds plus the one
  being drawn** — not `RECORD_CHANNEL_BOUND + 1`, which is record's answer and
  must stop being the only one.
- **The stream sheds by releasing its reference**, never by making the pool
  wait and never by blocking the draw. A stream frame that cannot be taken is a
  *stream* drop, counted as one, and AC-10 item 4 forbids it from becoming a
  View drop.
- **Record's shed-before-draw discipline stays exactly as it is.** Streaming
  must not change it, and must not make record's pool exhaustion depend on the
  network.

**Falsify the recommendation before building on it.** At minimum: a stalled
stream consumer must not raise `skipped_record_frames`, and must not raise
`droppedFramesTotal` at all. If your measurements say the shared-pool shape
cannot hold, that is a finding worth more than the schedule — report it and
stop.

### Gate G2: does the stream share the record take's *session* lifetime?

The record pool rides `RecordSession` deliberately, so three teardown paths
cannot forget it. A stream is not a take. Decide where a shared pool lives such
that (a) it outlives any single record take, (b) it is not leaked when both
outputs are idle (~25 MiB per 1080p surface), and (c) no teardown path can
forget it. State the answer and its guard.

---

## 3. Blockers — ALL FOUR DECIDED 2026-09-21, landed as SPEC v0.4.5

~~Each of these stops an executor on day one. **Do not work around them by
inventing a field or weakening a rule.** They need the user's word.~~ The word
was given for all four and `SPEC-REV` landed them. The analysis below is kept
per §2c — it is why each decision was needed — with each blocker's resolution
stated first.

| Blocker | Decision (2026-09-21) | Where it landed |
|---|---|---|
| **B1** the endpoint | **The manifest carries it.** `show.outputs.stream.url`, declarative, the way `outputs.record.directory` is. `stream.start`'s `url` is an override for the run, not the only source; a start with neither is `E_BAD_PAYLOAD` | §9.4's endpoint rule; `OutputDefaults.stream.url` |
| **B2** the override's field | **Landed.** `outputs.{record,stream}.tapPath: { enum: ["auto","cpuReadback"], default "auto" }`. **The field is law; the WIRING is owed to this prompt's execution** — reading it and passing it to `select_with_override` is your work | schema; v0.4.5 changelog row 5 |
| **B3** the `whip` manifest | **Refused at schema validation**, before load and before any command. The `protocol` enum narrowed to `["rtmp"]`; `nbe_core::validate` names the protocol *and* the reason it is deferred | §9.4's refusal rule; `ValidationError::RefusedTransport` |
| **B4** the refusal's code | **`E_NO_ZEROCOPY`, reused not invented.** It is in the §10.4 registry and in `stream.start`'s failure modes. Distinct from `E_NO_HARDWARE_ENCODER`: the encoder can be present and the chain absent | §10.4; §16.14 |

**One thing B2 does NOT give you:** a wired override. The field exists so the
wiring has a lawful home. `select_with_override` still has no caller, the ledger
sentence still stands, and closing it is a work item of this prompt, not a
precondition of it.

**And one thing B1 does not give you either, named because §3's table reads as
though it did.** The `url` **precedence rule — the manifest's
`outputs.stream.url` against `stream.start`'s `url` override — is prose-only and
untested.** §9.4 says the command's `url` is *"an override for the run, not the
only source"* and nothing resolves or guards it: `stream.start`'s payload type
carries `url?`, the manifest carries `outputs.stream.url`, and no code reads
either. Resolving precedence and guarding it is **the first thing
`stream.start` must get right**, and it is your work — not a blocker, but not
decided for you either. (Found by PR #29's two-key pass.)

### B1 — ~~there is nowhere for the stream endpoint to live. BLOCKING.~~ DECIDED: the manifest carries it (v0.4.5).

`schemas/manifest.v0.4.json` `$defs/OutputDefaults.stream` is, verbatim:

```json
"stream": {
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "protocol": { "enum": ["rtmp", "srt", "whip"] },
    "videoBitrateKbps": { "type": "integer", "minimum": 500, "maximum": 50000 },
    "audioBitrateKbps": { "type": "integer", "minimum": 96, "maximum": 320 }
  }
}
```

**There is no URL, endpoint, host, or stream-key field anywhere in the schema**
— checked for `url`, `endpoint`, `ingest`, `rtmpUrl`, `streamKey`: none
present. And `additionalProperties: false` means one cannot be added by a
package author.

§16.14 gives `stream.start` the payload `{ outputId?: string, url?: string }` —
`url` **optional**. So as the tree stands, a stream cannot be started from the
manifest at all: the only place an endpoint can come from is an optional command
field. The old draft's "otherwise the manifest's `outputs.stream` wins" is
unachievable — there is nothing there to win with.

Two lawful resolutions, both the user's word:

- **(i) `url` on `stream.start` is the endpoint, in practice required.** A
  §16.14 clarification saying the command's `url` is the only source, and
  `stream.start` without it fails (`E_BAD_PAYLOAD`). No schema change. Simplest,
  and it keeps stream keys out of package manifests — arguably a security
  improvement, since a manifest is a shared artifact and a stream key is a
  credential.
- **(ii) the schema gains an endpoint field.** A spec revision; the standards
  are explicit that `schemas/*.json` changes are spec revisions, not prompt
  work, and that alignment flows schema → code.

**Recommendation: (i).** It requires no schema revision, and a stream key in a
manifest is a credential in a file meant to be copied between machines.

### B2 — ~~wiring `select_with_override` needs a schema field that does not exist. BLOCKING for §0.7's debt.~~ DECIDED: the field landed in v0.4.5; the WIRING is owed to this prompt.

The ledger says the override "lands wherever a config surface next appears" and
names this prompt. But `outputs.stream` and `outputs.record` are both
`additionalProperties: false`, nothing in §9 or §16 mentions a tap-path
override, and the standards forbid a prompt from editing the schema.

**So Prompt 10 cannot close the ledger item as the tree stands.** The debt needs
a spec revision that authorizes the field — for example
`outputs.{record,stream}.tapPath: { enum: ["auto", "cpuReadback"] }`, where
`auto` is the table and `cpuReadback` restricts (never conjures, matching
`select_with_override`'s existing behaviour and its tests).

**Mark this prompt BLOCKED on that word for the override only.** Everything
else in Prompt 10 can proceed without it; the ledger sentence stays owed and
this paragraph is where the discrepancy is recorded. Do not add the field
yourself, and do not quietly drop the ledger item.

### B3 — ~~the schema's protocol enum permits a protocol the spec defers.~~ DECIDED: refused at schema validation (v0.4.5).

`stream.protocol` accepts `"whip"`; §9.1 item 5 makes WHIP a future
contribution output and §6 below forbids it. A package declaring
`protocol: "whip"` is therefore schema-valid and unbuildable. Decide and state
where it is refused — preflight (`E_PREFLIGHT_FAILED`, which §19.2 already
owns for semantic contradictions) is the better home than `stream.start`,
because the operator should learn at load, not at air. §17.5's precedent — a
schema-legal, semantically contradictory item is a preflight failure — is the
one to follow.

### B4 — ~~a refused stream on an incapable machine has no error code.~~ DECIDED: `E_NO_ZEROCOPY`, registered in v0.4.5.

§0.2's `select_stream` returns `None` on a machine with no zero-copy chain, and
that refusal is correct and ratified. But §16.14 gives `stream.start` only
`E_NO_HARDWARE_ENCODER` and `E_NETWORK`. Neither is honest for "this machine
cannot stream lawfully because it has no zero-copy chain".

Options: reuse `E_NO_HARDWARE_ENCODER` (dishonest — the encoder may be present),
reuse `E_NETWORK` (wrong subsystem), or state that the engine refuses with the
`E_NO_ZEROCOPY` token the tap already uses (`crates/nbe-decode/src/zerocopy.rs`)
carried inside an existing code. **Recommendation:** the third, and record it as
a §10.4-registry candidate rather than adding a code — the same shape
`recordTapPath` took before v0.4.4 ratified it. Do not invent a registry entry.

---

## 4. Quality bar

This prompt complies with `docs/implementation-standards.md`. The rules that
bite here, by name:

- **§2a rule 1** — every claimed behaviour needs a test that fails without it.
- **§2a rule 2** — falsify the production path, not the test.
- **§2a rule 3** — commit before falsifying; restore and re-run; rebuild after
  a falsification battery; untracked files restore from copies, not
  `git checkout --`.
- **§2a rule 4** — a test that passes with its behaviour deleted is a defect.
- **§2a rule 5** — evidence pastes are complete. No `head`/`tail` in
  falsification or gate evidence.
- **§2a rule 6** — a commit's message names what it carries.
- **§2a rule 7** — **every streaming test enters the streaming path by a real
  command.** Drive `stream.start` / `stream.stop` through `DirectiveHandler`;
  do not write `streamState` or a session into `EngineState` and assert on it.
  The record suites are the pattern (`zerocopy_migration.rs` drives real
  directives end to end); the three closed instances of this trap are the
  reason the rule exists.
- **§2a rule 8** — **any CI floor you add counts `ran` AND
  `exercised = ran - skipped`, with every skip printed as a `^SKIP` line.** A
  streaming suite that skips its whole transport on a runner with no network
  and still reports `ok` is the fifth instance of a shape this project has
  closed five times. State on every run what went unexercised.
- **Schema discipline** — `schemas/*.json` is normative and immutable to a
  prompt. See §3 blockers B1 and B2.

### Measurement discipline

Every number: **quiescent or VOID** (load ceiling 3.0, no `cargo`/`rustc`
running, loads pasted before and after), counts derived **two independent
ways**, and thresholds move to `docs/soak-protocol.md` rather than becoming test
assertions. `crates/nbe-engine/tests/zerocopy_bench.rs` is the harness pattern:
`#[ignore]`d so it is run deliberately, asserting its own load ceiling, printing
its table.

**The soak inherits streaming's thresholds.** `docs/soak-protocol.md` §1's
claim table gains rows for whatever streaming asserts that CI cannot see —
reconnect behaviour, stream-drop counts under load, the AC-10 span — and
`scripts/soak.sh` gains their capture. Add the rows **only when they are true**:
the record tap's row sat in the future tense for a whole phase and PR #24's
two-key pass called that out.

---

## 5. The telemetry obligation, stated up front

`streamState` and `streamBufferMs` are already law (§0.5). Satisfy them:
`streamState` is the control plane's, "as commanded", and `streamBufferMs` is
the render node's.

**Any NEW field is a §10.1 wire-addition candidate from day one**, and carries
every obligation v0.4.4 established for the record tap:

1. **Always emitted, never absent.** §10.1.1: *"The emitted field shape is
   always complete. A telemetry consumer MUST never see a missing field,
   whatever the engine's state — an absent field and a stubbed field are
   different failures and only one of them is diagnosable."* Phase 2 shipped
   `recordTapPath` as absent-until-selection and Phase 3b had to correct it; do
   not repeat that.
2. **Stubbed before the subsystem has run.** `nbe_protocol::tap_none()` is the
   shared stub, and the stub must not be a legal value of the field (`"none"` is
   not a path).
3. **Token-stable.** `path_tokens_are_stable` and `reason_tokens_are_stable`
   (`tap_path.rs`) are the precedent — the second exists because `"Override"`
   became a normative wire token rendered by `format!("{:?}")`, where a variant
   rename would have been a silent wire change with no guard. Any enum whose
   Debug or `as_str` reaches the wire gets a token test in the same shape.
4. **The mirror fixture samples a VALUE, not `None`.**
   `crates/nbe-protocol/tests/mirror.rs` audits the Rust field set against the
   TypeScript schema, and a `None` sample serialises to nothing, so the
   agreement holds vacuously while TypeScript knows nothing about the field.
   That exact trap is one of §2a rule 8's five cited instances.
5. **Marked as a candidate, ratified separately.** The field ships marked
   UNRATIFIED with its guard; ratification is the user's own change, never
   inside a feature PR (§4's counter-precedent is on the record).

---

## 6. Scope discipline

**Allowed:** one live streaming output per §9.1's ceiling, over the transport
USER CHOICE C1 settles, with the §9.4 stream shape, fed by shared surfaces per
§9.7, with automatic reconnect per §9.4 and §9.5.

**Forbidden:** WHIP output (§9.1 item 5 defers it; see B3 for where a
`whip` manifest is refused). WHEP preview (AC-20, explicitly post-v1 and named
as out of scope in the prompt map's queue row). CPU encode. **Any CPU readback
on the streaming path** — that is now a spec violation, not a shortcut. Any
streaming work on the render thread. Multiple concurrent stream outputs.

**Inherited, not rebuilt:** the guest-link JWT / `jti` revocation work and the
TURN credential derivation rule (`[RI-5]`, §5.1 item 11, §9.6.2) belong to the
WHIP/guest path, not to this one. Do not implement them here; do not break them.

---

## 7. Work items

Each step is its own commit stream with its falsification signature pasted
(§2a rules 2, 3, 5, 6).

1. ~~**Resolve the blockers.** Report §3's B1–B4 to the user and get the words
   for B1 and B2 before writing code that depends on them.~~ **Done — all four
   decided 2026-09-21 and landed as SPEC v0.4.5 (§3).** What remains of this
   item is the one piece the revision deliberately did not do: **wire
   `select_with_override` to `outputs.{record,stream}.tapPath`**, closing the
   ledger item. The field is there; nothing reads it.
2. **Gate G1, decided and falsified** (§2). Produce the pool-ownership answer
   with its measurement, and the guard that a stalled stream raises neither
   `skipped_record_frames` nor `droppedFramesTotal`.
3. **The stream encode session.** VideoToolbox H.264 High, 1080p30, the §9.4
   bitrate envelope, AAC 48 kHz 192 kbps, 1-second keyframe interval, fed by
   `encode_pixel_buffer`. Never recomposite; never read back.
4. **Selection at start.** `stream.start` probes, calls `select_stream`, and
   either runs zero-copy or refuses — there is no third path. Publish the
   selection for telemetry the way `record.start` does, with the same
   always-emitted discipline (§5).
5. **The publisher.** Off the render thread, over a bounded channel. A full
   channel drops **stream** frames and reports it. Exponential backoff,
   automatic, forever (§9.4, §9.5).
6. **Commands.** `stream.start` / `stream.stop` per §16.14, wired into §16.1's
   `show.stop` quiescence: internal `stream.stop`, up to 2 s graceful, then
   force with a warning. **The ack must wait for the transport to be gone** —
   see §11's note.
7. **Telemetry.** `streamState` / `streamBufferMs` satisfied; any new field per
   §5.
8. **Measurement and the soak.** Before/after with record alone, stream alone,
   and both concurrently — the §9.7 claim ("MUST NOT add CPU load beyond
   encoder-session overhead") is now a *measurable* claim and this is where it
   is measured. Quiescent, counts two ways, thresholds to the soak.

---

## 8. Tests required

1. **AC-10, the WAN-loss harness.** Stream live, network path cut in the test
   environment: local view continues, recording continues, stream enters
   reconnect/backoff, and **zero View frames drop as a result**. Drive it
   through real commands (rule 7).
2. **Reconnect.** Stop the local test server mid-stream; `streamState` goes
   live → reconnecting → live on recovery with no operator action.
3. **Stream shape.** The outbound stream verifies §9.4: H.264 High, 1-second
   keyframes, 48 kHz AAC, the configured bitrate envelope.
4. **Isolation under saturation.** A publisher on a failing network never
   blocks the render thread, never raises `droppedFramesTotal`, and never
   raises `skipped_record_frames` (G1's guard).
5. **The refusal.** `stream.start` on a machine with no zero-copy chain refuses,
   with the token B4 settles — and the refusal is *reported*, not silent.
6. **Quiescence.** `show.stop` with an active stream stops it gracefully; the
   force path warns and stops it anyway; the ack follows teardown.
7. **Concurrency.** Record and stream live together: one composite, two encoder
   sessions, no recomposite, and both files/streams correct.
8. **Token stability** for any new enum reaching the wire (§5 item 3).

**CI:** the existing `rust` and `control-plane` jobs. Any floor you add obeys
rule 8 — `ran` and `exercised`, skips printed. A transport test double
(pure-Rust, in-process) is preferred over a third-party server so CI exercises
something rather than skipping everything; where CI genuinely cannot (real WAN
loss), the gate says so on every run and the claim's home is the soak.

---

## 9. What Prompt 10 must NOT do

- **No retrograde on the record path.** Record's shed-before-draw discipline,
  its pool sizing, its per-take selection, and its Option A mid-take loss
  behaviour stay as they are. If streaming needs one of them changed, that is a
  finding to report, not a change to make quietly.
- **No weakening of the refusal row to make CI greener.** `select_stream(false)
  → None` is ratified law's mechanism. A machine that cannot stream lawfully
  must refuse. Turning the refusal into a readback fallback to get a green CI
  run would violate §0.1 assumption 24 as ratified.
- **No schema edits** except where §9 or §16 demand them, each justified against
  a quoted sentence. B1 and B2 are the two candidates and both need the user's
  word first.
- **No new telemetry field presented as ratified.** Ship it as a candidate with
  its guard (§5 item 5).
- **No CPU readback anywhere on the streaming path**, including "just for a
  test". If a test needs pixels, `readback_view` exists and is documented as
  inspection-and-test-only; a *production* path that reads back is the
  violation.

---

## 10. Done means

- The blockers of §3 are resolved or explicitly recorded as still-blocking, with
  the ledger item's status stated either way.
- Gate G1 is decided, measured, and falsified.
- A stream goes out over the chosen transport, from shared surfaces, with no
  readback, meeting §9.4's shape.
- Reconnect is automatic and AC-10 holds, with View drops at zero.
- Record and stream run concurrently without recomposite, measured against
  §9.7's claim.
- Telemetry satisfies §10.1 and any new field is a guarded candidate.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace` (counts two ways), `tsc`, the
  control-plane suite, and the dress rehearsal all green, verbatim.
- Three consecutive green rehearsals on the normative machine if the rehearsal
  gained a streaming step.
- Soak rows added only for claims that are true, with `scripts/soak.sh` capture.
- **Two-key pass runs before merge. The merge word is the user's.**

## 11. Carried notes

**The `appliedStateVersion` ack and stream teardown.** The old draft carried a
Prompt 03 review note saying the ack "fires on application, before the actual
stream session is torn down". **Check the premise before acting on it:** the
engine today has *no* `stream.*` handling at all — the control plane routes and
validates the commands (`dispatch.ts`, `protocol.ts`), and
`crates/nbe-engine/src/` contains no stream state, session, or handler. So this
is a hazard to avoid, not a bug to fix. The record path already solved the
analogous problem and is the pattern: `record.stop` calls `stop_and_finish`,
waits for the record thread's terminal report, and **withholds the ack** on
failure rather than reporting a success it cannot vouch for
(`record_stop_failure_withholds_ack_and_keeps_file`). §5.9.5 requires the ack to
reflect outputs actually stopped. Copy record's shape.

**Captions.** §10.6 specifies a WebVTT-class sidecar output *alongside the
stream*. It is not in this prompt's scope, but do not build a streaming path
that makes a sidecar impossible to attach later.

---

## 12. §2c — what this upgrade superseded

The 2026-09-10 draft is superseded in the following specific claims. Its text is
in git history at `agents/prompts/10-streaming.md` before this commit; what
changed and why:

1. ~~"fed by the shared GPU frames per Section 9.7 … Never recomposite; never
   read back to CPU"~~ — correct as an aim, and written before the mechanism
   existed. It now exists, is named in §0.3, and is *law* rather than
   aspiration (§0.1). The draft could not tell an executor where to get a
   shared surface; §0.3 and §2 can.
2. ~~"Allowed now: RTMP and SRT per `outputs.stream.protocol`"~~ — both at once,
   with no analysis of the cost. Replaced by USER CHOICE C1, which presents the
   sequencing decision with the FFI/`unsafe_code` consideration the draft could
   not have known to raise.
3. ~~"`stream.start` accepts an optional `url` override per the command schema;
   otherwise the manifest's `outputs.stream` wins"~~ — **unachievable.** There
   is no endpoint field in the schema to win with. Now blocker B1.
4. ~~"the streaming-output typed model (protocol and stream-state enums); these
   must be round-trip tested and enum-audited"~~ — still required, and now with
   the five concrete obligations v0.4.4 established, including the mirror
   fixture's value-not-`None` rule that the draft's "enum-audited" did not
   reach (§5).
5. The draft's Prompt 03 ack note is kept but **re-premised**: it read as a
   description of existing code and the code does not exist (§11).
6. The draft named §2a as the standards reference when it had six rules. It now
   has eight, and rules 7 and 8 both bite here (§0.6, §4).

Nothing in the old draft was *wrong* about the spec. What it lacked was the tree
the ZERO-COPY arc built under it, and three decisions that turn out to belong to
the user rather than to an executor.

---

## Report (what the executor's done-message must contain)

1. **The blockers**, each with its resolution or its still-blocked status:
   B1 (the endpoint), B2 (the override's schema field and the ledger item's
   standing), B3 (`whip` refusal), B4 (the refusal's error code).
2. **USER CHOICE C1's answer** and, if it differs from the recommendation, why.
3. **Gate G1's decision**, its measurement, and the falsification that a
   stalled stream moves neither `skipped_record_frames` nor
   `droppedFramesTotal`.
4. **Every falsification signature**, complete pastes, production paths, with
   the restore confirmed and the suite green after.
5. **The measurements**: record alone, stream alone, both concurrently, against
   §9.7's "no CPU load beyond encoder-session overhead" claim — quiescent,
   loads pasted, counts two ways, VOID declared rather than smoothed if a
   precondition failed.
6. **The telemetry fields** shipped, their candidate status, and their guards.
7. **The battery**, verbatim: fmt, clippy, workspace with counts derived two
   ways, tsc, control-plane, rehearsal, and any new CI floor's `ran` /
   `exercised` reading.
8. **Soak rows** added, and the sentence saying they are true rather than
   planned.
9. **What was left undone and why**, including the ledger item if B2 stayed
   blocked.

## Constraints (carried)

- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item` (`VOCABULARY.md`).
- The stream is a shared-surface encoder session with a bounded channel — the
  View never pays for the network.
