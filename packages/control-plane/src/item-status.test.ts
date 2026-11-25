//! SPEC §5.4.1, v0.4.21 row 1: the `itemStatus` push frame. One frame per
//! rundown item whose state changed, to every observer, whatever changed it:
//! an operator command or the render node's own `itemEvent`. Before it, an
//! item the engine ended, or found MISSING or in ERROR, changed state with
//! no frame naming the item (the v0.5 outline's §3a: the loop back to
//! whoever authored the item).

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { PROTOCOL_VERSION } from "./protocol.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { tempDir } from "./test-tmp.js";

const RENDER = "render-token";
const OPERATOR = "operator-token";

type Frame = Record<string, unknown>;

async function onAir(t: { after: (fn: () => void | Promise<void>) => void }) {
  const state = new ControlPlaneState();
  state.loadPackage({
    packagePath: "/tmp/none",
    showId: "s",
    houseRate: 30,
    qualityProfile: undefined,
    items: new Map([
      ["C1", { id: "C1", kind: "sceneRef", durationFrames: 300 }],
      ["C2", { id: "C2", kind: "sceneRef", durationFrames: 300 }],
    ]),
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
    auth: { tokens: { [RENDER]: "render", [OPERATOR]: "operator" } },
    audit: new AuditLog(join(tempDir("nbe-item-status-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
  t.after(() => server.close());
  const open = (token: string, role: string) =>
    new Promise<WebSocket>((resolve, reject) => {
      const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
        headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
      });
      t.after(() => ws.close());
      ws.once("open", () => resolve(ws));
      ws.once("error", reject);
    });
  const engine = await open(RENDER, "render");
  const operator = await open(OPERATOR, "operator");
  const seen: Frame[] = [];
  operator.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Frame;
    if (msg["kind"] === "itemStatus") seen.push(msg);
  });
  const until = async (check: () => boolean, ms = 3000) => {
    const end = Date.now() + ms;
    while (!check() && Date.now() < end) await new Promise((r) => setTimeout(r, 10));
    return check();
  };
  const send = (command: string, payload: Record<string, unknown>) =>
    new Promise<Frame>((resolve, reject) => {
      const id = randomUUID();
      const timer = setTimeout(() => reject(new Error(`no response to ${command}`)), 5000);
      const onMsg = (buf: Buffer) => {
        const msg = JSON.parse(buf.toString("utf8")) as Frame;
        if (msg["requestId"] !== id) return;
        clearTimeout(timer);
        operator.off("message", onMsg);
        resolve(msg);
      };
      operator.on("message", onMsg);
      operator.send(JSON.stringify({ v: PROTOCOL_VERSION, id, command, payload }));
    });
  return { state, engine, seen, until, send };
}

test("an item the render node ends reaches observers as itemStatus PLAYING → DONE, with the change's stateVersion", async (t) => {
  const r = await onAir(t);
  assert.equal(r.state.itemStateOf("C1"), "PLAYING");
  r.engine.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "itemEvent", itemRef: "C1", event: "end" }));
  assert.ok(await r.until(() => r.seen.length > 0), "an itemStatus frame arrived");
  assert.deepEqual(r.seen, [
    { v: PROTOCOL_VERSION, kind: "itemStatus", itemRef: "C1", from: "PLAYING", to: "DONE", stateVersion: r.state.stateVersion },
  ]);
});

test("a MISSING item and an ERROR item each name the item", async (t) => {
  const missing = await onAir(t);
  missing.engine.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "itemEvent", itemRef: "C1", event: "missing" }));
  assert.ok(await missing.until(() => missing.seen.length > 0));
  assert.equal(missing.seen[0]?.["itemRef"], "C1");
  assert.equal(missing.seen[0]?.["to"], "MISSING");

  const error = await onAir(t);
  error.engine.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "itemEvent", itemRef: "C1", event: "decodeError" }));
  assert.ok(await error.until(() => error.seen.length > 0));
  assert.equal(error.seen[0]?.["to"], "ERROR");
});

test("an operator command's item changes reach observers too: arming a READY item is READY → ARMED, and untouched items send nothing", async (t) => {
  const r = await onAir(t);
  const res = await r.send("item.arm", { itemId: "C2" });
  assert.equal(res["status"], "ok", JSON.stringify(res));
  assert.ok(await r.until(() => r.seen.length > 0));
  assert.deepEqual(
    r.seen.map((f) => [f["itemRef"], f["from"], f["to"]]),
    [["C2", "READY", "ARMED"]],
  );
  assert.equal(r.seen[0]?.["stateVersion"], res["stateVersion"]);
});
