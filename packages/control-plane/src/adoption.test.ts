//! Review4 step 7b (the user's ruling of 2026-11-16 23:45 on step 7): a
//! restarted control plane waits for the engine's hello and adopts the show
//! it is running when the package identity matches the journal's. Otherwise
//! it answers as before, and says why in the audit. While the journal says a
//! show was running, commands wait for the hello, up to the lock's length.

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { StatePersistence, type JournalBody } from "./persistence.js";
import { indexPackage } from "./package.js";
import { PROTOCOL_VERSION } from "./protocol.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";

function makePackage(): string {
  const dir = tempDir("nbe-adopt-");
  mkdirSync(join(dir, "media"), { recursive: true });
  for (const f of ["fallback", "A1", "A2", "B1"]) writeFileSync(join(dir, "media", `${f}.png`), "png");
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.3",
      network: { id: "nbe", name: "Test" },
      show: {
        id: "show-1",
        title: "Test",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets: ["fallback", "A1", "A2", "B1"].map((id) => ({ id, kind: "image", source: `media/${id}.png` })),
      scenes: ["A1", "A2", "B1"].map((id) => ({ id: `SCN_${id}`, elements: [{ id: `el_${id}`, kind: "clip", z: 1, assetId: id }] })),
      rundown: {
        id: "R",
        items: ["A1", "A2", "B1"].map((id) => ({ id, kind: "sceneRef", sceneRef: `SCN_${id}`, durationFrames: 900 })),
      },
      control: { bindings: [] },
    }),
  );
  return dir;
}

function journal(pkgPath: string, over: Record<string, unknown> = {}): JournalBody {
  return {
    stateVersion: 40,
    manifestIdentity: { packagePath: pkgPath, showId: "show-1" } as JournalBody["manifestIdentity"],
    recoveredAt: Date.now() - 200,
    journal: {
      showState: "RUNNING",
      packageLoadStateVersion: 5,
      viewItem: "A1",
      viewItemStartFrame: 90,
      previewItem: "A2",
      fallbackActive: false,
      itemStates: { A1: "PLAYING", A2: "PLAYING", B1: "DONE" },
      visibleOverlays: [],
      graphics: {},
      breakingVisible: false,
      breakingFields: null,
      tickerSource: "manual",
      tickerItems: [{ text: "FROM THE JOURNAL", priority: 3 }],
      audioBuses: {},
      guests: {},
      automationHold: true,
      automationRules: {},
      snapshots: {},
      markers: [],
      recordState: "recording",
      streamState: "idle",
      automationBacklog: 2,
      ...over,
    },
  };
}

const hello = (pkgPath: string, over: Record<string, unknown> = {}) => ({
  v: PROTOCOL_VERSION,
  kind: "engineHello",
  engineInstanceId: "engine-1",
  startedAtMs: Date.now() - 60_000,
  showRunning: true,
  masterFrame: 400,
  lastAppliedStateVersion: 44,
  package: { path: pkgPath, loadStateVersion: 5 },
  viewItem: "A1",
  viewItemStartFrame: 100,
  previewItem: "A2",
  visibleOverlays: [],
  fallbackActive: false,
  recording: true,
  streamTransportState: "none",
  ...over,
});

interface Rig {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  pkgPath: string;
  rows: () => Array<Record<string, unknown>>;
  /** A scripted engine: its directives, in order; it sends `say` on open. */
  engine: (say: Record<string, unknown> | null) => Promise<{ ws: WebSocket; directives: Array<Record<string, unknown>> }>;
  admin: () => Promise<WebSocket>;
}

