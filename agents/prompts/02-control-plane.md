# Agent Prompt 02 — Control Plane (packages/control-plane)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 5 (control plane), 16 (command API), 17 (state machine), 10.1 (telemetry), 10.7 (audit log), 3 (glossary). Prerequisite: Agent Prompt 01 merged (`nbe-core` validates manifests against `schemas/manifest.v0.3.json`).**

You are a senior TypeScript engineer building the `nbe` broadcast engine. Build the control plane: the authoritative show-state owner and the single WebSocket command bus that every client (operator UI, producer UI, iPhone, Companion bridge, render node) talks to.

Read these first:

- `docs/spec.v0.3.md` — Sections 5, 16, 17 are your normative contract. 10.1 for telemetry fields, 10.7 for the audit log, 3 for the glossary.
- `VOCABULARY.md` — term ledger. View, never Program. Element, never Layer.
- `agents/prompts/01-bootstrap-core-preflight.md` — the build order this follows.
- `docs/implementation-standards.md` — the NBE Implementation Standards; this prompt must comply.
- `docs/prompt-01-definition-of-done.md` — Prompt 01 is locked; do not reopen its behaviours.
- `agents/prompts/02a-architecture-addendum.md` — normative architecture decisions and semantics that must be pinned before proceeding.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the TypeScript wire protocol (envelope, command set, error-code registry) and the item/scene state machine; these must be round-trip tested and enum-audited against the Section 16 command/error tables (see Standards §1). Concretely: for every command in Section 16, define a zod schema, serialize a sample payload to JSON, deserialize it back, and assert equality; for the error-code registry, list every code in Section 16, map each to a TS enum member, and verify no omissions or mismatches.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2).
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).
- **Vocabulary discipline:** All new code must use `View`, `Element`, `Sequence`, `Item`. The strings `program` and `layer` may appear only in the alias table and its tests.

## Step 1: Project scaffold

In `packages/control-plane`:

- `package.json`: `"type": "module"`, `"engines": { "node": ">=20" }`.
- Dependencies: `ws`, `zod`, `uuid`. Dev: `typescript`, `tsx`, `@types/ws`, `@types/node`.
- `tsconfig.json`: strict mode, NodeNext modules, `noUncheckedIndexedAccess`.
- Scripts: `dev` (`tsx watch src/index.ts`), `build` (`tsc`), `test` (`tsx --test src/**/*.test.ts`).

## Step 2: Wire protocol (src/protocol.ts)

Implement SPEC Section 5.4 exactly:

- The envelope: `v` (const `"0.3"`), `id` (UUID), `command`, `payload`, optional `baseStateVersion`.
- The success response: `v`, `requestId`, `status: "ok"`, `stateVersion`, `data`.
- The error response: `v`, `requestId`, `status: "error"`, `stateVersion`, `error: { code, message }`.
- The error-code registry from Section 16 as a const enum — every code, no omissions.
- zod schemas for the envelope and for every command payload defined in Section 16's tables. Unknown commands fail with `E_UNSUPPORTED`.

## Step 3: Server (src/server.ts)

- WebSocket server on `ws://127.0.0.1:8462/nbe/v0.3`.
- Handshake auth per Section 5.3: `Authorization: Bearer <token>` plus `X-NBE-Role`. Tokens resolve to roles via local config. Failed auth closes the socket with an `E_AUTH` error frame first.
- Role permission enforcement per the Section 5.3 table: `monitor` is read-only; `operator` gets live commands; `producer` gets load/preflight/edit; `admin` gets everything; `render` is the internal directive channel.
- Track connected clients by role for telemetry broadcast.

## Step 4: State (src/state.ts)

- The item state machine from Section 17.3 as an explicit transition table with guards. Illegal transitions fail with `E_FORBIDDEN_STATE`. Item states: `READY`, `ARMED`, `LIVE`, `PLAYING`, `DONE`, `MISSING`, `ERROR`.
- Scene states (Section 17.2): `IDLE`, `ARMED`, `VIEW`, `TRANSITIONING`.
- Monotonic `stateVersion`; commands carrying `baseStateVersion` fail with `E_VERSION_CONFLICT` on mismatch (Section 5.5).
- Crash recovery: persist last known show state locally on every change; restore on boot (Section 5.1).
- `automationHold` flag: when held, suppress automation triggers and `autoFollow` within 1 frame (Section 13.5).

