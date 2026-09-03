# Agent Prompt 08 — Companion & Stream Deck Command Mapping

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 16 (command API), 5.3 (auth/roles), 10.7 (audit log), 19 (preflight, incl. the control-binding check), 21 (Companion misconfiguration risk), AC-12. Assumption 6: Companion is the Stream Deck path; no custom Stream Deck plugin is ever built. Prerequisites: Agent Prompts 01–07 merged.**

You are a senior TypeScript engineer building the `nbe` broadcast engine. This prompt puts the show on the Stream Deck XL: Companion drives the hardware through the command bus, the deck's layout is generated from the manifest's control bindings, and every physical button is a first-class command-bus citizen.

Read these first:

- `docs/spec.v0.3.md` — Sections 16, 5.3, and 19's binding check are your contract. AC-12 is the test you will be measured against.
- `VOCABULARY.md` — term ledger. A Binding is the manifest-level trigger→action mapping; an OS-level hotkey is not a Binding.
- `agents/prompts/02-control-plane.md` — the command pipeline you are adding a door to.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the binding generator and the HTTP command-endpoint typed model; the binding trigger/action enums must be round-trip tested and enum-audited against the ControlBinding definition.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2).
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: Bitfocus Companion driving the Stream Deck XL through the control plane. Forbidden forever: a custom Stream Deck plugin (Assumption 6 — locked decision, not a preference).

## Step 1: The HTTP command endpoint

- The control plane exposes `POST /nbe/v0.3/command`: an authenticated HTTP endpoint accepting the same Section 5.4 envelope, the same Bearer-token + role auth, and the same handler pipeline as the WebSocket bus.
- This is the machine door: Companion's generic HTTP/WebSocket module, OSC tools, scripts, and CI all submit commands here. One envelope, one pipeline, no side channels.

## Step 2: The binding generator

- From the manifest's `control.bindings` (the `ControlBinding` definition), generate a Companion configuration: pages, banks, and buttons laid out per each binding's `trigger` (page/bank/key), every button mapped to an HTTP POST of the binding's `action` + `payload`.
- The generator's output is deterministic: the same manifest produces the same Companion config, byte for byte. Generate at show load; regenerate on manifest change.

## Step 3: The default deck

Beyond manifest bindings, always generate the operator's core page: TAKE, CUT, arm-next, next-item, breaking show/hide, the soundboard pads, record and stream toggles, and fallback. The default deck exists even when the manifest declares no bindings — the show is always drivable.

## Step 4: Preflight validation

`nbe-preflight` validates every control binding (Section 19's check):

1. The binding's `action` is a registered command.
2. The `payload` validates against that command's Section 16 schema.
3. The `trigger` is complete (kind plus its required fields).

A binding referencing an unknown command fails preflight with a machine-readable error naming the binding id.

## Step 5: The live path

Companion button → HTTP POST → the control-plane pipeline → state change → audit log → telemetry. OSC, MIDI, and keyboard inputs ride the same bus through their own modules. The audit log records the origin of every command (Section 10.7).

## Step 6: Tests

1. **AC-12**: a Companion button (via the generic HTTP module) mapped to `view.take` causes a successful take through the command bus — no custom plugin anywhere in the path.
2. **Generator**: the generated Companion config parses, covers every manifest binding, and the default deck always contains TAKE and fallback.
3. **Preflight**: a binding with an unknown action fails with the machine-readable error; a valid manifest's bindings all validate.
4. **Auth**: the HTTP endpoint rejects a missing or bad token with `E_AUTH` before touching state.
5. **Audit**: a Companion-originated command lands in the audit log indistinguishable from a UI-originated one.

CI: the `control-plane` job runs the command-endpoint and generator tests; the `rust` job runs the preflight validation tests. `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and the control-plane job all pass.

## Constraints

- No custom Stream Deck plugin — ever. That is the locked decision, not a default.
- The HTTP endpoint speaks the same envelope as the WebSocket bus; do not invent a second protocol.
- The generator is deterministic: same manifest in, same config out.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`, `Binding`.
