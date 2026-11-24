//! The engine-restart failover, measured: how long from an engine killed
//! mid-show (SIGKILL) to the View, the recording and the stream restored on a
//! new engine. The supervisor restarts a dead engine at once (its first
//! attempt; `supervisor.ts`), so this script spawns the new engine at once too.
//! It is the evidence for the failover drill's N (`docs/v0.5-outline.md` §3a).
//!
//! NOT A TEST, and not in `npm test`: a timing claim from a shared runner is
//! noise. Run it on a quiescent machine, on AC power, under `caffeinate`
//! (`docs/soak-protocol.md` §2, precondition 6). Each sample waits for the
//! 1-minute load to fall under 2.5, and a sample that reads 3.0 or more at
//! either end is VOID. Paste the whole output.
//!
//!   caffeinate -i -s node --import tsx src/failover.measure.ts --n 5
//!
//! Each sample: a fresh control plane and engine, the dress package on air
//! (A1 on the View), a recording and an RTMP stream into a local sink, 3 s of
//! steady play, then the engine killed and a new one spawned. Every time is
//! from the kill, in ms.
//!
//! - Off the wire (the harness's tap): `helloWire`; `resyncSent`;
//!   `resyncApplied`, the new engine's acknowledgement of the `show.resync`
//!   that carries the package, the View item and the clock, which it draws
//!   from its next frame (the engine's telemetry carries no View item, so the
//!   picture is read here); and the re-issued `record.start` and
//!   `stream.start` applied.
//! - From the new engine's telemetry ticks (1 s apart, so coarse): `firstTick`,
//!   `recording` (its take is recording), `streamLive` (its stream transport
//!   is live).
//! - `streamBytes`: the sink's first byte after the reconnect.
//! - From the control plane's audit: `hello` (`engine.restarted`),
//!   `recordReissued`, `streamReissued`.

import { spawn, type ChildProcess } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { loadavg } from "node:os";
import { performance } from "node:perf_hooks";
import { join, resolve } from "node:path";

import { loadComplete, residency } from "./load-residency.js";
import { send, stack, startRtmpSink, type Stack } from "./show-stop.harness.js";
import { tempDir } from "./test-tmp.js";

const N = Number(process.argv[process.argv.indexOf("--n") + 1] ?? NaN) || 5;
const START_BELOW = 2.5;
const CEILING = 3.0;
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function ok(s: Stack, command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(s.ws, command, payload);
  if (r.status !== "ok") throw new Error(`${command}: ${JSON.stringify(r)}`);
  return r["stateVersion"] as number;
}

async function waitFor(check: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await sleep(10);
  }
  return check();
}

const auditRows = (s: Stack): Array<Record<string, unknown>> =>
  readFileSync(s.auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);

async function onAir(s: Stack, port: number): Promise<void> {
  const tmp = tempDir("nbe-failover-");
  const pkg = join(tmp, "dress_show");
  cpSync(DRESS, pkg, { recursive: true });
  const rec = join(tmp, "record-out");
  mkdirSync(rec, { recursive: true });
  const m = JSON.parse(readFileSync(join(pkg, "manifest.json"), "utf8")) as Record<string, Record<string, unknown>>;
  m["show"]!["outputs"] = { record: { directory: rec } };
  writeFileSync(join(pkg, "manifest.json"), JSON.stringify(m));
  const mark = s.log.length;
  if (!(await s.server.awaitApplied(await ok(s, "show.load", { packagePath: pkg }), 20_000))) throw new Error("show.load not applied");
  if (!(await s.server.awaitApplied(await ok(s, "show.start", { startClock: true }), 20_000))) throw new Error("show.start not applied");
  await ok(s, "view.cut", { itemRef: "A1" });
  if (!(await waitFor(() => loadComplete(residency(s.log.slice(mark))), 30_000))) throw new Error("load never resident");
  if (!(await s.server.awaitApplied(await ok(s, "record.start", {}), 10_000))) throw new Error("record.start not applied");
  if (!(await s.server.awaitApplied(await ok(s, "stream.start", { url: `rtmp://127.0.0.1:${port}/live/failover` }), 10_000)))
    throw new Error("stream.start not applied (no hardware encoder?)");
}

function respawn(s: Stack): ChildProcess {
  return spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${s.tap.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: "render-token",
      NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
      NBE_HOUSE_RATE: "30",
      RUST_LOG: "warn",
      NO_COLOR: "1",
    },
    stdio: "ignore",
  });
}

