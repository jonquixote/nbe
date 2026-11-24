//! The engine-restart failover drill (the v0.5 outline's §3a, defined
//! 2026-11-23): on the real engine under the show's supervisor, a show on air
//! with a recording and an RTMP stream, the engine is killed (SIGKILL). The
//! supervisor restarts it at once, the control plane resyncs the new engine,
//! and the owed outputs are re-sent (case E, #202; the owed re-issue, #205).
//!
//! Asserted, on every machine:
//! - Each point is reached:
//!   - the new engine applies the resync (the package, the View item, the
//!     overlays);
//!   - it applies the re-issued `record.start` and `stream.start`;
//!   - its take is recording and its stream live;
//!   - media reaches the sink again.
//! - Throughout the restart:
//!   - no belief correction;
//!   - no `stream.divergence` and no `command.unacknowledged` alert raised;
//!   - the supervisor saw one exit (the SIGKILL), restarted the engine once,
//!     at once, and did not give up;
//!   - the recording made before the kill is playable (AC-6): ffprobe reads a
//!     duration from it.
//! - **N = 5 s**, on the reference machine only (SPEC §0.3): every point above
//!   within 5000 ms of the kill. On a GitHub Actions runner it is skipped
//!   loudly and the times are logged; a timing claim from a shared runner is
//!   noise (R9). N was measured by `failover.measure.ts`: 2.61–2.85 s to the
//!   outputs, reference machine, 5 samples.
//!
//! Recorded, not asserted: the output gap (kill to the sink's first new
//! byte), and the master clock across the restart (the old engine's last
//! frame against the new engine's first).
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/failover.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine).

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { loadComplete, residency } from "./load-residency.js";
import { createControlPlaneServer } from "./server.js";
import { send, startRtmpSink, startTap, type TapFrame } from "./show-stop.harness.js";
import { ControlPlaneState } from "./state.js";
import { baseEnv, Supervisor, type SupervisorEvent } from "./supervisor.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
/** The drill's N (the v0.5 outline's §3a), on the reference machine. */
const N_MS = 5000;
/** How long each point may take before the drill fails it on any machine. */
const REACH_MS = 20_000;
const ON_CI = process.env.GITHUB_ACTIONS === "true";

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function waitFor(check: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await sleep(10);
  }
  return check();
}

function ffprobe(): string | null {
  for (const c of ["ffprobe", "/usr/local/bin/ffprobe", "/opt/homebrew/bin/ffprobe"]) {
    if (spawnSync(c, ["-version"], { encoding: "utf8" }).status === 0) return c;
  }
  return null;
}

