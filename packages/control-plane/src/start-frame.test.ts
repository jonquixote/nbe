// The take's start frame: the engine's clock, estimated now, not its last
// report (the recovery finding preserved from #131). The report trails the
// engine by up to a telemetry interval (1 s by default), and a resync of a
// healthy engine sets the on-air item's start from the control plane's
// belief (directive.rs on_resync), so a stale belief jumped the item forward
// by up to a second on every reconnect or gap resync.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ControlPlaneState, type PackageInfo } from "./state.js";

function state30(): ControlPlaneState {
  const s = new ControlPlaneState();
  s.loadPackage({
    packagePath: "/tmp/none",
    showId: "show-1",
    houseRate: 30,
    manifestVersion: "0.4",
    qualityProfile: undefined,
    items: new Map([["A1", { id: "A1", kind: "sceneRef", durationFrames: 300 }]]),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(),
    templates: new Set(),
    breakingTemplates: new Set(),
    tickerExists: false,
    clockElements: new Set(),
    plugins: new Set(),
    automationRules: new Set(),
    automation: [],
    bindings: new Map(),
    assets: new Map(),
    transitionPresets: new Map(),
    fallbackAssetId: undefined,
  } as PackageInfo);
  return s;
}

test("the engine's clock is extrapolated from its last report while it advances, within the freshness window", () => {
  const s = state30();
  s.noteMasterFrame(100, 10_000);
  assert.equal(s.estimatedMasterFrame(10_500), 100, "one report: not yet known to advance");
  s.noteMasterFrame(130, 11_000);
  assert.equal(s.estimatedMasterFrame(11_500), 145, "half a second at 30 fps past frame 130");
  assert.equal(s.estimatedMasterFrame(13_000), 190, "at the window's edge");
  assert.equal(s.estimatedMasterFrame(13_001), 130, "past the window, the report is all there is");
  s.noteMasterFrame(130, 12_000);
  assert.equal(s.estimatedMasterFrame(12_500), 130, "a clock that did not advance is not extrapolated");
});

test("a take's start frame is the estimate, and the resync carries it", () => {
  const s = state30();
  s.showState = "RUNNING";
  s.itemStates.set("A1", "ARMED");
  const now = Date.now();
  s.noteMasterFrame(100, now - 1500);
  s.noteMasterFrame(130, now - 500);
  s.take("A1");
  const start = s.viewItemStartFrame!;
  assert.ok(start >= 144 && start <= 147, `the start is the engine's frame now (about 145), not its last report (130): ${start}`);
  assert.equal(s.resyncSnapshot(Date.now()).viewItemStartFrame, start);
});

test("the resync carries the show's clock, continued past the freshness window, for a restarted engine to resume (SPEC §5.9.4, v0.4.16 row 1)", () => {
  const s = state30();
  s.noteMasterFrame(100, 10_000);
  s.noteMasterFrame(130, 11_000);
  assert.equal(s.continuedMasterFrame(12_000), null, "a show not RUNNING has no clock to continue");
  s.showState = "RUNNING";
  assert.equal(s.continuedMasterFrame(12_000), 160, "one second at 30 fps past frame 130");
  // The failover drill's case: the old engine's last report is 1 s old at the
  // kill, and the resync goes out 1.7–2.7 s later. Past the two-second window
  // the take's estimate stops at the report, and this does not.
  assert.equal(s.estimatedMasterFrame(14_700), 130, "the take's estimate is capped");
  assert.equal(s.continuedMasterFrame(14_700), 241, "the continued clock is not: 3.7 s past frame 130");
  assert.equal(s.resyncSnapshot(14_700)["masterFrame"], 241, "and the resync carries it");
  s.noteMasterFrame(130, 15_000);
  assert.equal(s.continuedMasterFrame(16_000), null, "a clock that did not advance is not continued");
});
