//! Section 16.6 overlay.show / overlay.hide: idempotency, ENOTFOUND, role
//! enforcement, snapshot surfacing, stateVersion accounting. Runs through the
//! same dispatch pipeline every command uses (no binary dependency).

import { test } from "node:test";
import assert from "node:assert/strict";

import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { MockRenderBridge } from "./render-bridge.js";
import { buildTick, newWorldTelemetry } from "./telemetry.js";
import { CpError } from "./protocol.js";

const noPersist = { onDirty: () => {}, flushNow: () => {} };

function loadedState(): ControlPlaneState {
  const state = new ControlPlaneState();
  const pkg: PackageInfo = {
    packagePath: "/tmp/none",
    showId: "show-1",
    houseRate: 30,
    manifestVersion: "0.3",
    qualityProfile: undefined,
    items: new Map(),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(["bug"]),
    templates: new Set(),
    breakingTemplates: new Set(),
    tickerExists: false,
    clockElements: new Set(),
    plugins: new Set(),
    automationRules: new Set(),
    assets: new Map(),
    transitionPresets: new Map(),
    fallbackAssetId: undefined,
  };
  state.loadPackage(pkg);
  return state;
}

function makeDeps(): { deps: DispatchDeps; state: ControlPlaneState; bridge: MockRenderBridge } {
  const state = loadedState();
  const bridge = new MockRenderBridge();
  return { deps: { state, bridge, persistence: noPersist }, state, bridge };
}

async function run(
  d: DispatchDeps,
  command: string,
  payload: Record<string, unknown>,
  role = "operator",
) {
  return dispatch(d, buildRegistry(d), {
    connectionId: "c1",
    role: role as never,
    envelope: { v: "0.3", id: "00000000-0000-0000-0000-000000000000", command, payload },
  });
}

test("overlay.show puts the overlay on air, bumps once, surfaces in the snapshot", async () => {
  const { deps, state, bridge } = makeDeps();
  const before = state.stateVersion;

  const r = await run(deps, "overlay.show", { overlayId: "bug", animation: { durationFrames: 20 } });

  assert.equal(r.stateVersion, before + 1, "one bump per accepted command");
  assert.ok(state.visibleOverlays.has("bug"));
  const snapshot = state.resyncSnapshot() as { overlays: Array<{ id: string; onAir: boolean; animationState: string }> };
  assert.deepEqual(snapshot.overlays, [
    { id: "bug", onAir: true, animationState: "enter" },
  ]);
  // The directive is the engine-facing wire contract: overlayId rides `target`
  // (what on_overlay reads), the optional animation rides `payload`.
  const sent = bridge.drain();
  assert.equal(sent.length, 1);
  assert.equal(sent[0]!.command, "overlay.show");
  assert.deepEqual(sent[0]!.target, { overlayId: "bug" }, "engine reads overlayId from target");
  assert.deepEqual(sent[0]!.payload, { animation: { durationFrames: 20 } }, "the animation override reaches the render node verbatim");
});

test("a noop overlay command forwards no directive", async () => {
  const { deps, bridge } = makeDeps();
  await run(deps, "overlay.show", { overlayId: "bug" });
  bridge.drain(); // drop the real transition's directive

  await run(deps, "overlay.show", { overlayId: "bug" }); // idempotent no-op
  assert.equal(bridge.drain().length, 0, "a noop must not re-trigger the render node");
});

test("overlay.show on an on-air overlay is an idempotent noop", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "overlay.show", { overlayId: "bug" });
  const before = state.stateVersion;

  const r = await run(deps, "overlay.show", { overlayId: "bug" });

  // Still a dispatched command (accepted accounting), marked as a no-op.
  assert.equal(r.stateVersion, before + 1);
  assert.deepEqual(r.data, { noop: true });
  const snapshot = state.resyncSnapshot() as { overlays: Array<{ id: string; onAir: boolean }> };
  assert.equal(snapshot.overlays.length, 1, "still exactly one on-air overlay");
});

test("overlay.hide removes the overlay; hide on hidden is a noop", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "overlay.show", { overlayId: "bug" });
  await run(deps, "overlay.hide", { overlayId: "bug" });
  assert.ok(!state.visibleOverlays.has("bug"));
  const snapshot = state.resyncSnapshot() as { overlays: unknown[] };
  assert.equal(snapshot.overlays.length, 0, "a hidden overlay is not on air");

  const r = await run(deps, "overlay.hide", { overlayId: "bug" });
  assert.deepEqual(r.data, { noop: true }, "hide on a hidden overlay is idempotent");
});

test("unknown overlay refuses with ENOTFOUND; monitor is denied with E_AUTH", async () => {
  const { deps, state } = makeDeps();
  const before = state.stateVersion;

  await assert.rejects(
    () => run(deps, "overlay.show", { overlayId: "does-not-exist" }),
    (e: unknown) => e instanceof CpError && e.code === "E_NOT_FOUND",
  );
  await assert.rejects(
    () => run(deps, "overlay.hide", { overlayId: "does-not-exist" }),
    (e: unknown) => e instanceof CpError && e.code === "E_NOT_FOUND",
  );
  assert.equal(state.stateVersion, before, "a rejected command must not bump");

  await assert.rejects(
    () => run(deps, "overlay.show", { overlayId: "bug" }, "monitor"),
    (e: unknown) => e instanceof CpError && e.code === "E_AUTH",
  );
  await assert.rejects(
    () => run(deps, "overlay.hide", { overlayId: "bug" }, "monitor"),
    (e: unknown) => e instanceof CpError && e.code === "E_AUTH",
  );
});

test("overlay on-air state surfaces in the telemetry tick", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "overlay.show", { overlayId: "bug" });

  const tick = buildTick(state, newWorldTelemetry(), Date.now());
  assert.deepEqual(tick.visibleOverlays, ["bug"], "show/hide must be telemetry-visible");
  assert.deepEqual(state.statusSnapshot().visibleOverlays, ["bug"], "and /status-visible");
  await run(deps, "overlay.hide", { overlayId: "bug" });
  assert.deepEqual(buildTick(state, newWorldTelemetry(), Date.now()).visibleOverlays, []);
});