async function rig(
  t: { after: (fn: () => void | Promise<void>) => void },
  opts: { restart?: boolean; journalOver?: Record<string, unknown>; lockMs?: number; helloWaitMs?: number } = {},
): Promise<Rig> {
  const pkgPath = makePackage();
  const auditPath = join(tempDir("nbe-adopt-audit-"), "audit.jsonl");
  const state = new ControlPlaneState();
  state.stateVersion = 40; // as restore() leaves it: the journal's version, UNLOADED
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    ...(opts.restart === false
      ? {}
      : {
          restart: {
            journal: journal(pkgPath, opts.journalOver),
            ...(opts.lockMs === undefined ? {} : { lockMs: opts.lockMs }),
            ...(opts.helloWaitMs === undefined ? {} : { helloWaitMs: opts.helloWaitMs }),
          },
        }),
  });
  t.after(() => server.close());
  // The listener is attached at construction: a frame the server sends at
  // connect can arrive with the upgrade, before an `await` would resume.
  const open = (role: string, token: string, onMessage?: (msg: Record<string, unknown>) => void): Promise<WebSocket> =>
    new Promise((resolve, reject) => {
      const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
        headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
      });
      if (onMessage) ws.on("message", (buf: Buffer) => onMessage(JSON.parse(buf.toString("utf8")) as Record<string, unknown>));
      t.after(() => ws.close());
      ws.once("open", () => resolve(ws));
      ws.once("error", reject);
    });
  return {
    server,
    state,
    pkgPath,
    rows: () =>
      existsSync(auditPath)
        ? readFileSync(auditPath, "utf8")
            .split("\n")
            .filter((l) => l.length > 0)
            .map((l) => JSON.parse(l) as Record<string, unknown>)
        : [],
    engine: async (say) => {
      const directives: Array<Record<string, unknown>> = [];
      const ws = await open("render", RENDER, (msg) => {
        if (msg.kind === "directive") directives.push(msg);
      });
      if (say !== null) ws.send(JSON.stringify(say));
      return { ws, directives };
    },
    admin: () => open("admin", ADMIN),
  };
}

function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`no response to ${command}`)), 5000);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      clearTimeout(timer);
      ws.off("message", onMsg);
      resolve(msg);
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function until(check: () => boolean, ms = 3000): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 10));
  }
  return check();
}

const resyncs = (d: Array<Record<string, unknown>>) => d.filter((x) => x.command === "show.resync");
const event = (r: Rig, name: string) => r.rows().filter((x) => x["event"] === name);

test("B: a matching running engine is adopted; its resync changes nothing on air and carries max(journal, engine) + 1", async (t) => {
  const r = await rig(t);
  const observer = await r.admin();
  const changes: Array<Record<string, unknown>> = [];
  observer.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === "stateChange") changes.push(msg);
  });
  const e = await r.engine(hello(r.pkgPath));
  assert.ok(await until(() => resyncs(e.directives).length === 1), "one resync, after the hello");
  const first = e.directives[0]!;
  assert.equal(first.command, "show.resync", "the first directive is the resync");
  const p = first.payload as Record<string, unknown>;
  assert.deepEqual(
    {
      showState: p.showState,
      packagePath: p.packagePath,
      packageLoadStateVersion: p.packageLoadStateVersion,
      viewItem: p.viewItem,
      viewItemStartFrame: p.viewItemStartFrame,
      previewItem: p.previewItem,
      hasEnd: "viewItemEnd" in p,
      stateVersion: first.stateVersion,
    },
    {
      showState: "RUNNING",
      packagePath: r.pkgPath,
      packageLoadStateVersion: 5,
      viewItem: "A1",
      viewItemStartFrame: 100,
      previewItem: "A2",
      hasEnd: false,
      stateVersion: 45,
    },
    "the engine's own facts, its own load identity (no reload), no end (it keeps its pending one), version max(40, 44) + 1",
  );
  assert.equal(r.state.showState, "RUNNING");
  assert.deepEqual(Object.fromEntries(r.state.itemStates), { A1: "PLAYING", A2: "DONE", B1: "DONE" }, "the View item PLAYING; another PLAYING is DONE");
  assert.equal(r.state.automationHold, true, "the journal's hold");
  assert.deepEqual(r.state.tickerItems, [{ text: "FROM THE JOURNAL", priority: 3 }], "the journal's ticker");
  assert.equal(r.state.recordState, "recording", "the engine records");
  const adopted = event(r, "controlPlane.adopted");
  assert.equal(adopted.length, 1);
  const d = adopted[0]!["detail"] as Record<string, unknown>;
  assert.deepEqual(
    [d["journalVersion"], d["engineLastApplied"], d["adoptedVersion"], d["automationDropped"], d["correctedToDone"], d["recordMismatch"], d["preflight"]],
    [40, 44, 45, 2, ["A2"], false, "not re-run"],
  );
  assert.ok(await until(() => changes.some((c) => (c.changed as string[]).includes("engine:adopted"))), "observers see the adopted state");
  // Its end, when a later resync carries one, is back-computed from the
  // engine's clock: 300 frames in, 600 of 900 left.
  const end = r.state.viewItemEnd();
  assert.ok(end !== undefined && Math.abs(end.remainingFrames - 600) <= 3, `remaining ${JSON.stringify(end)}`);
});

