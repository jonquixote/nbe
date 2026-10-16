//! The real-engine harness for `show.stop` (queue position 2): shared by
//! `show-stop-version.e2e.ts` (the guards) and `show-stop.measure.ts` (the
//! timing). Not a test file: it registers no tests.
//!
//! One leg is one control plane, one real engine binary and one operator
//! socket. Between the engine and the control plane sits a tap, a WebSocket
//! relay that records every frame on the render channel, in both directions,
//! with its time. The tap is how the guards read the wire itself: which
//! directives `show.stop` put on it, at which versions, and which versions
//! the engine acknowledged.
//!
//! The stream publishes into a minimal RTMP sink whose byte count is the
//! on-air observable.

import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { createServer, type Socket } from "node:net";
import { join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { WebSocket, WebSocketServer } from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
const APPLY_MS = 20_000;
/** An output start's ack wait: a refusal (CI) is decided by the engine log after it. */
const OUTPUT_ACK_MS = 5_000;
/** How long the outputs run before the stop, and how long each phase is watched. */
const RUN_MS = 2_000;
const WATCH_MS = 2_000;
/** The control plane's own window (`show.ts`, `showStopGraceMs ?? 2000`). */
export const GRACE_MS = 2_000;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
const stripAnsi = (s: string): string => s.replace(/\x1b\[[0-9;]*m/g, "");
/** Wall-clock milliseconds, fractional: comparable with the engine log's UTC stamps. */
const now = (): number => performance.timeOrigin + performance.now();

// --- A minimal RTMP sink: the dress rehearsal's responder, trimmed to a byte
// count. It completes the handshake, answers connect / createStream / publish
// with canned AMF0 replies, then counts every byte that arrives.
function amfString(s: string): Buffer {
  const b = Buffer.alloc(3 + s.length);
  b[0] = 0x02;
  b.writeUInt16BE(s.length, 1);
  b.write(s, 3, "latin1");
  return b;
}
function amfNumber(n: number): Buffer {
  const b = Buffer.alloc(9);
  b[0] = 0x00;
  b.writeDoubleBE(n, 1);
  return b;
}
const AMF_NULL = Buffer.from([0x05]);
function amfObject(props: Array<[string, Buffer]>): Buffer {
  const parts: Buffer[] = [Buffer.from([0x03])];
  for (const [k, v] of props) {
    const key = Buffer.alloc(2 + k.length);
    key.writeUInt16BE(k.length, 0);
    key.write(k, 2, "latin1");
    parts.push(key, v);
  }
  parts.push(Buffer.from([0x00, 0x00, 0x09]));
  return Buffer.concat(parts);
}
function rtmpCommand(streamId: number, payload: Buffer): Buffer {
  const h = Buffer.alloc(12);
  h[0] = 0x03;
  h.writeUIntBE(0, 1, 3);
  h.writeUIntBE(payload.length, 4, 3);
  h[7] = 0x14;
  h.writeUInt32LE(streamId, 8);
  return Buffer.concat([h, payload]);
}
export async function startRtmpSink(): Promise<{ port: number; bytes: () => number; close: () => void }> {
  let media = 0;
  const sockets = new Set<Socket>();
  const srv = createServer((sock) => {
    sockets.add(sock);
    sock.on("close", () => sockets.delete(sock));
    sock.on("error", () => {});
    let phase: "c0c1" | "c2" | "dialog" | "media" = "c0c1";
    let buf = Buffer.alloc(0);
    const answered = { connect: false, createStream: false, publish: false };
    sock.on("data", (chunk: Buffer) => {
      if (phase === "media") {
        media += chunk.length;
        return;
      }
      buf = Buffer.concat([buf, chunk]);
      if (phase === "c0c1" && buf.length >= 1537) {
        sock.write(Buffer.concat([Buffer.from([3]), Buffer.alloc(1536), buf.subarray(1, 1537)]));
        buf = buf.subarray(1537);
        phase = "c2";
      }
      if (phase === "c2" && buf.length >= 1536) {
        buf = buf.subarray(1536);
        phase = "dialog";
      }
      if (phase !== "dialog") return;
      if (!answered.connect && buf.includes(amfString("connect"))) {
        answered.connect = true;
        sock.write(rtmpCommand(0, Buffer.concat([amfString("_result"), amfNumber(1), AMF_NULL, amfObject([["code", amfString("NetConnection.Connect.Success")]])])));
      }
      if (!answered.createStream && buf.includes(amfString("createStream"))) {
        answered.createStream = true;
        sock.write(rtmpCommand(0, Buffer.concat([amfString("_result"), amfNumber(2), AMF_NULL, amfNumber(1)])));
      }
      if (!answered.publish && buf.includes(amfString("publish"))) {
        answered.publish = true;
        sock.write(rtmpCommand(1, Buffer.concat([amfString("onStatus"), amfNumber(0), AMF_NULL, amfObject([["code", amfString("NetStream.Publish.Start")]])])));
        phase = "media";
        buf = Buffer.alloc(0);
      }
    });
  });
  await new Promise<void>((r) => srv.listen(0, "127.0.0.1", () => r()));
  const port = (srv.address() as { port: number }).port;
  return {
    port,
    bytes: () => media,
    close: () => {
      for (const s of sockets) s.destroy();
      srv.close();
    },
  };
}

// --- The tap: a relay between the engine and the control plane.
export interface TapFrame {
  /** `down`: control plane → engine; `up`: engine → control plane. */
  dir: "down" | "up";
  t: number;
  msg: Record<string, unknown>;
}

interface Tap {
  port: number;
  frames: TapFrame[];
  /** Called as a frame is relayed, after it is recorded. */
  onFrame: ((f: TapFrame) => void) | null;
  close: () => void;
}

async function startTap(cpPort: number): Promise<Tap> {
  const wss = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  await new Promise<void>((r) => wss.once("listening", () => r()));
  const tap: Tap = {
    port: (wss.address() as { port: number }).port,
    frames: [],
    onFrame: null,
    close: () => {
      for (const c of wss.clients) c.terminate();
      wss.close();
    },
  };
  const record = (dir: "down" | "up", data: unknown): void => {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(String(data)) as Record<string, unknown>;
    } catch {
      return;
    }
    const f = { dir, t: now(), msg };
    tap.frames.push(f);
    tap.onFrame?.(f);
  };
  wss.on("connection", (engineWs, req) => {
    const headers: Record<string, string> = {};
    for (const h of ["authorization", "x-nbe-role"]) {
      const v = req.headers[h];
      if (typeof v === "string") headers[h] = v;
    }
    const cp = new WebSocket(`ws://127.0.0.1:${cpPort}${req.url ?? "/"}`, { headers });
    const early: Array<[unknown, boolean]> = [];
    cp.on("open", () => {
      for (const [d, b] of early) cp.send(d as Buffer, { binary: b });
      early.length = 0;
    });
    engineWs.on("message", (data, isBinary) => {
      record("up", data);
      if (cp.readyState === WebSocket.OPEN) cp.send(data, { binary: isBinary });
      else early.push([data, isBinary]);
    });
    cp.on("message", (data, isBinary) => {
      record("down", data);
      if (engineWs.readyState === WebSocket.OPEN) engineWs.send(data, { binary: isBinary });
    });
    cp.on("close", () => engineWs.close());
    engineWs.on("close", () => cp.close());
    cp.on("error", () => engineWs.terminate());
    engineWs.on("error", () => cp.terminate());
  });
  return tap;
}

// --- One leg's stack: a control plane, the tap, the real engine, an operator socket.
export interface Stack {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  engine: ChildProcess;
  tap: Tap;
  log: string[];
  ws: WebSocket;
  ticks: Array<Record<string, unknown>>;
  /** Every `stateChange` push frame on the operator socket, with its arrival time. */
  stateChanges: Array<{ t: number; stateVersion: number }>;
  warnings: string[];
  close: () => Promise<void>;
}

function operatorSocket(port: number): Promise<WebSocket> {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
  });
  return new Promise<WebSocket>((res, rej) => {
    ws.once("open", () => res(ws));
    ws.once("error", rej);
  });
}

