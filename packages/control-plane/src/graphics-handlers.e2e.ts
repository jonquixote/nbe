//! PR A (sprint 2026-11-12) — the engine handlers, end to end: the real
//! control plane, the real `nbe-engine` binary, the real protocol, and the
//! engine's own recording decoded frame by frame.
//!
//! Before this PR the engine logged `graphic.show`/`update`/`hide`,
//! `breaking.show`/`hide`, `ticker.override`/`clearOverride`,
//! `clock.configure`, `item.stop` and `preview.set` as "directive ignored (no
//! engine effect)". Each test here asserts what one command changed in two
//! places the control plane cannot fake: the pixels of the engine's recorded
//! file (decoded with ffmpeg, colour counted in the region the command
//! touches), and the engine's own report (`engineOverlayState`, the tick's
//! engine-owned on-air line). `preview.set` changes the Preview bus, which is
//! not recorded, so it is asserted on the engine's report only.
//!
//! The whole timeline runs once in `before` (one recording); each test reads
//! its own window of it. A command's window starts when the engine applied
//! it (its `appliedStateVersion`) and its frames are sampled 0.5 s later.
//!
//! Not in `npm test`: it needs the engine binary, a hardware H.264 encoder and
//! ffmpeg. CI runs it in the control-plane job with its own floors. Locally:
//!   node --import tsx --test src/graphics-handlers.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine; set
//! NBE_GRAPHICS_E2E_KEEP=/path/take.mp4 to keep the recorded take).

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { loadavg } from "node:os";
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
const FFMPEG = process.env.NBE_FFMPEG ?? "ffmpeg";
/** The engine reports once a second; two ticks' slack. */
const REPORT_MS = 5_000;
const APPLY_MS = 20_000;
const W = 1920;
const H = 1080;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess;
const engineLog: string[] = [];
let ws: WebSocket;
const ticks: Record<string, unknown>[] = [];
let recordDir = "";

/** Recording-relative times (s) at which each step's command was applied. */
const appliedAt: Record<string, number> = {};
/** The engine's on-air line as first reported after each step. */
const reported: Record<string, Record<string, unknown>> = {};
/** Per step, in order: the engine's View drop count at its window's end, and
 *  the runner's 1-minute load when the command was sent (the user's ruling of
 *  2026-11-12, 03:20, item 3). */
const stepLog: Array<{ name: string; at: number; dropsEnd: number; load1: number }> = [];
/** The control plane's View and Preview items as each step left them. */
const cpAt: Record<string, { viewItem: string | null; previewItem: string | null }> = {};
let recordStartMs = 0;
let recording = "";
let dropsAtStart = 0;
let dropsAtEnd = 0;
let skipReason: string | null = null;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

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

/** Send, require `ok`, and wait until the engine applied it. */
async function applied(command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  const sv = (r["stateVersion"] ?? 0) as number;
  assert.ok(await server.awaitApplied(sv, APPLY_MS), `the engine must apply ${command} (stateVersion ${sv})`);
  return Date.now();
}

/** The first fresh engine on-air line after `from` that satisfies `want`. */
async function engineSays(
  what: string,
  from: number,
  want: (s: Record<string, unknown>) => boolean,
): Promise<Record<string, unknown>> {
  const deadline = Date.now() + REPORT_MS;
  while (Date.now() < deadline) {
    for (const t of ticks.slice(from)) {
      const d = t["data"] as Record<string, unknown> | undefined;
      const s = d?.["engineOverlayState"] as Record<string, unknown> | null | undefined;
      if (d?.["engineConnected"] === true && s && want(s)) return s;
    }
    await sleep(50);
  }
  const last = (ticks.at(-1)?.["data"] as Record<string, unknown> | undefined)?.["engineOverlayState"];
  throw new Error(`the engine never reported ${what}; last on-air line: ${JSON.stringify(last)}`);
}