test("while the journal says a show was running, commands wait for the hello; reads do not", async (t) => {
  const r = await rig(t);
  const admin = await r.admin();
  const refused = await send(admin, "automation.hold", { hold: false });
  assert.equal(refused.status, "error");
  const err = refused.error as { code: string; message: string; details?: Record<string, unknown> };
  assert.equal(err.code, "E_FORBIDDEN_STATE");
  assert.equal(err.details?.["reason"], "recovering", err.message);
  assert.equal((await send(admin, "system.status")).status, "ok", "a read is answered");
  await r.engine(hello(r.pkgPath));
  assert.ok(await until(() => r.state.showState === "RUNNING"));
  assert.equal((await send(admin, "automation.hold", { hold: false })).status, "ok", "adopted: commands run");
});

test("the lock lifts after its length with no hello; a hello after it still adopts", async (t) => {
  const r = await rig(t, { lockMs: 300 });
  const admin = await r.admin();
  await new Promise((res) => setTimeout(res, 400));
  assert.equal((await send(admin, "automation.hold", { hold: true })).status, "ok", "unlocked by the timer");
  const e = await r.engine(hello(r.pkgPath));
  assert.ok(await until(() => resyncs(e.directives).length === 1));
  assert.equal((e.directives[0]!.payload as Record<string, unknown>).showState, "RUNNING", "a late hello is still case B");
});

test("D: an engine that is not running gets today's resync (UNLOADED); a saved file alone never starts a show", async (t) => {
  const r = await rig(t);
  const e = await r.engine(hello(r.pkgPath, { showRunning: false, viewItem: null, viewItemStartFrame: null, masterFrame: null }));
  assert.ok(await until(() => resyncs(e.directives).length === 1));
  assert.equal((e.directives[0]!.payload as Record<string, unknown>).showState, "UNLOADED");
  assert.equal(r.state.showState, "UNLOADED");
  assert.deepEqual(event(r, "controlPlane.notAdopted").map((x) => (x["detail"] as Record<string, unknown>)["reason"]), ["engineNotRunning"]);
  const admin = await r.admin();
  assert.equal((await send(admin, "automation.hold", { hold: true })).status, "ok", "commands run");
});

test("case C: a running engine whose load generation differs is not adopted, and not resynced either (that resync would stop its show); the operator's show.unload takes control", async (t) => {
  const r = await rig(t, { lockMs: 100 });
  const e = await r.engine(hello(r.pkgPath, { package: { path: r.pkgPath, loadStateVersion: 6 } }));
  // Held: past the hello wait, no resync reaches the engine, so its show runs on.
  await new Promise((res) => setTimeout(res, 2500));
  assert.equal(resyncs(e.directives).length, 0, "no resync for a show this control plane cannot identify");
  const rows = event(r, "controlPlane.notAdopted");
  assert.equal(rows.length, 1);
  assert.equal((rows[0]!["detail"] as Record<string, unknown>)["reason"], "identity");
  assert.equal(event(r, "controlPlane.adopted").length, 0);
  assert.equal(event(r, "controlPlane.refused").length, 1, "the refusal is audited");
  const alert = r.state.alerts.find((a) => a.id === "engine.unidentified");
  assert.ok(alert, "a critical alert says why");
  assert.equal(alert.severity, "critical");
  assert.match(alert.reason, /cannot identify \(it is not the package the journal names\)/);
  // The operator takes control: the held handshake completes with the
  // control plane's state, and the alert clears.
  const admin = await r.admin();
  assert.equal((await send(admin, "show.unload")).status, "ok");
  assert.ok(await until(() => resyncs(e.directives).length === 1), "the operator's choice sends the resync");
  assert.equal((e.directives[0]!.payload as Record<string, unknown>).showState, "UNLOADED");
  assert.ok(!r.state.alerts.some((a) => a.id === "engine.unidentified"), "the alert clears");
});

