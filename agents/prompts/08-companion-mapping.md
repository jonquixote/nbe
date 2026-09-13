# Agent Prompt 08 — Input Intent Layer (WS-only) [UPGRADED 2026-09-12]

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — Sections 16 (command API, WS-only per :2374), 16.0 (authorization matrix), 5.3 (WebSocket endpoint), 5.4 (envelope), 5.9 (render channel), 6.6 item 20 (:749), 10.7 item 4 + 10.7.1 (audit log :1806-1827), 19 (preflight, §19.3 row 9 :3073), 24 (Companion risk :3505), AC-12 (:3293-3295). Assumption 6 (:66): Companion emits NBE WebSocket commands; no custom Stream Deck plugin ever. Prerequisites: Prompts 01–07 merged (07 shipped overlay; 07b independent). Starting evidence: `agents/prep/08-upgrade-audit.md`, `agents/prep/08-exec-plan.md` (PRE-decision; P1 HTTP door REJECTED by user ruling herein).**

> Upgrade provenance: this document supersedes the 68-line HTTP-door mission. The audit verdict (GAP-laden, self-contradicting) is incorporated and verified below. Anything herein the tree contradicts is a defect in the prompt: say so and proceed on the tree.

You are a senior TypeScript + Rust engineer building the `nbe` broadcast engine. This prompt puts the show on the Stream Deck XL **without adding a command surface**: Companion drives hardware through the existing §16 WebSocket bus, deck layout is generated from the manifest's `control.bindings`, a keyboard adapter ships in the same prompt as the normative zero-change proof of generality, and every physical actuation is a first-class §16 command-bus citizen.

Read first:

- `docs/spec.v0.4.md` — §§16, 16.0, 5.3, 5.4, 6.6/19.3 binding rows, 10.7.1, 24, AC-12.
- `docs/v0.4-outline.md` §6 (:110-121) + `docs/prompt-map-07-13.md` :500-502 — the Input Intent mandate (data, not code; no second surface; keyboard adapter zero core changes).
- `docs/portability.md` :32 — boundary 1: §16 WS JSON is device-independent core.
- `VOCABULARY.md` — Binding is trigger→action; OS hotkey is not a Binding.
- `agents/prompts/02-control-plane.md` + `02a-architecture-addendum.md` — pipeline you translate into, not duplicate.
- `docs/implementation-standards.md` — §§1, 2, 2a (complete pastes, commit-restore-rebuild, rule 6, §2c), 2b, 3, 4.

## Quality bar (Standards §3)

- **Schema-driven typed models (§1):** Input Intent + binding trigger/action enums round-trip tested against `ControlBinding`, enum-audited literal-by-literal. Schema immutable: `schemas/*.json` unchanged unless ratified spec text requires otherwise; alignment schema → code.
- **Strict CI contracts (§2):** exact exit codes, key strings, behavioural invariants. Falsification battery §2a with complete pastes; counts from CI summary lines §2b; mandated text verbatim §2c.
- **Prompt structure (§3):** Forbidden / New tests / CI changes stated below.

## 0. As-Built Ledger (verified 2026-09-12, file:line)

| # | Claim | Tree truth | Verdict |
|---|---|---|---|
| A1 | HTTP serves only `GET /status` over HTTP; §16 WS-only | `packages/control-plane/src/server.ts:281-289` GET-only, else 404. Spec :2374 "All commands use the WebSocket envelope". `server.ts:1-3` comment names "(Prompt 08) the future Companion HTTP endpoint" — **stale comment, superseded by this upgrade (WS-only)** | HELD, comment superseded herein |
| A2 | Binding types exist, zero consumers | `crates/nbe-core/src/manifest.rs:540-579` `Control{bindings, companion?}`, `ControlBinding{id,description?,trigger?,action,payload?}`, `BindingTrigger{kind,page?,bank?,key?}`, `TriggerKind{CompanionKey,Hotkey,Midi,WebButton,Osc}`. Schema `schemas/manifest.v0.4.json:758-827`. `grep companion\|POST\|/command packages/control-plane/src` hits only the stale comment + generated types. `companion` free-form `manifest.rs:543` | HELD |
| A3 | Preflight binding check absent; spec two one-liners | `grep -c binding crates/nbe-preflight/src/main.rs` → `0` (790 lines). Spec §6.6 item 20 :749 "No missing control binding command"; §19.3 row 9 :3073 "Invalid hotkey action" | HELD |
| A4 | Audit record has no origin/identity field | `docs/spec.v0.4.md:1806-1827` fields ts/kind/outcome/role/tokenId/remote/requestId/command/rawCommand/errorCode/versions/reason — no origin. `packages/control-plane/src/audit.ts:13-40` mirrors spec | HELD |
| A5 | AC-12 says WS bus; old body said HTTP module | Spec :3293-3295 "via the WebSocket command bus". Old body :55 "via the generic HTTP module" — predating disagreement | HELD, corrected herein (own commit) |
| A6 | No OSC/MIDI/keyboard adapter modules | Tree search: none in `packages/control-plane/src` | HELD |
| A7 | Retarget #11 header-only | `5521838` fixed 08 header (§5.3→WS endpoint, §21→§24, +16.0/10.7.1); bodies untouched by design (`821bd06`, merged `cc3a487`, 0 files under crates/packages) | HELD |
| A8 | Outline §6 + map :500-502 mandate | `docs/v0.4-outline.md:110-121`: data-not-code Input Intent, per-device profiles user-editable, keyboard adapter zero core changes, wire-level contract with spec text at 08's moment. Map :502: §16 + auth + audit already device-independent core; 08 adds layer above, never beside | HELD |

