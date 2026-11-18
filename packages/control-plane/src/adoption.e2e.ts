//! Review4 step 7b end to end, on the real engine: start a show, kill and
//! restart the control plane, and the show never stops. The test review4
//! names: `masterClockFrame` never stops advancing, `viewItem` is unchanged,
//! and the next `view.take` applies.
//!
//! The control plane is restarted twice, on the same port and journal. The
//! second restart's hello is the engine's own account of what it held AFTER
//! the first adoption's resync, so it shows that resync changed nothing on
//! air: the same View item, from the same start frame.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { StatePersistence } from "./persistence.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const REPORT_MS = 20_000;
const FPS = 30;

let engine: ChildProcess | undefined;
const engineLog: string[] = [];
let port = 0;
const journalPath = join(tempDir("nbe-adopt-e2e-state-"), "control-plane-state.json");
const auditPath = join(tempDir("nbe-adopt-e2e-audit-"), "audit.jsonl");

interface Plane {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  admin: WebSocket;
}
let cp: Plane | undefined;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until(pred: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (pred()) return true;
    await sleep(25);
  }
  return pred();
}

function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error(`no response to ${command}`)), REPORT_MS);
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

async function ok(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(ws, command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  return r.stateVersion as number;
}

function rows(): Array<Record<string, unknown>> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

/** A control plane on `port` (0 the first time), on the one journal. A
 * journal it restores makes it a restarted one (as `index.ts` does). */
async function startControlPlane(): Promise<Plane> {
  const state = new ControlPlaneState();
  const persistence = new StatePersistence(state, journalPath);
  persistence.restore();
  const journal = persistence.restoredJournal();
  const server = await createControlPlaneServer({
    port,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence,
    warn: () => {},
    ...(journal === null ? {} : { restart: { journal } }),
  });
  port = server.port;
  const admin = new WebSocket(`ws://127.0.0.1:${port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
  });
  await new Promise<void>((res, rej) => {
    admin.once("open", () => res());
    admin.once("error", rej);
  });
  return { server, state, admin };
}

/** The control plane goes away as a crash would leave it: its sockets cut. */
async function stopControlPlane(p: Plane): Promise<void> {
  p.admin.close();
  await p.server.close();
}

function writePackage(): string {
  const dir = tempDir("nbe-adopt-e2e-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Adoption" },
      show: {
        id: "show-adoption",
        title: "Adoption",
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
          { id: "U1", kind: "sceneRef", sceneRef: "SCN" },
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
  cp = await startControlPlane();
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
      NBE_HOUSE_RATE: String(FPS),
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  engine.stdout?.on("data", (b: Buffer) => engineLog.push(b.toString()));
  engine.stderr?.on("data", (b: Buffer) => engineLog.push(b.toString()));
  const registered = await until(() => cp!.server.wsBridge.renderNodeCount() === 1, REPORT_MS);
  assert.ok(registered, `engine never registered:\n${engineLog.join("")}`);
});

after(async () => {
  engine?.kill("SIGKILL");
  if (cp) await stopControlPlane(cp);
});

test("[review4 step 7b] a control-plane restart mid-show: the clock never stops, the View item and its start frame stand, and the next take applies", async () => {
  // A show on air.
  const a = cp!;
  await ok(a.admin, "show.load", { packagePath: writePackage() });
  await ok(a.admin, "show.start");
  await ok(a.admin, "preview.set", { itemRef: "U1" });
  await ok(a.admin, "view.take");
  assert.ok(await until(() => a.state.lastKnownMasterFrame > 0, REPORT_MS), "the engine reports its clock");
  const startFrame = a.state.viewItemStartFrame;
  assert.ok(startFrame !== null);
  const beforeFrame = a.state.lastKnownMasterFrame;

  // Restart 1: the engine reconnects and is adopted.
  await stopControlPlane(a);
  const b = await startControlPlane();
  cp = b;
  assert.ok(
    await until(() => rows().some((r) => r["event"] === "controlPlane.adopted"), REPORT_MS),
    `adopted (rows: ${JSON.stringify(rows().filter((r) => String(r["event"]).startsWith("controlPlane")))})\n${engineLog.join("")}`,
  );
  assert.equal(b.state.showState, "RUNNING");
  assert.equal(b.state.viewItem, "U1");
  // The clock never stopped: two reports after the adoption, each later.
  const f1Seen = b.state.lastKnownMasterFrame;
  assert.ok(await until(() => b.state.lastKnownMasterFrame > Math.max(f1Seen, beforeFrame) + FPS / 2, REPORT_MS), "the clock advances after the adoption");
  const f1 = b.state.lastKnownMasterFrame;
  assert.ok(await until(() => b.state.lastKnownMasterFrame > f1, REPORT_MS), "and keeps advancing");
  assert.ok(f1 > beforeFrame, `frames continue from before the restart (${beforeFrame} -> ${f1}), not from zero`);

  // Restart 2: this hello is the engine's own account AFTER restart 1's
  // resync, so it shows that resync changed nothing on air.
  await stopControlPlane(b);
  const c = await startControlPlane();
  cp = c;
  assert.ok(await until(() => rows().filter((r) => r["event"] === "controlPlane.adopted").length === 2, REPORT_MS), "adopted again");
  assert.equal(c.state.viewItem, "U1", "the engine's View item after the first adoption");
  assert.equal(c.state.viewItemStartFrame, startFrame, "from the same start frame: no jump, no restart");

  // The next take applies.
  await ok(c.admin, "preview.set", { itemRef: "U2" });
  const sv = await ok(c.admin, "view.take");
  let applied: number | null = null;
  const deadline = Date.now() + REPORT_MS;
  while (Date.now() < deadline) {
    applied = await renderApplied();
    if (applied !== null && applied >= sv) break;
    await sleep(50);
  }
  assert.ok(applied !== null && applied >= sv, `the engine applied the take (stateVersion ${sv}; applied ${applied})`);
  const versions = rows()
    .filter((r) => r["event"] === "controlPlane.adopted")
    .map((r) => (r["detail"] as Record<string, unknown>)["adoptedVersion"] as number);
  assert.ok(versions[1]! > versions[0]!, `versions only go up across restarts (${versions})`);
});

/** The render node's last applied version, from `GET /status`. */
async function renderApplied(): Promise<number | null> {
  const s = (await (await fetch(`http://127.0.0.1:${port}/nbe/v0.3/status`)).json()) as {
    renderNode?: { lastAppliedStateVersion: number | null };
  };
  return s.renderNode?.lastAppliedStateVersion ?? null;
}
