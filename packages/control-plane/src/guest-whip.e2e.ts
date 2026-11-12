//! PR D (sprint 2026-11-12) — a WHIP guest, end to end, on the spike's
//! design: MediaMTX (started here, on its own ports), an ffmpeg WHIP client
//! as the guest (a test pattern and a 1 kHz tone at -20 dBFS), the real
//! control plane and the real `nbe-engine` binary, whose guest session pulls
//! the guest over RTSP and publishes the guest's mix-minus return as Opus.
//!
//! What is asserted, on the wire and in the engine's own recording:
//! - the guest's picture is on air (decoded from the take, it moves);
//! - the guest's tone is in the program (the take's audio), next to the
//!   program's own 440 Hz (the synthetic camera's mic, `NBE_MICS`);
//! - the guest's return (read back from the media server over RTSP) carries
//!   the program's 440 Hz and not the guest's own 1 kHz (AC-18's mix-minus,
//!   at most -80 dBFS);
//! - when the guest leaves, the guest layer on the View is a lost live
//!   source: the fallback slate.
//!
//! Needs ffmpeg with its WHIP muxer, MediaMTX (`NBE_MEDIAMTX_BIN`), the engine
//! binary and a hardware H.264 encoder. Locally:
//!   NBE_MEDIAMTX_BIN=/path/to/mediamtx node --import tsx --test src/guest-whip.e2e.ts

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
const MEDIAMTX = process.env.NBE_MEDIAMTX_BIN ?? "mediamtx";
const RTSP_PORT = 18554;
const WEBRTC_PORT = 18889;
const WHIP_URL = `http://127.0.0.1:${WEBRTC_PORT}/g1/whip`;
const APPLY_MS = 20_000;
const W = 480;
const H = 270;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess;
let mediamtx: ChildProcess;
let guest: ChildProcess | null = null;
const engineLog: string[] = [];
const mtxLog: string[] = [];
let ws: WebSocket;
const ticks: Array<{ at: number; data: Record<string, unknown> }> = [];
let recordDir = "";
let recording = "";
let returnPcm: Float32Array = new Float32Array(0);
let lostAfterLeave: boolean | null = null;
let dropsAtStart = 0;
let dropsAtEnd = 0;
let skipReason: string | null = null;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** The dress rehearsal's rule: a level or cadence claim about the reference
 *  machine, logged and skipped on a GitHub Actions runner. */
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