## 1. Mission, redefined (per queue note + outline §6)

Build the **Input Intent layer — data, not code**:

1. **Input Intent schema:** maps physical intents (Companion button, MIDI note, keyboard chord) to semantic §16 commands. Wire-level contract; per-device profiles are user-editable documents.
2. **Companion adapter (Stream Deck XL):** connects over **WebSocket as primary**, speaks the §5.4 envelope with token auth, translates button → §16 command. No HTTP door, no side channel, ever.
3. **Keyboard adapter:** ships in the same prompt as the normative zero-change proof of generality — works with **zero changes to the core**.
4. **Deck layout generated** from the manifest's `control.bindings` with triggers (`trigger` page/bank/key → pages/banks/buttons; `action` + `payload` → §16 command). Triggerless bindings are API-only intents: valid, but skipped by the generator since no button can fire them. Deterministic byte-for-byte; regenerate on manifest change. Default deck (TAKE, CUT, arm-next, next-item, breaking show/hide, soundboard pads, record/stream toggles, fallback) exists even with zero bindings; id-requiring defaults carry placeholder ids, labelled as such.
5. **The §16 WS bus is the device-independent core** (portability boundary 1). The intent layer TRANSLATES; the bus executes. Any path letting a device act without producing a §16 command is a defect.
6. Feedback flows through the existing push channel (`stateChange`); buttons reflect state, never poll.

## 2. Decision records

- **D1 WS-only, RATIFIED by user.** Companion speaks WebSocket per Assumption 6 (:66, :113). The audit/exec-plan P1 HTTP-door phase is **rejected**; its parity-test content survives translated to WS tests with distinct adapter identity. The `server.ts:1-3` "(Prompt 08) future Companion HTTP endpoint" comment is superseded — the HTTP listener is §10.4's health endpoint, full stop.
- **D2 Origin/identity: spec-first, RATIFIED by user 2026-09-12.** The Input Intent spec text below defines a device/profile identity field for audit + feedback, drafted spec-first and marked for user ratification. The §16 command surface itself is **not extended** — identity rides the audit/feedback path, never the command envelope.
- **D3 AC-12 correction (HTTP module → WS bus): marked mechanical spec-prompt alignment, own commit.** Corrects old-body :55 to spec :3295 wording. No normative spec-file change (spec already says WS).

## 3. Normative spec text — Input Intent contract [RATIFIED, landed in SPEC v0.4.1]

> Drafted spec-first per the v0.4 discipline (normative language + changelog entry), and **now law**: ratified by the user and landed in `docs/spec.v0.4.md` as patch release v0.4.1. The spec file is the normative copy; what follows is the draft it was ratified from, kept for provenance.
>
> **One correction on landing.** The draft said "§6.6 add (item 26)". Item 26 is "Plugin sandbox validation (Section 14)" — §6.6's check list already ran to 29, so the rule landed as **item 30**. The number was wrong in the draft and is corrected here rather than silently in the spec, because a reader comparing the two would otherwise find a rule that is not where the draft says it is.

**§6.6 add (item 26):** "Every `control.bindings[]` entry with a `trigger` maps one physical intent to one §16 command (`action` + `payload`). A binding whose `action` is not a registered §16 command (deprecated Assumption 17 aliases resolve first), whose `payload` fails that command's §16 schema, whose trigger lacks a known kind or a non-empty key, or whose trigger identically shadows another binding's trigger fails preflight (`E_PREFLIGHT_FAILED`), naming the binding `id`. A missing trigger is allowed: the intent is API-only and the deck generator skips it."

**§19.3 amend (row 9):** extend "Invalid hotkey action" to "Invalid input binding (unknown action, schema-invalid payload, or incomplete trigger — names the binding `id`)."

