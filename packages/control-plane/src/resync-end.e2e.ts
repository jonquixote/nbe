//! SPEC v0.4.8 row 3, end to end: a restarted engine still ends the timed item.
//! The real control plane, the real `nbe-engine` binary, killed and restarted
//! mid-item, and the real protocol between them.
//!
//! The defect it guards: `on_resync` re-applied the View but never scheduled
//! the on-air timed item's end, and the snapshot carried no duration. So an
//! engine restart stranded a timed item on air: no `itemEvent end` ever fired,
//! and autoFollow stopped dead mid-rundown. The resync now carries
//! `viewItemEnd`, the item and its REMAINING time (clamped at zero), and the
//! engine schedules it.
//!
//! Not in `npm test`: it needs the engine binary. CI builds it in the
//! control-plane job and runs this file with its own floors. Run locally:
//!   node --import tsx --test src/resync-end.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine).

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
/** The longest wait for an end, an advance, or an engine to (de)register. */
const REPORT_MS = 20_000;
/** The house rate the engine runs at here (`NBE_HOUSE_RATE`): one frame is 33.3 ms. */
const FPS = 30;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess | undefined;
const engineLog: string[] = [];
let ws: WebSocket;
let auditPath = "";

function send(command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error(`no response to ${command}`)), 120_000);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      clearTimeout(timer);
      ws.off("message", onMsg);
      res(msg);
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function ok(command: string, payload: Record<string, unknown> = {}): Promise<void> {
  const r = await send(command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
}

function auditRecords(): Array<Record<string, unknown>> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

/** The audit's autoFollow advances for `itemRef`: `[actor, outcome]`. */
function advancesOf(itemRef: string): Array<[string, string]> {
  return auditRecords()
    .filter((r) => r["event"] === "autoFollow.advance" && r["actor"] === `autoFollow:${itemRef}`)
    .map((r) => [String(r["actor"]), String(r["outcome"])]);
}

/** The resyncs that re-established `itemRef`'s end, as the audit recorded them. */
function resyncEndsOf(itemRef: string): Array<Record<string, unknown>> {
  return auditRecords()
    .filter((r) => r["event"] === "resync.viewItemEnd")
    .map((r) => r["detail"] as Record<string, unknown>)
    .filter((d) => d["itemRef"] === itemRef);
}

/** Wait for `pred`, polling; returns the elapsed ms, or null at the deadline. */
async function until(pred: () => boolean, ms: number): Promise<number | null> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (pred()) return Date.now() - t0;
    await new Promise((r) => setTimeout(r, 25));
  }
  return null;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Start the engine and wait until the control plane has registered it (and sent it `show.resync`). */
async function startEngine(): Promise<void> {
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_HOUSE_RATE: String(FPS),
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  engine.stdout?.on("data", (b: Buffer) => engineLog.push(b.toString()));
  engine.stderr?.on("data", (b: Buffer) => engineLog.push(b.toString()));
  const registered = await until(() => server.wsBridge.renderNodeCount() === 1, REPORT_MS);
  if (registered === null) throw new Error(`engine never registered:\n${engineLog.join("")}`);
}

/** Kill the engine outright (no shutdown path) and wait until the control plane has dropped it. */
async function killEngine(): Promise<void> {
  engine?.kill("SIGKILL");
  const gone = await until(() => server.wsBridge.renderNodeCount() === 0, REPORT_MS);
  if (gone === null) throw new Error("the control plane never noticed the engine go");
}

function writePackage(): string {
  const dir = tempDir("nbe-resync-end-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Resync end" },
      show: {
        id: "show-resync-end",
        title: "Resync end",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets: [{ id: "fallback", kind: "image", source: "media/fallback.png" }],
      scenes: [{ id: "SCN", elements: [{ id: "main", kind: "clip", z: 1, assetId: "fallback" }] }],
      rundown: {
        id: "R",
        items: [
          // 8 s, then autoFollow to R2: restarted mid-item.
          { id: "R1", kind: "sceneRef", sceneRef: "SCN", durationFrames: 240, autoFollow: true },
          { id: "R2", kind: "sceneRef", sceneRef: "SCN" },
          // 2 s, then autoFollow to S2: its duration elapses during the outage.
          { id: "S1", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
          { id: "S2", kind: "sceneRef", sceneRef: "SCN" },
          // Untimed, with autoFollow (inert on an untimed item): nothing ends.
          { id: "U1", kind: "sceneRef", sceneRef: "SCN", autoFollow: true },
          { id: "U2", kind: "sceneRef", sceneRef: "SCN" },
        ],
      },
      control: { bindings: [] },
    }),
  );
  return dir;
}