async function applied(command: string, payload: Record<string, unknown> = {}): Promise<void> {
  const r = await send(command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  assert.ok(await server.awaitApplied((r["stateVersion"] ?? 0) as number, APPLY_MS), `the engine must apply ${command}`);
}

/** Wait until the latest fresh engine report satisfies `want`. */
async function until(what: string, ms: number, want: (d: Record<string, unknown>) => boolean): Promise<Record<string, unknown>> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const d = ticks.at(-1)?.data;
    if (d && d["engineConnected"] === true && want(d)) return d;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}; last tick: ${JSON.stringify(ticks.at(-1)?.data?.["liveInputs"])}`);
}

function guestLine(d: Record<string, unknown>): Record<string, unknown> | undefined {
  return (d["liveInputs"] as Array<Record<string, unknown>> | undefined)?.find((l) => l["source"] === "guest:g1");
}

function droppedNow(): number {
  return (ticks.at(-1)?.data["droppedFramesTotal"] ?? 0) as number;
}

function writePackage(): string {
  const dir = tempDir("nbe-guest-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  recordDir = join(dir, "record-out");
  mkdirSync(recordDir, { recursive: true });
  copyFileSync(join(REPO, "tests/fixtures/dress_show/media/fallback.png"), join(dir, "media", "fallback.png"));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Guest" },
      show: {
        id: "show-guest",
        title: "WHIP guest",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
        outputs: { record: { directory: recordDir } },
      },
      qualityProfile: "consumer",
      assets: [{ id: "fallback", kind: "image", source: "media/fallback.png" }],
      scenes: [
        {
          id: "SCN_G",
          elements: [
            { id: "guest", kind: "guest", z: 1, guestId: "g1" },
            // The program's own sound: the synthetic camera's mic (440 Hz).
            { id: "cam", kind: "camera", z: 2, cameraId: "main", transform: { x: 0.78, y: 0.04, w: 0.2, h: 0.2 } },
          ],
        },
      ],
      rundown: { id: "R", items: [{ id: "G1", kind: "sceneRef", sceneRef: "SCN_G" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

function framesFrom(t: number, n: number): Buffer[] {
  const r = spawnSync(
    FFMPEG,
    ["-v", "error", "-ss", t.toFixed(3), "-i", recording, "-frames:v", String(n), "-vf", `scale=${W}:${H}`, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
    { maxBuffer: W * H * 3 * (n + 2) },
  );
  assert.equal(r.status, 0, `ffmpeg decode: ${r.stderr?.toString()}`);
  const size = W * H * 3;
  const out: Buffer[] = [];
  for (let i = 0; i + size <= r.stdout.length; i += size) out.push(r.stdout.subarray(i, i + size));
  return out;
}

/** Left-channel PCM, 48 kHz float, from a file or URL with extra input args. */
function pcm(inputArgs: string[], extra: string[] = []): Float32Array {
  const r = spawnSync(
    FFMPEG,
    ["-v", "error", ...inputArgs, ...extra, "-map", "0:a", "-af", "pan=mono|c0=c0", "-ar", "48000", "-f", "f32le", "-"],
    { maxBuffer: 48_000 * 4 * 30, timeout: 30_000 },
  );
  assert.equal(r.status, 0, `ffmpeg audio: ${r.stderr?.toString()}`);
  return new Float32Array(r.stdout.buffer, r.stdout.byteOffset, Math.floor(r.stdout.length / 4));
}

function toneDbfs(x: Float32Array, f: number): number {
  const k = 2 * Math.cos((2 * Math.PI * f) / 48_000);
  let s1 = 0, s2 = 0;
  for (const v of x) {
    const s0 = v + k * s1 - s2;
    s2 = s1;
    s1 = s0;
  }
  const amp = (2 * Math.sqrt(Math.max(s1 * s1 + s2 * s2 - k * s1 * s2, 0))) / Math.max(x.length, 1);
  return amp > 0 ? 20 * Math.log10(amp) : -Infinity;
}

/** Pixels whose RGB moved by more than 60 between two frames, in the guest's part of the picture. */
function changedInGuest(a: Buffer, b: Buffer): number {
  let n = 0;
  for (let y = Math.floor(0.3 * H); y < Math.floor(0.95 * H); y++) {
    for (let x = 0; x < Math.floor(0.7 * W); x++) {
      const i = (y * W + x) * 3;
      if (Math.abs(a[i]! - b[i]!) + Math.abs(a[i + 1]! - b[i + 1]!) + Math.abs(a[i + 2]! - b[i + 2]!) > 60) n++;
    }
  }
  return n;
}

before(async () => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN}`);
  assert.equal(spawnSync(FFMPEG, ["-version"]).status, 0, `ffmpeg must be on PATH (or NBE_FFMPEG)`);
  const muxers = spawnSync(FFMPEG, ["-hide_banner", "-muxers"]).stdout?.toString() ?? "";
  assert.ok(/\bwhip\b/.test(muxers), "this ffmpeg must have its WHIP muxer");
  assert.equal(spawnSync(MEDIAMTX, ["--version"]).status, 0, `MediaMTX must run: ${MEDIAMTX} (NBE_MEDIAMTX_BIN)`);

  // MediaMTX on its own ports, RTSP over TCP and WebRTC only.
  const mtxDir = tempDir("nbe-mediamtx-");
  const conf = join(mtxDir, "mediamtx.yml");
  writeFileSync(
    conf,
    [
      "logLevel: info",
      "rtsp: true",
      "rtspTransports: [tcp]",
      `rtspAddress: :${RTSP_PORT}`,
      "rtmp: false",
      "hls: false",
      "srt: false",
      "moq: false",
      "webrtc: true",
      `webrtcAddress: :${WEBRTC_PORT}`,
      "webrtcLocalUDPAddress: :18189",
      "paths:",
      "  all_others:",
      "",
    ].join("\n"),
  );
  mediamtx = spawn(MEDIAMTX, [conf], { stdio: ["ignore", "pipe", "pipe"] });
  mediamtx.stdout?.on("data", (b: Buffer) => mtxLog.push(b.toString()));
  mediamtx.stderr?.on("data", (b: Buffer) => mtxLog.push(b.toString()));
  const mtxDeadline = Date.now() + 15_000;
  while (!mtxLog.join("").includes("[WebRTC] started")) {
    if (mediamtx.exitCode !== null || Date.now() > mtxDeadline) throw new Error(`MediaMTX did not start:\n${mtxLog.join("")}`);
    await sleep(100);
  }

  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-guest-audit-"), "audit.jsonl")),
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
      NBE_MICS: "main=synthetic:440",
      NBE_MEDIA_SERVER: "127.0.0.1",
      NBE_MEDIA_RTSP_PORT: String(RTSP_PORT),
      NBE_FFMPEG: FFMPEG,
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

  // The guest: a WHIP publisher, as the browser would be.
  guest = spawn(
    FFMPEG,
    [
      "-v", "error", "-re",
      "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30",
      "-f", "lavfi", "-i", "aevalsrc=0.1*sin(2*PI*1000*t):s=48000:c=stereo",
      "-t", "60",
      "-c:v", "libx264", "-profile:v", "baseline", "-tune", "zerolatency", "-bf", "0", "-g", "30", "-pix_fmt", "yuv420p", "-b:v", "2500k",
      "-c:a", "libopus", "-ar", "48000", "-ac", "2", "-b:a", "128k",
      "-f", "whip", WHIP_URL,
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );

  await applied("show.load", { packagePath: writePackage() });
  await applied("show.start", { startClock: true });
  await applied("guest.connect", { guestId: "g1", whipUrl: WHIP_URL });
  await applied("preview.set", { itemRef: "G1" });
  await applied("view.take", { transition: "cut" });
  await until("the guest's frames", 20_000, (d) => ((guestLine(d)?.["framesIn"] as number) ?? 0) > 60);
  await sleep(500);

  const rec = await send("record.start", {});
  assert.equal(rec.status, "ok");
  if (!(await server.awaitApplied((rec["stateVersion"] ?? 0) as number, APPLY_MS))) {
    assert.ok(engineLog.some((l) => l.includes("E_NO_HARDWARE_ENCODER")), "record.start: no ack and no encoder refusal");
    skipReason = "no hardware H.264 encoder on this machine";
    return;
  }
  dropsAtStart = droppedNow();
  // The guest's return, read back from the media server while the take runs.
  // A return that never reaches the server fails the AC-18 test, not the file.
  try {
    returnPcm = pcm(["-rtsp_transport", "tcp", "-i", `rtsp://127.0.0.1:${RTSP_PORT}/g1-return`], ["-t", "4"]);
  } catch (e) {
    console.log(`guest e2e: the return could not be read: ${String(e).slice(0, 300)}`);
  }
  await sleep(1000);
  dropsAtEnd = droppedNow();

  // The guest leaves.
  guest.kill("SIGKILL");
  guest = null;
  // The slate, AND the engine's own line saying the guest's layer is what
  // was lost: a slate engaged by anything else (the watchdog under load)
  // must not pass for the guest's loss.
  const ANSI = /\x1b\[[0-9;]*m/g;
  const lostLine = () =>
    engineLog
      .join("")
      .replace(ANSI, "")
      .split("\n")
      .some((l) => l.includes("live source lost on the View") && l.includes("guest:g1"));
  try {
    await until("the slate after the guest left", 8_000, (d) => d["fallbackActive"] === true && lostLine());
    lostAfterLeave = true;
  } catch {
    lostAfterLeave = false;
  }
  await sleep(500);
  const stop = await send("record.stop", {});
  assert.equal(stop.status, "ok");
  assert.ok(await server.awaitApplied((stop["stateVersion"] ?? 0) as number, 60_000), "record.stop applied");
  await applied("guest.disconnect", { guestId: "g1" });
  const mp4s = readdirSync(recordDir).filter((f) => f.endsWith(".mp4"));
  assert.equal(mp4s.length, 1, `one recording: ${JSON.stringify(mp4s)}`);
  recording = join(recordDir, mp4s[0]!);
  const keep = process.env.NBE_GUEST_E2E_KEEP;
  if (keep) copyFileSync(recording, keep);
  console.log(`guest e2e: View drops during the take ${dropsAtStart} -> ${dropsAtEnd}; guest line ${JSON.stringify(guestLine(ticks.at(-1)?.data ?? {}))}`);
});

