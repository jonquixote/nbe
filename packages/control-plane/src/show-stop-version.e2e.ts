//! Queue position 2, establish-first (the user's word of 2026-10-12): what
//! `show.stop`'s `stopVersion` does on the real gate. NO FIX. These tests
//! record today's behaviour so that the design question is decided on
//! evidence. The fix's work order inverts them.
//!
//! The read under test:
//! - `show.stop` sends `record.stop` and `stream.stop` at `ctx.stateVersion`
//!   whenever any output is active (`commands/show.ts`, `emitDirectivesNow`).
//! - The dispatcher then forwards `show.stop` itself at the same version.
//! - The engine's gate skips a directive whose version is not above the last
//!   it applied (`channel.rs` `classify`, `<=`).
//! - A skipped directive never advances `expected_seq`, so the next directive
//!   meets a sequence gap, and that requests a resync (§5.9.4 rule 5).
//! - `on_resync` has no output handling.
//!
//! Four legs, each on its own control plane and engine:
//! - E1: record and stream active;
//! - E2: stream only;
//! - E3: record only;
//! - E4: neither, the control case.
//!
//! The real engine binary runs on real sockets, and the stream publishes into
//! a minimal RTMP sink whose byte count is the on-air observable.
//!
//! The record and stream legs need a hardware H.264 encoder, so the engine
//! refuses them on CI's runner. They skip loudly there, on the engine log's
//! own refusal, as the dress rehearsal's capability gate does. On the
//! normative machine every leg must run.
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/show-stop-version.e2e.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { createServer, type Socket } from "node:net";
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
const DRESS = join(REPO, "tests/fixtures/dress_show");
const APPLY_MS = 20_000;
/** An output start's ack wait: a refusal (CI) is decided by the engine log after it. */
const OUTPUT_ACK_MS = 5_000;
/** How long the outputs run before the stop, and how long each phase is watched. */
const RUN_MS = 2_000;
const WATCH_MS = 2_000;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
const stripAnsi = (s: string): string => s.replace(/\x1b\[[0-9;]*m/g, "");

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
async function startRtmpSink(): Promise<{ port: number; bytes: () => number; close: () => void }> {
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

// --- One leg's stack: a control plane, the real engine, an operator socket.
interface Stack {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  engine: ChildProcess;
  log: string[];
  ws: WebSocket;
  ticks: Array<Record<string, unknown>>;
  close: () => Promise<void>;
}

async function stack(): Promise<Stack> {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  const state = new ControlPlaneState();
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-ssv-audit-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  const log: string[] = [];
  const engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
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
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
  });
  const ticks: Array<Record<string, unknown>> = [];
  ws.on("message", (raw: Buffer) => {
    const f = JSON.parse(raw.toString()) as Record<string, unknown>;
    if (f["kind"] === "telemetry") ticks.push((f["data"] ?? {}) as Record<string, unknown>);
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  return {
    server,
    state,
    engine,
    log,
    ws,
    ticks,
    close: async () => {
      engine.kill("SIGKILL");
      ws.close();
      await server.close();
    },
  };
}

function send(s: Stack, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error(`no response to ${command}`)), 120_000);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      clearTimeout(timer);
      s.ws.off("message", onMsg);
      res(msg);
    };
    s.ws.on("message", onMsg);
    s.ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}
