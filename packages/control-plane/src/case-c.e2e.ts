//! Review4 case C without a journal, end to end on the real engine (#199,
//! #202): a control plane that restarts with no journal (a first start, or
//! both journal files lost) meets an engine running a show it cannot know. It
//! holds that engine unregistered, so no resync stops its show, and raises
//! `engine.unidentified`; `show.adopt {force: true}` then takes the show over
//! as it runs. The scripted-engine tests (adoption.test.ts) cover the
//! decisions; this covers the real engine's side: its clock never stops
//! through the hold or the adoption, and a third control plane restoring the
//! second's journal adopts it (case B) at the same View item and start frame,
//! so neither the hold nor the adopting resync changed anything on air.
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/case-c.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine).

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
const auditPath = join(tempDir("nbe-case-c-e2e-audit-"), "audit.jsonl");
const journalOf = (name: string): string => join(tempDir(`nbe-case-c-e2e-${name}-`), "control-plane-state.json");

interface Plane {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  persistence: StatePersistence;
  journalPath: string;
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
const detail = (r: Record<string, unknown> | undefined): Record<string, unknown> => (r?.["detail"] ?? {}) as Record<string, unknown>;

/** A control plane on `port` (0 the first time) with its own journal file, as
 * `index.ts` starts one: a journal it restores makes it a restarted one; with
 * none, it waits for its first engine's hello (`awaitFirstHello`). */
async function startControlPlane(journalPath: string): Promise<Plane> {
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
    awaitFirstHello: {},
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
  return { server, state, persistence, journalPath, admin };
}

/** The control plane goes away as a crash would leave it: its sockets cut. */
async function stopControlPlane(p: Plane): Promise<void> {
  p.admin.close();
  await p.server.close();
}

function writePackage(): string {
  const dir = tempDir("nbe-case-c-e2e-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Case C" },
      show: {
        id: "show-case-c",
        title: "Case C",
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

/** The render node's last applied version, from `GET /status`. */
async function renderApplied(): Promise<number | null> {
  const s = (await (await fetch(`http://127.0.0.1:${port}/nbe/v0.3/status`)).json()) as {
    renderNode?: { lastAppliedStateVersion: number | null };
  };
  return s.renderNode?.lastAppliedStateVersion ?? null;
}

before(async () => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  cp = await startControlPlane(journalOf("first"));
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

test("[review4 case C, no journal] the show is held on air, show.adopt takes it over as it runs, and a later restart adopts it at the same start frame", async () => {
  // A show on air under control plane 1.
  const a = cp!;
  await ok(a.admin, "show.load", { packagePath: writePackage() });
  await ok(a.admin, "show.start");
  await ok(a.admin, "preview.set", { itemRef: "U1" });
  await ok(a.admin, "view.take");
  assert.ok(await until(() => a.state.lastKnownMasterFrame > 0, REPORT_MS), "the engine reports its clock");
  const beforeFrame = a.state.lastKnownMasterFrame;

  // Control plane 2 starts with no journal: the engine's show is one it cannot know.
  await stopControlPlane(a);
  const b = await startControlPlane(journalOf("lost"));
  cp = b;
  assert.ok(
    await until(() => rows().some((r) => r["event"] === "controlPlane.refused"), REPORT_MS),
    `the engine is held (rows: ${JSON.stringify(rows().filter((r) => String(r["event"]).startsWith("controlPlane")))})\n${engineLog.join("")}`,
  );
  const refused = detail(rows().find((r) => r["event"] === "controlPlane.refused"));
  assert.equal(refused["reason"], "noJournal");
  assert.equal(refused["viewItem"], "U1", "the hello says what the engine holds");
  assert.ok(
    b.state.alerts.some((x) => x.id === "engine.unidentified" && x.severity === "critical"),
    `engine.unidentified raised: ${JSON.stringify(b.state.alerts)}`,
  );
  assert.equal(b.server.wsBridge.renderNodeCount(), 0, "held unregistered: no resync, no directive");
  assert.notEqual(b.state.showState, "RUNNING", "control plane 2 knows no show");
  // The engine plays on through the hold: its clock keeps advancing, from before the restart.
  assert.ok(
    await until(() => b.state.lastKnownMasterFrame > beforeFrame + FPS, REPORT_MS),
    `the engine's clock advances through the hold (${beforeFrame} -> ${b.state.lastKnownMasterFrame})`,
  );
  await sleep(2_000);
  const duringHold = b.state.lastKnownMasterFrame;

  // The operator's third choice: take the show over as it runs.
  await ok(b.admin, "show.adopt", { force: true });
  assert.equal(b.state.showState, "RUNNING");
  assert.equal(b.state.viewItem, "U1");
  assert.ok(await until(() => b.server.wsBridge.renderNodeCount() === 1, REPORT_MS), "registered on the adoption");
  assert.ok(
    await until(() => !b.state.alerts.some((x) => x.id === "engine.unidentified"), REPORT_MS),
    `engine.unidentified cleared: ${JSON.stringify(b.state.alerts)}`,
  );
  const forced = detail(rows().find((r) => r["event"] === "controlPlane.adopted" && r["command"] === "show.adopt"));
  assert.equal(forced["forced"], true);
  assert.equal(forced["heldFor"], "noJournal");
  const engineStart = b.state.viewItemStartFrame;
  assert.ok(engineStart !== null, "the engine's start frame, from its hello");
  assert.ok(
    await until(() => b.state.lastKnownMasterFrame > duringHold + FPS, REPORT_MS),
    `the clock keeps advancing after the adopting resync (${duringHold} -> ${b.state.lastKnownMasterFrame})`,
  );

  // Control plane 3 restores control plane 2's journal. This hello is the
  // engine's own account AFTER the adopting resync.
  b.persistence.flushNow();
  await stopControlPlane(b);
  const c = await startControlPlane(b.journalPath);
  cp = c;
  assert.ok(
    await until(() => rows().filter((r) => r["event"] === "controlPlane.adopted").length === 2, REPORT_MS),
    `adopted from the forced adoption's journal (rows: ${JSON.stringify(rows().filter((r) => String(r["event"]).startsWith("controlPlane")).map((r) => [r["event"], detail(r)["reason"] ?? null]))})`,
  );
  assert.equal(c.state.viewItem, "U1", "the engine's View item after the forced adoption");
  assert.equal(c.state.viewItemStartFrame, engineStart, "from the same start frame: the hold and the adoption changed nothing on air");
  console.log(`CASE-C ${JSON.stringify({ beforeFrame, duringHold, engineStart, afterFrame: c.state.lastKnownMasterFrame })}`);

  // And the show is controllable: the next take applies.
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
});
