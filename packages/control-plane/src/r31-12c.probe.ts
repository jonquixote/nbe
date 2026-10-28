//! R31's 12c probe (docs/soak-protocol.md, R31): the user's specification of
//! 2026-10-26, and its go of the same day.
//!
//! NOT A TEST, and not in `npm test`. Run it on the normative machine, on AC
//! power under `caffeinate` (`docs/soak-protocol.md` §2, precondition 6):
//!
//!   npm run probe:r31 -- --state idle                 # 30 minutes
//!   npm run probe:r31 -- --state ambient --minutes 30
//!   npm run probe:r31 -- --analyse <run directory>    # re-read a kept run
//!   npm run probe:r31 -- --state ambient --tap cpu    # the A/B: CPU readback
//!   npm run probe:r31 -- --state ambient --gpu low    # the integrated GPU
//!   npm run probe:r31 -- --state ambient --path stream # R35: the stream path
//!
//! R31's sighting 1 was the dress's step 12c reading frame 41's picture in
//! frame 43's slot, and its recording was lost with the temp directory it was
//! written to. So this probe's first rule is that its evidence survives: every
//! run writes to a directory under `.git/nbe-staged/r31/` (or `--out`), and
//! nothing there is deleted. It keeps the recording, the engine's log, every
//! telemetry tick, every take it sent, the machine's samples and the result.
//!
//! What it runs: step 12c's record path, for `--minutes` (30 by default). The
//! real control plane and the real engine (the debug binary, the dress's),
//! the dress package with counter_150 on air, recorded through the zero-copy
//! tap. The take alternates: the counter clip (150 frames, 5 s, plus half a
//! second of its held last frame), then an empty scene for a second, whose
//! cleared View records black and separates one take of the counter from the
//! next. One recording spans the whole run.
//!
//! The trace: the probe runs the engine with `NBE_RECORD_TRACE` pointed into
//! the run's directory, so the record thread logs, for every frame it was
//! handed, the master frame the View drew and the encoder's submit count. The
//! record path's PTS is that count, so every recorded frame joins to the frame
//! its tick drew.
//!
//! What it asserts, per recorded frame, read with step 12c's classifier
//! (`r20-counter.ts`), in each take of the counter:
//! 1. the recorded sequence never goes backwards (a frame shows a lower
//!    index than the frame before it: step 12c's assertion, S1's signature);
//! 2. no frame's slot carries an earlier frame's picture: each recorded
//!    picture is the one its tick drew. Within a take the counter's index is
//!    the master frame less a constant (the take's offset, read as the mode of
//!    master frame minus index); a frame whose picture sits below that is
//!    STALE. The clip's held last frame (index 149) is by design and excluded.
//!
//! Any occurrence is a sighting: R31 reopens for a real hunt. A clean run is
//! the specification's demotion to watch, the user's to rule. R31's fix (the
//! record thread waits for the View's draw before encoding) is falsified by
//! this probe: 0 stale frames on both GPUs, and the class returning with the
//! wait removed (the user's ruling of 2026-10-26).
//!
//! R33 is counted beside it, never folded in: consecutive recorded frames
//! whose ticks drew the same master frame (a repeat) or skipped one (a skip).
//! The render loop's cadence, not the tap; reported per run, measured. A run
//! read without a trace (an older run) falls back to clause 1 and 2 read by
//! index alone, and reports repeats as HELD.
//!
//! `--path stream` (R35, the user's ruling of 2026-10-26): the same takes on
//! the stream path instead, stream-only, so the View draws into the stream's
//! own pool surface and the stream thread encodes it (R31's shape). A local
//! ffmpeg RTMP listener receives the stream into the run's directory, and the
//! received frames are read as the recording is. No trace is needed: the
//! stream thread's PTS index is the master frame less the stream's first, so
//! each received frame's timestamp names the frame its tick drew, and the
//! constant cancels in each take's offset.
//!
//! Counted per run: the read-ahead's misses, the View's dropped frames, the
//! machine's swap-ins and swap-outs (and the swap in use), and the load
//! average, sampled every 10 s.

