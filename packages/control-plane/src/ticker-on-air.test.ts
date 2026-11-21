//! `ticker.notOnAir` (the user's ruling of 2026-11-17 20:15): a ticker change
//! the engine did not apply is no longer silent. Once a ticker command has
//! settled (2 s), a fresh engine report whose `tickerOverrideItems` differs
//! from the control plane's count raises a critical alert with both counts;
//! it clears when they agree. No wire change: the count is on the tick (#102).

import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import WebSocket from "ws";

import { ALERT, AlertLane, OperatorSignals, TICKER_SETTLE_MS } from "./alerts.js";
import { AuditLog } from "./audit.js";
import { PROTOCOL_VERSION } from "./protocol.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import type { EngineReport } from "./telemetry.js";
import { tempDir } from "./test-tmp.js";

const items = (n: number) => Array.from({ length: n }, (_, i) => ({ text: `T${i}`, priority: 0 }));

/** The lane alone, on a clock the test moves by hand. */
function rig() {
  const state = new ControlPlaneState();
  const rows: Array<{ edge: string; id: string; event: string | null; cause: string | null }> = [];
  let t = 1_000_000;
  let report: EngineReport | null = null;
  const now = () => t;
  const lane = new AlertLane(state, (edge, a, cause) => rows.push({ edge, id: a.id, event: a.auditEvent, cause }), now);
  const signals = new OperatorSignals({
    state,
    lane,
    now,
    windowMs: 40,
    ttlMs: 2000,
    engineReport: () => report,
    auditSinkOk: () => true,
    correct: () => {},
  });
  signals.onRegistered();
  return {
    state,
    rows,
    signals,
    advance: (ms: number) => {
      t += ms;
    },
    /** A fresh report, received now, holding `n` ticker items (or no overlay line). */
    engineHolds: (n: number | null) => {
      const frame = {
        streamTransportState: "none",
        ...(n === null
          ? {}
          : {
              overlayState: {
                overlays: [],
                graphics: [],
                breaking: false,
                tickerOverrideItems: n,
                tickerSource: "manual",
                clocksConfigured: [],
                previewItem: null,
                textPrepMissesTotal: 0,
              },
            }),
      };
      report = { frame: frame as unknown as EngineReport["frame"], receivedAt: t };
    },
    alert: () => state.alerts.find((a) => a.id === ALERT.tickerNotOnAir.id),
  };
}

test("agreeing counts raise nothing", () => {
  const r = rig();
  r.state.tickerItems = items(3);
  r.advance(TICKER_SETTLE_MS);
  r.engineHolds(3);
  r.signals.evaluate();
  assert.equal(r.alert(), undefined);
});

test("a ticker change the engine did not apply raises once it has settled, critical, with both counts; it clears when they agree", () => {
  const r = rig();
  r.advance(TICKER_SETTLE_MS);
  r.state.tickerItems = items(3);
  r.engineHolds(3);
  r.signals.evaluate();
  // The operator's override: five items. The engine refuses it and keeps three.
  r.state.tickerItems = items(5);
  r.signals.onAccepted("ticker.override", 7);
  r.advance(TICKER_SETTLE_MS - 1);
  r.engineHolds(3);
  r.signals.evaluate();
  assert.equal(r.alert(), undefined, "not before the change has settled");
  r.advance(1);
  r.engineHolds(3);
  r.signals.evaluate();
  const a = r.alert();
  assert.ok(a, "raised once settled");
  assert.equal(a.severity, "critical");
  assert.match(a.reason, /the engine holds 3 items, the control plane 5/);
  assert.deepEqual(
    r.rows.map((x) => [x.edge, x.id, x.event]),
    [["raised", "ticker.notOnAir", "ticker.notOnAir"]],
    "one audit row, on the raise",
  );
  r.signals.evaluate();
  assert.equal(r.rows.length, 1, "edges only, never a row per evaluation");
  r.advance(1000);
  r.engineHolds(5);
  r.signals.evaluate();
  assert.equal(r.alert(), undefined, "cleared when they agree");
  assert.deepEqual(r.rows.at(-1), {
    edge: "cleared",
    id: "ticker.notOnAir",
    event: "ticker.notOnAir",
    cause: "the engine holds the 5 items commanded",
  });
});

test("a report from before the settle point decides nothing, however late it is read", () => {
  const r = rig();
  r.advance(TICKER_SETTLE_MS);
  r.state.tickerItems = items(4);
  r.signals.onAccepted("ticker.override", 3);
  r.engineHolds(0); // received at the change itself
  r.advance(TICKER_SETTLE_MS * 0.5);
  r.signals.evaluate();
  assert.equal(r.alert(), undefined);
});

