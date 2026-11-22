//! Review4 step 2 (the user's ruling of 2026-11-16 23:45, work order 6): the
//! ticker's caps (30 items, 120 characters, refused by name, never truncated)
//! and its TTL, enforced by the control plane on a 1 s timer that sends the
//! engine the resolved queue. The engine never sees a TTL.

import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { AuditLog } from "./audit.js";
import { expireTickerItems } from "./commands/ticker.js";
import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { CpError } from "./protocol.js";
import { MockRenderBridge } from "./render-bridge.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { tempDir } from "./test-tmp.js";

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

const items = (n: number, text = "ITEM"): Array<{ text: string; priority: number }> =>
  Array.from({ length: n }, (_, i) => ({ text: `${text} ${i}`, priority: 0 }));

async function refused(p: Promise<unknown>, pattern: RegExp): Promise<void> {
  await assert.rejects(p, (e: unknown) => {
    assert.ok(e instanceof CpError, `a CpError: ${String(e)}`);
    assert.equal(e.code, "E_BAD_PAYLOAD");
    assert.match(e.message, pattern);
    return true;
  });
}

test("30 items are accepted; 31 are refused, naming the limit", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", { mode: "replace", items: items(30) });
  assert.equal(state.tickerItems.length, 30);
  await refused(run(deps, "ticker.override", { mode: "replace", items: items(31) }), /at most 30 items/);
  assert.equal(state.tickerItems.length, 30, "nothing changed");
});

test("120 characters are accepted; 121 are refused, naming the limit, never truncated", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", { mode: "replace", items: [{ text: "x".repeat(120) }] });
  assert.equal(state.tickerItems[0]!.text.length, 120);
  await refused(run(deps, "ticker.override", { mode: "replace", items: [{ text: "y".repeat(121) }] }), /at most 120 characters/);
  assert.equal(state.tickerItems[0]!.text, "x".repeat(120), "nothing changed");
});

test("an append or prepend past 30 is refused by its total", async () => {
  const { deps, state } = makeDeps();
  await run(deps, "ticker.override", { mode: "replace", items: items(29) });
  await refused(run(deps, "ticker.override", { mode: "append", items: items(2, "MORE") }), /this append would make 31/);
  await refused(run(deps, "ticker.override", { mode: "prepend", items: items(2, "MORE") }), /this prepend would make 31/);
  assert.equal(state.tickerItems.length, 29, "nothing changed");
});

test("an item's TTL counts from acceptance; it expires on the control plane's clock, and the engine never sees a TTL", async () => {
  const { deps, state } = makeDeps();
  const bridge = deps.bridge as MockRenderBridge;
  const before = Date.now();
  await run(deps, "ticker.override", { mode: "replace", items: [{ text: "SOON", ttlSec: 2 }, { text: "STAYS" }] });
  const at = state.tickerItems.find((i) => i.text === "SOON")!.expiresAtMs!;
  assert.ok(at >= before + 2000 && at <= Date.now() + 2000, `expires 2 s after acceptance: ${at - before} ms`);
  const sent = bridge.drain().find((d) => d.command === "ticker.override")!;
  assert.deepEqual(
    (sent.payload as { items: unknown[] }).items,
    [
      { text: "SOON", priority: 0 },
      { text: "STAYS", priority: 0 },
    ],
    "no ttlSec, no expiry on the wire",
  );
  assert.equal(expireTickerItems(state, at - 1), 0, "not before");
  assert.equal(expireTickerItems(state, at), 1, "at its time");
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    ["STAYS"],
  );
});

test("the server's 1 s timer expires an item and sends the engine the resolved queue", async (t) => {
  const state = new ControlPlaneState();
  const bridge = new MockRenderBridge();
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: {} },
    audit: new AuditLog(join(tempDir("nbe-ticker-ttl-"), "audit.jsonl")),
    state,
    bridge,
    persistence: noPersist,
  });
  t.after(() => server.close());
  state.tickerItems = [
    { text: "GOES", priority: 0, ttlSec: 1, expiresAtMs: Date.now() + 200 },
    { text: "STAYS", priority: 0 },
  ];
  const v0 = state.stateVersion;
  const deadline = Date.now() + 3000;
  while (state.tickerItems.length !== 1 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
  assert.deepEqual(
    state.tickerItems.map((i) => i.text),
    ["STAYS"],
    "expired by the timer",
  );
  assert.equal(state.stateVersion, v0 + 1, "one detection transaction");
  const sent = bridge.drain().filter((d) => d.command === "ticker.override");
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0]!.payload, { items: [{ text: "STAYS", priority: 0 }], mode: "replace" });
  assert.equal(sent[0]!.stateVersion, v0 + 1);
});

test("with the audit file unwritable, the timer still expires the item, and the process lives (a throw would leave the timer and end it)", async (t) => {
  const state = new ControlPlaneState();
  const bridge = new MockRenderBridge();
  const auditPath = join(tempDir("nbe-ticker-ttl-ro-"), "audit.jsonl");
  writeFileSync(auditPath, "");
  chmodSync(auditPath, 0o444);
  t.after(() => chmodSync(auditPath, 0o644));
  const audit = new AuditLog(auditPath);
  const server = await createControlPlaneServer({ port: 0, auth: { tokens: {} }, audit, state, bridge, persistence: noPersist });
  t.after(() => server.close());
  state.tickerItems = [{ text: "GOES", priority: 0, ttlSec: 1, expiresAtMs: Date.now() + 200 }];
  const deadline = Date.now() + 3000;
  while (state.tickerItems.length !== 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
  assert.equal(state.tickerItems.length, 0, "expired by the timer");
  assert.equal(audit.sinkOk, false, "its audit row failed, as arranged");
  await new Promise((r) => setTimeout(r, 1200)); // one more timer tick, alive
  assert.deepEqual(bridge.drain().filter((d) => d.command === "ticker.override").length, 1);
});
