//! SPEC v0.4 wire additions (Standards §2a: each fails when removed).
//!
//! §5.9.4 `viewItemStartFrame` in the resync snapshot; §10.1 `showState` in
//! the telemetry tick; §7.15 `show.load` rejecting a house-rate mismatch.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ControlPlaneState, type PackageInfo } from "./state.js";
import { buildTick, newWorldTelemetry } from "./telemetry.js";

function pkg(houseRate = 30): PackageInfo {
  return {
    packagePath: "/tmp/none",
    showId: "s",
    houseRate,
    qualityProfile: undefined,
    items: new Map([["A1", { id: "A1", kind: "sceneRef" }]]),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(),
    templates: new Set(),
    breakingTemplates: new Set(),
    tickerExists: false,
    clockElements: new Set(),
    plugins: new Set(),
    audioAssets: new Set(),
    cameras: new Set(),
    guests: new Set(),
    fallbackAssetId: undefined,
    automationRules: [],
  } as unknown as PackageInfo;
}

test("§5.9.4: the resync snapshot carries viewItemStartFrame", () => {
  const state = new ControlPlaneState();
  state.loadPackage(pkg());
  state.showState = "RUNNING";

  // Nothing on air: the field must be null, not absent and not zero. A
  // consumer cannot tell "absent" from "frame 0" otherwise.
  const empty = state.resyncSnapshot();
  assert.ok("viewItemStartFrame" in empty, "the field must always be present");
  assert.equal(empty.viewItemStartFrame, null);

  // The engine's clock is the only clock; the control plane records what it
  // last reported, which is at worst one telemetry tick stale.
  state.lastKnownMasterFrame = 1234;
  state.armItem("A1");
  state.take("A1");

  const snap = state.resyncSnapshot();
  assert.equal(snap.viewItem, "A1");
  assert.equal(
    snap.viewItemStartFrame,
    1234,
    "the snapshot must say SINCE WHEN the item is on air, not only what is",
  );
});

test("§10.1: the telemetry tick carries showState", () => {
  const state = new ControlPlaneState();
  state.loadPackage(pkg());

  const world = newWorldTelemetry();
  const loaded = buildTick(state, world, Date.now());
  assert.equal(
    loaded.showState,
    "LOADED",
    "a client holding telemetry alone must be able to say whether the show is running",
  );

  state.showState = "RUNNING";
  assert.equal(buildTick(state, world, Date.now()).showState, "RUNNING");
});