export async function stack(): Promise<Stack> {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  const state = new ControlPlaneState();
  const warnings: string[] = [];
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-ssv-audit-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: (m) => warnings.push(m),
  });
  const tap = await startTap(server.port);
  const log: string[] = [];
  const engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${tap.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_HOUSE_RATE: "30",
      RUST_LOG: "warn,nbe_engine::directive=info",
      NO_COLOR: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  for (const s of [engine.stdout, engine.stderr]) {
    s?.on("data", (b: Buffer) => {
      for (const line of stripAnsi(b.toString()).split("\n")) if (line.trim()) log.push(line);
    });
  }
  const deadline = Date.now() + APPLY_MS;
  while (server.wsBridge.renderNodeCount() !== 1) {
    if (Date.now() > deadline) throw new Error(`engine never registered:\n${log.join("\n")}`);
    await sleep(20);
  }
  const ws = await operatorSocket(server.port);
  const ticks: Array<Record<string, unknown>> = [];
  const stateChanges: Stack["stateChanges"] = [];
  ws.on("message", (raw: Buffer) => {
    const f = JSON.parse(raw.toString()) as Record<string, unknown>;
    if (f["kind"] === "telemetry") ticks.push((f["data"] ?? {}) as Record<string, unknown>);
    if (f["kind"] === "stateChange") stateChanges.push({ t: now(), stateVersion: f["stateVersion"] as number });
  });
  return {
    server,
    state,
    engine,
    tap,
    log,
    ws,
    ticks,
    stateChanges,
    warnings,
    close: async () => {
      engine.kill("SIGKILL");
      ws.close();
      tap.close();
      await server.close();
    },
  };
}