/** One step: the command applied, its time recorded, the engine's report awaited, the window held. */
async function step(
  name: string,
  command: string,
  payload: Record<string, unknown>,
  want: (s: Record<string, unknown>) => boolean,
): Promise<void> {
  const from = ticks.length;
  const load1 = loadavg()[0] ?? 0;
  const at = await applied(command, payload);
  appliedAt[name] = (at - recordStartMs) / 1000;
  // A report that never comes fails that step's test, not the whole timeline.
  try {
    reported[name] = await engineSays(`${name} (${command})`, from, want);
  } catch (e) {
    reported[name] = { error: String(e) };
  }
  // The window: at least 1.1 s after the apply, so a sample at +0.5 s and
  // one at +0.9 s both sit inside it.
  const held = Date.now() - at;
  if (held < 1100) await sleep(1100 - held);
  stepLog.push({ name, at: appliedAt[name]!, dropsEnd: droppedNow(), load1 });
  cpAt[name] = { viewItem: state.viewItem, previewItem: state.previewItem };
}

function droppedNow(): number {
  return ((ticks.at(-1)?.["data"] as Record<string, unknown> | undefined)?.["droppedFramesTotal"] ?? 0) as number;
}

function writePackage(): string {
  const dir = tempDir("nbe-graphics-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  recordDir = join(dir, "record-out");
  mkdirSync(recordDir, { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/counter_150.mp4"), join(dir, "media", "counter.mp4"));
  copyFileSync(
    join(REPO, "tests/fixtures/overlay_show/media/Amiri-Regular.ttf"),
    join(dir, "media", "Amiri-Regular.ttf"),
  );
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Graphics" },
      show: {
        id: "show-graphics",
        title: "Engine handlers",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
        outputs: { record: { directory: recordDir } },
      },
      qualityProfile: "consumer",
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        { id: "counter", kind: "video", source: "media/counter.mp4", format: "h264" },
        { id: "amiri", kind: "font", source: "media/Amiri-Regular.ttf" },
      ],
      templates: [
        { id: "solid", kind: "generic" },
        { id: "tpl_lt", kind: "lowerThirdHeadline", fontAssetIds: ["amiri"], fields: [{ name: "headline" }, { name: "subhead" }] },
        { id: "tpl_banner", kind: "breakingBanner", fontAssetIds: ["amiri"], fields: [{ name: "headline" }] },
        { id: "tpl_ticker", kind: "ticker", fontAssetIds: ["amiri"], fields: [{ name: "text" }] },
        { id: "tpl_clock", kind: "generic", fontAssetIds: ["amiri"] },
      ],
      scenes: [
        { id: "SCN_COUNTER", elements: [{ id: "main", kind: "clip", z: 1, assetId: "counter" }] },
        { id: "SCN_DARK", elements: [{ id: "dark", kind: "graphic", z: 1, templateId: "solid", fields: { color: "#101010" } }] },
        { id: "SCN_GREY", elements: [{ id: "grey", kind: "graphic", z: 1, templateId: "solid", fields: { color: "#404040" } }] },
      ],
      overlays: [
        {
          id: "ol_ticker",
          elements: [
            { id: "ticker_el", kind: "ticker", z: 1, templateId: "tpl_ticker", fields: { color: "#ffff00", text: "-" }, transform: { x: 0, y: 0.9, w: 1, h: 0.1 } },
          ],
        },
        {
          id: "ol_clock",
          elements: [
            {
              id: "clock_el",
              kind: "clock",
              z: 1,
              templateId: "tpl_clock",
              fields: { color: "#00ff00" },
              transform: { x: 0, y: 0.2, w: 1, h: 0.06 },
              clock: { mode: "showElapsed", format: "HH:mm:ss", blinkColon: false },
            },
          ],
        },
        {
          // Never shown: the box and colour a breaking banner is drawn in
          // (graphics.rs: the first declared element using the template).
          id: "ol_banner_host",
          elements: [
            { id: "banner_host", kind: "graphic", z: 5, templateId: "tpl_banner", fields: { color: "#00ffff" }, transform: { x: 0, y: 0.1, w: 1, h: 0.08 } },
          ],
        },
      ],
      rundown: {
        id: "R",
        items: [
          { id: "A1", kind: "sceneRef", sceneRef: "SCN_COUNTER", durationFrames: 150 },
          { id: "A2", kind: "sceneRef", sceneRef: "SCN_DARK" },
          { id: "A3", kind: "sceneRef", sceneRef: "SCN_GREY" },
        ],
      },
      control: { bindings: [] },
    }),
  );
  return dir;
}

