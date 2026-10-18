//! The stream belief follows the truth (SPEC §10.1.3–§10.1.4, v0.4.8 rows 10
//! and 11; R17 spine part B), on a real server and real sockets, the engine
//! scripted by the test:
//! - `held` is output-active, and the output commands name it (D3);
//! - a correction lands as a detection transaction: one bump, one
//!   `belief.corrected` row, no `stateChange` frame and no automation (D4);
//! - live→held fires on the command path, with its actual values, once, and a
//!   `to: "idle"` rule stays silent on a hold (§13.2's transition rule).

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { preflightBin } from "./package.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const WINDOW = 150;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let auditPath: string;

const preflightAvailable = (): boolean => existsSync(preflightBin());

/** A v0.4 package whose rules watch the stream belief: each adds a marker named for itself. */
function makePackage(automation: unknown[] = []): string {
  const dir = tempDir("nbe-belief-");
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Test" },
      show: {
        id: "show-belief",
        title: "Belief",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        { id: "A1_img", kind: "image", source: "media/A1.png" },
      ],
      scenes: [{ id: "SCN", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_img" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN" }] },
      control: { bindings: [] },
      automation,
    }),
  );
  return dir;
}

const rule = (id: string, params: Record<string, unknown>) => ({
  id,
  trigger: { kind: "stateChange", params: { field: "streamState", ...params } },
  action: { command: "marker.add", payload: { name: id } },
});

function conn(role: string, token: string): WebSocket {
  return new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
  });
}

function connect(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
}

function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`no response to ${command}`)), 15_000);
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

function collect(ws: WebSocket, kind: string): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = [];
  ws.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === kind) out.push(msg);
  });
  return out;
}

async function until(check: () => boolean, ms = 2000): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 10));
  }
  return check();
}

/** A scripted engine: it reports every 100 ms, and acknowledges every directive except those in `refuse`. */
async function engine(t: { after: (fn: () => void) => void }, transport: () => string, refuse: Set<string>): Promise<void> {
  const ws = conn("render", RENDER);
  ws.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind !== "directive" || refuse.has(msg.command as string)) return;
    ws.send(JSON.stringify({ v: "0.3", kind: "appliedStateVersion", stateVersion: msg.stateVersion }));
  });
  await connect(ws);
  const report = () =>
    ws.send(
      JSON.stringify({
        v: "0.3",
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
        streamTransportState: transport(),
      }),
    );
  report();
  const timer = setInterval(report, 100);
  t.after(() => {
    clearInterval(timer);
    ws.close();
  });
}

function rows(): Array<Record<string, unknown>> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

const markers = (): string[] => state.markers.map((m) => m.name);
const idsOf = (alerts: unknown): string[] => ((alerts as Array<{ id: string }>) ?? []).map((a) => a.id);

beforeEach(async () => {
  state = new ControlPlaneState();
  auditPath = join(tempDir("nbe-belief-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    showStopGraceMs: WINDOW,
    warn: () => {},
  });
});

afterEach(async () => {
  if (server) await server.close();
});

async function loadAndStart(automation: unknown[] = []): Promise<WebSocket> {
  const admin = conn("admin", ADMIN);
  await connect(admin);
  assert.equal((await send(admin, "show.load", { packagePath: makePackage(automation) })).status, "ok");
  assert.equal((await send(admin, "show.start", {})).status, "ok");
  await server.automation.settled();
  return admin;
}

// --- D3: held is output-active, and the output commands name it ------------------

test("D3: stream.start refuses held by name; stream.stop accepts held and refuses idle by name", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  const admin = await loadAndStart();
  t.after(() => admin.close());
  state.streamState = "held"; // §5.9.5's limbo, with no engine to correct it
  assert.equal(state.outputsActive(), true, "held is output-active");
  const start = await send(admin, "stream.start", { url: "rtmp://127.0.0.1:9/live/k" });
  assert.equal(start.status, "error");
  assert.deepEqual(start.error, { code: "E_FORBIDDEN_STATE", message: "stream is held: its stop was not acknowledged" });
  const stop = await send(admin, "stream.stop", {});
  assert.equal(stop.status, "ok", "stream.stop is the operator's way out of a hold");
  assert.equal(state.streamState, "idle");
  const again = await send(admin, "stream.stop", {});
  assert.equal(again.status, "error");
  assert.deepEqual(again.error, { code: "E_FORBIDDEN_STATE", message: "stream is idle" });
});