export function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
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
async function ok(s: Stack, command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(s.ws, command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  return r["stateVersion"] as number;
}

/** A fresh copy of the dress package with a record directory. */
function stagePackage(): { pkg: string; rec: string } {
  const tmp = tempDir("nbe-ssv-pkg-");
  const pkg = join(tmp, "dress_show");
  cpSync(DRESS, pkg, { recursive: true });
  const rec = join(tmp, "record-out");
  mkdirSync(rec, { recursive: true });
  const m = JSON.parse(readFileSync(join(pkg, "manifest.json"), "utf8")) as Record<string, Record<string, unknown>>;
  m["show"]!["outputs"] = { record: { directory: rec } };
  writeFileSync(join(pkg, "manifest.json"), JSON.stringify(m));
  return { pkg, rec };
}

/** Bytes written into the record directory so far (the take's media). */
function recordedBytes(dir: string): number {
  return readdirSync(dir).reduce((n, f) => n + statSync(join(dir, f)).size, 0);
}

const linesSince = (s: Stack, from: number, needle: string): string[] => s.log.slice(from).filter((l) => l.includes(needle));
/** The engine's clock frame on the latest tick. */
const frame = (s: Stack): number => (s.ticks.at(-1)?.["masterClockFrame"] ?? 0) as number;
/** An engine log line's UTC stamp (`tracing`'s default format), as wall-clock ms. */
const logTime = (line: string): number | null => {
  const m = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)/.exec(line.trim());
  if (!m) return null;
  const frac = /\.(\d+)Z$/.exec(m[1]!)?.[1] ?? "";
  return Date.parse(m[1]!.replace(/\.\d+Z$/, "Z")) + Number(`0.${frac || "0"}`) * 1000;
};

const MARKERS = ["ssv-open", "ssv-mid"];

function ffprobe(): string | null {
  for (const c of ["ffprobe", "/usr/local/bin/ffprobe", "/opt/homebrew/bin/ffprobe"]) {
    if (spawnSync(c, ["-version"], { encoding: "utf8" }).status === 0) return c;
  }
  return null;
}

export interface Outputs {
  record: boolean;
  stream: boolean;
}

export interface LegOptions extends Outputs {
  /** A second operator connection sends `view.fallback` inside the stop's ack window. */
  concurrent?: boolean;
  /** The record directory goes read-only before the stop, so the finalize's
   * sidecar write fails: a real quiesce failure, and the engine withholds the ack. */
  failRecordFinalize?: boolean;
}