/** One decoded frame of the recording at `t` seconds, RGB24, full size. */
function frameAt(t: number): Buffer {
  const r = spawnSync(
    FFMPEG,
    ["-v", "error", "-ss", t.toFixed(3), "-i", recording, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
    { maxBuffer: W * H * 3 * 2 },
  );
  assert.equal(r.status, 0, `ffmpeg decode at ${t.toFixed(3)} s: ${r.stderr?.toString()}`);
  assert.equal(r.stdout.length, W * H * 3, `one ${W}x${H} RGB frame at ${t.toFixed(3)} s`);
  return r.stdout;
}

type Rgb = (r: number, g: number, b: number) => boolean;
const MAGENTA: Rgb = (r, g, b) => r > 150 && b > 150 && g < 100;
const CYAN: Rgb = (r, g, b) => r < 100 && g > 150 && b > 150;
const YELLOW: Rgb = (r, g, b) => r > 150 && g > 150 && b < 100;
const GREEN: Rgb = (r, g, b) => r < 100 && g > 150 && b < 100;

/** Pixels in the normalized rect `[x, y, w, h]` that match `colour`. */
function count(frame: Buffer, rect: [number, number, number, number], colour: Rgb): number {
  const x0 = Math.floor(rect[0] * W);
  const y0 = Math.floor(rect[1] * H);
  const x1 = Math.min(W, Math.ceil((rect[0] + rect[2]) * W));
  const y1 = Math.min(H, Math.ceil((rect[1] + rect[3]) * H));
  let n = 0;
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const i = (y * W + x) * 3;
      if (colour(frame[i]!, frame[i + 1]!, frame[i + 2]!)) n++;
    }
  }
  return n;
}

/** Pixels in a rect whose summed RGB difference between two frames exceeds 60. */
function changed(a: Buffer, b: Buffer, rect: [number, number, number, number]): number {
  const x0 = Math.floor(rect[0] * W);
  const y0 = Math.floor(rect[1] * H);
  const x1 = Math.ceil((rect[0] + rect[2]) * W);
  const y1 = Math.ceil((rect[1] + rect[3]) * H);
  let n = 0;
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const i = (y * W + x) * 3;
      const d = Math.abs(a[i]! - b[i]!) + Math.abs(a[i + 1]! - b[i + 1]!) + Math.abs(a[i + 2]! - b[i + 2]!);
      if (d > 60) n++;
    }
  }
  return n;
}

/** A frame inside a step's window: `dt` seconds after the engine applied it. */
function sample(name: string, dt = 0.5): Buffer {
  const at = appliedAt[name];
  assert.ok(at !== undefined, `step ${name} ran`);
  return frameAt(at + dt);
}

// Regions, normalized: where each command draws (graphics.rs `default_rect`
// for the lower third; the declared hosts above for the rest).
const LOWER_THIRD: [number, number, number, number] = [0.05, 0.75, 0.9, 0.12];
const BANNER: [number, number, number, number] = [0, 0.1, 1, 0.08];
const TICKER: [number, number, number, number] = [0, 0.9, 1, 0.1];
const CLOCK: [number, number, number, number] = [0, 0.2, 1, 0.06];
const PICTURE: [number, number, number, number] = [0, 0.3, 1, 0.4];

