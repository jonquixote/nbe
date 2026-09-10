# Agent Prompt 13 — Swift Operator Shell (apps/nbe-macos)

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — the operator surface (Section 5.8 operator topology, 10.8 failure UI, AC-16 single-operator usability), the command surface it binds to (Section 16), and the telemetry it reflects (Section 10.1). NOTE: this prompt previously cited "Section 11" for the operator surface; §11 is the master clock and always was. Prerequisites: Agent Prompts 01–12 merged — the engine is the product; this is the window onto it.**

You are a senior macOS engineer building the `nbe` operator shell: a native Swift app that embeds the engine and gives a human hands on it. The shell has no brain. Every button is a Section 16 command; every indicator is Section 10 telemetry; every pixel of state comes from `show.getState`.

Read these first:

- `docs/spec.v0.4.md` — Sections 5.8, 10.1, 10.8, 16, and AC-16 are your contract.
- `agents/prompts/02-command-surface.md` — the commands and fixtures you bind to.
- `VOCABULARY.md` — term ledger. The UI speaks the vocabulary and nothing else.

## Step 0: Scope discipline

Allowed now: the native shell — program view, sequencer, audio meters, output/health panel — embedding `nbe-engine` in-process. Forbidden: business logic in the UI, a second source of state truth, any command that bypasses the Section 16 surface.

## Step 1: Embedding

- The shell hosts `nbe-engine` as a library in the same process; the composited View renders into a `CAMetalLayer` the compositor already owns. No preview re-encode, no second compositor.
- The same engine runs headless when the shell is absent — the shell is an attachment, not a requirement.

## Step 2: The panels

- Program view with live audio meters (10 Hz, per Section 15).
- Sequencer: Sequences, Items, cue sheets, and the Marker list, driven entirely by `show.getState` and the Section 14 commands.
- Outputs and health: stream/record state, `recordSpaceMib`, watchdog tier, `droppedFramesTotal` — the operator sees degradation before the audience does.

## Step 3: Binding without drift

- The shell's command bindings are generated from (or tested against) the Section 17 schema: if the engine adds or changes a command, the shell's binding breaks loudly at build or test time, never silently at showtime.
- Keyboard map for the show-critical commands; everything reachable without a mouse.

## Step 4: Tests

1. **Fixture-driven rendering**: the 15 golden command/telemetry fixtures from Prompt 02 render correctly in snapshot tests.
2. **Schema parity**: shell bindings match the Section 17 schema; drift fails CI.
3. **Smoke**: the app launches, attaches the engine, starts a show, and renders the program view on macos-14 CI.

CI: add an `app` job on macos-14 building the Swift package; `cargo check/clippy/test` stay green.

## Constraints

- The UI is a reflection, not a brain. `anyhow`/`thiserror` discipline holds on the Rust side; Swift errors surface as engine errors, not UI inventions.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`, `Marker` — on screen as in the spec.