async function sample(i: number): Promise<void> {
  while (loadavg()[0]! >= START_BELOW) await sleep(5_000);
  const loadStart = loadavg()[0]!;
  const s = await stack();
  const sink = await startRtmpSink();
  let second: ChildProcess | null = null;
  try {
    await onAir(s, sink.port);
    await sleep(3_000);
    const at: Record<string, number | null> = {
      gone: null, connected: null, firstTick: null, recording: null, streamLive: null, streamBytes: null,
      beliefLive: null,
    };
    const tapAtKill = s.tap.frames.length;
    const before = s.tap.frames.filter((f) => f.dir === "up" && f.msg["kind"] === "engineTelemetry");
    const gaps = before.slice(-20).map((f, k, a) => (k ? f.t - a[k - 1]!.t : NaN)).filter((g) => !Number.isNaN(g)).sort((a, b) => a - b);
    const tickPeriodMs = gaps.length ? Math.round(gaps[Math.floor(gaps.length / 2)]!) : null;
    /** The new engine's telemetry, read off the tap (engine -> control plane). */
    const newTicks = (): Array<Record<string, unknown>> =>
      s.tap.frames
        .slice(tapAtKill)
        .filter((f) => f.dir === "up" && f.msg["kind"] === "engineTelemetry")
        .map((f) => (f.msg["data"] ?? f.msg) as Record<string, unknown>);
    const killEpoch = Date.now();
    const t0 = performance.now();
    s.engine.kill("SIGKILL");
    second = respawn(s);
    let bytesAtConnect = Number.POSITIVE_INFINITY;
    const ms = (): number => Math.round(performance.now() - t0);
    const deadline = t0 + 30_000;
    while (performance.now() < deadline) {
      const nodes = s.server.wsBridge.renderNodeCount();
      if (at.gone === null && nodes === 0) at.gone = ms();
      if (at.gone !== null && at.connected === null && nodes === 1) {
        at.connected = ms();
        bytesAtConnect = sink.bytes();
      }
      const fresh = newTicks();
      if (at.firstTick === null && fresh.length > 0) at.firstTick = ms();
      const last = fresh.at(-1);
      if (last) {
        const take = last["recordTake"] as { state?: string } | undefined;
        if (at.recording === null && take?.state === "recording") at.recording = ms();
        if (at.streamLive === null && last["streamTransportState"] === "live") at.streamLive = ms();
      }
      if (at.streamBytes === null && sink.bytes() > bytesAtConnect) at.streamBytes = ms();
      if (at.beliefLive === null && at.connected !== null && s.state.streamState === "live" && s.state.recordState === "recording")
        at.beliefLive = ms();
      if (Object.values(at).every((v) => v !== null)) break;
      await sleep(10);
    }
    // The wire, from the tap (its clock is `performance.timeOrigin + now`).
    const killWire = performance.timeOrigin + t0;
    const wire = s.tap.frames.slice(tapAtKill);
    const sinceKill = (t: number | undefined): number | null => (t === undefined ? null : Math.round(t - killWire));
    const isDirective = (f: (typeof wire)[number], name: string): boolean =>
      f.dir === "down" && f.msg["kind"] === "directive" && f.msg["command"] === name;
    const resync = wire.find((f) => isDirective(f, "show.resync"));
    const ackOf = (v: unknown): number | undefined =>
      wire.find((f) => f.dir === "up" && f.msg["kind"] === "appliedStateVersion" && (f.msg["stateVersion"] as number) >= (v as number))?.t;
    const reissue = (name: string) => wire.find((f) => isDirective(f, name));
    const rec = reissue("record.start");
    const str = reissue("stream.start");
    const onWire = {
      helloWire: sinceKill(wire.find((f) => f.dir === "up" && f.msg["kind"] === "engineHello")?.t),
      resyncSent: sinceKill(resync?.t),
      resyncApplied: resync ? sinceKill(ackOf(resync.msg["stateVersion"])) : null,
      recordStartApplied: rec ? sinceKill(ackOf(rec.msg["stateVersion"])) : null,
      streamStartApplied: str ? sinceKill(ackOf(str.msg["stateVersion"])) : null,
    };
    const rows = auditRows(s);
    const audit = (event: string): number | null => {
      const r = rows.find((x) => x["event"] === event && (x["ts"] as number) >= killEpoch);
      return r ? (r["ts"] as number) - killEpoch : null;
    };
    const loadEnd = loadavg()[0]!;
    const verdict = loadStart >= CEILING || loadEnd >= CEILING ? "VOID" : "KEPT";
    console.log(
      `FAILOVER ${JSON.stringify({
        sample: i, verdict, load: [loadStart, loadEnd].map((x) => Math.round(x * 100) / 100), tickPeriodMs, ...onWire, ...at,
        hello: audit("engine.restarted"), recordReissued: audit("record.reissued"), streamReissued: audit("stream.reissued"),
        alerts: s.state.alerts.map((a) => a.id),
      })}`,
    );
  } finally {
    second?.kill("SIGKILL");
    sink.close();
    await s.close();
  }
}

for (let i = 1; i <= N; i++) await sample(i);