before(async () => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  assert.equal(spawnSync(FFMPEG, ["-version"]).status, 0, `ffmpeg must be on PATH (or NBE_FFMPEG): ${FFMPEG}`);
  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-graphics-audit-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
      NBE_HOUSE_RATE: "30",
      RUST_LOG: process.env.RUST_LOG ?? "info",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  engine.stdout?.on("data", (b: Buffer) => engineLog.push(b.toString()));
  engine.stderr?.on("data", (b: Buffer) => engineLog.push(b.toString()));
  const deadline = Date.now() + 30_000;
  while (server.wsBridge.renderNodeCount() === 0) {
    if (engine.exitCode !== null || Date.now() > deadline) throw new Error(`engine never registered:\n${engineLog.join("")}`);
    await sleep(100);
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
  const sub = await send("system.telemetry.subscribe", { intervalMs: 1000 });
  assert.equal(sub.status, "ok");

  await applied("show.load", { packagePath: writePackage() });
  await applied("show.start", { startClock: true });
  await applied("preview.set", { itemRef: "A1" });
  await applied("view.take", { transition: "cut" });
  await applied("overlay.show", { overlayId: "ol_ticker" });
  await applied("overlay.show", { overlayId: "ol_clock" });
  // The clip is 150 frames; a take is resident and playing well within one second.
  await sleep(300);

  const rec = await send("record.start", {});
  assert.equal(rec.status, "ok", `record.start: ${JSON.stringify(rec)}`);
  if (!(await server.awaitApplied((rec["stateVersion"] ?? 0) as number, APPLY_MS))) {
    const refused = engineLog.some((l) => l.includes("E_NO_HARDWARE_ENCODER"));
    assert.ok(refused, `record.start produced no ack and no encoder refusal: ${engineLog.slice(-3).join("")}`);
    skipReason = "no hardware H.264 encoder on this machine";
    return;
  }
  recordStartMs = Date.now();
  dropsAtStart = droppedNow();

  // 1. The clip plays (baseline), then item.stop freezes it.
  await sleep(1100);
  appliedAt["playing"] = 0.1;
  await step("stop", "item.stop", { itemId: "A1" }, () => true);

  // 2. A dark, still picture under every graphic from here on.
  await step("dark", "view.cut", { itemRef: "A2" }, () => true);
  await step("lt_show", "graphic.show", { templateId: "tpl_lt", fields: { headline: "FIRST HEADLINE TEXT", color: "#ff00ff" } }, (s) =>
    (s["graphics"] as string[]).includes("tpl_lt-graphic"),
  );
  await step("lt_update", "graphic.update", { elementId: "tpl_lt-graphic", fields: { headline: "TWO" } }, (s) =>
    (s["graphics"] as string[]).includes("tpl_lt-graphic"),
  );
  await step("lt_hide", "graphic.hide", { elementId: "tpl_lt-graphic" }, (s) => (s["graphics"] as string[]).length === 0);
  await step("breaking_show", "breaking.show", { headline: "BREAKING NEWS NOW" }, (s) => s["breaking"] === true);
  await step("breaking_hide", "breaking.hide", {}, (s) => s["breaking"] === false);
  await step(
    "ticker_override",
    "ticker.override",
    {
      items: [
        { text: "SECOND ITEM IN THE OVERRIDE QUEUE", priority: 0 },
        { text: "FIRST ITEM BY PRIORITY IN THE OVERRIDE", priority: 5 },
      ],
      mode: "replace",
    },
    (s) => s["tickerOverrideItems"] === 2,
  );
  await step("ticker_clear", "ticker.clearOverride", {}, (s) => s["tickerOverrideItems"] === 0);
  await step("clock_configure", "clock.configure", { elementId: "clock_el", clock: { format: "HH:mm" } }, (s) =>
    (s["clocksConfigured"] as string[]).includes("clock_el"),
  );
  await step("preview", "preview.set", { itemRef: "A3" }, (s) => s["previewItem"] === "A3");

  // #102's open question: the hold item.stop puts on A1 is released by a cut
  // and by a take. A cut back to A1 after a cut away, then a second stop and
  // a take of the same item.
  await step("recut", "view.cut", { itemRef: "A1" }, () => true);
  await step("restop", "item.stop", { itemId: "A1" }, () => true);
  await applied("preview.set", { itemRef: "A1" });
  await step("retake", "view.take", { transition: "cut" }, () => true);

  dropsAtEnd = droppedNow();
  const stop = await send("record.stop", {});
  assert.equal(stop.status, "ok", `record.stop: ${JSON.stringify(stop)}`);
  assert.ok(await server.awaitApplied((stop["stateVersion"] ?? 0) as number, 60_000), "record.stop applied (file finalized)");
  const mp4s = readdirSync(recordDir).filter((f) => f.endsWith(".mp4"));
  assert.equal(mp4s.length, 1, `one recording in ${recordDir}: ${JSON.stringify(mp4s)}`);
  recording = join(recordDir, mp4s[0]!);
  // How long each command's text took to shape and upload off the render
  // thread (the engine's own log line), so the cost a command waits is read.
  const ANSI = /\x1b\[[0-9;]*m/g;
  const prepMs = engineLog
    .join("")
    .replace(ANSI, "")
    .split("\n")
    .filter((l) => l.includes("text prepared off the render thread"))
    .map((l) => Number(/elapsed_ms=([\d.]+)/.exec(l)?.[1] ?? NaN).toFixed(2));
  console.log(`graphics e2e: text prepared off the render thread, ms per command: ${prepMs.join(", ")}`);
  // The demo: keep the take to watch (`NBE_GRAPHICS_E2E_KEEP=/path/take.mp4`).
  const keep = process.env.NBE_GRAPHICS_E2E_KEEP;
  if (keep) copyFileSync(recording, keep);
  // Drops and load per step: the window of each step is from its apply to
  // the next step's apply; the engine's counter is read at each window's end.
  let prevDrops = dropsAtStart;
  for (const st of stepLog) {
    console.log(
      `graphics e2e step: ${st.name.padEnd(16)} applied ${st.at.toFixed(2)} s, View drops in its window ${st.dropsEnd - prevDrops} (total ${st.dropsEnd}), runner load1 ${st.load1.toFixed(2)}`,
    );
    prevDrops = st.dropsEnd;
  }
  console.log(
    `graphics e2e: recording ${recording}; steps applied at ${JSON.stringify(
      Object.fromEntries(Object.entries(appliedAt).map(([k, v]) => [k, Number(v.toFixed(2))])),
    )} s; View drops during the take ${dropsAtStart} -> ${dropsAtEnd}`,
  );
});

after(async () => {
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

function skipIfNoRecording(t: { skip: (m: string) => void }): boolean {
  if (skipReason !== null) {
    t.skip(skipReason);
    return true;
  }
  return false;
}

test("[A] item.stop: the playing clip freezes on its frame; the View is not emptied", (t) => {
  if (skipIfNoRecording(t)) return;
  const moving = changed(frameAt(0.3), frameAt(0.8), PICTURE);
  const frozen = changed(sample("stop", 0.5), sample("stop", 0.9), PICTURE);
  const lit = count(sample("stop", 0.5), PICTURE, (r, g, b) => r + g + b > 120);
  console.log(`item.stop: changed pixels while playing ${moving}, after the stop ${frozen}; lit after the stop ${lit}`);
  assert.ok(moving > 500, `the counter clip must move before the stop (changed ${moving})`);
  assert.ok(frozen < 50, `the stopped clip must hold its frame (changed ${frozen})`);
  assert.ok(lit > 500, `the View keeps the stopped item's picture, not black (lit ${lit})`);
  assert.equal(cpAt["dark"]?.viewItem, "A2", "precondition for the graphics steps: the cut put A2 up");
});

/**
 * Consecutive decoded frames from `t` for `secs`, at a quarter of 1080p,
 * classified one by one against the previous: "moving" (more than 300 pixels
 * of the picture band changed by more than 60), "still", or "dark" (the
 * band is not lit: the A2 solid, not the counter clip).
 *
 * Timeline-free on purpose. The recording's PTS is the encoder's submit
 * count (`record/thread.rs`), so a frame the record path sheds under load
 * makes the file shorter than the wall clock, and a sample taken at a wall
 * time late in the take lands in the next step's window. PR C's CI run at
 * 0f31b9c read the second stop "moving" that way while the first stop, early
 * in the take, read still. A run of still, lit frames between moving ones is
 * the hold, wherever it falls in the file.
 */
function motionRun(t: number, secs: number): string[] {
  const QW = 480;
  const QH = 270;
  const r = spawnSync(
    FFMPEG,
    ["-v", "error", "-ss", Math.max(0, t).toFixed(3), "-t", secs.toFixed(3), "-i", recording, "-vf", `scale=${QW}:${QH}`, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
    { maxBuffer: QW * QH * 3 * Math.ceil(secs * 30 + 30) },
  );
  assert.equal(r.status, 0, `ffmpeg decode: ${r.stderr?.toString()}`);
  const size = QW * QH * 3;
  const frames: Buffer[] = [];
  for (let i = 0; i + size <= r.stdout.length; i += size) frames.push(r.stdout.subarray(i, i + size));
  const y0 = Math.floor(PICTURE[1] * QH);
  const y1 = Math.floor((PICTURE[1] + PICTURE[3]) * QH);
  const out: string[] = [];
  for (let k = 1; k < frames.length; k++) {
    const a = frames[k - 1]!;
    const b = frames[k]!;
    let moved = 0;
    let lit = 0;
    for (let y = y0; y < y1; y++) {
      for (let x = 0; x < QW; x++) {
        const i = (y * QW + x) * 3;
        if (Math.abs(a[i]! - b[i]!) + Math.abs(a[i + 1]! - b[i + 1]!) + Math.abs(a[i + 2]! - b[i + 2]!) > 60) moved++;
        if (b[i]! + b[i + 1]! + b[i + 2]! > 120) lit++;
      }
    }
    out.push(lit < 300 ? "dark" : moved > 300 ? "moving" : "still");
  }
  return out;
}

test("[A] item.stop's hold is released by a cut and by a take: A1 stopped, cut away, cut back plays; stopped again, taken plays", (t) => {
  if (skipIfNoRecording(t)) return;
  // From before the cut back to well after the take: moving (the cut back
  // plays A1), a still run (the second stop holds), moving (the take plays).
  const from = appliedAt["recut"]! - 0.5;
  const secs = appliedAt["retake"]! - from + 2.5;
  const run = motionRun(from, secs);
  const code = run.map((c) => (c === "moving" ? "m" : c === "still" ? "s" : "d")).join("");
  console.log(`hold release: ${run.length} frames from ${from.toFixed(2)} s: ${code}`);
  // The longest run of still, lit frames, and what surrounds it.
  const m = [...code.matchAll(/s+/g)].sort((x, y) => y[0].length - x[0].length)[0];
  assert.ok(m, `a still (held) run between the cut back and the take: ${code}`);
  const start = m.index!;
  const end = start + m[0].length;
  assert.ok(m[0].length >= 10, `the second stop holds for at least 10 recorded frames (held ${m[0].length}): ${code}`);
  assert.ok(code.slice(0, start).includes("mmm"), `the cut back to the stopped item plays it before the hold: ${code}`);
  assert.ok(code.slice(end).includes("mmm"), `the take of the stopped item plays it after the hold: ${code}`);
});

test("[A] graphic.show / update / hide: the lower third appears, changes, and goes", (t) => {
  if (skipIfNoRecording(t)) return;
  const base = count(sample("dark"), LOWER_THIRD, MAGENTA);
  const shown = count(sample("lt_show"), LOWER_THIRD, MAGENTA);
  const updated = count(sample("lt_update"), LOWER_THIRD, MAGENTA);
  const hidden = count(sample("lt_hide"), LOWER_THIRD, MAGENTA);
  console.log(`lower third magenta pixels: base ${base}, shown ${shown}, updated ${updated}, hidden ${hidden}`);
  assert.ok(shown > base + 2000, `graphic.show draws the headline (base ${base}, shown ${shown})`);
  assert.ok(updated > base + 200 && updated < shown * 0.6, `graphic.update redraws the shorter headline (shown ${shown}, updated ${updated})`);
  assert.ok(hidden < base + 100, `graphic.hide removes it (base ${base}, hidden ${hidden})`);
  assert.deepEqual(reported["lt_show"]?.["graphics"], ["tpl_lt-graphic"]);
  assert.deepEqual(reported["lt_hide"]?.["graphics"], []);
});

test("[A] breaking.show / hide: the banner appears in its template's declared box, and goes", (t) => {
  if (skipIfNoRecording(t)) return;
  const base = count(sample("lt_hide"), BANNER, CYAN);
  const shown = count(sample("breaking_show"), BANNER, CYAN);
  const hidden = count(sample("breaking_hide"), BANNER, CYAN);
  console.log(`banner cyan pixels: base ${base}, shown ${shown}, hidden ${hidden}`);
  assert.ok(shown > base + 2000, `breaking.show draws the headline (base ${base}, shown ${shown})`);
  assert.ok(hidden < base + 100, `breaking.hide removes it (base ${base}, hidden ${hidden})`);
  assert.equal(reported["breaking_show"]?.["breaking"], true);
  assert.equal(reported["breaking_hide"]?.["breaking"], false);
});

test("[A] ticker.override / clearOverride: the declared ticker shows the queue, then its own text again", (t) => {
  if (skipIfNoRecording(t)) return;
  const declared = count(sample("breaking_hide"), TICKER, YELLOW);
  const overridden = count(sample("ticker_override"), TICKER, YELLOW);
  const cleared = count(sample("ticker_clear"), TICKER, YELLOW);
  console.log(`ticker yellow pixels: declared ${declared}, overridden ${overridden}, cleared ${cleared}`);
  assert.ok(overridden > declared + 2000, `the override's items fill the band (declared ${declared}, overridden ${overridden})`);
  assert.ok(cleared < overridden / 4, `clearOverride returns the declared text (overridden ${overridden}, cleared ${cleared})`);
  assert.equal(reported["ticker_override"]?.["tickerOverrideItems"], 2);
  assert.equal(reported["ticker_clear"]?.["tickerOverrideItems"], 0);
});

test("[A] clock.configure: HH:mm:ss becomes HH:mm on air", (t) => {
  if (skipIfNoRecording(t)) return;
  const before = count(sample("ticker_clear"), CLOCK, GREEN);
  const after = count(sample("clock_configure"), CLOCK, GREEN);
  console.log(`clock green pixels: HH:mm:ss ${before}, HH:mm ${after}`);
  assert.ok(before > 500, `the declared clock is on air (${before})`);
  assert.ok(after > 200 && after < before * 0.85, `fewer digits after the reconfigure (before ${before}, after ${after})`);
  assert.deepEqual(reported["clock_configure"]?.["clocksConfigured"], ["clock_el"]);
});

test("[A] preview.set: the engine's Preview bus follows (reported; the Preview bus is not recorded)", (t) => {
  if (skipIfNoRecording(t)) return;
  assert.equal(reported["preview"]?.["previewItem"], "A3");
  assert.equal(cpAt["preview"]?.previewItem, "A3", "the control plane agrees");
});

test("[A] every command's text was shaped before it was drawn: no prepared-text misses", (t) => {
  if (skipIfNoRecording(t)) return;
  const last = ticks
    .map((k) => k["data"] as Record<string, unknown> | undefined)
    .filter((d) => d?.["engineConnected"] === true && d?.["engineOverlayState"])
    .at(-1)?.["engineOverlayState"] as Record<string, unknown> | undefined;
  assert.ok(last, "a fresh engine on-air line");
  assert.equal(last["textPrepMissesTotal"], 0, `misses: ${JSON.stringify(last)}`);
});
