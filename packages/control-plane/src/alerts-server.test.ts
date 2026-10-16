//! The operator-signal lane on a real server and real sockets (SPEC §10.1.2,
//! v0.4.8 row 8; R17 spine part A), the engine scripted by the test: the lane
//! rides every shared surface, the held alert comes from the real dispatcher
//! path, an unacknowledged start raises after the window and not before, R18's
//! fallback answers a sink that really fails, and nothing a client read before
//! changes meaning.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
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
let warnings: string[];
let pkgPath: string;

function makePackage(): string {
  const dir = tempDir("nbe-lane-");
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
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
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        { id: "A1_clip", kind: "image", source: "media/A1.png" },
      ],
      scenes: [{ id: "SCN_A1", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_clip" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN_A1" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

const preflightAvailable = (): boolean => existsSync(preflightBin());

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

/**
 * A scripted engine: it reports every 100 ms (so its report stays fresh), and
 * acknowledges every directive except the commands in `refuse`.
 */
async function engine(
  t: { after: (fn: () => void) => void },
  transport: () => string,
  refuse: Set<string>,
): Promise<{ ws: WebSocket }> {
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
  return { ws };
}

function alertRows(): Array<Record<string, unknown>> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>)
    .filter((r) => r["command"] === "alert");
}

const idsOf = (alerts: unknown): string[] => ((alerts as Array<{ id: string }>) ?? []).map((a) => a.id);

async function httpStatus(): Promise<Record<string, unknown>> {
  return (await (await fetch(`http://127.0.0.1:${server.port}/nbe/v0.3/status`)).json()) as Record<string, unknown>;
}

beforeEach(async () => {
  pkgPath = makePackage();
  state = new ControlPlaneState();
  warnings = [];
  auditPath = join(tempDir("nbe-lane-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    showStopGraceMs: WINDOW,
    warn: (m) => warnings.push(m),
  });
});

afterEach(async () => {
  if (server) await server.close();
});

test("an unacknowledged stream.start raises after the window, not before, on every shared surface alike", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  // The engine refuses the start, and the probe command below too, so that no
  // later acknowledgement proves the start processed (that hands (b) to (a)).
  const refuse = new Set(["stream.start", "automation.hold"]);
  await engine(t, () => "none", refuse);
  const admin = conn("admin", ADMIN);
  t.after(() => admin.close());
  const ticks = collect(admin, "telemetry");
  const changes = collect(admin, "stateChange");
  await connect(admin);
  await send(admin, "system.telemetry.subscribe", { intervalMs: 100 });
  await send(admin, "show.load", { packagePath: pkgPath });
  await send(admin, "show.start", {});
  assert.equal((await send(admin, "stream.start", { url: "rtmp://127.0.0.1:9/live/k" })).status, "ok");
  const before = idsOf(((await send(admin, "system.status")).data as Record<string, unknown>).alerts);
  assert.deepEqual(before, [], "inside the window: nothing");
  assert.ok(await until(() => state.alerts.length > 0, 2000), "the window elapses and the alert raises");
  // Another command, so a stateChange frame is pushed after the raise.
  await send(admin, "automation.hold", { hold: true });
  await until(() => ticks.length > 0 && idsOf((ticks.at(-1)!.data as Record<string, unknown>).alerts).length > 0, 1000);
  const surfaces = {
    systemStatus: idsOf(((await send(admin, "system.status")).data as Record<string, unknown>).alerts),
    httpStatus: idsOf((await httpStatus()).alerts),
    stateChange: idsOf((changes.at(-1)!.state as Record<string, unknown>).alerts),
    tick: idsOf((ticks.at(-1)!.data as Record<string, unknown>).alerts),
  };
  assert.deepEqual(surfaces, {
    systemStatus: ["command.unacknowledged.stream"],
    httpStatus: ["command.unacknowledged.stream"],
    stateChange: ["command.unacknowledged.stream"],
    tick: ["command.unacknowledged.stream"],
  });
  const alert = state.alerts[0]!;
  assert.deepEqual(Object.keys(alert).sort(), ["auditEvent", "id", "reason", "severity", "since"], "the alert's shape");
  assert.match(alert.reason, /stream\.start \(stateVersion \d+\) was not acknowledged within 150 ms/);
  assert.deepEqual(
    alertRows().map((r) => [r["event"], (r["detail"] as Record<string, unknown>)["edge"], (r["detail"] as Record<string, unknown>)["alert"]]),
    [["command.unacknowledged", "raised", "command.unacknowledged.stream"]],
    "one audit row, on the raise",
  );
  assert.equal(state.streamState, "live", "part A changes no belief: the control plane still believes live");
  // The handoff: a later acknowledgement proves the start was processed (a
  // refusal), so (b) clears and (a), the standing contradiction, raises.
  refuse.delete("automation.hold");
  await send(admin, "automation.hold", { hold: false });
  assert.ok(await until(() => idsOf(state.alerts).includes("stream.divergence")), "(a) takes the contradiction over");
  assert.deepEqual(idsOf(state.alerts), ["stream.divergence"]);
  assert.match(state.alerts[0]!.reason, /believes the stream live; the engine reports it none/);
});