## Step 5: Commands (src/commands/)

One handler module per Section 16 command family: `show`, `view`/`preview`, `scene`, `sequence`/`item`, `element`/`graphic`/`breaking`, `overlay`, `ticker`, `soundboard`/`audio`, `guest`, `automation`, `snapshot`/`marker`, `plugin`, `clock`, `record`/`stream`, `system`.

Every handler follows the same pipeline:

1. Validate payload against its zod schema (`E_BAD_PAYLOAD` on failure).
2. Check role permission (`E_AUTH`).
3. Check preconditions against current state (`E_FORBIDDEN_STATE`, `E_NOT_FOUND`, `E_ASSET_MISSING` as applicable).
4. Mutate state and bump `stateVersion`.
5. Emit the state-change event to all connected clients.
6. Append the command to the audit log (Section 10.7).
7. Forward the render directive over the render bridge when the command affects the render node.

## Step 6: Deprecation aliases (Assumption 17)

- `program.*` maps 1:1 to `view.*`; `layer.*` maps to `element.*`.
- Each aliased command executes normally AND emits a deprecation warning in telemetry.

## Step 7: Telemetry (src/telemetry.ts)

- `system.telemetry.subscribe` starts a per-client 1-second broadcast of the Section 10.1 field shape, including `qualityProfile`, `degradationRung`, `automationHold`, and any deprecation warnings since the last tick.
- `system.telemetry.unsubscribe` stops it.
- Include a test that sends a deprecated `program.take` command and verifies the next telemetry tick includes a deprecation warning.

## Step 8: Render bridge (src/render-bridge.ts)

- Define the directive protocol the future `nbe-engine` render node will consume: command name, resolved target references, payload, and the `stateVersion` it was issued at.
- Ship a loopback/mock bridge that accepts directives and records them for tests.
- The control plane MUST NOT block on the bridge: bridge writes are fire-and-forget with a bounded queue and overflow logging.
- The loopback/mock bridge must expose a test hook that lets tests assert which directives were received, in order, with their `stateVersion`. This hook is required for the CI gate; it does not introduce engine dependencies.

## Step 9: Tests and CI

`tsx --test` suites covering:

1. Envelope validation: malformed envelopes rejected with `E_BAD_PAYLOAD`.
2. Version conflict: stale `baseStateVersion` rejected with `E_VERSION_CONFLICT`.
3. Role matrix: each role's allowed/forbidden commands verified.
4. State machine: every legal transition in Section 17.3 executes; sampled illegal transitions rejected.
5. Aliases: `program.take` executes and emits a deprecation warning.
6. Audit log: every accepted command and every auth event is appended.
7. Telemetry: subscriber receives the Section 10.1 shape at the configured interval.
8. Snapshots: `snapshot.save` → `snapshot.recall` round-trip restores View state.
9. Automation hold: `automation.hold` suppresses triggers within one frame-tick.
10. `show.stop` quiescence truth table (Section 16.1).

Add a `control-plane` job to `.github/workflows/ci.yml`:

```yaml
  control-plane:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npm ci --prefix packages/control-plane
      - run: npx tsc --noEmit -p packages/control-plane
      - run: npm test --prefix packages/control-plane
```

The `control-plane` job must fail if any of the following are not true:

1. `npx tsc --noEmit -p packages/control-plane` passes.
2. All `tsx --test` suites pass.
3. The mock bridge test hook confirms that directives are recorded in order with correct `stateVersion`s.

## Constraints

- No engine dependencies and no video anywhere in this package. The render bridge is an interface with a mock implementation.
- All architecture decisions and semantics in `agents/prompts/02a-architecture-addendum.md` are normative for this prompt.
- Every command is schema-validated before any state mutation. Never mutate, then validate.
- `tsc` strict, `tsx --test` green, and the CI job must pass.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`. The strings `program` and `layer` appear only in the alias table and its tests.
