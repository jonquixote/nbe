//! Prompt 10 WU6 (SPEC §10.1 + §10.1.1): telemetry conformance for
//! `streamState` (the control plane's, "as commanded") + `streamBufferMs`
//! (the render node's).
//!
//! TDD: written BEFORE any WU6 implementation change (RED first). The
//! control-plane half of the split lives here; the engine half
//! (`streamBufferMs` on every engine tick: idle, live, stopped) lives in
//! `crates/nbe-engine/tests/prompt10_telemetry.rs`. Together they assert the
//! DoD: both fields present in EVERY emitted tick with lawful stubs.
//!
//! Ownership (§10.1's table): `streamState` is control-plane state and MUST
//! survive engine loss — a stale or absent engine report stubs the
//! engine-owned `streamBufferMs` but never moves `streamState`. The tests
//! below assert parsed VALUES equal known inputs (readability — an operator
//! can read them), not mere tolerance.

import { test } from "node:test";
import assert from "node:assert/strict";

import { EngineTelemetryFrameSchema } from "./protocol.js";
import {
  buildTick,
  ingestEngineFrame,
  newWorldTelemetry,
  ENGINE_TELEMETRY_TTL_MS,
} from "./telemetry.js";
import { ControlPlaneState } from "./state.js";

/** A complete engine frame; only `streamBufferMs` varies per test. */
function engineFrame(streamBufferMs: number): Record<string, unknown> {
  return {
    v: "0.3",
    kind: "engineTelemetry",
    ts: Date.now(),
    masterClockFrame: 54000,
    droppedFramesTotal: 0,
    renderGpuTimeMs: 4.7,
    decodeSessions: 4,
    vramUsedMib: 1830,
    textureCacheUsedMib: 512,
    streamBufferMs,
    recordSpaceMib: 512000,
    masterClockDriftMs: 0.2,
    fallbackActive: false,
    degradationRung: 0,
  };
}

// ---------------------------------------------------------------------------
// Token stability (the `path_tokens_are_stable` precedent): the three
// `streamState` tokens on the control-plane tick are normative wire tokens —
// a rename is a silent wire change with no guard unless pinned here.
// ---------------------------------------------------------------------------

test("stream state tokens are stable: idle, live, reconnecting", () => {
  const state = new ControlPlaneState();
  assert.equal(state.streamState, "idle", "a fresh control plane commands idle");

  for (const token of ["idle", "live", "reconnecting"] as const) {
    state.streamState = token;
    const tick = buildTick(state, newWorldTelemetry(), Date.now());
    assert.equal(
      tick.streamState,
      token,
      `streamState token "${token}" must reach the tick spelled exactly that way`,
    );
  }
});

// ---------------------------------------------------------------------------
// Parse: the engine schema REQUIRES streamBufferMs (always emitted, §10.1.1)
// and parses a distinctive value — never a stub colliding with the test.
// ---------------------------------------------------------------------------

test("engine telemetry requires streamBufferMs and parses a distinctive value", () => {
  const parsed = EngineTelemetryFrameSchema.safeParse(engineFrame(210));
  assert.equal(parsed.success, true, "a tick carrying streamBufferMs: 210 must parse");
  if (parsed.success) {
    assert.equal(parsed.data.streamBufferMs, 210, "the parsed value must equal the known input");
  }

  const { streamBufferMs: _dropped, ...without } = engineFrame(210);
  const missing = EngineTelemetryFrameSchema.safeParse(without);
  assert.equal(
    missing.success,
    false,
    "a tick WITHOUT streamBufferMs must be rejected whole — the engine always emits it (§10.1.1)",
  );
});

// ---------------------------------------------------------------------------
// Forward + readability: streamState + streamBufferMs present in EVERY
// control-plane tick (idle pre-start, live, stopped/stale) with lawful stubs.
// ---------------------------------------------------------------------------

test("ticks carry streamState and streamBufferMs in every phase, stubbed lawfully", () => {
  // Idle pre-start: no engine report yet. Both keys present, stubbed —
  // streamState "idle" (as commanded), streamBufferMs 0 (nothing to measure).
  {
    const state = new ControlPlaneState();
    const tick = buildTick(state, newWorldTelemetry(), Date.now());
    assert.ok("streamState" in tick, "pre-start tick must carry streamState, never omit it");
    assert.ok("streamBufferMs" in tick, "pre-start tick must carry streamBufferMs, never omit it");
    assert.equal(tick.streamState, "idle");
    assert.equal(tick.streamBufferMs, 0);
    assert.equal(tick.engineConnected, false);
  }

  // Live: a distinctive engine value (210, not the 0 stub) parses AND forwards,
  // and the commanded state reads back — readable, not merely tolerated.
  {
    const state = new ControlPlaneState();
    state.streamState = "live";
    const world = newWorldTelemetry();
    const now = Date.now();
    const parsed = EngineTelemetryFrameSchema.parse(engineFrame(210));
    ingestEngineFrame(world, parsed, now);
    const tick = buildTick(state, world, now);
    assert.equal(tick.streamState, "live", "the commanded stream state must be readable");
    assert.equal(
      tick.streamBufferMs,
      210,
      "the engine's buffered-ms value must survive the wire intact",
    );
  }

  // Stopped / engine lost: a stale report stubs the engine-owned field but
  // MUST NOT move the control-plane-owned one — "as commanded" outlives the
  // engine that stopped reporting.
  {
    const state = new ControlPlaneState();
    state.streamState = "live";
    const world = newWorldTelemetry();
    const then = Date.now() - ENGINE_TELEMETRY_TTL_MS - 1000;
    ingestEngineFrame(world, EngineTelemetryFrameSchema.parse(engineFrame(210)), then);
    const tick = buildTick(state, world, Date.now());
    assert.ok("streamState" in tick && "streamBufferMs" in tick, "stale ticks stay complete");
    assert.equal(tick.streamState, "live", "engine loss must not rewrite the commanded state");
    assert.equal(tick.streamBufferMs, 0, "a stale engine report stubs the buffer to 0");
    assert.equal(tick.engineConnected, false);
  }
});

// ---------------------------------------------------------------------------
// Stub collision (v0.4.4 stub rule): the engine's NEVER-RAN sentinel is -1.0 —
// negative ms is impossible — so it never collides with an honest drained-live
// 0.0. Both must survive the parse-and-forward wire intact and differ there.
// (The `?? 0` missing-field default for old engines is a distinct concern and
// stays; this pins the sentinel's readability, not the default.)
// ---------------------------------------------------------------------------

test("engine stub -1.0 forwards intact and differs from drained 0.0 on the wire", () => {
  const state = new ControlPlaneState();
  state.streamState = "live";
  const now = Date.now();

  const worldStub = newWorldTelemetry();
  ingestEngineFrame(worldStub, EngineTelemetryFrameSchema.parse(engineFrame(-1)), now);
  const stubTick = buildTick(state, worldStub, now);
  assert.equal(
    stubTick.streamBufferMs,
    -1,
    "the NEVER-RAN sentinel must survive the wire intact",
  );

  const worldDrained = newWorldTelemetry();
  ingestEngineFrame(worldDrained, EngineTelemetryFrameSchema.parse(engineFrame(0)), now);
  const drainedTick = buildTick(state, worldDrained, now);
  assert.equal(
    drainedTick.streamBufferMs,
    0,
    "a drained-live session honestly reports 0.0",
  );

  assert.notEqual(
    stubTick.streamBufferMs,
    drainedTick.streamBufferMs,
    "stub (-1.0) and drained-live (0.0) must be distinguishable on the wire",
  );
});