test("[drill] engine restart: killed mid-show under the supervisor, the View and both outputs are back on a new engine, with no correction and no alert", async (t) => {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN}`);
  const probe = ffprobe();
  assert.ok(probe, "ffprobe is needed to read the recording made before the kill (AC-6)");

  // The control plane, as `stack()` builds it, with a tap between it and the
  // engine, so the wire can be timed.
  const state = new ControlPlaneState();
  const auditPath = join(tempDir("nbe-failover-audit-"), "audit.jsonl");
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  const tap = await startTap(server.port);
  const sink = await startRtmpSink();

  // The engine under the show's supervisor, as `run.ts` runs it.
  const logDir = tempDir("nbe-failover-logs-");
  const events: Array<SupervisorEvent & { t: number }> = [];
  const sup = new Supervisor(
    [
      {
        name: "engine",
        command: ENGINE_BIN,
        env: {
          ...baseEnv(),
          NBE_CP_URL: `ws://127.0.0.1:${tap.port}/nbe/v0.3`,
          NBE_RENDER_TOKEN: RENDER,
          NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
          NBE_HOUSE_RATE: "30",
          RUST_LOG: "warn,nbe_engine::directive=info,nbe_engine::render=info",
          NO_COLOR: "1",
        },
        restart: true,
        ready: async () => server.wsBridge.renderNodeCount() === 1,
        readyMs: 20_000,
      },
    ],
    { logDir, caffeinate: false, onEvent: (e) => events.push({ ...e, t: performance.now() }) },
  );
  let admin: WebSocket | undefined;
  t.after(async () => {
    admin?.close();
    await sup.stop();
    sink.close();
    tap.close();
    await server.close();
  });
  await sup.start();
  // The operator socket, opened after the start: its `open` must not fire
  // before the listener is attached.
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
  });
  admin = ws;
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  const engineLog = (): string[] => {
    const p = join(logDir, "engine.log");
    return existsSync(p) ? readFileSync(p, "utf8").split("\n") : [];
  };
  const ok = async (command: string, payload: Record<string, unknown> = {}): Promise<number> => {
    const r = await send(ws, command, payload);
    assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
    return r["stateVersion"] as number;
  };

  // On air: the dress package, A1 on the View, a recording and a stream.
  const tmp = tempDir("nbe-failover-");
  const pkg = join(tmp, "dress_show");
  cpSync(DRESS, pkg, { recursive: true });
  const rec = join(tmp, "record-out");
  mkdirSync(rec, { recursive: true });
  const m = JSON.parse(readFileSync(join(pkg, "manifest.json"), "utf8")) as Record<string, Record<string, unknown>>;
  m["show"]!["outputs"] = { record: { directory: rec } };
  writeFileSync(join(pkg, "manifest.json"), JSON.stringify(m));
  const mark = engineLog().length;
  assert.ok(await server.awaitApplied(await ok("show.load", { packagePath: pkg }), 20_000), "show.load applied");
  assert.ok(await server.awaitApplied(await ok("show.start", { startClock: true }), 20_000), "show.start applied");
  await ok("view.cut", { itemRef: "A1" });
  assert.ok(await waitFor(() => loadComplete(residency(engineLog().slice(mark))), 30_000), "the load completes");
  assert.ok(await server.awaitApplied(await ok("record.start", {}), 10_000), "record.start applied");
  if (!(await server.awaitApplied(await ok("stream.start", { url: `rtmp://127.0.0.1:${sink.port}/live/drill` }), 10_000))) {
    assert.ok(
      engineLog().some((l) => l.includes("E_NO_HARDWARE_ENCODER") || l.includes("E_NO_ZEROCOPY")),
      "stream.start: no ack and no capability refusal",
    );
    console.log("SKIP: the engine refused stream.start (no hardware H.264 encoder on this machine)");
    t.skip("no hardware H.264 encoder");
    return;
  }
  await sleep(3_000);
  const takesBefore = readdirSync(rec).filter((f) => f.endsWith(".mp4"));
  assert.equal(takesBefore.length, 1, `one take on disk before the kill: ${JSON.stringify(takesBefore)}`);
  const ticks = (frames: TapFrame[]): Array<Record<string, unknown>> =>
    frames
      .filter((f) => f.dir === "up" && f.msg["kind"] === "engineTelemetry")
      .map((f) => (f.msg["data"] ?? f.msg) as Record<string, unknown>);
  const lastOld = ticks(tap.frames).at(-1);
  const lastOldAt = tap.frames.filter((f) => f.dir === "up" && f.msg["kind"] === "engineTelemetry").at(-1)?.t ?? NaN;

  // The kill.
  const tapAtKill = tap.frames.length;
  const auditAtKill = readFileSync(auditPath, "utf8").split("\n").filter((l) => l.length > 0).length;
  const eventsAtKill = events.length;
  const pid = sup.pids()["engine"];
  assert.ok(pid, "the supervised engine has a pid");
  const killWire = performance.timeOrigin + performance.now();
  const bytesAtKill = sink.bytes();
  process.kill(pid, "SIGKILL");

  const wire = (): TapFrame[] => tap.frames.slice(tapAtKill);
  const directive = (name: string) => wire().find((f) => f.dir === "down" && f.msg["kind"] === "directive" && f.msg["command"] === name);
  const applied = (name: string): number | null => {
    const d = directive(name);
    if (!d) return null;
    const a = wire().find((f) => f.dir === "up" && f.msg["kind"] === "appliedStateVersion" && (f.msg["stateVersion"] as number) >= (d.msg["stateVersion"] as number));
    return a ? a.t - killWire : null;
  };
  let sinkBack: number | null = null;
  let reconnected = false;
  const newTick = (pred: (t: Record<string, unknown>) => boolean): number | null => {
    const f = wire().find((x) => x.dir === "up" && x.msg["kind"] === "engineTelemetry" && pred((x.msg["data"] ?? x.msg) as Record<string, unknown>));
    return f ? f.t - killWire : null;
  };
  const points = () => ({
    resyncApplied: applied("show.resync"),
    recordStartApplied: applied("record.start"),
    streamStartApplied: applied("stream.start"),
    recording: newTick((d) => (d["recordTake"] as { state?: string } | undefined)?.state === "recording"),
    streamLive: newTick((d) => d["streamTransportState"] === "live"),
    sinkBytes: sinkBack,
  });
  const reached = await waitFor(() => {
    if (!reconnected && wire().some((f) => f.dir === "up" && f.msg["kind"] === "engineHello")) reconnected = true;
    // The sink's bytes count from the kill; the old engine's last bytes may
    // still drain for a moment, so a new byte is one after the reconnect.
    if (sinkBack === null && reconnected && sink.bytes() > bytesAtKill) sinkBack = performance.timeOrigin + performance.now() - killWire;
    return Object.values(points()).every((v) => v !== null);
  }, REACH_MS);
  const at = points();
  const firstNew = ticks(wire()).find((d) => d["masterClockState"] !== "STOPPED" && (d["masterClockFrame"] as number) > 0);
  const firstNewAt = wire().find((f) => f.dir === "up" && f.msg["kind"] === "engineTelemetry" && ((f.msg["data"] ?? f.msg) as Record<string, unknown>) === firstNew)?.t;
  const rows = readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .slice(auditAtKill)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
  const raised = rows
    .filter((r) => r["command"] === "alert" && (r["detail"] as Record<string, unknown>)["edge"] === "raised")
    .map((r) => String((r["detail"] as Record<string, unknown>)["alert"]));
  const corrections = rows.filter((r) => r["event"] === "belief.corrected").map((r) => r["detail"]);
  const sup2 = events.slice(eventsAtKill).map((e) => ({ kind: e.kind, ...("attempt" in e ? { attempt: e.attempt } : {}), ...("afterMs" in e ? { afterMs: e.afterMs } : {}), ...("signal" in e ? { signal: e.signal } : {}) }));
  const round = (v: number | null): number | null => (v === null ? null : Math.round(v));
  const expectedFrame = lastOld && firstNewAt !== undefined ? (lastOld["masterClockFrame"] as number) + ((firstNewAt - lastOldAt) * 30) / 1000 : null;
  console.log(
    `DRILL ${JSON.stringify({
      ci: ON_CI,
      ...Object.fromEntries(Object.entries(at).map(([k, v]) => [k, round(v)])),
      outputGapMs: round(at.sinkBytes),
      clock: {
        lastOldFrame: lastOld?.["masterClockFrame"] ?? null,
        firstNewFrame: firstNew?.["masterClockFrame"] ?? null,
        jumpFrames: expectedFrame !== null && firstNew ? Math.round((firstNew["masterClockFrame"] as number) - expectedFrame) : null,
      },
      supervisor: sup2,
      raised,
      corrections: corrections.length,
    })}`,
  );

  assert.ok(reached, `every point is reached within ${REACH_MS} ms of the kill: ${JSON.stringify(at)}`);
  assert.deepEqual(corrections, [], "no belief correction across the restart (#205: the outputs are owed, not lost)");
  assert.ok(!raised.includes("stream.divergence"), `no stream.divergence raised: ${JSON.stringify(raised)}`);
  assert.ok(!raised.some((a) => a.startsWith("command.unacknowledged")), `no unacknowledged command raised: ${JSON.stringify(raised)}`);
  assert.equal(sup2.filter((e) => e.kind === "exited").length, 1, `one exit: ${JSON.stringify(sup2)}`);
  assert.ok(sup2.some((e) => e.kind === "exited" && e.signal === "SIGKILL"), `the exit is the kill: ${JSON.stringify(sup2)}`);
  assert.deepEqual(
    sup2.filter((e) => e.kind === "restarting"),
    [{ kind: "restarting", attempt: 1, afterMs: 0 }],
    "restarted once, at once",
  );
  assert.ok(!sup2.some((e) => e.kind === "gaveUp"), "the supervisor did not give up");
  const run = spawnSync(probe!, ["-v", "error", "-show_format", "-of", "json", join(rec, takesBefore[0]!)], { encoding: "utf8" });
  assert.equal(run.status, 0, `ffprobe reads the take made before the kill: ${run.stderr}`);
  const durationS = Number((JSON.parse(run.stdout) as { format: { duration?: string } }).format.duration ?? 0);
  assert.ok(durationS > 0, `the take made before the kill is playable (AC-6): duration ${durationS} s`);

  if (ON_CI) {
    console.log("# SKIP normative: N = 5 s from the kill to the View and both outputs back — a GitHub Actions runner (GITHUB_ACTIONS=true), not the reference machine (SPEC §0.3)");
  } else {
    const late = Object.entries(at).filter(([, v]) => v === null || v > N_MS);
    assert.deepEqual(late, [], `every point within N = ${N_MS} ms of the kill on the reference machine: ${JSON.stringify(at)}`);
  }
});