/** What one leg observed, as one value: a failure shows everything at once. */
export interface Observed {
  stopVersion: number;
  /** Every directive the control plane put on the wire from the stop on: [command, stateVersion]. */
  directivesFromStop: Array<[string, number]>;
  /** Every version the engine acknowledged on the wire from the stop on, in arrival order. */
  acksFromStop: number[];
  skippedFromStop: number;
  failedFromStop: string[];
  seqGapsFromStop: number;
  resyncsFromStop: number;
  appliedVersionAfterStop: number | null;
  clockRanAfterStop: boolean;
  streamOnAirAfterStop: boolean | null;
  controlPlaneStreamState: string;
  controlPlaneRecordState: string;
  /** The control plane's outputs at the moment `show.stop` crossed the wire: between the send and the ack. */
  controlPlaneAtSend: { record: string; stream: string };
  stopWarned: boolean;
  /** The engine's own report on the latest tick after the stop. */
  engineStreamTransportAfterStop: string;
  /** The control plane's own `streamState` on that same tick. */
  tickStreamStateAfterStop: string;
  /** The take's file grew while the stop was watched (null with no recording). */
  recordingGrowingAfterStop: boolean | null;
  /** The marker sidecar's names, in order (null: no sidecar, or no recording). */
  sidecarMarkers: string[] | null;
  /** ffprobe read a finished take: 2 streams and a duration (null: no recording, or no ffprobe). */
  finalized: { streams: number; durationS: number } | null;
  clockStoppedAfterNext: boolean;
  streamOnAirAfterNext: boolean | null;
  packageReloadsFromStop: number;
  /** After a held stop, the operator's explicit `record.stop` / `stream.stop`:
   * the control plane's outputs afterwards (null when nothing was held). */
  afterHeal: { record: string; stream: string } | null;
}

export interface Timing {
  /** The stop's send to the engine's ack reaching the control plane. */
  stopToAckMs: number;
  /** The stop's send to its response. */
  stopToResponseMs: number;
  /** From the stop crossing the tap to the engine's own log lines and its ack crossing back. */
  wireToStreamQuiescedMs: number | null;
  wireToRecordQuiescedMs: number | null;
  wireToAckMs: number | null;
}

export interface LegResult {
  observed: Observed;
  timing: Timing;
  /** The second connection's command, in the concurrent leg. */
  concurrent: { stateVersion: number } | null;
  /** The engine's gate and quiesce lines from the stop on, for the record. */
  gateLines: string[];
  stateChangeOrder: number[];
}

