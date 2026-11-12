//! PR C (sprint 2026-11-12) — the live camera layer, end to end: the real
//! control plane, the real `nbe-engine` binary with its synthetic camera
//! (`NBE_CAMERAS=main=synthetic`), the engine's own recording decoded frame
//! by frame.
//!
//! The synthetic camera stamps each frame with its sequence number as sixteen
//! black-or-white blocks (`live.rs` `synthetic_frame`), so the recording
//! says, frame by frame, which camera frame went to air. It stalls once for
//! 1.5 s (`NBE_SYNTHETIC_CAMERA_STALL`), which must put the fallback slate up
//! after `LIVE_LOSS_MS` and take it down after frames return.
//!
//! Not in `npm test`: it needs the engine binary, a hardware H.264 encoder and
//! ffmpeg. CI runs it in the control-plane job with its own floors. Locally:
//!   node --import tsx --test src/camera-capture.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine; set
//! NBE_CAMERA_E2E_KEEP=/path/take.mp4 to keep the recorded take).

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
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
const APPLY_MS = 20_000;
/** The synthetic camera stalls after this many frames (counted from show.load), for STALL_MS. */
const STALL_AFTER = 150;
const STALL_MS = 1500;
/** Decoded at a quarter of 1080p: enough for 24-pixel code blocks. */
const W = 480;
const H = 270;
// The code blocks' geometry in the camera frame (live.rs CODE_*): 16 blocks of
// 5% of the width, from 10% across, 45% down; a block is 64 px tall in a
// 720-line frame.
const CODE_X0 = 0.1;
const CODE_Y0 = 0.45;
const CODE_BLOCK = 0.05;
const CODE_BLOCK_H = 64 / 720;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess;
const engineLog: string[] = [];
let ws: WebSocket;
const ticks: Array<{ at: number; data: Record<string, unknown> }> = [];
let recordDir = "";
let recording = "";
let recordStartMs = 0;
let lossAtMs = 0;
let backAtMs = 0;
let dropsAtStart = 0;
let dropsAtEnd = 0;
let skipReason: string | null = null;
/** Set when the slate never went up, or never came down, during the stall. */
let lossError: string | null = null;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/**
 * A reference-machine claim (the dress rehearsal's rule, `normativeOnly`):
 * asserted on the normative machine, logged and skipped on a GitHub Actions
 * runner, refused anywhere else. PR C's first CI run, at load 14 on 3 CPUs,
 * starved the upload thread (63 of 192 frames overwritten before upload) and
 * the null sink's graph (the mic read in bursts): the counters registered
 * every one, and the cadence claims below are about the reference machine.
 */
function normativeOnly(what: string, check: () => void): void {
  const ci = process.env["CI"] ?? "";
  const actions = process.env["GITHUB_ACTIONS"] ?? "";
  if (ci === "" && actions === "") return check();
  if (actions === "true") {
    console.log(`SKIP normative: ${what} — a GitHub Actions runner, not the reference machine (SPEC §0.3)`);
    return;
  }
  assert.fail(`refusing to skip "${what}" on an unidentified machine (CI=${ci}, GITHUB_ACTIONS=${actions})`);
}

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

async function applied(command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  const sv = (r["stateVersion"] ?? 0) as number;
  assert.ok(await server.awaitApplied(sv, APPLY_MS), `the engine must apply ${command} (stateVersion ${sv})`);
  return Date.now();
}

/** Wall time of the first fresh engine report after `from` with `fallbackActive === want`. */
async function fallbackBecomes(want: boolean, fromMs: number, withinMs: number): Promise<number> {
  const deadline = Date.now() + withinMs;
  while (Date.now() < deadline) {
    const hit = ticks.find((t) => t.at >= fromMs && t.data["engineConnected"] === true && t.data["fallbackActive"] === want);
    if (hit) return hit.at;
    await sleep(50);
  }
  throw new Error(`the engine never reported fallbackActive=${want} within ${withinMs} ms`);
}

