//! The Prompt 13 re-plan's P1, the recall leg — end to end: the real control
//! plane, the real `nbe-engine` binary, the real protocol between them.
//!
//! The split brain it guards: `snapshot.recall` moved the control plane's
//! `viewItem` while the engine — which routed nothing for the command — kept
//! the old View on air. The engine's report carries no `viewItem` (§10.1.1:
//! the control plane owns it), so the witness here is one only the engine's
//! application can produce: the clip bus. Item C is a clip with an AAC track
//! and item G is a still, so the clip bus rises exactly when C is on the
//! engine's View. The control plane's state and the engine's report are
//! asserted TOGETHER after each recall. The View's pixels live in the engine
//! process; they are asserted with the engine's tick in
//! `crates/nbe-engine/tests/prompt13_recall.rs`.
//!
//! Not in `npm test`: it needs the engine binary. CI builds it in the
//! control-plane job and runs this file with its own floors. Run locally:
//!   node --import tsx --test src/recall.e2e.ts
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
/**
 * The engine reports once a second, and a meter window is a second
 * (`audio_driver.rs`), so a level change can take two reports to show.
 * Five seconds is that with slack, then fail with what was seen.
 */
const REPORT_MS = 5_000;
/** Audible, as the dress rehearsal reads the clip bus (step 4). */
const AUDIBLE_DBFS = -60;

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

type Report = Record<string, unknown>;

const clipDbfs = (d: Report): number => ((d["busPeakDbfs"] as Record<string, number> | undefined)?.["clip"] ?? -120);

/** The first fresh engine report after `from` that satisfies `want`, or the last one seen. */
async function engineReports(want: (d: Report) => boolean, from: number): Promise<Report> {
  const deadline = Date.now() + REPORT_MS;
  while (Date.now() < deadline) {
    const hit = ticks
      .slice(from)
      .map((t) => t["data"] as Report)
      .find((d) => d?.["engineConnected"] === true && want(d));
    if (hit) return hit;
    await new Promise((r) => setTimeout(r, 50));
  }
  return (ticks.at(-1)?.["data"] as Report | undefined) ?? {};
}

/** The next `n` fresh engine reports after `from`. */
async function nextReports(from: number, n: number): Promise<Report[]> {
  const deadline = Date.now() + REPORT_MS + n * 1000;
  for (;;) {
    const fresh = ticks
      .slice(from)
      .map((t) => t["data"] as Report)
      .filter((d) => d?.["engineConnected"] === true);
    if (fresh.length >= n || Date.now() > deadline) return fresh.slice(0, n);
    await new Promise((r) => setTimeout(r, 50));
  }
}