async function ok(s: Stack, command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(s, command, payload);
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

interface Outputs {
  record: boolean;
  stream: boolean;
}

/** What one leg observed, as one value: a failure shows everything at once. */
interface Observed {
  skippedAtStop: number;
  failedAtStop: string[];
  appliedVersionAfterStop: number | null;
  stopVersion: number;
  clockRanAfterStop: boolean;
  streamOnAirAfterStop: boolean | null;
  controlPlaneStreamState: string;
  controlPlaneRecordState: string;
  /** The engine's own report on the latest tick after the stop. */
  engineStreamTransportAfterStop: string;
  /** The take's file grew while the stop was watched (null with no recording). */
  recordingGrowingAfterStop: boolean | null;
  gapThenResyncOnTheNextDirective: boolean;
  clockStoppedAfterResync: boolean;
  streamOnAirAfterResync: boolean | null;
  engineStreamTransportAfterResync: string;
  packageReloadsOnResync: number;
}

async function leg(outputs: Outputs, skip: (why: string) => void): Promise<Observed | null> {
  const s = await stack();
  const sink = outputs.stream ? await startRtmpSink() : null;
  try {
    await ok(s, "system.telemetry.subscribe", { intervalMs: 250 });
    const staged = stagePackage();
    const loadV = await ok(s, "show.load", { packagePath: staged.pkg });
    assert.ok(await s.server.awaitApplied(loadV, APPLY_MS), "the engine applies show.load");
    const startV = await ok(s, "show.start", { startClock: true });
    assert.ok(await s.server.awaitApplied(startV, APPLY_MS), "the engine applies show.start");
    await ok(s, "view.cut", { itemRef: "A1" });

    // The outputs, with the dress rehearsal's capability gate: a refusal is
    // told from a wedged pipeline by the engine's own log.
    const refusal = (): boolean => s.log.some((l) => l.includes("E_NO_HARDWARE_ENCODER") || l.includes("E_NO_ZEROCOPY"));
    if (outputs.record) {
      const v = await ok(s, "record.start", { outputId: "ssv" });
      if (!(await s.server.awaitApplied(v, OUTPUT_ACK_MS))) {
        assert.ok(refusal(), `record.start: no ack and no capability refusal (last log: ${JSON.stringify(s.log.slice(-3))})`);
        console.log("SKIP: engine refused record.start (no hardware H.264 encoder on this machine)");
        skip("no hardware H.264 encoder: the engine refused record.start");
        return null;
      }
    }
    if (outputs.stream && sink) {
      const v = await ok(s, "stream.start", { url: `rtmp://127.0.0.1:${sink.port}/live/ssv` });
      if (!(await s.server.awaitApplied(v, OUTPUT_ACK_MS))) {
        assert.ok(refusal(), `stream.start: no ack and no capability refusal (last log: ${JSON.stringify(s.log.slice(-3))})`);
        console.log("SKIP: engine refused stream.start (no hardware H.264 encoder / zero-copy chain on this machine)");
        skip("no hardware H.264 encoder: the engine refused stream.start");
        return null;
      }
    }
    await sleep(RUN_MS);
    if (sink) assert.ok(sink.bytes() > 0, "precondition: the stream is on air before the stop");

    // The stop. Nothing else is sent until the next directive below.
    const mark = s.log.length;
    const stopVersion = await ok(s, "show.stop", {});
    const frameAtStop = frame(s);
    const bytesAtStop = sink?.bytes() ?? 0;
    // The take's file after the stop: settled once the finish has run.
    await sleep(500);
    const recordedAtStop = outputs.record ? recordedBytes(staged.rec) : 0;
    await sleep(WATCH_MS);
    const recordingGrowingAfterStop = outputs.record ? recordedBytes(staged.rec) > recordedAtStop : null;
    const appliedAfterStop = (await (await fetch(`http://127.0.0.1:${s.server.port}/nbe/v0.3/status`)).json()) as {
      renderNode?: { lastAppliedStateVersion?: number | null };
    };
    const clockRanAfterStop = frame(s) > frameAtStop;
    const streamOnAirAfterStop = sink ? sink.bytes() > bytesAtStop : null;
    const skippedAtStop = linesSince(s, mark, "stale stateVersion; skipping").length;
    const failedAtStop = linesSince(s, mark, "directive failed").map((l) => /command=([a-z.]+)/.exec(l)?.[1] ?? l);
    const cpStream = s.state.streamState;
    const cpRecord = s.state.recordState;
    const engineAfterStop = s.ticks.at(-1) ?? {};
    const gateLines = (from: number) =>
      s.log.slice(from).filter((l) => /stale stateVersion|directive failed|seq gap|show\.resync applied|package was reloaded/.test(l));
    const stopLines = gateLines(mark);

    // The next directive: one that changes no output and no package.
    const nextMark = s.log.length;
    await ok(s, "view.fallback", {});
    await sleep(WATCH_MS);
    const gap = linesSince(s, nextMark, "seq gap; requesting resync").length > 0;
    const resynced = linesSince(s, nextMark, "show.resync applied").length > 0;
    const frameAfterResync = frame(s);
    const bytesAfterResync = sink?.bytes() ?? 0;
    await sleep(WATCH_MS);
    const engineAfterResync = s.ticks.at(-1) ?? {};
    const observed: Observed = {
      skippedAtStop,
      failedAtStop,
      appliedVersionAfterStop: appliedAfterStop.renderNode?.lastAppliedStateVersion ?? null,
      stopVersion,
      clockRanAfterStop,
      streamOnAirAfterStop,
      controlPlaneStreamState: cpStream,
      controlPlaneRecordState: cpRecord,
      engineStreamTransportAfterStop: String(engineAfterStop["streamTransportState"]),
      recordingGrowingAfterStop,
      gapThenResyncOnTheNextDirective: gap && resynced,
      clockStoppedAfterResync: frame(s) === frameAfterResync,
      streamOnAirAfterResync: sink ? sink.bytes() > bytesAfterResync : null,
      engineStreamTransportAfterResync: String(engineAfterResync["streamTransportState"]),
      packageReloadsOnResync: linesSince(s, nextMark, "show.resync: the package was reloaded").length,
    };
    console.log(`SSV-LEG ${JSON.stringify({ outputs, ...observed })}`);
    for (const l of [...stopLines, ...gateLines(nextMark)]) console.log(`SSV-GATE ${l.slice(0, 220)}`);
    return observed;
  } finally {
    sink?.close();
    await s.close();
  }
}

test("[queue 2] E1, record and stream active: record.stop applies; stream.stop and show.stop are skipped; the stream stays on air through the resync", async (t) => {
  const o = await leg({ record: true, stream: true }, (why) => t.skip(why));
  if (!o) return;
  assert.deepEqual(
    { ...o, appliedVersionAfterStop: o.appliedVersionAfterStop === o.stopVersion },
    {
      ...o,
      skippedAtStop: 2,
      failedAtStop: [],
      appliedVersionAfterStop: true,
      clockRanAfterStop: true,
      streamOnAirAfterStop: true,
      controlPlaneStreamState: "idle",
      controlPlaneRecordState: "idle",
      engineStreamTransportAfterStop: "live",
      recordingGrowingAfterStop: false,
      gapThenResyncOnTheNextDirective: true,
      clockStoppedAfterResync: true,
      streamOnAirAfterResync: true,
      engineStreamTransportAfterResync: "live",
      packageReloadsOnResync: 0,
    },
    "the read's prediction for E1",
  );
});

test("[queue 2] E2, stream only: record.stop fails, stream.stop applies, show.stop is skipped", async (t) => {
  const o = await leg({ record: false, stream: true }, (why) => t.skip(why));
  if (!o) return;
  assert.deepEqual(
    { ...o, appliedVersionAfterStop: o.appliedVersionAfterStop === o.stopVersion },
    {
      ...o,
      skippedAtStop: 1,
      failedAtStop: ["record.stop"],
      appliedVersionAfterStop: true,
      clockRanAfterStop: true,
      streamOnAirAfterStop: false,
      controlPlaneStreamState: "idle",
      engineStreamTransportAfterStop: "closed",
      gapThenResyncOnTheNextDirective: true,
      clockStoppedAfterResync: true,
      streamOnAirAfterResync: false,
      packageReloadsOnResync: 0,
    },
    "the read's prediction for E2",
  );
});

test("[queue 2] E3, record only: record.stop applies; stream.stop and show.stop are skipped", async (t) => {
  const o = await leg({ record: true, stream: false }, (why) => t.skip(why));
  if (!o) return;
  assert.deepEqual(
    { ...o, appliedVersionAfterStop: o.appliedVersionAfterStop === o.stopVersion },
    {
      ...o,
      skippedAtStop: 2,
      failedAtStop: [],
      appliedVersionAfterStop: true,
      clockRanAfterStop: true,
      controlPlaneRecordState: "idle",
      recordingGrowingAfterStop: false,
      gapThenResyncOnTheNextDirective: true,
      clockStoppedAfterResync: true,
      packageReloadsOnResync: 0,
    },
    "the read's prediction for E3",
  );
});

test("[queue 2] E4, neither output active (control): show.stop applies, the clock stops, nothing is skipped and no gap follows", async (t) => {
  const o = await leg({ record: false, stream: false }, (why) => t.skip(why));
  if (!o) return;
  assert.deepEqual(
    { ...o, appliedVersionAfterStop: o.appliedVersionAfterStop === o.stopVersion },
    {
      ...o,
      skippedAtStop: 0,
      failedAtStop: [],
      appliedVersionAfterStop: true,
      clockRanAfterStop: false,
      gapThenResyncOnTheNextDirective: false,
      packageReloadsOnResync: 0,
    },
    "the control case",
  );
});