import { randomUUID } from "node:crypto";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { cpSync, createWriteStream, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync, type WriteStream } from "node:fs";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { loadComplete, residency } from "./load-residency.js";
import { R20_FRAMES, R20_ITEM, r20Classify } from "./r20-counter.js";
import { startFlvReceiver, type FlvReceiver } from "./rtmp-flv-receiver.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/debug/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
const HOUSE_RATE = 30;
const BLACK_ITEM = "R31B";
/** The counter on air: its 150 frames, then half a second of its last. */
const COUNTER_HOLD_MS = (R20_FRAMES / HOUSE_RATE) * 1000 + 500;
/** The separator: a cleared View, recorded black. */
const BLACK_HOLD_MS = 1000;
/** A run of at least this many black frames separates two takes. */
const SEPARATOR_FRAMES = 5;
const COMMAND_MS = 5_000;
const LOAD_MS = 180_000;
const RESIDENT_MS = 30_000;
/** A 30-minute recording's finalize is a real mux finish. */
const RECORD_STOP_MS = 120_000;
const FFMPEG_FALLBACKS = ["/usr/local/bin/ffmpeg", "/opt/homebrew/bin/ffmpeg"];

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`);
  return i < 0 ? undefined : process.argv[i + 1];
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** The machine, now: load, swap traffic, swap in use. */
function machine(): Record<string, unknown> {
  const vm = spawnSync("vm_stat", { encoding: "utf8" }).stdout ?? "";
  const pages = (label: string): number | null => {
    const m = vm.match(new RegExp(`^${label}:\\s+(\\d+)`, "m"));
    return m?.[1] === undefined ? null : Number(m[1]);
  };
  const swap = spawnSync("sysctl", ["-n", "vm.swapusage"], { encoding: "utf8" }).stdout ?? "";
  const used = swap.match(/used = ([\d.]+)M/)?.[1];
  const [l1, l5, l15] = loadavg();
  return {
    at: new Date().toISOString(),
    load1: l1,
    load5: l5,
    load15: l15,
    swapins: pages("Swapins"),
    swapouts: pages("Swapouts"),
    swapUsedMib: used === undefined ? null : Number(used),
  };
}

function battery(): string {
  return (spawnSync("pmset", ["-g", "batt"], { encoding: "utf8" }).stdout ?? "").trim();
}

function resolveTool(name: string, fallbacks: string[]): string | null {
  for (const dir of (process.env["PATH"] ?? "").split(":")) {
    if (dir && existsSync(join(dir, name))) return join(dir, name);
  }
  return fallbacks.find((f) => existsSync(f)) ?? null;
}

/**
 * The dress package with counter_150 and the separator: a copy under the run's
 * directory, recording into the run's directory. Step 12b's staging, plus the
 * empty scene.
 */
function stagePackage(out: string, tap: "zeroCopy" | "cpuReadback"): { pkgDir: string; recordDir: string } {
  const pkgDir = join(out, "package");
  const recordDir = join(out, "record");
  mkdirSync(recordDir, { recursive: true });
  cpSync(DRESS, pkgDir, { recursive: true });
  const manifestPath = join(pkgDir, "manifest.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as Record<string, unknown>;
  // `--tap cpu`: the A/B against the zero-copy tap, through the manifest's
  // own preference (SPEC v0.4.5 `tapPath`), so nothing in the engine changes.
  (manifest["show"] as Record<string, unknown>)["outputs"] = {
    record: tap === "cpuReadback" ? { directory: recordDir, tapPath: "cpuReadback" } : { directory: recordDir },
  };
  (manifest["assets"] as unknown[]).push({
    id: "R20_counter",
    kind: "video",
    source: "media/counter_150.mp4",
    format: "h264",
  });
  (manifest["scenes"] as unknown[]).push(
    { id: "SCN_R20", elements: [{ id: "main", kind: "clip", z: 1, assetId: "R20_counter" }] },
    { id: "SCN_R31B", elements: [] },
  );
  ((manifest["rundown"] as Record<string, unknown>)["items"] as unknown[]).push(
    { id: R20_ITEM, kind: "sceneRef", sceneRef: "SCN_R20", durationFrames: R20_FRAMES },
    { id: BLACK_ITEM, kind: "sceneRef", sceneRef: "SCN_R31B" },
  );
  writeFileSync(manifestPath, JSON.stringify(manifest));
  return { pkgDir, recordDir };
}

interface Take {
  /** The counter's take: its index in each frame, in recorded order, and the
   *  master frame its tick drew when the run was traced. */
  frames: { t: number; k: number; f?: number }[];
  /** Frames inside the take that read neither the counter nor black. */
  other: number;
  /** Short black runs inside the take (a dropped layer, step 12c's third assertion). */
  blackInside: number;
}

interface Analysis {
  /** Which path was read: the recording, or the stream as received (R35). */
  path: "record" | "stream";
  recording: string;
  recordedFrames: number;
  takesRead: number;
  /** Clause 1: a frame showing a lower index than the frame before it. */
  backwards: { take: number; at: { t: number; k: number }; prev: number }[];
  /** Clause 2: a frame showing a lower index than one already shown in its take. */
  earlier: { take: number; at: { t: number; k: number }; shown: number }[];
  /** Held: a frame repeating the one before it, short of the clip's held last frame. */
  repeats: number;
  holds: { take: number; at: { t: number; k: number } }[];
  /** Diagnostic: a frame more than one index past the one before it. */
  skips: number;
  /** Diagnostic: takes that never reached frame 120 (step 12c's primed-window check). */
  shortTakes: number[];
  other: number;
  blackInside: number;
  /** Whether the run carried the record trace (the exact clause 2). */
  traced: boolean;
  /** Recorded frames the trace does not name (should be 0 on a traced run). */
  untraced: number;
  /** Clause 2, exact: a picture older than the frame its tick drew. */
  stale: { take: number; at: { t: number; k: number; f: number }; offset: number }[];
  /** R33: consecutive recorded frames drawn at the same master frame, or a
   *  master frame apart by more than one. */
  r33: { repeats: number; skips: number };
}

/** The record trace: the master frame each submitted frame was drawn at, by
 *  its submit index (the recording's PTS index). */
function readTrace(out: string): Map<number, number> | null {
  const path = join(out, "trace.log");
  if (!existsSync(path)) return null;
  const byIndex = new Map<number, number>();
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const kv = Object.fromEntries(line.split(" ").slice(1).map((x) => x.split("=") as [string, string]));
    if (kv["submitted"] !== "true" || kv["frame"] === undefined || kv["frame"] === "-") continue;
    byIndex.set(Number(kv["submits"]) - 1, Number(kv["frame"]));
  }
  return byIndex;
}

/** Read the run's recording back, frame by frame, with step 12c's classifier. */
function analyse(out: string): Analysis {
  const ffmpeg = resolveTool("ffmpeg", FFMPEG_FALLBACKS);
  const ffprobe = ffmpeg?.replace(/ffmpeg$/, "ffprobe") ?? null;
  if (ffmpeg === null || ffprobe === null) throw new Error("ffmpeg and ffprobe are required to read the recording");
  const received = join(out, "stream", "stream.flv");
  const streamed = existsSync(received);
  let recording = received;
  if (!streamed) {
    const recordDir = join(out, "record");
    const mp4s = readdirSync(recordDir).filter((f) => f.endsWith(".mp4"));
    if (mp4s.length !== 1 || mp4s[0] === undefined) throw new Error(`expected one recording in ${recordDir}, saw ${JSON.stringify(mp4s)}`);
    recording = join(recordDir, mp4s[0]);
  }
  // Step 12c's read: each recorded frame's centre pixel, in presentation order.
  const px = spawnSync(
    ffmpeg,
    ["-v", "error", "-i", recording, "-map", "0:v", "-fps_mode", "passthrough",
     "-vf", "crop=8:8:(iw-8)/2:(ih-8)/2,scale=1:1:flags=area", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
    { maxBuffer: 512 * 1024 * 1024 },
  );
  if (px.status !== 0) throw new Error(`ffmpeg could not decode the recording: ${px.stderr?.toString()}`);
  const probe = spawnSync(
    ffprobe,
    ["-v", "error", "-select_streams", "v", "-show_entries", "frame=pts_time", "-of", "csv=p=0", recording],
    { encoding: "utf8", maxBuffer: 512 * 1024 * 1024 },
  );
  if (probe.status !== 0) throw new Error(`ffprobe could not read the recording: ${probe.stderr}`);
  const pts = probe.stdout.split("\n").filter((l) => l.trim() !== "").map((l) => parseFloat(l));
  const bytes = px.stdout as Buffer;
  if (bytes.length !== pts.length * 3) throw new Error(`one centre pixel per frame: ${bytes.length} bytes for ${pts.length} frames`);
  const frames = pts.map((t, i) => ({
    t,
    k: r20Classify([bytes[3 * i] as number, bytes[3 * i + 1] as number, bytes[3 * i + 2] as number]),
  }));
  writeFileSync(join(out, "sequence.txt"), frames.map((f) => `${f.t.toFixed(3)}:${f.k}`).join("\n") + "\n");
  const trace = streamed ? null : readTrace(out);
  // The stream's own join (R35): its PTS index is the master frame less the
  // stream's first, so the timestamp names the tick, up to a constant.
  const tickFrame = (t: number): number | undefined =>
    streamed ? Math.round(t * HOUSE_RATE) : trace?.get(Math.round(t * HOUSE_RATE));

  // Split into takes at each separator: a run of SEPARATOR_FRAMES black frames
  // or more. A shorter black run inside a take is step 12c's black on air.
  const takes: Take[] = [];
  let cur: Take | null = null;
  let blackRun = 0;
  for (const f of frames) {
    if (f.k === "black") {
      blackRun += 1;
      continue;
    }
    if (blackRun >= SEPARATOR_FRAMES || cur === null) {
      cur = { frames: [], other: 0, blackInside: 0 };
      takes.push(cur);
    } else if (blackRun > 0) {
      cur.blackInside += 1;
    }
    blackRun = 0;
    if (f.k === "other") cur.other += 1;
    else {
      const tf = tickFrame(f.t);
      cur.frames.push(tf === undefined ? { t: f.t, k: f.k } : { t: f.t, k: f.k, f: tf });
    }
  }

  const result: Analysis = {
    path: streamed ? "stream" : "record",
    recording,
    recordedFrames: frames.length,
    takesRead: takes.length,
    backwards: [],
    earlier: [],
    repeats: 0,
    holds: [],
    skips: 0,
    shortTakes: [],
    other: 0,
    blackInside: 0,
    traced: streamed || trace !== null,
    untraced: streamed || trace !== null ? frames.filter((f) => tickFrame(f.t) === undefined).length : frames.length,
    stale: [],
    r33: { repeats: 0, skips: 0 },
  };
  if (result.traced) {
    // R33, over every recorded frame in submit order: the ticks' master frames.
    let prevF: number | undefined;
    for (const f of frames) {
      const tf = tickFrame(f.t);
      if (tf !== undefined && prevF !== undefined) {
        if (tf === prevF) result.r33.repeats += 1;
        else if (tf > prevF + 1) result.r33.skips += 1;
      }
      prevF = tf;
    }
  }
  takes.forEach((take, n) => {
    result.other += take.other;
    result.blackInside += take.blackInside;
    let shown = -1;
    take.frames.forEach((f, i) => {
      const prev = take.frames[i - 1];
      if (prev !== undefined) {
        if (f.k < prev.k) result.backwards.push({ take: n, at: f, prev: prev.k });
        if (f.k === prev.k && f.k < R20_FRAMES - 1) {
          result.repeats += 1;
          result.holds.push({ take: n, at: f });
        }
        if (f.k > prev.k + 1) result.skips += 1;
      }
      if (f.k < shown) result.earlier.push({ take: n, at: f, shown });
      shown = Math.max(shown, f.k);
    });
    if (shown < 120) result.shortTakes.push(n);
    // Clause 2, exact: the take's offset is the mode of master frame less
    // index; a picture below it is older than the frame its tick drew.
    const counts = new Map<number, number>();
    for (const f of take.frames) {
      if (f.f !== undefined && f.k < R20_FRAMES - 1) counts.set(f.f - f.k, (counts.get(f.f - f.k) ?? 0) + 1);
    }
    let offset: number | undefined;
    let best = 0;
    for (const [o, c] of counts) {
      if (c > best) {
        best = c;
        offset = o;
      }
    }
    if (offset === undefined) return;
    for (const f of take.frames) {
      if (f.f !== undefined && f.k < R20_FRAMES - 1 && f.f - f.k > offset) {
        result.stale.push({ take: n, at: { t: f.t, k: f.k, f: f.f }, offset });
      }
    }
  });
  return result;
}

async function run(
  out: string,
  minutes: number,
  stateLabel: string,
  tap: "zeroCopy" | "cpuReadback",
  gpu: "high" | "low",
  path: "record" | "stream",
): Promise<Record<string, unknown>> {
  if (!existsSync(ENGINE_BIN)) throw new Error(`the engine binary must be built first: ${ENGINE_BIN} (cargo build -p nbe-engine)`);
  mkdirSync(out, { recursive: true });
  const { pkgDir } = stagePackage(out, tap);
  const meta: Record<string, unknown> = {
    probe: path === "stream" ? "R35 stream (12c's takes)" : "R31 12c",
    path,
    state: stateLabel,
    minutes,
    tap,
    gpu,
    engine: ENGINE_BIN,
    // Every NBE_* variable the engine inherits: an experiment's switch is
    // part of the run's identity. Never a credential (NBE_RENDER_TOKEN and its
    // kind): the meta is evidence that may be quoted into the records.
    engineEnv: Object.fromEntries(
      Object.entries(process.env).filter(
        ([k]) => (k.startsWith("NBE_") || k.startsWith("WGPU_")) && !/TOKEN|SECRET|KEY|PASS|CRED/i.test(k),
      ),
    ),
    head: spawnSync("git", ["-C", REPO, "rev-parse", "HEAD"], { encoding: "utf8" }).stdout.trim(),
    startedAt: new Date().toISOString(),
    machineAtStart: machine(),
    batteryAtStart: battery(),
  };
  writeFileSync(join(out, "meta.json"), JSON.stringify(meta, null, 2));

  const machineLog = createWriteStream(join(out, "machine.jsonl"));
  const telemetryLog = createWriteStream(join(out, "telemetry.jsonl"));
  const takeLog = createWriteStream(join(out, "takes.jsonl"));
  const engineLogFile = createWriteStream(join(out, "engine.log"));
  const line = (w: WriteStream, o: unknown): boolean => w.write(JSON.stringify(o) + "\n");

  const state = new ControlPlaneState();
  const server: ControlPlaneServer = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(out, "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  // The load's residency lines are read from memory until the load completes;
  // after that the log goes to its file only.
  let engineLog: string[] = [];
  let keepInMemory = true;
  const engine: ChildProcess = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      // The silent sink, as the dress runs (the device-audio design record, D7).
      NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
      NBE_HOUSE_RATE: String(HOUSE_RATE),
      RUST_LOG: "info",
      // The record trace: each handed-off frame's master frame and submit
      // count, so every recorded frame joins to the frame its tick drew.
      NBE_RECORD_TRACE: join(out, "trace.log"),
      // `--gpu low`: wgpu's own preference variable selects the integrated
      // GPU (the engine's default is HighPerformance).
      ...(gpu === "low" ? { WGPU_POWER_PREF: "low" } : {}),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const onEngine = (b: Buffer): void => {
    engineLogFile.write(b);
    if (keepInMemory) engineLog.push(b.toString());
  };
  engine.stdout?.on("data", onEngine);
  engine.stderr?.on("data", onEngine);
  const sampler = setInterval(() => line(machineLog, machine()), 10_000);
  line(machineLog, machine());

  const ticks: Record<string, unknown>[] = [];
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { Authorization: `Bearer ${ADMIN}`, "X-NBE-Role": "admin" },
  });
  ws.on("message", (raw: Buffer) => {
    const frame = JSON.parse(raw.toString()) as Record<string, unknown>;
    if (frame["kind"] !== "telemetry") return;
    line(telemetryLog, frame);
    ticks.push(frame);
    if (ticks.length > 10) ticks.shift();
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  ws.send(JSON.stringify({ v: "0.3", id: randomUUID(), command: "system.telemetry.subscribe", payload: { intervalMs: 1000 } }));
  const tick = (): Record<string, unknown> => (ticks.at(-1)?.["data"] ?? {}) as Record<string, unknown>;

  const send = (command: string, payload: unknown, ms: number): Promise<Record<string, unknown>> => {
    const id = randomUUID();
    return new Promise((res, rej) => {
      const timer = setTimeout(() => rej(new Error(`no response to ${command} within ${ms} ms`)), ms);
      const onMsg = (raw: Buffer): void => {
        const msg = JSON.parse(raw.toString()) as Record<string, unknown>;
        if (msg["requestId"] !== id) return;
        clearTimeout(timer);
        ws.off("message", onMsg);
        res(msg);
      };
      ws.on("message", onMsg);
      ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
    });
  };
  const ok = async (command: string, payload: unknown = {}, ms = COMMAND_MS): Promise<number> => {
    const r = await send(command, payload, ms);
    if (r["status"] !== "ok") throw new Error(`${command}: ${JSON.stringify(r["error"] ?? r)}`);
    return (r["stateVersion"] ?? 0) as number;
  };
  const applied = async (command: string, payload: unknown, ms: number, what: string): Promise<void> => {
    if (!(await server.awaitApplied(await ok(command, payload, ms), ms))) throw new Error(`the engine never applied ${what}`);
  };
  const until = async (what: string, pred: () => boolean, ms: number): Promise<void> => {
    const deadline = Date.now() + ms;
    while (!pred()) {
      if (Date.now() > deadline) throw new Error(`waited ${ms} ms for ${what}`);
      await sleep(50);
    }
  };

  // The stream's receiver (R35): what arrives, written with its timestamps
  // into the run's directory.
  let receiver: FlvReceiver | null = null;
  try {
    await until("the engine's registration", () => server.wsBridge.renderNodeCount() === 1, 30_000);
    await applied("show.load", { packagePath: pkgDir }, LOAD_MS, "the probe package's load");
    await until(
      "the load's rings resident",
      () => loadComplete(residency(engineLog.join("").split("\n"))),
      RESIDENT_MS,
    );
    keepInMemory = false;
    engineLog = [];
    await applied("show.start", { startClock: true }, COMMAND_MS, "show.start");
    if (path === "stream") {
      mkdirSync(join(out, "stream"), { recursive: true });
      receiver = await startFlvReceiver(join(out, "stream", "stream.flv"));
      const streamUrl = `rtmp://127.0.0.1:${receiver.port}/live/r35probe`;
      meta["streamUrl"] = streamUrl;
      await applied("stream.start", { url: streamUrl }, COMMAND_MS, "stream.start (no hardware encoder?)");
      await sleep(2000);
      meta["streamTransportState"] = tick()["streamTransportState"];
      if (tick()["streamState"] !== "live") throw new Error(`stream.start applied; the tick reads streamState ${JSON.stringify(tick()["streamState"])}`);
    } else {
      await applied("record.start", { outputId: "r31probe" }, COMMAND_MS, "record.start (no hardware encoder?)");
      await sleep(2000);
      const tapPath = tick()["recordTapPath"];
      meta["recordTapPath"] = tapPath;
      if (tapPath !== tap) throw new Error(`the probe asked for the ${tap} tap; the tick reads recordTapPath ${JSON.stringify(tapPath)}`);
    }
    const before = { ...tick() };

    // A timed item ends DONE, and a DONE item takes again only after
    // `item.reset` (SPEC §16.4), as an operator replays it.
    const takeOn = async (item: string, cycle: number): Promise<void> => {
      if (item === R20_ITEM && cycle > 0) {
        await until(`${item} DONE`, () => state.itemStates.get(item) === "DONE", 5_000);
      }
      if (state.itemStates.get(item) === "DONE") await ok("item.reset", { itemId: item });
      await ok("preview.set", { itemRef: item });
      const sentAt = new Date().toISOString();
      const sv = await ok("view.take", { transition: "cut", audio: { transition: "follow" } });
      line(takeLog, { cycle, item, sentAt, stateVersion: sv });
    };
    const deadline = Date.now() + minutes * 60_000;
    let cycles = 0;
    while (Date.now() < deadline) {
      await takeOn(R20_ITEM, cycles);
      await sleep(COUNTER_HOLD_MS);
      await takeOn(BLACK_ITEM, cycles);
      await sleep(BLACK_HOLD_MS);
      cycles += 1;
    }
    const after = { ...tick() };
    if (path === "stream") {
      await applied("stream.stop", {}, COMMAND_MS, "stream.stop");
      meta["streamVideoMessages"] = receiver?.videoMessages();
      meta["streamParseError"] = receiver?.parseError();
    } else {
      await applied("record.stop", {}, RECORD_STOP_MS, "record.stop");
    }
    const delta = (k: string): number => ((after[k] ?? 0) as number) - ((before[k] ?? 0) as number);
    Object.assign(meta, {
      cycles,
      readAheadMisses: delta("readAheadMissesTotal"),
      droppedFrames: delta("droppedFramesTotal"),
      watchdogTrips: delta("watchdogTripsTotal"),
      ...(path === "stream"
        ? { streamTransportStateAtEnd: after["streamTransportState"], streamBufferMsAtEnd: after["streamBufferMs"] }
        : { recordTapPathAtEnd: after["recordTapPath"] }),
    });
  } finally {
    clearInterval(sampler);
    await receiver?.close();
    const end = machine();
    line(machineLog, end);
    const start = meta["machineAtStart"] as Record<string, number | null>;
    Object.assign(meta, {
      endedAt: new Date().toISOString(),
      machineAtEnd: end,
      batteryAtEnd: battery(),
      swapinsDelta: (end["swapins"] as number) - (start["swapins"] ?? 0),
      swapoutsDelta: (end["swapouts"] as number) - (start["swapouts"] ?? 0),
    });
    engine.kill("SIGTERM");
    ws.close();
    await server.close();
    for (const w of [machineLog, telemetryLog, takeLog, engineLogFile]) w.end();
    await sleep(200);
    meta["adapter"] =
      readFileSync(join(out, "engine.log"), "utf8")
        .replace(/\x1b\[[0-9;]*m/g, "")
        .match(/gpu adapter selected adapter=(.*?) backend=/)?.[1] ?? null;
    writeFileSync(join(out, "meta.json"), JSON.stringify(meta, null, 2));
  }
  return meta;
}