test("held: a show.stop the engine never acknowledges raises outputs.held with its audit row; the explicit stops clear it", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  let transport = "live";
  await engine(t, () => transport, new Set(["show.stop"]));
  const admin = conn("admin", ADMIN);
  t.after(() => admin.close());
  await connect(admin);
  await send(admin, "show.load", { packagePath: pkgPath });
  await send(admin, "show.start", {});
  await send(admin, "record.start", {});
  await send(admin, "stream.start", { url: "rtmp://127.0.0.1:9/live/k" });
  await send(admin, "show.stop", {});
  transport = "closed";
  assert.ok(await until(() => idsOf(state.alerts).includes("outputs.held")), "the held alert raises");
  assert.deepEqual(idsOf(state.alerts), ["outputs.held"], "and only it: live beside closed is the held alert's, not a divergence");
  assert.ok(warnings.some((w) => w.startsWith("show.stop: no acknowledgement")), "the console warn stays as it is");
  await send(admin, "record.stop", {});
  await send(admin, "stream.stop", {});
  assert.ok(await until(() => state.alerts.length === 0), "the heal clears it");
  const held = alertRows().filter((r) => (r["detail"] as Record<string, unknown>)["alert"] === "outputs.held");
  assert.deepEqual(
    held.map((r) => [r["event"], (r["detail"] as Record<string, unknown>)["edge"], (r["detail"] as Record<string, unknown>)["severity"]]),
    [
      ["show.stopHeld", "raised", "warning"],
      ["show.stopHeld", "cleared", "warning"],
    ],
  );
  assert.equal((held[1]!["detail"] as Record<string, unknown>)["cause"], "the held outputs were stopped");
});

test("R18: a sink that really fails raises audit.sinkFailed and the health flag, prints one stderr line, and writes no row for itself", async (t) => {
  const admin = conn("admin", ADMIN);
  t.after(() => admin.close());
  await connect(admin);
  await send(admin, "automation.hold", { hold: true }); // the file exists, written once
  // While the sink is broken a command's reply is R18's recorded E_ENGINE frame
  // under a fresh requestId, so these are sent without awaiting a reply.
  const fire = (hold: boolean) => admin.send(JSON.stringify({ v: "0.3", id: randomUUID(), command: "automation.hold", payload: { hold } }));
  const lines: string[] = [];
  const write = process.stderr.write.bind(process.stderr);
  process.stderr.write = ((chunk: string | Uint8Array, ...rest: unknown[]) => {
    if (String(chunk).startsWith("nbe audit sink failed")) lines.push(String(chunk));
    return (write as (...a: unknown[]) => boolean)(chunk, ...rest);
  }) as typeof process.stderr.write;
  t.after(() => {
    process.stderr.write = write;
    chmodSync(auditPath, 0o644);
  });
  chmodSync(auditPath, 0o444);
  fire(false); // its audit write fails
  fire(true); // and again: still one line per episode
  assert.ok(await until(() => state.alerts.some((a) => a.id === "audit.sinkFailed")), "the alert raises");
  const status = await httpStatus();
  assert.deepEqual(
    { alerts: idsOf(status.alerts), health: status.health, stderr: lines.length },
    { alerts: ["audit.sinkFailed"], health: { auditSinkOk: false }, stderr: 1 },
  );
  chmodSync(auditPath, 0o644);
  await send(admin, "automation.hold", { hold: false }); // the sink accepts a write
  assert.ok(await until(() => state.alerts.length === 0), "it clears when the sink works again");
  assert.deepEqual((await httpStatus()).health, { auditSinkOk: true });
  assert.deepEqual(
    alertRows().filter((r) => (r["detail"] as Record<string, unknown>)["alert"] === "audit.sinkFailed"),
    [],
    "no audit row was ever written for the sink's own failure",
  );
});

test("additive: the snapshot keeps every field it had, with the same meaning; lastError is still preflight's", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  const admin = conn("admin", ADMIN);
  t.after(() => admin.close());
  await connect(admin);
  await send(admin, "show.load", { packagePath: pkgPath });
  state.lastError = "a previous error";
  await send(admin, "show.preflight", {});
  const data = (await send(admin, "system.status")).data as Record<string, unknown>;
  assert.deepEqual(
    Object.keys(data).sort(),
    [
      "alerts",
      "automationHold",
      "fallbackActive",
      "health",
      "lastError",
      "masterClockState",
      "package",
      "preflightPassed",
      "previewItem",
      "qualityProfile",
      "recordState",
      "renderNode",
      "showState",
      "stateVersion",
      "streamState",
      "viewItem",
      "visibleOverlays",
    ],
    "the pre-existing fields, plus alerts and health",
  );
  assert.equal(data.lastError, null, "preflight still writes lastError (a pass clears it)");
});
