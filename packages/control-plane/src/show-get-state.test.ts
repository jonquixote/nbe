//! PROPOSAL (operator-shell draft, finding 1): `show.getState`, the loaded
//! show as an operator surface draws it. Not in SPEC v0.4 §16.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ControlPlaneState, type PackageInfo } from "./state.js";
import { roleAllowed } from "./dispatch.js";

function pkg(): PackageInfo {
  const items = new Map([
    ["A1", { id: "A1", kind: "clip", durationFrames: 150, autoFollow: true }],
    ["B2", { id: "B2", kind: "scene" }],
    ["B3", { id: "B3", kind: "scene" }],
  ]);
  return {
    packagePath: "/shows/dress",
    showId: "dress",
    houseRate: 30,
    manifestVersion: "0.4",
    qualityProfile: "consumer",
    items,
    sequences: new Set(["rundown"]),
    scenes: new Map(),
    elements: new Map([["lt1", { id: "lt1", kind: "lowerThird" }]]),
    overlays: new Set(["lt1"]),
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
  };
}

test("show.getState with no package: no rundown, no elements, the buses empty", () => {
  const v = new ControlPlaneState().showStateView();
  assert.equal(v.rundown, null);
  assert.equal(v.elements, null);
  assert.equal(v.package, null);
  assert.equal(v.viewItem, null);
});

test("show.getState: the rundown in manifest order, each item with its §17.1 state", () => {
  const s = new ControlPlaneState();
  s.loadPackage(pkg());
  s.armItem("B3");
  const v = s.showStateView() as {
    rundown: { id: string; items: { id: string; state: string; durationFrames: number | null; autoFollow: boolean }[] };
    elements: { id: string }[];
  };
  assert.equal(v.rundown.id, "rundown");
  assert.deepEqual(
    v.rundown.items.map((i) => [i.id, i.state]),
    [
      ["A1", "READY"],
      ["B2", "READY"],
      ["B3", "ARMED"],
    ],
  );
  assert.equal(v.rundown.items[0]!.durationFrames, 150);
  assert.equal(v.rundown.items[0]!.autoFollow, true);
  assert.deepEqual(v.elements.map((e) => e.id), ["lt1"]);
});

test("show.getState is read-only, so every role but render may issue it", () => {
  for (const role of ["monitor", "operator", "producer", "admin"] as const) {
    assert.ok(roleAllowed(role, "show.getState"), role);
  }
  assert.equal(roleAllowed("render", "show.getState"), false);
});