async function main(): Promise<void> {
  const again = arg("analyse");
  const out = resolve(again ?? arg("out") ?? join(REPO, ".git/nbe-staged/r31", new Date().toISOString().replace(/[:.]/g, "-")));
  if (again === undefined) {
    const minutes = Number(arg("minutes") ?? 30);
    const tap = arg("tap") === "cpu" ? "cpuReadback" : "zeroCopy";
    const path = arg("path") === "stream" ? "stream" : "record";
    const meta = await run(out, minutes, arg("state") ?? "unlabelled", tap, arg("gpu") === "low" ? "low" : "high", path);
    console.log(`${path === "stream" ? "R35" : "R31"} PROBE RUN: ${JSON.stringify(meta)}`);
  }
  const a = analyse(out);
  const name = a.path === "stream" ? "R35 PROBE (stream)" : "R31 PROBE";
  // Traced: clause 2 is exact, and every repeat is attributed (R33's, or
  // stale). Untraced (an older run): clause 2 by index alone, repeats HELD.
  const sighting = a.traced
    ? a.backwards.length > 0 || a.stale.length > 0
    : a.backwards.length > 0 || a.earlier.length > 0;
  const verdict = sighting ? "SIGHTING" : !a.traced && a.repeats > 0 ? "HELD" : "CLEAN";
  writeFileSync(join(out, "result.json"), JSON.stringify({ ...a, verdict }, null, 2));
  console.log(
    `${name} READ: ${a.recordedFrames} recorded frames, ${a.takesRead} takes of the counter; ` +
      `backwards ${a.backwards.length}, earlier ${a.earlier.length}; repeats ${a.repeats}, skips ${a.skips}, ` +
      `short takes ${a.shortTakes.length}, other ${a.other}, black inside ${a.blackInside}`,
  );
  if (a.traced) {
    console.log(
      `${name} TRACE: stale ${a.stale.length} (clause 2, exact); untraced frames ${a.untraced}; ` +
        `R33: ${a.r33.repeats} repeated master frames, ${a.r33.skips} skipped`,
    );
  }
  if (sighting) {
    console.log(`${name}: SIGHTING. First: ${JSON.stringify(a.stale[0] ?? a.backwards[0] ?? a.earlier[0])}. Kept in ${out}`);
    process.exitCode = 1;
  } else if (verdict === "HELD") {
    console.log(`${name}: HELD. No frame went backwards; ${a.repeats} frame(s) held (the slot reading of clause 2, the user's to rule). Kept in ${out}`);
  } else {
    console.log(`${name}: CLEAN. Kept in ${out}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
