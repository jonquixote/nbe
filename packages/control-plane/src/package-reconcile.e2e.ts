//! SPEC v0.4.8 row 5, end to end: a resync reconciles the package by load
//! identity. The real control plane, the real `nbe-engine` binary killed and
//! restarted mid-show, and the real protocol between them.
//!
//! The defect it guards: the resync carried `packagePath` and the engine
//! ignored it, and a package loads only at `show.load`. So a restarted engine
//! rejoined holding no package, for the rest of the show, and the manual
//! recovery was a stop-load-start on air. The snapshot now carries the
//! control plane's load generation beside the path; the engine reloads when it
//! differs from the load it applied, and a load that fails is reported
//! (`itemEvent missing`), never fatal.
//!
//! The witness that a restarted engine holds the package is one only a loaded
//! manifest produces: the package declares a record directory, and the
//! engine's report measures `recordSpaceMib` against it (0 with no package).
//! The pixels are asserted in `crates/nbe-engine/tests/package_reconcile.rs`.
//!
//! Not in `npm test`: it needs the engine binary. CI builds it in the
//! control-plane job and runs this file with its own floors. Run locally:
//!   node --import tsx --test src/package-reconcile.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine).

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
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
/** The longest wait for an engine to (de)register, or for a report to show a change. */
const REPORT_MS = 20_000;
/** How long the connection is watched for a reconnect storm after a failed load. */
const STORM_WATCH_MS = 3000;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess | undefined;
const engineLog: string[] = [];
let ws: WebSocket;
let auditPath = "";
let pkgDir = "";
/** The stateVersion `show.load` was applied at: the control plane's load generation. */
let loadStateVersion = -1;
const ticks: Array<Record<string, unknown>> = [];

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

async function ok(command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const r = await send(command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  return r;
}

function auditRecords(): Array<Record<string, unknown>> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

/** Render registrations so far: every accepted render-role handshake. */
function renderRegistrations(): number {
  return auditRecords().filter((r) => r["kind"] === "auth" && r["outcome"] === "ok" && r["role"] === "render").length;
}

async function until(pred: () => boolean, ms: number): Promise<number | null> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (pred()) return Date.now() - t0;
    await new Promise((r) => setTimeout(r, 25));
  }
  return null;
}

/** The first fresh engine report after tick `from` satisfying `want`, or the last one seen. */
async function engineReport(want: (d: Record<string, unknown>) => boolean, from: number): Promise<Record<string, unknown>> {
  const deadline = Date.now() + REPORT_MS;
  while (Date.now() < deadline) {
    const hit = ticks
      .slice(from)
      .map((t) => t["data"] as Record<string, unknown>)
      .find((d) => d?.["engineConnected"] === true && want(d));
    if (hit) return hit;
    await new Promise((r) => setTimeout(r, 50));
  }
  return (ticks.at(-1)?.["data"] as Record<string, unknown> | undefined) ?? {};
}

async function startEngine(): Promise<void> {
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
  const registered = await until(() => server.wsBridge.renderNodeCount() === 1, REPORT_MS);
  if (registered === null) throw new Error(`engine never registered:\n${engineLog.join("")}`);
}

async function killEngine(): Promise<void> {
  engine?.kill("SIGKILL");
  const gone = await until(() => server.wsBridge.renderNodeCount() === 0, REPORT_MS);
  if (gone === null) throw new Error("the control plane never noticed the engine go");
}

function writePackage(): string {
  const dir = tempDir("nbe-package-reconcile-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  mkdirSync(join(dir, "rec"), { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Package reconcile" },
      show: {
        id: "show-package-reconcile",
        title: "Package reconcile",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
        // The witness: only an engine that read this manifest measures it.
        outputs: { record: { directory: "rec" } },
      },
      qualityProfile: "consumer",
      assets: [{ id: "fallback", kind: "image", source: "media/fallback.png" }],
      scenes: [{ id: "SCN", elements: [{ id: "main", kind: "clip", z: 1, assetId: "fallback" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

before(async () => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  state = new ControlPlaneState();
  auditPath = join(tempDir("nbe-package-reconcile-audit-"), "audit.jsonl");
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
  ws.on("message", (raw: Buffer) => {
    const frame = JSON.parse(raw.toString()) as Record<string, unknown>;
    if (frame["kind"] === "telemetry") ticks.push(frame);
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  await ok("system.telemetry.subscribe", { intervalMs: 500 });
  pkgDir = writePackage();
  const loaded = await ok("show.load", { packagePath: pkgDir });
  loadStateVersion = loaded["stateVersion"] as number;
  await ok("show.start", {});
  await ok("view.cut", { itemRef: "A1" });
});

after(async () => {
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[v0.4.8 row 5] an engine restart mid-show reloads the package: the restarted engine measures the package's record target", async () => {
  // The snapshot names the package and the generation `show.load` applied at.
  const snap = state.resyncSnapshot() as Record<string, unknown>;
  assert.deepEqual(
    { packagePath: snap["packagePath"], packageLoadStateVersion: snap["packageLoadStateVersion"] },
    { packagePath: pkgDir, packageLoadStateVersion: loadStateVersion },
    "the resync carries the package and its load generation",
  );
  let from = ticks.length;
  const before = await engineReport((d) => (d["recordSpaceMib"] as number) > 0, from);
  assert.ok((before["recordSpaceMib"] as number) > 0, `precondition: the loaded engine measures the record target: ${JSON.stringify(before)}`);

  await killEngine();
  await startEngine();
  // Before the fix the restarted engine held no package: its report measured
  // nothing (0), for the rest of the show.
  from = ticks.length;
  const after = await engineReport((d) => (d["recordSpaceMib"] as number) > 0, from);
  assert.deepEqual(
    {
      reloaded: (after["recordSpaceMib"] as number) > 0,
      a1: state.itemStateOf("A1"),
      view: state.viewItem,
      slate: state.fallbackActive,
    },
    { reloaded: true, a1: "LIVE", view: "A1", slate: false },
    `the restarted engine reloaded the package and A1 stays on air: ${JSON.stringify(after)}`,
  );
});

test("[v0.4.8 row 5] a package that cannot load: A1 is reported missing, the slate is up, and the connection stays up", async () => {
  // The package disappears while the engine is away.
  await killEngine();
  const moved = `${pkgDir}.moved`;
  renameSync(pkgDir, moved);
  try {
    const registrationsBefore = renderRegistrations();
    const from = ticks.length;
    await startEngine();
    const missing = await until(() => state.itemStateOf("A1") === "MISSING", REPORT_MS);
    const report = await engineReport((d) => d["fallbackActive"] === true, from);
    // No storm: the connection stays up, and nothing re-registers.
    await new Promise((r) => setTimeout(r, STORM_WATCH_MS));
    assert.deepEqual(
      {
        a1: missing === null ? `never missing: ${state.itemStateOf("A1")}` : state.itemStateOf("A1"),
        controlPlaneSlate: state.fallbackActive,
        engineSlate: report["fallbackActive"],
        audited: auditRecords().some((r) => r["command"] === "engine:missing"),
        connected: server.wsBridge.renderNodeCount(),
        newRegistrations: renderRegistrations() - registrationsBefore,
      },
      { a1: "MISSING", controlPlaneSlate: true, engineSlate: true, audited: true, connected: 1, newRegistrations: 1 },
      "the failure surfaced as missing and the slate, on one connection that stayed up",
    );
  } finally {
    renameSync(moved, pkgDir);
  }
});