before(async () => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  state = new ControlPlaneState();
  auditPath = join(tempDir("nbe-resync-end-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  await startEngine();
  ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  await ok("show.load", { packagePath: writePackage() });
  await ok("show.start", {});
});

after(async () => {
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[v0.4.8 row 3] an engine restart mid-item: the item ends at its remaining time, and autoFollow advances once", async () => {
  // Before the fix the restarted engine never scheduled R1's end: the View
  // stayed on R1 for good, and autoFollow never advanced.
  await ok("view.cut", { itemRef: "R1" });
  const t0 = Date.now();
  await sleep(3000);
  const down = Date.now();
  await killEngine();
  await startEngine();
  const outageMs = Date.now() - down;
  const advanced = await until(() => state.viewItem === "R2", REPORT_MS);
  const endedAfterMs = Date.now() - t0;
  assert.ok(
    advanced !== null,
    `autoFollow must advance R1 -> R2 at R1's end after the restart; the View is still ${state.viewItem} (outage ${outageMs} ms)`,
  );
  // R1 is 8000 ms from the take. Its full duration from the restart would end
  // it near 3000 + outage + 8000 ms; never ending it is the pre-fix tree.
  assert.ok(
    endedAfterMs >= 8000 * 0.9 && endedAfterMs <= 8000 + 1500,
    `R1 must end at its own 8000 ms from the take, not its full duration from the restart: ended ${endedAfterMs} ms after the take (delta ${endedAfterMs - 8000} ms; outage ${outageMs} ms)`,
  );
  const [end] = resyncEndsOf("R1");
  assert.deepEqual(
    {
      r1: state.itemStateOf("R1"),
      view: state.viewItem,
      advances: advancesOf("R1"),
      resyncCarriedTimeLeft: end !== undefined && (end["remainingFrames"] as number) > 0 && (end["remainingFrames"] as number) < 240,
    },
    { r1: "DONE", view: "R2", advances: [["autoFollow:R1", "ok"]], resyncCarriedTimeLeft: true },
    `R1 completed once, the View moved on, and the resync carried R1's time left: ${JSON.stringify(end)}`,
  );
});

test("[v0.4.8 row 3] an item whose duration elapsed during the outage ends on receipt of the resync, and the audit says so", async () => {
  // The clamp: S1 is 2000 ms, and the engine is away from 500 ms to 3000 ms.
  // Its end is overdue when the engine returns, so it fires on receipt.
  await ok("view.cut", { itemRef: "S1" });
  const t0 = Date.now();
  await sleep(500);
  await killEngine();
  await sleep(Math.max(0, t0 + 3000 - Date.now()));
  await startEngine();
  const registeredAt = Date.now();
  const advanced = await until(() => state.viewItem === "S2", REPORT_MS);
  assert.ok(advanced !== null, `autoFollow must advance S1 -> S2 once the engine is back; the View is still ${state.viewItem}`);
  const sinceResyncMs = Date.now() - registeredAt;
  assert.ok(sinceResyncMs <= 1500, `the overdue end fires on receipt of the resync, not later: ${sinceResyncMs} ms after the engine returned`);
  const [end] = resyncEndsOf("S1");
  assert.ok(end !== undefined, "the audit records the resync that re-established S1's end");
  assert.deepEqual(
    {
      s1: state.itemStateOf("S1"),
      advances: advancesOf("S1"),
      remainingFrames: end["remainingFrames"],
      overdue: (end["overdueMs"] as number) >= 500,
    },
    { s1: "DONE", advances: [["autoFollow:S1", "ok"]], remainingFrames: 0, overdue: true },
    `S1 ended late by the outage, on purpose, and the audit says what happened: ${JSON.stringify(end)}`,
  );
});

test("[v0.4.8 row 3] an untimed item across a restart: nothing ends", async () => {
  // "An untimed item never ends" (v0.4.8 row 2) holds across a restart: the
  // resync carries no end for U1, so autoFollow (inert on an untimed item)
  // never advances.
  await ok("view.cut", { itemRef: "U1" });
  await killEngine();
  await startEngine();
  await sleep(2500);
  assert.deepEqual(
    { view: state.viewItem, u1: state.itemStateOf("U1"), advances: advancesOf("U1"), resyncEnds: resyncEndsOf("U1") },
    { view: "U1", u1: "LIVE", advances: [], resyncEnds: [] },
    "U1 stays on air, untimed: no end re-established, no advance",
  );
});