test("D3 and §5.9.5: show.stop unacknowledged makes a live stream held, raises the held alert, and leaves the recording recording", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  const admin = await loadAndStart();
  t.after(() => admin.close());
  // No render node: nothing acknowledges, nothing reports, so nothing corrects.
  assert.equal((await send(admin, "stream.start", { url: "rtmp://127.0.0.1:9/live/k" })).status, "ok");
  assert.equal((await send(admin, "record.start", {})).status, "ok");
  assert.equal((await send(admin, "show.stop", { quiesceOutputs: true })).status, "ok");
  assert.equal(state.streamState, "held");
  assert.equal(state.recordState, "recording", "§13.2's exception 2: no held for the record");
  assert.ok(idsOf(state.alerts).includes("outputs.held"), "the held alert raises");
});

// --- D4: a correction is a detection transaction ------------------------------------

test("D4: a correction is one bump and one belief.corrected row, with no stateChange frame and no automation", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  await engine(t, () => "none", new Set(["stream.start"]));
  const admin = await loadAndStart([rule("left-air", { from: "live" }), rule("to-idle", { to: "idle" })]);
  t.after(() => admin.close());
  const changes = collect(admin, "stateChange");
  assert.equal((await send(admin, "stream.start", { url: "rtmp://127.0.0.1:9/live/k" })).status, "ok");
  await server.automation.settled();
  const framesAfterStart = changes.length;
  const versionAfterStart = state.stateVersion;
  assert.ok(await until(() => state.streamState === "idle", 3000), "the expired claim gives way to the fresh report");
  await server.automation.settled();
  assert.equal(state.stateVersion, versionAfterStart + 1, "one bump");
  const corrected = rows().filter((r) => r["event"] === "belief.corrected");
  assert.equal(corrected.length, 1, "one audit row");
  assert.equal(corrected[0]!["kind"], "command");
  assert.equal(corrected[0]!["command"], "engine:report");
  const detail = corrected[0]!["detail"] as Record<string, unknown>;
  assert.deepEqual({ ...detail, reportAgeMs: typeof detail["reportAgeMs"] }, {
    field: "streamState",
    from: "live",
    to: "idle",
    report: "none",
    reportAgeMs: "number",
  });
  assert.equal(corrected[0]!["stateVersionAfter"], versionAfterStart + 1);
  await new Promise((r) => setTimeout(r, 300));
  assert.equal(changes.length, framesAfterStart, "no stateChange frame: §5.4.1 is one per accepted command");
  assert.deepEqual(markers(), [], "no automation: a from-live rule and a to-idle rule stay silent on a correction");
});

// --- §13.2's transition rule: live→held on the command path --------------------------

test("§13.2: live→held fires from:\"live\" and to:\"held\" once, on the command path; to:\"idle\" stays silent, and the correction after fires nothing", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  let transport = "live";
  await engine(t, () => transport, new Set(["show.stop"]));
  const admin = await loadAndStart([
    rule("left-air", { from: "live" }),
    rule("to-held", { to: "held" }),
    rule("to-idle", { to: "idle" }),
  ]);
  t.after(() => admin.close());
  assert.equal((await send(admin, "stream.start", { url: "rtmp://127.0.0.1:9/live/k" })).status, "ok");
  await server.automation.settled();
  transport = "closed"; // the engine quiesces, and withholds show.stop's acknowledgement
  assert.equal((await send(admin, "show.stop", { quiesceOutputs: true })).status, "ok");
  await server.automation.settled();
  // The witness is automation's own audit row per attempt: the rules' actions
  // (marker.add) are refused once the show is STOPPED, but each firing is a row.
  const attempts = () =>
    rows()
      .filter((r) => r["kind"] === "automation" && r["event"] === "automation.action")
      .map((r) => [r["actor"], (r["detail"] as Record<string, unknown>)["trigger"]]);
  const atHold = attempts();
  assert.deepEqual(
    atHold.map(([a]) => a).sort(),
    ["automation:left-air", "automation:to-held"],
    "live→held, once each; to:\"idle\" does not match a hold",
  );
  for (const [, trigger] of atHold) {
    assert.deepEqual(trigger, { kind: "stateChange", field: "streamState", from: "live", to: "held" }, "the actual values");
  }
  assert.ok(await until(() => state.streamState === "idle", 3000), "the next fresh report corrects held");
  await server.automation.settled();
  await new Promise((r) => setTimeout(r, 300));
  assert.deepEqual(attempts().length, atHold.length, "the correction held→idle fires nothing");
  assert.ok(idsOf(state.alerts).includes("outputs.held"), "and clears no alert");
});
