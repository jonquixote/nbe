//! SPEC v0.4.8 row 2, end to end: a timed item ends at ITS duration, on a cut
//! and a mix alike, and autoFollow advances from that end, once. This runs the
//! real control plane, the real `nbe-engine` binary, and the real protocol
//! between them.
//!
//! The defect it guards: the engine scheduled an item's end from the take's
//! `durationFrames`, which is the TRANSITION's length. A cut carries none, so a
//! timed item taken by a cut never ended, and autoFollow never advanced. A
//! 6-frame mix ended a 2-second item 0.2 s in, so autoFollow ran away. The take
//! payload now carries the item's own `itemDurationFrames`.
//!
//! Not in `npm test`: it needs the engine binary. CI builds it in the
//! control-plane job and runs this file with its own floors. Run locally:
//!   node --import tsx --test src/take-duration.e2e.ts
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
/**
 * The longest wait for an end to arrive and autoFollow to land. The items here
 * last 1 s and 2 s, and the end reaches the control plane on the engine's next
 * outgoing frame. Ten seconds is generous on a loaded runner, and a wait ends
 * the moment its condition holds.
 */
const REPORT_MS = 10_000;
/** The house rate the engine runs at here (`NBE_HOUSE_RATE`): one frame is 33.3 ms. */
const FPS = 30;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess;
const engineLog: string[] = [];
let ws: WebSocket;

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

let auditPath = "";

/** The audit's autoFollow advances so far: `[actor, outcome]`. */
function advances(): Array<[string, string]> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>)
    .filter((r) => r["event"] === "autoFollow.advance")
    .map((r) => [String(r["actor"]), String(r["outcome"])]);
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

function writePackage(): string {
  const dir = tempDir("nbe-take-duration-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Take duration" },
      show: {
        id: "show-take-duration",
        title: "Take duration",
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
          // 1 s, then autoFollow to T2.
          { id: "T1", kind: "sceneRef", sceneRef: "SCN", durationFrames: 30, autoFollow: true },
          { id: "T2", kind: "sceneRef", sceneRef: "SCN" },
          // 2 s, then autoFollow to M2.
          { id: "M1", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
          { id: "M2", kind: "sceneRef", sceneRef: "SCN" },
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
  auditPath = join(tempDir("nbe-take-duration-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
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
  const deadline = Date.now() + 30_000;
  while (server.wsBridge.renderNodeCount() === 0) {
    if (engine.exitCode !== null || Date.now() > deadline) throw new Error(`engine never registered:\n${engineLog.join("")}`);
    await new Promise((r) => setTimeout(r, 100));
  }
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

test("[v0.4.8 row 2] a cut to a timed item: it ends at its own duration, and autoFollow advances once", async () => {
  // Before the fix the cut carried no duration the engine would read, so T1
  // never ended and the View stayed on T1 for good.
  await ok("view.cut", { itemRef: "T1" });
  const advancedAfter = await until(() => state.viewItem === "T2", REPORT_MS);
  assert.ok(advancedAfter !== null, `autoFollow must advance T1 -> T2 at T1's end; the View is still ${state.viewItem}`);
  assert.ok(
    advancedAfter >= (30 / FPS) * 1000 * 0.8,
    `T1 is 30 frames (1000 ms); it must not end early, advanced after ${advancedAfter} ms`,
  );
  assert.deepEqual(
    { t1: state.itemStateOf("T1"), view: state.viewItem, advances: advances() },
    { t1: "DONE", view: "T2", advances: [["autoFollow:T1", "ok"]] },
    "T1 completed, the View moved on, and autoFollow advanced exactly once",
  );
});

test("[v0.4.8 row 2] a 6-frame mix to a 60-frame item: no advance at the mix's length, one at the item's", async () => {
  // Before the fix the 6-frame mix (200 ms) was read as M1's duration, so M1
  // "ended" 0.2 s in and autoFollow ran away to M2.
  await ok("preview.set", { itemRef: "M1" });
  await ok("view.take", { transition: "mix", durationFrames: 6 });
  await new Promise((r) => setTimeout(r, 1000));
  assert.equal(state.viewItem, "M1", "at 1000 ms M1 is still on air: the mix's 200 ms did not end it");
  const advancedAfter = await until(() => state.viewItem === "M2", REPORT_MS);
  assert.ok(advancedAfter !== null, `autoFollow must advance M1 -> M2 at M1's end; the View is still ${state.viewItem}`);
  assert.deepEqual(
    { m1: state.itemStateOf("M1"), view: state.viewItem, advances: advances().filter(([a]) => a === "autoFollow:M1") },
    { m1: "DONE", view: "M2", advances: [["autoFollow:M1", "ok"]] },
    "M1 completed at its own length, and autoFollow advanced exactly once",
  );
});