test("a report with no overlay line decides nothing", () => {
  const r = rig();
  r.state.tickerItems = items(2);
  r.advance(TICKER_SETTLE_MS * 2);
  r.engineHolds(null);
  r.signals.evaluate();
  assert.equal(r.alert(), undefined);
});

test("a command that is not a ticker change does not move the settle point", () => {
  const r = rig();
  r.state.tickerItems = items(2);
  r.signals.onAccepted("ticker.override", 2);
  r.advance(TICKER_SETTLE_MS);
  r.signals.onAccepted("automation.hold", 3);
  r.engineHolds(0);
  r.signals.evaluate();
  assert.ok(r.alert(), "an unrelated command right before the read does not hide the mismatch");
});

test("an engine that stops reporting clears it (engine.notReporting says why)", () => {
  const r = rig();
  r.state.tickerItems = items(2);
  r.advance(TICKER_SETTLE_MS);
  r.engineHolds(0);
  r.signals.evaluate();
  assert.ok(r.alert());
  r.advance(2001);
  r.signals.evaluate();
  assert.equal(r.alert(), undefined);
  assert.equal(r.rows.at(-1)?.cause, "the engine is not reporting");
});

test("end to end through the server: an override the engine does not apply raises ticker.notOnAir within the settle and a tick; it clears when the engine holds it", async (t) => {
  const ADMIN = "admin-token";
  const RENDER = "render-token";
  const state = new ControlPlaneState();
  state.loadPackage({
    packagePath: "/tmp/none",
    showId: "s",
    houseRate: 30,
    qualityProfile: undefined,
    items: new Map(),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(["ol_ticker"]),
    templates: new Set(["tpl_ticker"]),
    breakingTemplates: new Set(),
    tickerExists: true,
    clockElements: new Set(),
    plugins: new Set(),
    automationRules: new Set(),
    automation: [],
    bindings: new Map(),
    assets: new Map(),
    transitionPresets: new Map(),
    fallbackAssetId: undefined,
  } as unknown as PackageInfo);
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-ticker-on-air-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
  t.after(() => server.close());
  const open = (role: string, token: string) =>
    new Promise<WebSocket>((resolve, reject) => {
      const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
        headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
      });
      t.after(() => ws.close());
      ws.once("open", () => resolve(ws));
      ws.once("error", reject);
    });
  // A scripted engine that applies nothing of the ticker: it reports what it holds.
  let holds = 0;
  const engine = await open("render", RENDER);
  const tick = () =>
    engine.send(
      JSON.stringify({
        v: PROTOCOL_VERSION,
        kind: "engineTelemetry",
        ts: Date.now(),
        masterClockFrame: 10,
        droppedFramesTotal: 0,
        renderGpuTimeMs: 1,
        decodeSessions: 0,
        vramUsedMib: 0,
        textureCacheUsedMib: 0,
        streamBufferMs: -1,
        recordSpaceMib: 0,
        masterClockDriftMs: 0,
        fallbackActive: false,
        degradationRung: 0,
        streamTransportState: "none",
        overlayState: {
          overlays: [],
          graphics: [],
          breaking: false,
          tickerOverrideItems: holds,
          tickerSource: "manual",
          clocksConfigured: [],
          previewItem: null,
          textPrepMissesTotal: 0,
        },
      }),
    );
  tick();
  const timer = setInterval(tick, 100);
  t.after(() => clearInterval(timer));
  const admin = await open("admin", ADMIN);
  const reply = await new Promise<Record<string, unknown>>((resolve) => {
    admin.on("message", (b: Buffer) => {
      const msg = JSON.parse(b.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId === "00000000-0000-0000-0000-000000000001") resolve(msg);
    });
    admin.send(
      JSON.stringify({ v: "0.3", id: "00000000-0000-0000-0000-000000000001", command: "ticker.override", payload: { mode: "replace", items: items(2) } }),
    );
  });
  assert.equal(reply.status, "ok", JSON.stringify(reply));
  const until = async (pred: () => boolean, ms: number) => {
    const end = Date.now() + ms;
    while (Date.now() < end && !pred()) await new Promise((r) => setTimeout(r, 25));
    return pred();
  };
  const raisedIn = Date.now();
  assert.ok(await until(() => state.alerts.some((a) => a.id === "ticker.notOnAir"), TICKER_SETTLE_MS + 2000), "raised");
  assert.ok(Date.now() - raisedIn >= TICKER_SETTLE_MS - 100, "not before the settle");
  assert.match(state.alerts.find((a) => a.id === "ticker.notOnAir")!.reason, /the engine holds 0 items, the control plane 2/);
  holds = 2;
  assert.ok(await until(() => !state.alerts.some((a) => a.id === "ticker.notOnAir"), 2000), "cleared when the engine holds it");
});