after(async () => {
  guest?.kill("SIGKILL");
  engine?.kill("SIGKILL");
  mediamtx?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[D] the guest's picture is on air: it moves in the recorded take", (t) => {
  if (skipReason) return t.skip(skipReason);
  const [a, b] = [framesFrom(1.0, 1)[0]!, framesFrom(1.5, 1)[0]!];
  const moved = changedInGuest(a, b);
  console.log(`guest picture: ${moved} pixels moved in 0.5 s in the guest's part of the View`);
  assert.ok(moved > 2000, `the guest's test pattern moves on air (${moved})`);
});

test("[D] the guest's 1 kHz is in the program next to the program's own 440 Hz", (t) => {
  if (skipReason) return t.skip(skipReason);
  const x = pcm(["-ss", "1.0", "-t", "2", "-i", recording]);
  const g = toneDbfs(x, 1000);
  const p = toneDbfs(x, 440);
  console.log(`program (the take): guest 1 kHz ${g.toFixed(2)} dBFS, program 440 Hz ${p.toFixed(2)} dBFS`);
  assert.ok(g > -60, `the guest is heard in the program (${g.toFixed(2)})`);
  assert.ok(p > -60, `the program's mic is there too (${p.toFixed(2)})`);
  normativeOnly("both tones at -20 dBFS within 3 dB (no gaps)", () => {
    assert.ok(g > -23 && g < -17, `the guest's level (${g.toFixed(2)})`);
    assert.ok(p > -23 && p < -17, `the program mic's level (${p.toFixed(2)})`);
  });
});