**§10.7.1 add (field, additive):** `intentSource` — optional, `adapter/profile:intent` (e.g. `companion/xl-a:take-1`), format-enforced by the server. Records which physical intent produced a command; identical state changes from different sources differ only in this field. Absent = software/direct command.

**Changelog entry (landed as SPEC v0.4.1):** Input Intent mapping rule (§6.6 item 30, §19.3 row 9), `intentSource` audit field (§10.7.1). WS-only reaffirmed; no §16 surface change; schema unchanged.

## Step 0: Scope discipline

Allowed: Input Intent schema + Companion WS adapter + keyboard adapter + deck generation + preflight binding check + `intentSource` audit/feedback. Forbidden forever: custom Stream Deck plugin (Assumption 6, locked); any second command surface — no HTTP door, no side channel, no direct-to-state device path.

## Work items

1. **Intent model (TS, schema-driven):** typed `InputIntent` / profile documents mirroring `ControlBinding`; round-trip fixture→typed→serialize→re-validate vs schema; enum audit `TriggerKind` literal-by-literal. Files: `packages/control-plane/src/intent.ts` (new), `intent.test.ts` (new).
2. **Companion WS adapter:** button → §5.4 envelope → existing `dispatch()` with token+role; distinct adapter identity in audit via `intentSource`. No new transport, no auth fork. Files: `packages/control-plane/src/companion.ts` (new), `companion.test.ts` (new).
3. **Keyboard adapter (zero-core-change proof):** same intent path, different profile; passes with zero changes to `dispatch`/`server` core beyond what items 1–2 added. Files: `packages/control-plane/src/keyboard.ts` (new), covered in `companion.test.ts` or `keyboard.test.ts`.
4. **Deck generation (deterministic):** manifest bindings → pages/banks/buttons; sorted-by-id stable JSON; default deck always (TAKE/CUT/arm-next/next/breaking/soundboard/record/stream/fallback); empty bindings still drivable; no key collision. Covered in generator tests.
5. **Preflight binding validation (Rust):** after existing checks in `crates/nbe-preflight/src/main.rs`: unknown action → exit 2 `airReady:false` naming binding id (`E_PREFLIGHT_FAILED`); bad payload → exit 2; incomplete trigger → exit 2; valid → exit 0. Fixtures: `binding_invalid`, `binding_valid`.
6. **Audit + feedback:** `intentSource` on audit rows (additive, no token log); state feedback via existing `stateChange` push; buttons reflect, never poll.

## Tests & CI

1. **AC-12 (WS):** Companion button mapped to `view.take` causes take via WS bus, no plugin in path.
2. **Adapters:** Companion + keyboard intents produce identical §16 commands, differ only in `intentSource`; keyboard passes with zero core changes (assert by construction + review).
3. **Generator:** parses, covers every binding, default deck has TAKE + fallback, byte-identical ×2.
4. **Preflight:** unknown/bad/incomplete → exit 2 naming id; valid → exit 0 `airReady:true`.
5. **Auth:** WS missing/bad token → `E_AUTH` before state touch; `render` role receive-only.
6. **Audit:** Companion/keyboard commands in audit log, identical to UI except `intentSource`; rejections still audited.
7. **Falsification battery (§2a, complete pastes):** delete Companion core knowledge → keyboard still passes; remove `intentSource` → audit test fails; binding with unknown command → preflight exit 2 naming binding; drop default TAKE → generator test fails; break determinism → byte-identical test fails.

CI: `control-plane` job runs adapter + generator + audit tests; `rust` job runs preflight binding tests + exit-2 grep gate. `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, control-plane `tsc` + `npm test` all pass.

## Constraints (Standards §3)

- **Forbidden:** custom Stream Deck plugin ever; HTTP command door / second protocol / second auth path / second port; schema edits unless ratified text requires; precedence-guess on contradiction (name + fail); device path bypassing §16; polling for feedback.
- **New tests required:** `intent.test.ts`, `companion.test.ts` (+keyboard), preflight binding fixtures/tests, audit `intentSource` test, AC-12 WS test.
- **CI changes required:** control-plane job (adapter/generator/audit), rust job (binding exit-2 gate + valid gate).
- Vocabulary: `View`, `Element`, `Sequence`, `Item`, `Binding`.

## Reporting obligations (Done means)

Upgraded doc committed first alone (rule 6); spec text own commit (marked unratified at the time, ratified as v0.4.1 on 2026-09-12); battery verbatim with complete pastes; falsification signatures; records entries quoted; dress-rehearsal green on normative machine if touched. Push authorized; open PR; do NOT merge — two-key pass follows.