test("an engine that never says hello gets today's resync after the wait", async (t) => {
  const r = await rig(t, { helloWaitMs: 200 });
  const e = await r.engine(null);
  await new Promise((res) => setTimeout(res, 100));
  assert.equal(e.directives.length, 0, "nothing before the wait ends");
  assert.ok(await until(() => resyncs(e.directives).length === 1));
  assert.equal((e.directives[0]!.payload as Record<string, unknown>).showState, "UNLOADED");
  assert.deepEqual(event(r, "controlPlane.notAdopted").map((x) => (x["detail"] as Record<string, unknown>)["reason"]), ["noHello"]);
});

test("a control plane that did not restart resyncs at connect, as before, hello or not", async (t) => {
  const r = await rig(t, { restart: false });
  const e = await r.engine(null);
  assert.ok(await until(() => resyncs(e.directives).length === 1), "resync with no hello");
  const admin = await r.admin();
  assert.equal((await send(admin, "automation.hold", { hold: true })).status, "ok", "no lock");
});

test("after an adoption, a reconnect of the engine is answered at once with the adopted state (case A)", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine(hello(r.pkgPath));
  assert.ok(await until(() => resyncs(e1.directives).length === 1));
  e1.ws.close();
  await new Promise((res) => setTimeout(res, 50));
  const e2 = await r.engine(null);
  assert.ok(await until(() => resyncs(e2.directives).length === 1), "no wait for a hello this time");
  const p = e2.directives[0]!.payload as Record<string, unknown>;
  assert.equal(p.showState, "RUNNING");
  assert.equal(p.viewItem, "A1");
  assert.equal(event(r, "controlPlane.adopted").length, 1, "adopted once");
});

test("the journal carries the load generation and the automation backlog, and restore() keeps it for adoption", async () => {
  const state = new ControlPlaneState();
  state.loadPackage(indexPackage(makePackage()));
  state.packageLoadStateVersion = 5;
  state.stateVersion = 9;
  const path = join(tempDir("nbe-adopt-journal-"), "control-plane-state.json");
  const out = new StatePersistence(state, path);
  out.journalExtras(() => ({ automationBacklog: 3 }));
  out.onDirty();
  await out.flush();
  const back = new StatePersistence(new ControlPlaneState(), path);
  back.restore();
  const j = back.restoredJournal();
  assert.ok(j !== null, "a schema-2 journal is kept");
  assert.equal(j.journal["packageLoadStateVersion"], 5);
  assert.equal(j.journal["automationBacklog"], 3);
  assert.equal(j.stateVersion, 9);
});

test("case C: a running engine beside a journal that says no show was running is held too; its disconnect clears the alert", async (t) => {
  const r = await rig(t, { journalOver: { showState: "STOPPED" } });
  const e = await r.engine(hello(r.pkgPath));
  assert.ok(await until(() => r.state.alerts.some((a) => a.id === "engine.unidentified")), "the alert is raised");
  assert.match(r.state.alerts.find((a) => a.id === "engine.unidentified")!.reason, /the journal says no show was running/);
  assert.equal(resyncs(e.directives).length, 0, "no resync while held");
  e.ws.close();
  assert.ok(await until(() => !r.state.alerts.some((a) => a.id === "engine.unidentified")), "the alert clears when the engine goes");
});
