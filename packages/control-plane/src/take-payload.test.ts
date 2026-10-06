//! PR #37's fix-forward: the take invariant, and the 0-frame mix that broke it.
//!
//! The invariant (the user's word of 2026-10-06): no command path mutates
//! state before its directive validates. The order is build → validate →
//! mutate → send, on all three take-class paths: view.take, view.cut and
//! snapshot.recall. The two-key pass found the other order (P1): a spec-legal
//! `view.take { transition: "mix", durationFrames: 0 }` resolved to an
//! `audio.durationFrames` of 0, the take payload's parse refused it AFTER
//! `state.take`, and the control plane's View moved while the engine never
//! heard of the take.
//!
//! Through the same dispatch pipeline every command uses (no binary needed).
//! The end-to-end leg, with the real engine, is in `take-duration.e2e.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";

import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { ControlPlaneState, type PackageInfo, type PackageItem } from "./state.js";
import { MockRenderBridge } from "./render-bridge.js";

const noPersist = { onDirty: () => {}, flushNow: () => {} };

function makeDeps(items: PackageItem[]): { deps: DispatchDeps; state: ControlPlaneState; bridge: MockRenderBridge } {
  const state = new ControlPlaneState();
  const pkg: PackageInfo = {
    packagePath: "/tmp/none",
    showId: "show-1",
    houseRate: 30,
    manifestVersion: "0.4",
    qualityProfile: undefined,
    items: new Map(items.map((i) => [i.id, i])),
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
    automation: [],
    bindings: new Map(),
    assets: new Map(),
    transitionPresets: new Map(),
    fallbackAssetId: undefined,
  };
  state.loadPackage(pkg);
  const bridge = new MockRenderBridge();
  return { deps: { state, bridge, persistence: noPersist }, state, bridge };
}

async function run(d: DispatchDeps, command: string, payload: Record<string, unknown>) {
  return dispatch(d, buildRegistry(d), {
    connectionId: "c1",
    role: "operator" as never,
    envelope: { v: "0.3", id: "00000000-0000-0000-0000-000000000000", command, payload },
  });
}

/** Everything a take-class command can write, as one comparable value. */
function written(state: ControlPlaneState): Record<string, unknown> {
  return {
    viewItem: state.viewItem,
    viewItemStartFrame: state.viewItemStartFrame,
    previewItem: state.previewItem,
    itemStates: Object.fromEntries(state.itemStates),
    visibleOverlays: Array.from(state.visibleOverlays),
    fallbackActive: state.fallbackActive,
    automationHold: state.automationHold,
    stateVersion: state.stateVersion,
  };
}

test("[P1] a 0-frame mix is a cut: accepted, and the control plane and its directive agree", async () => {
  const { deps, state, bridge } = makeDeps([{ id: "A", kind: "sceneRef", sceneRef: "S", durationFrames: 30 }]);
  await run(deps, "preview.set", { itemRef: "A" });
  bridge.drain();
  const before = state.stateVersion;
  let outcome = "ok";
  try {
    await run(deps, "view.take", { transition: "mix", durationFrames: 0 });
  } catch (e) {
    outcome = `refused: ${String(e).replace(/\s+/g, " ").slice(0, 120)}`;
  }
  // One value, so a failure shows the whole picture: whether the take was
  // accepted, what the control plane says is on air, and what the engine got.
  assert.deepEqual(
    {
      outcome,
      viewItem: state.viewItem,
      a: state.itemStateOf("A"),
      bumps: state.stateVersion - before,
      directives: bridge.drain().map((d) => ({ command: d.command, target: d.target, payload: d.payload })),
    },
    {
      outcome: "ok",
      viewItem: "A",
      a: "PLAYING",
      bumps: 1,
      directives: [
        {
          command: "view.take",
          target: { itemRef: "A" },
          // A cut: no transition length, so no audio length to copy. The
          // item's own duration rides as always.
          payload: { transition: "cut", audio: { transition: "follow" }, itemDurationFrames: 30 },
        },
      ],
    },
    "a 0-frame mix resolves as a cut, and the control plane and the engine's directive say the same",
  );
});

// The invariant, path by path. No accepted command produces an unreadable take
// payload now, so each test forces one: item Z's duration is 0, which the
// manifest schema refuses (`Item.durationFrames` minimum 1) but this in-test
// package carries, and `itemDurationFrames: 0` fails the take payload's parse.
// Each refusal must leave the control plane exactly where it was, with nothing
// sent: the refusal comes before the first write.

const Z: PackageItem = { id: "Z", kind: "sceneRef", sceneRef: "S", durationFrames: 0 };
const B: PackageItem = { id: "B", kind: "sceneRef", sceneRef: "S" };

async function refusedUntouched(
  deps: DispatchDeps,
  state: ControlPlaneState,
  bridge: MockRenderBridge,
  command: string,
  payload: Record<string, unknown>,
): Promise<void> {
  bridge.drain();
  const before = written(state);
  await assert.rejects(
    run(deps, command, payload),
    (e: unknown) => String(e).includes("itemDurationFrames"),
    `${command}: refused by the take payload's own parse (not some earlier guard)`,
  );
  assert.deepEqual(
    { state: written(state), directives: bridge.drain().map((d) => d.command) },
    { state: before, directives: [] },
    `${command}: the refusal came before any write, and nothing was sent`,
  );
}

test("the take invariant: view.take refuses an unreadable directive with the control plane untouched", async () => {
  const { deps, state, bridge } = makeDeps([Z, B]);
  await run(deps, "view.cut", { itemRef: "B" }); // something on air to leave
  await run(deps, "preview.set", { itemRef: "Z" }); // Z armed; preview.set builds no take payload
  await refusedUntouched(deps, state, bridge, "view.take", {});
});

test("the take invariant: view.cut refuses an unreadable directive with the control plane untouched", async () => {
  const { deps, state, bridge } = makeDeps([Z, B]);
  await run(deps, "view.cut", { itemRef: "B" });
  // Z is READY, so the cut would arm it first: the arm is a write too.
  await refusedUntouched(deps, state, bridge, "view.cut", { itemRef: "Z" });
});

test("the take invariant: snapshot.recall refuses an unreadable directive with the control plane untouched", async () => {
  const { deps, state, bridge } = makeDeps([Z, B]);
  await run(deps, "view.cut", { itemRef: "B" });
  // A snapshot whose View is Z, written directly: Z could never be taken here.
  state.snapshots.set("on-z", {
    viewItem: "Z",
    previewItem: null,
    itemStates: { Z: "PLAYING", B: "READY" },
    visibleOverlays: ["bug"],
    automationHold: true,
  });
  await refusedUntouched(deps, state, bridge, "snapshot.recall", { name: "on-z" });
});
