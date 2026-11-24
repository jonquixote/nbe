//! SPEC §16.2, §7.16 (v0.4.20 row 1): `view.ftb` fades the whole View to
//! black, or back. The control plane holds `ftbActive`, forwards the command
//! as sent, carries it in the resync and the journal, and clears it on a load.
//! A take does not lift it.

import { test } from "node:test";
import assert from "node:assert/strict";

import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { journalOf } from "./persistence.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { MockRenderBridge } from "./render-bridge.js";

const noPersist = { onDirty: () => {}, flushNow: () => {} };

function makeDeps(): { deps: DispatchDeps; state: ControlPlaneState; bridge: MockRenderBridge } {
  const state = new ControlPlaneState();
  state.loadPackage({
    packagePath: "/tmp/none",
    showId: "show-1",
    houseRate: 30,
    manifestVersion: "0.3",
    qualityProfile: undefined,
    items: new Map(),
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
  const bridge = new MockRenderBridge();
  return { deps: { state, bridge, persistence: noPersist }, state, bridge };
}

async function run(d: DispatchDeps, command: string, payload: Record<string, unknown>, role = "operator") {
  return dispatch(d, buildRegistry(d), {
    connectionId: "c1",
    role: role as never,
    envelope: { v: "0.3", id: "00000000-0000-0000-0000-000000000000", command, payload },
  });
}

test("view.ftb fades the View: ftbActive, the command forwarded as sent, the resync and the journal carry it", async () => {
  const { deps, state, bridge } = makeDeps();
  const r = await run(deps, "view.ftb", { on: true, durationFrames: 45 });
  assert.deepEqual(r.data, { ftbActive: true });
  assert.equal(state.ftbActive, true);
  const sent = bridge.drain();
  assert.equal(sent.length, 1);
  assert.equal(sent[0]!.command, "view.ftb");
  assert.deepEqual(sent[0]!.payload, { on: true, durationFrames: 45 }, "the render node gets the ramp as sent");
  assert.equal(state.resyncSnapshot()["ftbActive"], true, "a resynced render node lands faded");
  assert.equal(journalOf(state)["ftbActive"], true, "a restarted control plane remembers it");

  await run(deps, "view.ftb", { on: false });
  assert.equal(state.ftbActive, false, "and it fades back");
});

test("view.ftb refuses a payload without `on`, and a monitor", async () => {
  const { deps } = makeDeps();
  await assert.rejects(() => run(deps, "view.ftb", {}), /E_BAD_PAYLOAD|on/);
  await assert.rejects(() => run(deps, "view.ftb", { on: true, durationFrames: 0 }), /E_BAD_PAYLOAD|durationFrames/);
  await assert.rejects(() => run(deps, "view.ftb", { on: true }, "monitor"), /E_AUTH|role/);
});
