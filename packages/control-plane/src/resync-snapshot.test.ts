//! SPEC v0.4.8 row 3: the resync carries the on-air timed item's end.
//!
//! `show.resync` re-applied the View on a restarted engine but carried no
//! duration, so the item's end was never scheduled again: an engine restart
//! stranded a timed item on air, and autoFollow stopped mid-rundown. The
//! snapshot now carries `viewItemEnd`, the item and its REMAINING time: its own
//! `itemDurationFrames` less the wall-clock time since the control plane's take,
//! rounded up to a whole frame and clamped to `[0, duration]`. These pin the
//! computation; `resync-end.e2e.ts` restarts a real engine.
//!
//! Through the same dispatch pipeline every command uses (no binary needed).
//! The snapshot takes `now` explicitly, so the elapsed time is exact.

import { test } from "node:test";
import assert from "node:assert/strict";

import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { ControlPlaneState, type PackageInfo, type PackageItem } from "./state.js";
import { MockRenderBridge } from "./render-bridge.js";

const noPersist = { onDirty: () => {}, flushNow: () => {} };

/** T1 is 90 frames (3000 ms at 30 fps), U1 is untimed. */
const T1: PackageItem = { id: "T1", kind: "sceneRef", sceneRef: "S", durationFrames: 90 };
const U1: PackageItem = { id: "U1", kind: "sceneRef", sceneRef: "S" };

function makeDeps(): { deps: DispatchDeps; state: ControlPlaneState } {
  const state = new ControlPlaneState();
  const pkg: PackageInfo = {
    packagePath: "/tmp/none",
    showId: "show-1",
    houseRate: 30,
    manifestVersion: "0.4",
    qualityProfile: undefined,
    items: new Map([T1, U1].map((i) => [i.id, i])),
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

/** The snapshot's end, `elapsedMs` after the take. */
function endAfter(state: ControlPlaneState, elapsedMs: number): unknown {
  const takenAt = state.viewItemTakenAtMs;
  assert.notEqual(takenAt, null, "the take recorded its wall-clock time");
  return (state.resyncSnapshot(takenAt! + elapsedMs) as { viewItemEnd?: unknown }).viewItemEnd;
}

test("[v0.4.8 row 3] the snapshot carries a timed item's remaining time from the take, rounded up", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "view.cut", { itemRef: "T1" });
  assert.deepEqual(
    {
      at0: endAfter(state, 0),
      at1000: endAfter(state, 1000),
      // 1990 ms left is 59.7 frames: rounded UP, so rounding never ends it early.
      at1010: endAfter(state, 1010),
      at2990: endAfter(state, 2990),
    },
    {
      at0: { itemRef: "T1", remainingFrames: 90 },
      at1000: { itemRef: "T1", remainingFrames: 60 },
      at1010: { itemRef: "T1", remainingFrames: 60 },
      at2990: { itemRef: "T1", remainingFrames: 1 },
    },
    "T1 is 3000 ms; the remaining frames are what is left of it",
  );
});

test("[v0.4.8 row 3] the remaining time is clamped: zero once the duration has elapsed, never more than the duration", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "view.cut", { itemRef: "T1" });
  assert.deepEqual(
    {
      atTheEnd: endAfter(state, 3000),
      longOverdue: endAfter(state, 60_000),
      // A wall clock stepped backwards must not lengthen the item.
      clockBehind: endAfter(state, -5000),
    },
    {
      atTheEnd: { itemRef: "T1", remainingFrames: 0 },
      longOverdue: { itemRef: "T1", remainingFrames: 0 },
      clockBehind: { itemRef: "T1", remainingFrames: 90 },
    },
    "an overdue item ends on receipt (zero); the duration is the ceiling",
  );
});

test("[v0.4.8 row 3] no end for an untimed item, an item that already ended, or an empty View", async () => {
  const { deps, state } = makeDeps();
  const snap = () => (state.resyncSnapshot() as { viewItemEnd?: unknown }).viewItemEnd;
  const empty = snap();
  await run(deps, "view.cut", { itemRef: "U1" });
  const untimed = snap();
  await run(deps, "view.cut", { itemRef: "T1" });
  state.markDone("T1"); // the engine's end arrived: PLAYING -> DONE
  const done = snap();
  assert.deepEqual(
    { empty, untimed, done, keyPresent: "viewItemEnd" in (state.resyncSnapshot() as object) },
    { empty: undefined, untimed: undefined, done: undefined, keyPresent: false },
    "the key is absent, not null: only a PLAYING timed item has an end to re-establish",
  );
});

test("[v0.4.8 row 3] a recall onto a timed item starts its clock at the recall", async () => {
  // A recall starts the item now (v0.4.8 rows 1 and 2), so a resync after it
  // counts from the recall, not from the take the snapshot was saved under.
  const { deps, state } = makeDeps();
  await run(deps, "view.cut", { itemRef: "T1" });
  await run(deps, "snapshot.save", { name: "on-t1" });
  await run(deps, "view.cut", { itemRef: "U1" });
  state.viewItemTakenAtMs = null; // poison: the recall must set it afresh
  await run(deps, "snapshot.recall", { name: "on-t1" });
  assert.deepEqual(endAfter(state, 500), { itemRef: "T1", remainingFrames: 75 }, "500 ms after the recall, 2500 ms remain");
});

test("[v0.4.8 row 5] the snapshot names the package with its load generation, and no package names neither", async () => {
  // `show.load`'s handler records the generation (its own stateVersion); here
  // it is set as the handler sets it. The e2e asserts the real handler.
  const { state } = makeDeps();
  const pair = () => {
    const s = state.resyncSnapshot() as Record<string, unknown>;
    return { packagePath: s["packagePath"], packageLoadStateVersion: s["packageLoadStateVersion"] };
  };
  const ungenerationed = pair(); // loaded outside the command path: no generation
  state.packageLoadStateVersion = 4;
  const loaded = pair();
  state.unloadPackage();
  assert.deepEqual(
    { ungenerationed, loaded, unloaded: pair() },
    {
      ungenerationed: { packagePath: "/tmp/none", packageLoadStateVersion: null },
      loaded: { packagePath: "/tmp/none", packageLoadStateVersion: 4 },
      unloaded: { packagePath: null, packageLoadStateVersion: null },
    },
    "the pair travels together; an unload clears both",
  );
});
