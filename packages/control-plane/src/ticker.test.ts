//! Section 16.7 ticker ordering. Prompt 07b Step 6 test 3.
//!
//! The ordering rules were implemented before this file existed and were never
//! asserted — only the payload's *shape* was, in `protocol.test.ts`. A sort
//! nothing tests is a sort that drifts, and the insertion-order tiebreak is
//! exactly the property a non-stable sort silently breaks.

import { test } from "node:test";
import assert from "node:assert/strict";

import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { MockRenderBridge } from "./render-bridge.js";

const noPersist = { onDirty: () => {}, flushNow: () => {} };

function makeDeps(): { deps: DispatchDeps; state: ControlPlaneState } {
  const state = new ControlPlaneState();
  const pkg: PackageInfo = {
    packagePath: "/tmp/none",
    showId: "show-1",
    houseRate: 30,
    manifestVersion: "0.4",
    qualityProfile: undefined,
    items: new Map(),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(["ol_ticker"]),
    templates: new Set(["tpl_ticker"]),
    breakingTemplates: new Set(),
    // The precondition every ticker command checks.
    tickerExists: true,
    clockElements: new Set(),
    plugins: new Set(),
    automationRules: new Set(),
    assets: new Map(),
    transitionPresets: new Map(),
    fallbackAssetId: undefined,
  };
  state.loadPackage(pkg);
  return { deps: { state, bridge: new MockRenderBridge(), persistence: noPersist }, state };
}

async function run(d: DispatchDeps, command: string, payload: Record<string, unknown>) {
  return dispatch(d, buildRegistry(d), {
    connectionId: "c1",
    role: "operator" as never,
    envelope: { v: "0.3", id: "00000000-0000-0000-0000-000000000000", command, payload },
  });
}

test("§16.7 rule 2: higher priority appears before lower", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", {
    mode: "replace",
    items: [
      { text: "low", priority: 1 },
      { text: "high", priority: 90 },
      { text: "middle", priority: 50 },
    ],
  });
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    ["high", "middle", "low"],
  );
});

test("§16.7 rule 3: equal priority preserves insertion order", async () => {
  const { deps, state } = makeDeps();
  // Ten items at one priority. A non-stable sort reorders some of these and a
  // three-item test would very likely miss it; ten makes an accidental pass
  // improbable rather than merely unlikely.
  const items = Array.from({ length: 10 }, (_, i) => ({ text: `item${i}`, priority: 5 }));
  await run(deps, "ticker.override", { mode: "replace", items });
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    items.map((i) => i.text),
    "equal priority must come back in the order it went in",
  );
});

test("§16.7 rules 2 and 3 together: priority wins, insertion breaks ties", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", {
    mode: "replace",
    items: [
      { text: "a", priority: 10 },
      { text: "b", priority: 20 },
      { text: "c", priority: 10 },
      { text: "d", priority: 20 },
      { text: "e", priority: 10 },
    ],
  });
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    ["b", "d", "a", "c", "e"],
    "the two 20s keep their relative order, then the three 10s keep theirs",
  );
});

test("prepend and append place items, then the ordering rules apply to the whole queue", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", { mode: "replace", items: [{ text: "first", priority: 5 }] });
  await run(deps, "ticker.override", { mode: "append", items: [{ text: "appended", priority: 5 }] });
  assert.deepEqual(state.tickerItems.map((i) => i.text), ["first", "appended"]);

  await run(deps, "ticker.override", { mode: "prepend", items: [{ text: "urgent", priority: 5 }] });
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    ["urgent", "first", "appended"],
    "prepend puts the item ahead of equal-priority items already queued",
  );

  // Priority still outranks position: a low-priority prepend does not jump the
  // queue just because it was prepended.
  await run(deps, "ticker.override", { mode: "prepend", items: [{ text: "quiet", priority: 0 }] });
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    ["urgent", "first", "appended", "quiet"],
    "priority outranks insertion position",
  );
});

test("§16.7 rule 4: language is metadata and does not reorder", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", {
    mode: "replace",
    items: [
      { text: "arabic", priority: 5, language: "ar" },
      { text: "english", priority: 5, language: "en" },
    ],
  });
  assert.deepEqual(state.tickerItems.map((i) => i.text), ["arabic", "english"]);
  assert.equal(state.tickerItems[0]!.language, "ar", "language survives as metadata");
});

test("clearOverride empties the queue", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", { mode: "replace", items: [{ text: "x", priority: 1 }] });
  await run(deps, "ticker.clearOverride", {});
  assert.deepEqual(state.tickerItems, []);
});
