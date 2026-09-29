//! PR #34's fix round — the operator's slate, end to end: the real control
//! plane, the real `nbe-engine` binary, the real protocol between them.
//!
//! The split brain it guards: `view.fallback` holds the engine's slate; a take
//! cleared the control plane's `fallbackActive` while the engine kept the
//! slate on air and kept REPORTING `fallbackActive: true`. Here the engine's
//! report (the §10.1 tick, forwarded while `engineConnected`) and the control
//! plane's own state are asserted TOGETHER after the take and after a cut.
//! The View's pixels live in the engine process only; they are asserted with
//! the engine's tick in `crates/nbe-engine/tests/prompt11_slate.rs`, and the
//! two tests meet at the one bit both the renderer and the tick read
//! (`EngineState::fallback_active()`).
//!
//! Not in `npm test`: it needs the engine binary. CI builds it in the
//! control-plane job and runs this file with its own floors. Run locally:
//!   node --import tsx --test src/slate-release.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine).

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
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
/** The engine reports once a second; two ticks' slack, then fail with what was seen. */
const REPORT_MS = 5_000;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess;
const engineLog: string[] = [];
let ws: WebSocket;
const ticks: Record<string, unknown>[] = [];

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

/** The first tick after `from` whose engine report says `fallbackActive === want`. */
async function engineReports(want: boolean, from: number): Promise<Record<string, unknown>> {
  const deadline = Date.now() + REPORT_MS;
  while (Date.now() < deadline) {
    const hit = ticks
      .slice(from)
      .map((t) => t["data"] as Record<string, unknown>)
      .find((d) => d?.["engineConnected"] === true && d?.["fallbackActive"] === want);
    if (hit) return hit;
    await new Promise((r) => setTimeout(r, 50));
  }
  const last = ticks.at(-1)?.["data"] as Record<string, unknown> | undefined;
  return last ?? {};
}

/** The next `n` ticks after `from` that carry a fresh engine report. */
async function nextReports(from: number, n: number): Promise<Record<string, unknown>[]> {
  const deadline = Date.now() + REPORT_MS + n * 1000;
  for (;;) {
    const fresh = ticks
      .slice(from)
      .map((t) => t["data"] as Record<string, unknown>)
      .filter((d) => d?.["engineConnected"] === true);
    if (fresh.length >= n || Date.now() > deadline) return fresh.slice(0, n);
    await new Promise((r) => setTimeout(r, 50));
  }
}

function writePackage(): string {
  const dir = tempDir("nbe-slate-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Slate" },
      show: {
        id: "show-slate",
        title: "Slate release",
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
          { id: "A1", kind: "sceneRef", sceneRef: "SCN" },
          { id: "A2", kind: "sceneRef", sceneRef: "SCN" },
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
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-slate-audit-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_HOUSE_RATE: "30",
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
  ws.on("message", (raw: Buffer) => {
    const frame = JSON.parse(raw.toString()) as Record<string, unknown>;
    if (frame["kind"] === "telemetry") ticks.push(frame);
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  await ok("system.telemetry.subscribe", { intervalMs: 1000 });
  await ok("show.load", { packagePath: writePackage() });
  await ok("show.start", {});
  await ok("view.cut", { itemRef: "A1" });
});

after(async () => {
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[PR34] view.fallback then view.take: the engine's report and the control plane agree the slate is down", async () => {
  let from = ticks.length;
  await ok("view.fallback", {});
  const up = await engineReports(true, from);
  assert.equal(up["fallbackActive"], true, `precondition: the engine reports the slate: ${JSON.stringify(up)}`);
  assert.equal(state.fallbackActive, true, "precondition: the control plane holds it too");

  await ok("preview.set", { itemRef: "A2" });
  from = ticks.length;
  await ok("view.take", {});
  const down = await engineReports(false, from);
  // Together: the engine's own report (forwarded while engineConnected), and
  // the control plane's state. Before the fix the first read `true` for good
  // while the second read `false` — the split brain.
  assert.deepEqual(
    { engineConnected: down["engineConnected"], engineReport: down["fallbackActive"], controlPlane: state.fallbackActive },
    { engineConnected: true, engineReport: false, controlPlane: false },
    "after the take, the engine and the control plane agree: no slate",
  );
});

test("[PR34] view.fallback then view.cut: the cut releases the slate too", async () => {
  let from = ticks.length;
  await ok("view.fallback", {});
  const up = await engineReports(true, from);
  assert.equal(up["fallbackActive"], true, `precondition: the engine reports the slate: ${JSON.stringify(up)}`);

  from = ticks.length;
  await ok("view.cut", { itemRef: "A1" });
  const down = await engineReports(false, from);
  assert.deepEqual(
    { engineConnected: down["engineConnected"], engineReport: down["fallbackActive"], controlPlane: state.fallbackActive },
    { engineConnected: true, engineReport: false, controlPlane: false },
    "after the cut, the engine and the control plane agree: no slate",
  );
});

// PR #34's fix round, release parity: the control plane's `loadPackage`
// clears `fallbackActive`, so the engine must let go of the operator's slate
// on `show.load` too — the two-key pass of 2026-09-29 found a new show airing
// under the old show's slate. Asserted after the load, and again after
// show.start with NO take in between (a take would release it anyway and
// hide the load's own release).

async function slateThenStop(): Promise<void> {
  const from = ticks.length;
  await ok("view.fallback", {});
  const up = await engineReports(true, from);
  assert.equal(up["fallbackActive"], true, `precondition: the engine reports the slate: ${JSON.stringify(up)}`);
  assert.equal(state.fallbackActive, true, "precondition: the control plane holds it too");
  await ok("show.stop", {});
}

async function airsWithoutTheSlate(what: string): Promise<void> {
  let from = ticks.length;
  const down = await engineReports(false, from);
  assert.deepEqual(
    { engineConnected: down["engineConnected"], engineReport: down["fallbackActive"], controlPlane: state.fallbackActive },
    { engineConnected: true, engineReport: false, controlPlane: false },
    `after ${what}, the engine and the control plane agree: no slate`,
  );
  from = ticks.length;
  await ok("show.start", {});
  const aired = await nextReports(from, 2);
  assert.ok(aired.length === 2, `two fresh engine reports after show.start, saw ${aired.length}`);
  assert.deepEqual(
    aired.map((d) => d["fallbackActive"]),
    [false, false],
    `the new show airs without the slate after ${what} (no take in between)`,
  );
}

test("[PR34] view.fallback, show.stop, show.unload, show.load: the new show airs without the slate", async () => {
  await slateThenStop();
  await ok("show.unload", {});
  await ok("show.load", { packagePath: writePackage() });
  await airsWithoutTheSlate("unload then load");
});

test("[PR34] view.fallback, show.stop, show.load (reload): the reloaded show airs without the slate", async () => {
  await slateThenStop();
  await ok("show.load", { packagePath: writePackage(), mode: "reload" });
  await airsWithoutTheSlate("a reload");
});