function droppedNow(): number {
  return (ticks.at(-1)?.data["droppedFramesTotal"] ?? 0) as number;
}

function writePackage(): string {
  const dir = tempDir("nbe-camera-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  recordDir = join(dir, "record-out");
  mkdirSync(recordDir, { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Camera" },
      show: {
        id: "show-camera",
        title: "Camera capture",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
        outputs: { record: { directory: recordDir } },
      },
      qualityProfile: "consumer",
      assets: [{ id: "fallback", kind: "image", source: "media/fallback.png" }],
      scenes: [{ id: "SCN_CAM", elements: [{ id: "cam", kind: "camera", z: 1, cameraId: "main" }] }],
      rundown: { id: "R", items: [{ id: "C1", kind: "sceneRef", sceneRef: "SCN_CAM" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

/** `n` decoded frames from `t` seconds, at W x H RGB24, one per output frame. */
function framesFrom(t: number, n: number): Buffer[] {
  const r = spawnSync(
    FFMPEG,
    ["-v", "error", "-ss", t.toFixed(3), "-i", recording, "-frames:v", String(n), "-vf", `scale=${W}:${H}`, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
    { maxBuffer: W * H * 3 * (n + 2) },
  );
  assert.equal(r.status, 0, `ffmpeg decode at ${t.toFixed(3)} s: ${r.stderr?.toString()}`);
  const size = W * H * 3;
  const out: Buffer[] = [];
  for (let i = 0; i + size <= r.stdout.length; i += size) out.push(r.stdout.subarray(i, i + size));
  return out;
}

/** The synthetic camera's sequence number in a frame, or null if the blocks are not there. */
function readCode(f: Buffer): number | null {
  let v = 0;
  const y = Math.round((CODE_Y0 + CODE_BLOCK_H / 2) * H);
  for (let bit = 0; bit < 16; bit++) {
    const x = Math.round((CODE_X0 + (bit + 0.5) * CODE_BLOCK) * W);
    const i = (y * W + x) * 3;
    const luma = (f[i]! + f[i + 1]! + f[i + 2]!) / 3;
    // Blocks are neutral black or white; a coloured pixel (the slate's red)
    // is not a code.
    if (Math.max(f[i]!, f[i + 1]!, f[i + 2]!) - Math.min(f[i]!, f[i + 1]!, f[i + 2]!) > 40) return null;
    // Blocks are 0 or 255 on a 64 background; anything mid-grey is not a code.
    if (luma > 40 && luma < 200) return null;
    v = (v << 1) | (luma >= 200 ? 1 : 0);
  }
  return v;
}

function meanRgb(f: Buffer): [number, number, number] {
  const n = f.length / 3;
  let r = 0, g = 0, b = 0;
  for (let i = 0; i < f.length; i += 3) {
    r += f[i]!;
    g += f[i + 1]!;
    b += f[i + 2]!;
  }
  return [r / n, g / n, b / n];
}

/** Wall time (ms) of the engine's first log line containing `needle`. */
function engineLogAt(needle: string): number | null {
  const ANSI = /\x1b\[[0-9;]*m/g;
  const l = engineLog.join("").replace(ANSI, "").split("\n").find((x) => x.includes(needle));
  const m = l ? /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z/.exec(l) : null;
  return m ? Date.parse(`${m[1]!.slice(0, 23)}Z`) : null;
}

/** Milliseconds from the engine's "synthetic camera stalls" line to its "live source lost" line. */
function engineLossLatencyMs(): number | null {
  const a = engineLogAt("synthetic camera stalls");
  const b = engineLogAt("live source lost on the View");
  return a !== null && b !== null ? b - a : null;
}

before(async () => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  assert.equal(spawnSync(FFMPEG, ["-version"]).status, 0, `ffmpeg must be on PATH (or NBE_FFMPEG): ${FFMPEG}`);
  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-camera-audit-"), "audit.jsonl")),
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
      NBE_CAMERAS: "main=synthetic",
      // The camera's mic: a 440 Hz sine at -20 dBFS per channel (live.rs SyntheticMic).
      NBE_MICS: "main=synthetic:440",
      NBE_SYNTHETIC_CAMERA_STALL: `${STALL_AFTER}:${STALL_MS}`,
      // Each frame up to 12 ms early or late, so arrivals straddle the house
      // ticks and the frame synchroniser has something to do (without it, the
      // newest-frame rule repeats and skips; PR C's mutant C3).
      NBE_SYNTHETIC_CAMERA_JITTER_MS: "12",
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
    if (frame["kind"] === "telemetry") ticks.push({ at: Date.now(), data: frame["data"] as Record<string, unknown> });
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  assert.equal((await send("system.telemetry.subscribe", { intervalMs: 250 })).status, "ok");

  const loadAt = await applied("show.load", { packagePath: writePackage() });
  await applied("show.start", { startClock: true });
  await applied("preview.set", { itemRef: "C1" });
  await applied("view.take", { transition: "cut" });
  // The camera started at the load; its frames are on air well inside a second.
  await sleep(600);

  const rec = await send("record.start", {});
  assert.equal(rec.status, "ok", `record.start: ${JSON.stringify(rec)}`);
  if (!(await server.awaitApplied((rec["stateVersion"] ?? 0) as number, APPLY_MS))) {
    assert.ok(engineLog.some((l) => l.includes("E_NO_HARDWARE_ENCODER")), "record.start: no ack and no encoder refusal");
    skipReason = "no hardware H.264 encoder on this machine";
    return;
  }
  recordStartMs = Date.now();
  dropsAtStart = droppedNow();
  // The stall begins STALL_AFTER frames after the load (30 fps).
  const stallAt = loadAt + (STALL_AFTER * 1000) / 30;
  // A loss path that never engages fails the loss and recovery tests, not
  // the whole file: the take still runs its course.
  try {
    lossAtMs = await fallbackBecomes(true, recordStartMs, Math.max(0, stallAt - Date.now()) + 5_000);
    backAtMs = await fallbackBecomes(false, lossAtMs, STALL_MS + 5_000);
  } catch (e) {
    lossError = String(e);
    await sleep(Math.max(0, stallAt + STALL_MS + 1000 - Date.now()));
  }
  await sleep(1500);
  dropsAtEnd = droppedNow();

  const stop = await send("record.stop", {});
  assert.equal(stop.status, "ok");
  assert.ok(await server.awaitApplied((stop["stateVersion"] ?? 0) as number, 60_000), "record.stop applied");
  const mp4s = readdirSync(recordDir).filter((f) => f.endsWith(".mp4"));
  assert.equal(mp4s.length, 1, `one recording: ${JSON.stringify(mp4s)}`);
  recording = join(recordDir, mp4s[0]!);
  const keep = process.env.NBE_CAMERA_E2E_KEEP;
  if (keep) copyFileSync(recording, keep);
  console.log(
    `camera e2e: loss reported ${((lossAtMs - recordStartMs) / 1000).toFixed(2)} s into the take, recovery ${((backAtMs - recordStartMs) / 1000).toFixed(2)} s; View drops during the take ${dropsAtStart} -> ${dropsAtEnd}`,
  );
});

after(async () => {
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[C] the camera's frames reach the recording in order, about one new camera frame per output frame", (t) => {
  if (skipReason) return t.skip(skipReason);
  const codes = framesFrom(0.3, 60).map(readCode);
  const read = codes.filter((c): c is number => c !== null);
  const steps = read.slice(1).map((c, i) => c - read[i]!);
  const ones = steps.filter((d) => d === 1).length;
  const repeats = steps.filter((d) => d === 0).length;
  const skips = steps.filter((d) => d >= 2).length;
  console.log(
    `camera frames: ${read.length} of ${codes.length} output frames carry a code, ${read[0]} .. ${read.at(-1)}; steps +1 ${ones}, repeats ${repeats}, skips ${skips}, backwards ${steps.filter((d) => d < 0).length}`,
  );
  assert.ok(read.length >= 55, `the camera picture is on air in the take (${read.length} of ${codes.length} frames)`);
  assert.ok(steps.every((d) => d >= 0), `camera frames never go backwards: ${JSON.stringify(steps)}`);
  assert.ok(read.at(-1)! - read[0]! >= 40, `the camera advanced in 2 s (${read[0]} -> ${read.at(-1)})`);
  // The frame synchroniser steps one camera frame per house frame; what it
  // could not do is counted by the engine (`liveInputs`), read in the next test.
  normativeOnly("one camera frame per output frame (90% of steps)", () =>
    assert.ok(ones >= steps.length * 0.9, `one camera frame per output frame: ${ones} of ${steps.length}`),
  );
});

test("[C] a stalled camera puts the fallback slate up within LIVE_LOSS_MS and a frame, and takes it down when frames return", (t) => {
  if (skipReason) return t.skip(skipReason);
  assert.equal(lossError, null, "the slate went up during the stall and came down after it");
  const latency = engineLossLatencyMs();
  console.log(`camera loss: the engine's stall-to-slate latency ${latency} ms`);
  assert.ok(latency !== null, "the engine logged the stall and the loss");
  assert.ok(latency >= 450 && latency <= 650, `the slate goes up ${latency} ms after the last frame (bound: 500 ms + one frame, with log slack)`);
  // Sampled from the engine's own loss line (same machine clock as the take's
  // start): the tick that reported it may lag it by up to a second.
  const lostAt = engineLogAt("live source lost on the View") ?? lossAtMs;
  const slate = framesFrom((lostAt - recordStartMs) / 1000 + 0.3, 1)[0]!;
  const [r, g, b] = meanRgb(slate);
  console.log(`during the stall the View's mean RGB is ${r.toFixed(1)}, ${g.toFixed(1)}, ${b.toFixed(1)} (the slate is 127, 0, 0)`);
  // The slate PNG's red is 127; through the H.264 take it decodes near 115.
  assert.ok(r > 90 && r < 145 && g < 12 && b < 12, "the fallback slate is on air during the stall");
  assert.equal(readCode(slate), null, "no camera code shows through the slate");
});

test("[C] the engine reports the camera: frames in, uploaded, drawn, and every repeat or skip counted", (t) => {
  if (skipReason) return t.skip(skipReason);
  const line = ticks
    .filter((k) => k.data["engineConnected"] === true)
    .map((k) => (k.data["liveInputs"] as Array<Record<string, unknown>> | undefined)?.find((l) => l["source"] === "camera:main"))
    .filter((l) => l !== undefined)
    .at(-1);
  console.log(`camera telemetry: ${JSON.stringify(line)}`);
  assert.ok(line, "the engine reports camera:main");
  const framesIn = line["framesIn"] as number;
  const uploaded = line["framesUploaded"] as number;
  const overwritten = line["framesOverwritten"] as number;
  assert.ok(framesIn > 150, `frames arrived (${framesIn})`);
  // Every frame is accounted for: uploaded, or overwritten before upload and
  // counted (one may be in flight).
  const unaccounted = framesIn - uploaded - overwritten;
  assert.ok(unaccounted >= 0 && unaccounted <= 2, `in ${framesIn} = uploaded ${uploaded} + overwritten ${overwritten} (+ at most 2 in flight)`);
  normativeOnly("the upload thread keeps up (no frame overwritten before upload)", () =>
    assert.ok(overwritten <= 2, `overwritten before upload: ${overwritten}`),
  );
  assert.ok((line["drawnSeq"] as number) > STALL_AFTER, "the View drew camera frames after the stall");
  assert.equal(typeof line["repeats"], "number");
  assert.equal(typeof line["skips"], "number");
});

/** Left-channel PCM of the take from `t` for `secs`, 48 kHz float. */
function audioFrom(t: number, secs: number): Float32Array {
  const r = spawnSync(
    FFMPEG,
    ["-v", "error", "-ss", t.toFixed(3), "-t", secs.toFixed(3), "-i", recording, "-map", "0:a", "-af", "pan=mono|c0=c0", "-ar", "48000", "-f", "f32le", "-"],
    { maxBuffer: 48_000 * 4 * (secs + 1) },
  );
  assert.equal(r.status, 0, `ffmpeg audio decode: ${r.stderr?.toString()}`);
  return new Float32Array(r.stdout.buffer, r.stdout.byteOffset, Math.floor(r.stdout.length / 4));
}

/** The level of a sine at `f` in `x`, dBFS of a full-scale sine (Goertzel). */
function toneDbfs(x: Float32Array, f: number): number {
  const k = 2 * Math.cos((2 * Math.PI * f) / 48_000);
  let s1 = 0, s2 = 0;
  for (const v of x) {
    const s0 = v + k * s1 - s2;
    s2 = s1;
    s1 = s0;
  }
  const amp = (2 * Math.sqrt(Math.max(s1 * s1 + s2 * s2 - k * s1 * s2, 0))) / x.length;
  return amp > 0 ? 20 * Math.log10(amp) : -Infinity;
}

test("[C] the camera's mic reaches the mic bus: its 440 Hz is in the take at about -20 dBFS", (t) => {
  if (skipReason) return t.skip(skipReason);
  const x = audioFrom(0.5, 2);
  const tone = toneDbfs(x, 440);
  const other = toneDbfs(x, 1000);
  const line = ticks
    .filter((k) => k.data["engineConnected"] === true)
    .map((k) => (k.data["liveInputs"] as Array<Record<string, unknown>> | undefined)?.find((l) => l["source"] === "camera:main"))
    .filter((l) => l !== undefined)
    .at(-1);
  const micPeaks = ticks
    .map((k) => (k.data["busPeakDbfs"] as Record<string, number> | undefined)?.["mic"])
    .filter((v): v is number => typeof v === "number");
  console.log(
    `camera mic: 440 Hz ${tone.toFixed(2)} dBFS, 1 kHz ${other.toFixed(2)} dBFS in the take; mic bus peak max ${Math.max(...micPeaks).toFixed(2)} dBFS; mic frames in ${line?.["micFramesIn"]}, underrun ${line?.["micUnderrunFrames"]}, trimmed ${line?.["micTrimmedFrames"]}`,
  );
  assert.equal(line?.["mic"], true, "the engine reports the camera's mic running");
  assert.equal(typeof line?.["micUnderrunFrames"], "number", "underruns are counted");
  assert.equal(typeof line?.["micTrimmedFrames"], "number", "trims are counted");
  assert.ok(tone > -60, `the mic's tone is in the take (${tone.toFixed(2)} dBFS)`);
  assert.ok(other < -60, `and nothing else is (1 kHz ${other.toFixed(2)})`);
  normativeOnly("the mic's tone at -20 dBFS within 3 dB (no gaps)", () =>
    assert.ok(tone > -23 && tone < -17, `the mic's tone is in the take at about -20 dBFS (${tone.toFixed(2)})`),
  );
  assert.ok(micPeaks.some((p) => p > -23 && p < -17), "the mic bus meter reads the tone");
});

test("[C] after the stall the camera is back on air with later frames", (t) => {
  if (skipReason) return t.skip(skipReason);
  assert.equal(lossError, null, "the stall's loss and recovery were reported");
  const before = framesFrom(0.3, 1).map(readCode)[0];
  const backAt = engineLogAt("live source recovered") ?? backAtMs;
  const after = framesFrom((backAt - recordStartMs) / 1000 + 0.3, 5).map(readCode).filter((c): c is number => c !== null);
  console.log(`camera code before ${before}, after recovery ${JSON.stringify(after)}`);
  assert.ok(after.length >= 4, "the camera picture is back");
  assert.ok(before !== null && before !== undefined && after[0]! > before + STALL_MS / 33, "the frames after recovery are the camera's later frames");
  assert.equal(state.viewItem, "C1", "the control plane's View item never changed");
});