export async function leg(opts: LegOptions, skip: (why: string) => void): Promise<LegResult | null> {
  const s = await stack();
  const sink = opts.stream ? await startRtmpSink() : null;
  const staged = stagePackage();
  try {
    await ok(s, "system.telemetry.subscribe", { intervalMs: 250 });
    const loadV = await ok(s, "show.load", { packagePath: staged.pkg });
    assert.ok(await s.server.awaitApplied(loadV, APPLY_MS), "the engine applies show.load");
    const startV = await ok(s, "show.start", { startClock: true });
    assert.ok(await s.server.awaitApplied(startV, APPLY_MS), "the engine applies show.start");
    await ok(s, "view.cut", { itemRef: "A1" });

    // The outputs, with the dress rehearsal's capability gate: a refusal is
    // told from a wedged pipeline by the engine's own log.
    const refusal = (): boolean => s.log.some((l) => l.includes("E_NO_HARDWARE_ENCODER") || l.includes("E_NO_ZEROCOPY"));
    if (opts.record) {
      const v = await ok(s, "record.start", { outputId: "ssv" });
      if (!(await s.server.awaitApplied(v, OUTPUT_ACK_MS))) {
        assert.ok(refusal(), `record.start: no ack and no capability refusal (last log: ${JSON.stringify(s.log.slice(-3))})`);
        console.log("SKIP: engine refused record.start (no hardware H.264 encoder on this machine)");
        skip("no hardware H.264 encoder: the engine refused record.start");
        return null;
      }
    }
    if (opts.stream && sink) {
      const v = await ok(s, "stream.start", { url: `rtmp://127.0.0.1:${sink.port}/live/ssv` });
      if (!(await s.server.awaitApplied(v, OUTPUT_ACK_MS))) {
        assert.ok(refusal(), `stream.start: no ack and no capability refusal (last log: ${JSON.stringify(s.log.slice(-3))})`);
        console.log("SKIP: engine refused stream.start (no hardware H.264 encoder / zero-copy chain on this machine)");
        skip("no hardware H.264 encoder: the engine refused stream.start");
        return null;
      }
    }
    if (opts.record) {
      // Chapters for the sidecar the finalize must write.
      for (const name of MARKERS) {
        const v = await ok(s, "marker.add", { name });
        assert.ok(await s.server.awaitApplied(v, OUTPUT_ACK_MS), `the engine applies marker.add ${name}`);
        await sleep(RUN_MS / 2);
      }
    } else {
      await sleep(RUN_MS);
    }
    if (sink) assert.ok(sink.bytes() > 0, "precondition: the stream is on air before the stop");
    if (opts.failRecordFinalize) chmodSync(staged.rec, 0o555);

    // The stop. The tap sees it cross the wire; at that moment the control
    // plane has sent it and cannot yet have its acknowledgement.
    const mark = s.log.length;
    const tapMark = s.tap.frames.length;
    const ws2 = opts.concurrent ? await operatorSocket(s.server.port) : null;
    let atSend: Observed["controlPlaneAtSend"] | null = null;
    let wireT: number | null = null;
    let concurrentReply: Promise<Record<string, unknown>> | null = null;
    s.tap.onFrame = (f) => {
      if (f.dir !== "down" || f.msg["kind"] !== "directive" || f.msg["command"] !== "show.stop" || atSend) return;
      atSend = { record: s.state.recordState, stream: s.state.streamState };
      wireT = f.t;
      if (ws2) concurrentReply = send(ws2, "view.fallback", {});
    };
    const predicted = s.state.stateVersion + 1;
    let ackT: number | null = null;
    const acked = s.server.awaitApplied(predicted, GRACE_MS * 3).then((a) => {
      if (a) ackT = now();
    });
    const t0 = now();
    const stopReply = await send(s.ws, "show.stop", {});
    const tResp = now();
    assert.equal(stopReply.status, "ok", `show.stop: ${JSON.stringify(stopReply)}`);
    const stopVersion = stopReply["stateVersion"] as number;
    if (!opts.concurrent) assert.equal(stopVersion, predicted, "nothing else was dispatched before the stop");
    await acked;
    s.tap.onFrame = null;
    const concurrent = concurrentReply ? { stateVersion: (await concurrentReply)["stateVersion"] as number } : null;
    if (opts.concurrent) assert.ok(concurrent, "the second connection's command was sent inside the window");

    const frameAtStop = frame(s);
    const bytesAtStop = sink?.bytes() ?? 0;
    // The take's file after the stop: settled once the finish has run.
    await sleep(500);
    const recordedAtStop = opts.record ? recordedBytes(staged.rec) : 0;
    await sleep(WATCH_MS);
    const recordingGrowingAfterStop = opts.record ? recordedBytes(staged.rec) > recordedAtStop : null;
    const status = (await (await fetch(`http://127.0.0.1:${s.server.port}/nbe/v0.3/status`)).json()) as {
      renderNode?: { lastAppliedStateVersion?: number | null };
    };
    const clockRanAfterStop = frame(s) > frameAtStop;
    const streamOnAirAfterStop = sink ? sink.bytes() > bytesAtStop : null;
    const cpStream = s.state.streamState;
    const cpRecord = s.state.recordState;
    const tickAfterStop = s.ticks.at(-1) ?? {};

    let sidecarMarkers: string[] | null = null;
    let finalized: Observed["finalized"] = null;
    if (opts.record) {
      const files = readdirSync(staged.rec);
      const sidecar = files.find((f) => f.endsWith(".markers.json"));
      if (sidecar) {
        const body = JSON.parse(readFileSync(join(staged.rec, sidecar), "utf8")) as { markers: Array<{ name: string; frame: number }> };
        const frames = body.markers.map((m) => m.frame);
        assert.deepEqual(frames, [...frames].sort((a, b) => a - b), "the sidecar's chapters are in frame order");
        sidecarMarkers = body.markers.map((m) => m.name);
      }
      const video = files.find((f) => f.endsWith(".mp4"));
      const probe = ffprobe();
      if (video && probe) {
        const run = spawnSync(probe, ["-v", "error", "-show_streams", "-show_format", "-of", "json", join(staged.rec, video)], { encoding: "utf8" });
        if (run.status === 0) {
          const p = JSON.parse(run.stdout) as { streams: unknown[]; format: { duration?: string } };
          finalized = { streams: p.streams.length, durationS: Number(p.format.duration ?? 0) };
        }
      } else if (!probe) {
        console.log("NOTE: ffprobe absent; the finalize is read from the sidecar alone");
      }
    }

    // The next directive: one that changes no output and no package. The
    // concurrent leg's second command already was it.
    if (!opts.concurrent) await ok(s, "view.fallback", {});
    await sleep(WATCH_MS);
    const frameAfterNext = frame(s);
    const bytesAfterNext = sink?.bytes() ?? 0;
    await sleep(WATCH_MS);

    // A held stop heals by the operator's explicit stops; the engine, already
    // idle, refuses them harmlessly.
    let afterHeal: Observed["afterHeal"] = null;
    if (s.state.recordState === "recording" || s.state.streamState === "live") {
      if (s.state.recordState === "recording") await ok(s, "record.stop", {});
      if (s.state.streamState === "live") await ok(s, "stream.stop", {});
      await sleep(WATCH_MS / 4);
      afterHeal = { record: s.state.recordState, stream: s.state.streamState };
    }

    const wire = s.tap.frames.slice(tapMark);
    const directivesFromStop = wire
      .filter((f) => f.dir === "down" && f.msg["kind"] === "directive")
      .map((f) => [f.msg["command"] as string, f.msg["stateVersion"] as number] as [string, number]);
    const acksFromStop = wire.filter((f) => f.dir === "up" && f.msg["kind"] === "appliedStateVersion").map((f) => f.msg["stateVersion"] as number);
    const ackOnWire = wire.find((f) => f.dir === "up" && f.msg["kind"] === "appliedStateVersion" && f.msg["stateVersion"] === stopVersion);
    const lineAt = (needle: string): number | null => {
      const l = linesSince(s, mark, needle)[0];
      return l ? logTime(l) : null;
    };
    const since = (t: number | null): number | null => (t !== null && wireT !== null ? t - wireT : null);

    const observed: Observed = {
      stopVersion,
      directivesFromStop,
      acksFromStop,
      skippedFromStop: linesSince(s, mark, "stale stateVersion; skipping").length,
      failedFromStop: linesSince(s, mark, "directive failed").map((l) => /command=([a-z.]+)/.exec(l)?.[1] ?? l),
      seqGapsFromStop: linesSince(s, mark, "seq gap; requesting resync").length,
      resyncsFromStop: linesSince(s, mark, "show.resync applied").length,
      appliedVersionAfterStop: status.renderNode?.lastAppliedStateVersion ?? null,
      clockRanAfterStop,
      streamOnAirAfterStop,
      controlPlaneStreamState: cpStream,
      controlPlaneRecordState: cpRecord,
      controlPlaneAtSend: atSend ?? { record: "(never crossed)", stream: "(never crossed)" },
      stopWarned: s.warnings.some((w) => w.startsWith("show.stop:")),
      engineStreamTransportAfterStop: String(tickAfterStop["streamTransportState"]),
      tickStreamStateAfterStop: String(tickAfterStop["streamState"]),
      recordingGrowingAfterStop,
      sidecarMarkers,
      finalized,
      clockStoppedAfterNext: frame(s) === frameAfterNext,
      streamOnAirAfterNext: sink ? sink.bytes() > bytesAfterNext : null,
      packageReloadsFromStop: linesSince(s, mark, "show.resync: the package was reloaded").length,
      afterHeal,
    };
    const timing: Timing = {
      stopToAckMs: ackT !== null ? ackT - t0 : Number.NaN,
      stopToResponseMs: tResp - t0,
      wireToStreamQuiescedMs: since(lineAt("show.stop: stream quiesced")),
      wireToRecordQuiescedMs: since(lineAt("show.stop: recording quiesced")),
      wireToAckMs: since(ackOnWire?.t ?? null),
    };
    const gateLines = s.log
      .slice(mark)
      .filter((l) => /stale stateVersion|directive failed|seq gap|show\.resync applied|package was reloaded|show\.stop:/.test(l))
      .map((l) => l.slice(0, 220));
    ws2?.close();
    return { observed, timing, concurrent, gateLines, stateChangeOrder: s.stateChanges.map((c) => c.stateVersion) };
  } finally {
    if (opts.failRecordFinalize) chmodSync(staged.rec, 0o755);
    sink?.close();
    await s.close();
  }
}
