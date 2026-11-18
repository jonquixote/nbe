//! The held prefix, the control plane's side (the user's ruling of 2026-11-18
//! 00:15, item 3). The engine reports a clip that stopped decoding mid-play,
//! the View holding its last frame, as `itemEvent decodeError` whose detail
//! begins `HELD_DETAIL_PREFIX` ("held:"). The control plane reads exactly that
//! prefix: the item goes ERROR without the fallback and `item.failed` raises.
//! Any other decodeError is the plain path: ERROR with the fallback. The
//! engine's side is `clip_retry.rs`; the two constants are pinned equal by
//! `mirror.rs`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { HELD_DETAIL_PREFIX, PROTOCOL_VERSION } from "./protocol.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { tempDir } from "./test-tmp.js";

const RENDER = "render-token";

async function onAir(t: { after: (fn: () => void | Promise<void>) => void }) {
  const state = new ControlPlaneState();
  state.loadPackage({
    packagePath: "/tmp/none",
    showId: "s",
    houseRate: 30,
    qualityProfile: undefined,
    items: new Map([["C1", { id: "C1", kind: "sceneRef", durationFrames: 300 }]]),
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
  } as unknown as PackageInfo);
  state.showState = "RUNNING";
  state.armItem("C1");
  state.take("C1");
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-held-prefix-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
  t.after(() => server.close());
  const engine = await new Promise<WebSocket>((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: `Bearer ${RENDER}`, "x-nbe-role": "render" },
    });
    t.after(() => ws.close());
    ws.once("open", () => resolve(ws));
    ws.once("error", reject);
  });
  const decodeError = async (detail: string | undefined) => {
    const before = state.stateVersion;
    engine.send(
      JSON.stringify({ v: PROTOCOL_VERSION, kind: "itemEvent", itemRef: "C1", event: "decodeError", ...(detail === undefined ? {} : { detail }) }),
    );
    const end = Date.now() + 2000;
    while (state.stateVersion === before && Date.now() < end) await new Promise((r) => setTimeout(r, 10));
    assert.ok(state.stateVersion > before, "the item event was handled");
  };
  return { state, decodeError };
}

test("the prefix is \"held:\"", () => {
  assert.equal(HELD_DETAIL_PREFIX, "held:");
});

test("a decodeError whose detail begins \"held:\" is ERROR without the fallback, and item.failed raises with the engine's words", async (t) => {
  const r = await onAir(t);
  const detail = `${HELD_DETAIL_PREFIX} E_CLIP_DECODE: clip c1 stopped decoding at source frame 42; the View holds its last frame`;
  await r.decodeError(detail);
  assert.equal(r.state.itemStateOf("C1"), "ERROR");
  assert.equal(r.state.fallbackActive, false, "the slate is not on air, so no fallback");
  assert.equal(r.state.heldFailureOf("C1"), detail);
  const alert = r.state.alerts.find((a) => a.id === "item.failed");
  assert.ok(alert, "item.failed raises");
  assert.ok(alert.reason.includes("E_CLIP_DECODE"), alert.reason);
});

test("a decodeError whose detail only starts with the letters \"held\" is the plain path: ERROR with the fallback, no item.failed", async (t) => {
  const r = await onAir(t);
  await r.decodeError("heldover: the decoder rejected the stream");
  assert.equal(r.state.itemStateOf("C1"), "ERROR");
  assert.equal(r.state.fallbackActive, true, "the plain path raises the fallback");
  assert.equal(r.state.heldFailureOf("C1"), null);
  assert.equal(
    r.state.alerts.some((a) => a.id === "item.failed"),
    false,
  );
});

test("a decodeError with no detail is the plain path", async (t) => {
  const r = await onAir(t);
  await r.decodeError(undefined);
  assert.equal(r.state.fallbackActive, true);
  assert.equal(r.state.heldFailureOf("C1"), null);
});
