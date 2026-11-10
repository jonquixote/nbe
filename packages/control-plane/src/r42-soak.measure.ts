// R42's closure soak, one show (docs/soak-protocol.md section 8; the user's rulings of
// 2026-11-08, 01:55 and 03:45). An AC-5-style show (docs/spec.v0.3.md AC-5, its
// measurement `droppedFramesTotal == 0` over the full show): one continuous show on the
// release engine at 1080p30, at section 20's ceiling where a laptop with no camera can
// reach it: the View and the preview, the recording and one RTMP output live together,
// two overlays up, three clips and a graphic taken in turn by mix and cut, one take
// every 20 s. No camera and no WHIP guest: neither exists here, so this is AC-5-style,
// not AC-5. It is the harness of R42's sightings (2026-11-01 and -02, kept as
// `.git/nbe-staged/ac5/`), brought into the tree, with R42's own column added: both
// outputs' durations against their wall spans, beside the drop counter and the panics.
// Measurement only: it asserts nothing, and scripts/soak-r42.sh reads its result.json.
// Run from packages/control-plane:
//   NBE_ENGINE_BIN=<bin> node --import tsx src/r42-soak.measure.ts --out <dir> --minutes 30
import { randomUUID } from "node:crypto";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { cpSync, createWriteStream, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { loadComplete, residency } from "./load-residency.js";
import { startFlvReceiver } from "./rtmp-flv-receiver.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
const arg = (n: string, d: string): string => (process.argv.includes(`--${n}`) ? (process.argv[process.argv.indexOf(`--${n}`) + 1] ?? d) : d);
const out = resolve(arg("out", "ac5-out"));
const MINUTES = Number(arg("minutes", "30"));
const TAKE_EVERY_MS = 20_000;

/** ffprobe on PATH first (R49's follow-up), then Homebrew's two prefixes. */
function ffprobe(): string | null {
  const onPath = spawnSync("/usr/bin/which", ["ffprobe"], { encoding: "utf8" }).stdout?.trim();
  for (const p of [onPath, "/opt/homebrew/bin/ffprobe", "/usr/local/bin/ffprobe"]) if (p && existsSync(p)) return p;
  return null;
}
const FFPROBE = ffprobe();
/** One stream's frame count and duration in seconds, as ffprobe reads the file; null without ffprobe. */
function probe(file: string, stream: "v:0" | "a:0"): { frames: number | null; seconds: number | null } | null {
  if (FFPROBE === null) return null;
  const r = spawnSync(FFPROBE, ["-v", "error", "-select_streams", stream, "-count_packets", "-show_entries", "stream=nb_read_packets,duration", "-show_entries", "format=duration", "-of", "json", file], { encoding: "utf8" });
  try {
    const j = JSON.parse(r.stdout) as { streams?: Array<{ nb_read_packets?: string; duration?: string }>; format?: { duration?: string } };
    const s = j.streams?.[0];
    if (s === undefined) return { frames: null, seconds: null };
    const sec = Number(s.duration ?? j.format?.duration ?? NaN);
    return { frames: s.nb_read_packets === undefined ? null : Number(s.nb_read_packets), seconds: Number.isFinite(sec) ? sec : null };
  } catch {
    return { frames: null, seconds: null };
  }
}

mkdirSync(join(out, "record"), { recursive: true });
mkdirSync(join(out, "stream"), { recursive: true });
const pkgDir = join(out, "package");
cpSync(DRESS, pkgDir, { recursive: true });
const manifest = JSON.parse(readFileSync(join(pkgDir, "manifest.json"), "utf8")) as Record<string, unknown>;
(manifest["show"] as Record<string, unknown>)["outputs"] = { record: { directory: join(out, "record") } };
manifest["overlays"] = [
  { id: "ol_ticker", elements: [{ id: "ticker", kind: "graphic", z: 5, templateId: "TPL_GENERIC", fields: { color: "#c0392b" }, transform: { x: 0, y: 0.89, w: 1, h: 0.08 } }] },
  { id: "ol_bug", elements: [{ id: "bug", kind: "graphic", z: 6, templateId: "TPL_GENERIC", fields: { color: "#f1c40f" }, transform: { x: 0.88, y: 0.04, w: 0.08, h: 0.08 } }] },
];
writeFileSync(join(pkgDir, "manifest.json"), JSON.stringify(manifest));

const state = new ControlPlaneState();
const server = await createControlPlaneServer({
  port: 0,
  auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
  audit: new AuditLog(join(out, "audit.jsonl")),
  state,
  persistence: { onDirty: () => {}, flushNow: () => {} },
  warn: () => {},
});
const logFile = createWriteStream(join(out, "engine.log"));
let engineLog = "";
const engine: ChildProcess = spawn(ENGINE_BIN, [], {
  env: {
    ...process.env,
    NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
    NBE_RENDER_TOKEN: RENDER,
    NBE_AUDIO_SINK: "null",
    NBE_HOUSE_RATE: "30",
    RUST_LOG: "info",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
const onEngine = (b: Buffer): void => {
  logFile.write(b);
  engineLog += b.toString();
};
engine.stdout?.on("data", onEngine);
engine.stderr?.on("data", onEngine);

const ticks: Record<string, unknown>[] = [];
const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
  headers: { Authorization: `Bearer ${ADMIN}`, "X-NBE-Role": "admin" },
});
ws.on("message", (raw: Buffer) => {
  const f = JSON.parse(raw.toString()) as Record<string, unknown>;
  if (f["kind"] === "telemetry") ticks.push(f);
});
await new Promise<void>((res, rej) => {
  ws.once("open", () => res());
  ws.once("error", rej);
});
ws.send(JSON.stringify({ v: "0.3", id: randomUUID(), command: "system.telemetry.subscribe", payload: { intervalMs: 250 } }));
const tick = (): Record<string, unknown> => (ticks.at(-1)?.["data"] ?? {}) as Record<string, unknown>;
const dropped = (): number => Number(tick()["droppedFramesTotal"] ?? NaN);
const send = (command: string, payload: unknown, ms = 120_000): Promise<Record<string, unknown>> => {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error(`no response to ${command}`)), ms);
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
const applied = async (command: string, payload: unknown): Promise<void> => {
  const r = await send(command, payload);
  if (r["status"] !== "ok") throw new Error(`${command}: ${JSON.stringify(r["error"] ?? r)}`);
  if (!(await server.awaitApplied((r["stateVersion"] ?? 0) as number, 120_000))) throw new Error(`${command} never applied`);
};
const until = async (what: string, pred: () => boolean, ms: number): Promise<void> => {
  const end = Date.now() + ms;
  while (!pred()) {
    if (Date.now() > end) throw new Error(`waited for ${what}`);
    await sleep(50);
  }
};


const clean = (): string => engineLog.replace(/\x1b\[[0-9;]*m/g, "");
const count = (re: RegExp): number => (clean().match(re) ?? []).length;
const loads: number[] = [];
try {
  await until("the engine's registration", () => server.wsBridge.renderNodeCount() === 1, 30_000);
  const logFrom = engineLog.length;
  await applied("show.load", { packagePath: pkgDir });
  await until("the load's rings resident", () => loadComplete(residency(engineLog.slice(logFrom).split("\n"))), 60_000);
  const receiver = await startFlvReceiver(join(out, "stream", "stream.flv"));
  await sleep(1000);
  await applied("show.start", { startClock: true });
  const started = Date.now();
  await applied("preview.set", { itemRef: "A2" });
  await applied("view.take", { transition: "cut" });
  await applied("overlay.show", { overlayId: "ol_ticker" });
  await applied("overlay.show", { overlayId: "ol_bug" });
  await applied("record.start", { outputId: "ac5" });
  const recordStartedAt = Date.now();
  await applied("stream.start", { url: `rtmp://127.0.0.1:${receiver.port}/live/ac5` });
  const streamStartedAt = Date.now();
  await until("the stream live", () => tick()["streamState"] === "live", 20_000);
  const ITEMS = ["A3", "G", "A1", "A2"];
  let n = 0;
  while (Date.now() - started < MINUTES * 60_000) {
    await sleep(TAKE_EVERY_MS);
    loads.push(loadavg()[0]!);
    const item = ITEMS[n % ITEMS.length]!;
    // A timed item ends DONE and takes again only after `item.reset` (SPEC §16.4), as an operator replays it.
    if (state.itemStates.get(item) === "DONE") await applied("item.reset", { itemId: item });
    await applied("preview.set", { itemRef: item });
    await applied("view.take", n % 2 === 0 ? { transition: "mix", durationFrames: 15 } : { transition: "cut" });
    n += 1;
  }
  const end = tick();
  await applied("stream.stop", {});
  const streamStoppedAt = Date.now();
  let recordStopError: string | null = null;
  await applied("record.stop", {}).catch((e: unknown) => {
    recordStopError = String(e);
  });
  const recordStoppedAt = Date.now();
  await sleep(1000);
  const atStop = tick();
  await applied("show.stop", {});
  await sleep(1000);
  await receiver.close();
  const text = clean();
  const run = text.match(/run ended after (\d+) ticks: repeats (\d+), skips (\d+), drops (\d+); woke late mean (\S+ \S+), max (\S+ [^;\s]+)/);
  const file = readdirSync(join(out, "record")).filter((f) => /\.(mp4|mov|mkv)$/.test(f)).map((f) => join(out, "record", f))[0];
  const flv = join(out, "stream", "stream.flv");
  const result = {
    minutes: (Date.now() - started) / 60_000,
    takes: n,
    droppedFramesTotal: atStop["droppedFramesTotal"],
    droppedAtLastTake: end["droppedFramesTotal"],
    run: run ? { ticks: Number(run[1]), repeats: Number(run[2]), skips: Number(run[3]), drops: Number(run[4]), wakeMean: run[5], wakeMax: run[6] } : null,
    panics: count(/panicked at/g),
    panicLines: clean().split("\n").filter((l) => l.includes("panicked at")).slice(0, 10),
    recordStopError,
    lastTakeTick: end,
    neverDrawnLines: count(/were never drawn/g),
    chainLost: count(/chain lost mid-take/g),
    errors: count(/ ERROR /g),
    watchdogTrips: atStop["watchdogTripsTotal"],
    underruns: atStop["audioUnderrunsTotal"],
    skippedRecordFrames: atStop["skippedRecordFrames"],
    streamMessages: receiver.videoMessages(),
    streamParseError: receiver.parseError(),
    // R42's column: each output's duration against its wall span (the command's
    // acknowledgement to the stop's). A dead encode thread leaves its output short.
    ffprobe: FFPROBE,
    record: file === undefined ? null : {
      bytes: statSync(file).size,
      wallSeconds: (recordStoppedAt - recordStartedAt) / 1000,
      video: probe(file, "v:0"),
      audio: probe(file, "a:0"),
    },
    stream: existsSync(flv) ? {
      bytes: statSync(flv).size,
      wallSeconds: (streamStoppedAt - streamStartedAt) / 1000,
      video: probe(flv, "v:0"),
    } : null,
    tapPath: atStop["recordTapPath"],
    load: { max: Math.max(...loads), mean: loads.reduce((a, b) => a + b, 0) / loads.length },
  };
  writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
  console.log(`AC5 ${JSON.stringify(result)}`);
  console.log("AC5 DONE");
} finally {
  engine.kill("SIGTERM");
  ws.close();
  await server.close();
}