test("[D] AC-18: the guest's return carries the program and not the guest (1 kHz at most -80 dBFS)", (t) => {
  if (skipReason) return t.skip(skipReason);
  const x = returnPcm.subarray(Math.min(returnPcm.length, 24_000));
  const p = toneDbfs(x, 440);
  const g = toneDbfs(x, 1000);
  console.log(`return (read back over RTSP, ${(x.length / 48_000).toFixed(2)} s): program 440 Hz ${p.toFixed(2)} dBFS, guest 1 kHz ${g.toFixed(2)} dBFS`);
  assert.ok(x.length > 48_000, `at least a second of return audio arrived (${x.length} samples)`);
  assert.ok(p > -60, `the program is in the return (${p.toFixed(2)})`);
  // AC-18 holds on every machine: the exclusion is structural, not timing.
  assert.ok(g <= -80, `mix-minus: the guest's own tone is not (${g.toFixed(2)} dBFS)`);
  normativeOnly("the program in the return at -20 dBFS within 3 dB", () =>
    assert.ok(p > -23 && p < -17, `the program's level in the return (${p.toFixed(2)})`),
  );
});

test("[D] when the guest leaves, the guest on the View is a lost live source: the slate", (t) => {
  if (skipReason) return t.skip(skipReason);
  assert.equal(lostAfterLeave, true, "the engine put the slate up because the guest's layer lost its frames (its own log line names guest:g1)");
});
