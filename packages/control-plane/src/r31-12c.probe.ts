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
//! What it asserts, per recorded frame, read with step 12c's classifier
//! (`r20-counter.ts`), in each take of the counter:
//! 1. the recorded sequence never goes backwards (a frame shows a lower
//!    index than the frame before it: step 12c's assertion, S1's signature);
//! 2. no frame's slot carries an earlier frame's picture (a frame shows a
//!    lower index than one already shown in that take).
//!
//! Any occurrence is a sighting: R31 reopens for a real hunt. A clean run is
//! the specification's demotion to watch, the user's to rule.
//!
//! Held frames are counted apart: a frame recorded twice, the next frame's
//! slot carrying it and the next frame never recorded (k, k, k + 2). Read by
//! slot, clause 2 would count each one; read as S1 was, it does not (S1 was
//! an older picture after a newer one). The probe reports them as HELD, with
//! their positions, for the user to rule, and never folds them into CLEAN.
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
  /** The counter's take: its index in each frame, in recorded order. */
  frames: { t: number; k: number }[];
  /** Frames inside the take that read neither the counter nor black. */
  other: number;
  /** Short black runs inside the take (a dropped layer, step 12c's third assertion). */
  blackInside: number;
}

interface Analysis {
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
}

/** Read the run's recording back, frame by frame, with step 12c's classifier. */
function analyse(out: string): Analysis {
  const ffmpeg = resolveTool("ffmpeg", FFMPEG_FALLBACKS);
  const ffprobe = ffmpeg?.replace(/ffmpeg$/, "ffprobe") ?? null;
  if (ffmpeg === null || ffprobe === null) throw new Error("ffmpeg and ffprobe are required to read the recording");
  const recordDir = join(out, "record");
  const mp4s = readdirSync(recordDir).filter((f) => f.endsWith(".mp4"));
  if (mp4s.length !== 1 || mp4s[0] === undefined) throw new Error(`expected one recording in ${recordDir}, saw ${JSON.stringify(mp4s)}`);
  const recording = join(recordDir, mp4s[0]);
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
    else cur.frames.push({ t: f.t, k: f.k });
  }

  const result: Analysis = {
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
  };
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
  });
  return result;
}

async function run(out: string, minutes: number, stateLabel: string, tap: "zeroCopy" | "cpuReadback"): Promise<Record<string, unknown>> {
  if (!existsSync(ENGINE_BIN)) throw new Error(`the engine binary must be built first: ${ENGINE_BIN} (cargo build -p nbe-engine)`);
  mkdirSync(out, { recursive: true });
  const { pkgDir } = stagePackage(out, tap);
  const meta: Record<string, unknown> = {
    probe: "R31 12c",
    state: stateLabel,
    minutes,
    tap,
    engine: ENGINE_BIN,
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
    await applied("record.start", { outputId: "r31probe" }, COMMAND_MS, "record.start (no hardware encoder?)");
    await sleep(2000);
    const tapPath = tick()["recordTapPath"];
    meta["recordTapPath"] = tapPath;
    if (tapPath !== tap) throw new Error(`the probe asked for the ${tap} tap; the tick reads recordTapPath ${JSON.stringify(tapPath)}`);
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
    await applied("record.stop", {}, RECORD_STOP_MS, "record.stop");
    const delta = (k: string): number => ((after[k] ?? 0) as number) - ((before[k] ?? 0) as number);
    Object.assign(meta, {
      cycles,
      readAheadMisses: delta("readAheadMissesTotal"),
      droppedFrames: delta("droppedFramesTotal"),
      watchdogTrips: delta("watchdogTripsTotal"),
      recordTapPathAtEnd: after["recordTapPath"],
    });
  } finally {
    clearInterval(sampler);
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
    const meta = await run(out, minutes, arg("state") ?? "unlabelled", tap);
    console.log(`R31 PROBE RUN: ${JSON.stringify(meta)}`);
  }
  const a = analyse(out);
  const sighting = a.backwards.length > 0 || a.earlier.length > 0;
  const verdict = sighting ? "SIGHTING" : a.repeats > 0 ? "HELD" : "CLEAN";
  writeFileSync(join(out, "result.json"), JSON.stringify({ ...a, verdict }, null, 2));
  console.log(
    `R31 PROBE READ: ${a.recordedFrames} recorded frames, ${a.takesRead} takes of the counter; ` +
      `backwards ${a.backwards.length}, earlier ${a.earlier.length}; repeats ${a.repeats}, skips ${a.skips}, ` +
      `short takes ${a.shortTakes.length}, other ${a.other}, black inside ${a.blackInside}`,
  );
  if (sighting) {
    console.log(`R31 PROBE: SIGHTING. First: ${JSON.stringify(a.backwards[0] ?? a.earlier[0])}. Kept in ${out}`);
    process.exitCode = 1;
  } else if (verdict === "HELD") {
    console.log(`R31 PROBE: HELD. No frame went backwards; ${a.repeats} frame(s) held (the slot reading of clause 2, the user's to rule). Kept in ${out}`);
  } else {
    console.log(`R31 PROBE: CLEAN. Kept in ${out}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