function writePackage(): string {
  const dir = tempDir("nbe-recall-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  const fixture = join(REPO, "tests/fixtures/dress_show/media");
  copyFileSync(join(fixture, "fallback.png"), join(dir, "media", "fallback.png"));
  copyFileSync(join(fixture, "A1.mp4"), join(dir, "media", "clip.mp4"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Recall" },
      show: {
        id: "show-recall",
        title: "Recall",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        // 5 s of H.264 with an AAC track (the dress rehearsal's A1).
        { id: "clip", kind: "video", source: "media/clip.mp4", format: "h264", expectedDurationFrames: 150 },
      ],
      scenes: [
        { id: "SCN_G", elements: [{ id: "still", kind: "clip", z: 1, assetId: "fallback" }] },
        { id: "SCN_C", elements: [{ id: "main", kind: "clip", z: 1, assetId: "clip" }] },
      ],
      rundown: {
        id: "R",
        items: [
          { id: "G", kind: "sceneRef", sceneRef: "SCN_G" },
          { id: "C", kind: "sceneRef", sceneRef: "SCN_C" },
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
    audit: new AuditLog(join(tempDir("nbe-recall-audit-"), "audit.jsonl")),
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
  // C on air, audibly — the witness works on this machine — and saved.
  let from = ticks.length;
  await ok("view.cut", { itemRef: "C" });
  const up = await engineReports((d) => clipDbfs(d) > AUDIBLE_DBFS, from);
  assert.ok(clipDbfs(up) > AUDIBLE_DBFS, `precondition: C's audio reaches the clip bus: ${JSON.stringify(up)}`);
  await ok("snapshot.save", { name: "on-c" });
  // Then G: the clip bus goes quiet.
  from = ticks.length;
  await ok("view.cut", { itemRef: "G" });
  const quiet = await engineReports((d) => clipDbfs(d) <= AUDIBLE_DBFS, from);
  assert.ok(clipDbfs(quiet) <= AUDIBLE_DBFS, `precondition: G silences the clip bus: ${JSON.stringify(quiet)}`);
});

after(async () => {
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[P1] snapshot.recall: the control plane and the engine agree on the recalled View", async () => {
  // G on air; recall the snapshot whose View is C. Before the fix the control
  // plane said C while the engine kept G — and the clip bus stayed silent.
  const from = ticks.length;
  await ok("snapshot.recall", { name: "on-c" });
  const report = await engineReports((d) => clipDbfs(d) > AUDIBLE_DBFS, from);
  assert.deepEqual(
    {
      controlPlaneView: state.viewItem,
      engineConnected: report["engineConnected"],
      engineClipAudible: clipDbfs(report) > AUDIBLE_DBFS,
      engineSlate: report["fallbackActive"],
      controlPlaneSlate: state.fallbackActive,
    },
    { controlPlaneView: "C", engineConnected: true, engineClipAudible: true, engineSlate: false, controlPlaneSlate: false },
    `after the recall, both say C is on air: ${JSON.stringify(report)}`,
  );
});

test("[P1] a recall under the operator's slate: both keep the slate, and the engine still applies the View", async () => {
  // Release parity (§10.3): the engine releases the operator's slate exactly
  // where the control plane's clear reaches it, and a recall clears nothing.
  let from = ticks.length;
  await ok("view.cut", { itemRef: "G" });
  const quiet = await engineReports((d) => clipDbfs(d) <= AUDIBLE_DBFS, from);
  assert.ok(clipDbfs(quiet) <= AUDIBLE_DBFS, `precondition: G on air, silent: ${JSON.stringify(quiet)}`);
  from = ticks.length;
  await ok("view.fallback", {});
  const slate = await engineReports((d) => d["fallbackActive"] === true, from);
  assert.equal(slate["fallbackActive"], true, `precondition: the engine reports the slate: ${JSON.stringify(slate)}`);

  from = ticks.length;
  await ok("snapshot.recall", { name: "on-c" });
  // The recall applied beneath the slate: C's audio reaches the clip bus.
  const applied = await engineReports((d) => clipDbfs(d) > AUDIBLE_DBFS, from);
  // And the slate stays, on two fresh reports from here.
  const after = await nextReports(ticks.length, 2);
  assert.deepEqual(
    {
      controlPlaneView: state.viewItem,
      controlPlaneSlate: state.fallbackActive,
      engineClipAudible: clipDbfs(applied) > AUDIBLE_DBFS,
      engineSlate: after.map((d) => d["fallbackActive"]),
    },
    { controlPlaneView: "C", controlPlaneSlate: true, engineClipAudible: true, engineSlate: [true, true] },
    `after the recall under the slate: ${JSON.stringify({ applied, after })}`,
  );
  // The slate's own release point still works.
  from = ticks.length;
  await ok("view.cut", { itemRef: "G" });
  const down = await engineReports((d) => d["fallbackActive"] === false, from);
  assert.deepEqual(
    { engineSlate: down["fallbackActive"], controlPlaneSlate: state.fallbackActive },
    { engineSlate: false, controlPlaneSlate: false },
    "a cut brings the slate down in both",
  );
});
