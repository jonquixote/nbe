//! [RI-1] The dress rehearsal — the midpoint integration review's centerpiece.
//!
//! Every other test in this repo is scoped to one subsystem. This is the first
//! that fails when the parts do not compose: a real control plane, the real
//! engine binary as a separate process, the real WebSocket protocol between
//! them, and a real show package played from `show.load` to `show.stop`.
//!
//! Scope is the happy path, deliberately. Control-plane disconnects, engine
//! kill/OOM and network chaos belong to the AC-5 soak and `tests/reconnect.rs`;
//! this gate proves composition, not chaos.
//!
//! TIMEOUTS ARE CHOSEN PER STEP, NOT INHERITED. A real-time gate is slow to
//! fail by nature, and an inherited default costs the whole budget on every
//! red run — the engine smoke test cost 30 s per failure until its timeout was
//! set on purpose. Each constant below says what it is waiting for and why
//! that number.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, existsSync, readFileSync, mkdirSync, writeFileSync, readdirSync, statSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";

// --- Thresholds, measured from the code (charter [RI-1]) -------------------

/** SPEC §10.1 / `channel.rs`'s `telemetry_interval_ms: 1000`. */
const TICK_MS = 1000;
/** SPEC §5.9.5 / `show.ts`'s `showStopGraceMs ?? 2000`. */
const GRACE_MS = 2000;
/** "Rises above floor" = within 2 ticks of the triggering appliedStateVersion. */
const RISE_MS = 2 * TICK_MS;

// --- Timeouts, each justified ----------------------------------------------

/**
 * Engine process start → render-role session registered.
 *
 * Covers process spawn, wgpu adapter/device init and the WS handshake. wgpu
 * init dominates and is the one step here whose cost is hardware-dependent,
 * so this is the most generous number in the file.
 */
const ENGINE_READY_MS = 30_000;
/** A single command round trip on loopback. Generous for a local socket. */
const COMMAND_MS = 5_000;
/**
 * `show.load` only.
 *
 * It shells `nbe-preflight`, which DECODES the package's media. MEASURED, not
 * guessed: 24-29 s for this 5-second 1080p package running preflight alone,
 * and **46 s** for the same load driven over the wire with the engine process
 * also running. This is not a slow socket, it is real work.
 *
 * It is also the clearest argument for per-step timeouts: at COMMAND_MS
 * (5 s) this step failed with "timeout waiting for show.load" and told us
 * nothing, and at 90 s it still failed — under contention the real number
 * sits between them. 180 s is ~4x the measured worst case.
 */
const LOAD_MS = 180_000;
/** Waiting for a telemetry predicate: 3 ticks, so a missed tick is not a flake. */
const TELEMETRY_MS = 3 * TICK_MS;
/** `show.stop` must acknowledge inside its own grace window, plus slack. */
const STOP_MS = GRACE_MS + 2_000;

const ADMIN = "admin-token";
const RENDER = "render-token";
const OPERATOR = "operator-token";

const PKG = resolve(import.meta.dirname, "../../../tests/fixtures/dress_show");
const ENGINE_BIN = resolve(import.meta.dirname, "../../../target/debug/nbe-engine");

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess;
let engineLog: string[] = [];
let ws: WebSocket;

/** Telemetry frames seen on the operator connection, in order. */
const ticks: Record<string, unknown>[] = [];
/** Every server-pushed frame, for the gapless-version and event assertions. */
const pushes: Record<string, unknown>[] = [];
/** showState values seen on stateChange frames, in order. */
const showStates: string[] = [];
/** Per-command timings — a charter artifact, and the only way to tell an
 *  engine latency from a measurement that started inside a 46-second load. */
const timings: { command: string; atMs: number; tookMs: number }[] = [];
/** When the harness started, so every timing is relative to one origin. */
const T0 = Date.now();
/** First tick at which the clock was observed advancing, relative to T0. */
let clockMovedAtMs: number | null = null;
/** First tick at which the clip bus was observed above the floor. */
let clipAudibleAtMs: number | null = null;

function send(
  command: string,
  payload: unknown = {},
  timeoutMs: number = COMMAND_MS,
): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((resolvePromise, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timeout after ${timeoutMs} ms waiting for ${command}`)),
      timeoutMs,
    );
    const onMessage = (raw: Buffer): void => {
      const frame = JSON.parse(raw.toString()) as Record<string, unknown>;
      // SPEC §5.4: the response correlates by `requestId`, not `id`. Matching
      // on `id` silently never resolves — every step timed out and blamed the
      // engine for a bug in this file.
      if (frame["requestId"] !== id) return;
      clearTimeout(timer);
      ws.off("message", onMessage);
      resolvePromise(frame);
    };
    ws.on("message", onMessage);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function ok(
  command: string,
  payload: unknown = {},
  timeoutMs: number = COMMAND_MS,
): Promise<Record<string, unknown>> {
  const startedAt = Date.now();
  const reply = await send(command, payload, timeoutMs);
  timings.push({
    command,
    atMs: startedAt - T0,
    tookMs: Date.now() - startedAt,
  });
  assert.equal(
    reply["status"],
    "ok",
    `${command} must succeed; got ${JSON.stringify(reply["error"] ?? reply)}`,
  );
  return reply;
}

/** Wait until a telemetry tick satisfies `pred`, or fail with what was seen. */
async function untilTelemetry(
  what: string,
  pred: (t: Record<string, unknown>) => boolean,
  ms: number = TELEMETRY_MS,
): Promise<Record<string, unknown>> {
  const deadline = Date.now() + ms;
  const seen = ticks.length;
  while (Date.now() < deadline) {
    const hit = ticks.slice(seen).find(pred);
    if (hit) return hit;
    await new Promise((r) => setTimeout(r, 50));
  }
  const last = ticks.at(-1);
  throw new Error(
    `waited ${ms} ms for ${what}; last telemetry was ${JSON.stringify(last ?? null)}`,
  );
}

before(async () => {
  assert.ok(
    existsSync(ENGINE_BIN),
    `the engine binary must be built before the dress rehearsal: ${ENGINE_BIN}\n` +
      `run: cargo build -p nbe-engine`,
  );
  assert.ok(existsSync(join(PKG, "manifest.json")), `dress package missing at ${PKG}`);

  const tmp = mkdtempSync(join(tmpdir(), "nbe-dress-"));
  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render", [OPERATOR]: "operator" } },
    audit: new AuditLog(join(tmp, "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    // The real §5.9.5 window: this gate measures the production number.
    showStopGraceMs: GRACE_MS,
    warn: () => {},
  });

  // The engine is a separate process reached over the real protocol — no
  // in-process bridge, no mock. That is the whole point of this test.
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_HOUSE_RATE: "30",
      RUST_LOG: "info",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const record = (b: Buffer): void => {
    engineLog.push(b.toString());
  };
  engine.stdout?.on("data", record);
  engine.stderr?.on("data", record);

  // Operator connection: what a human's dashboard would hold.
  // §16.0: `operator` may not run show.load or show.start — only show.stop, in
  // an emergency. A scripted show drives the whole lifecycle, so the rehearsal
  // holds an admin connection.
  ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { Authorization: `Bearer ${ADMIN}`, "X-NBE-Role": "admin" },
  });
  ws.on("message", (raw: Buffer) => {
    const frame = JSON.parse(raw.toString()) as Record<string, unknown>;
    if (frame["kind"] === "telemetry") {
      ticks.push(frame);
      const d = frame["data"] as Record<string, unknown> | undefined;
      if (clockMovedAtMs === null && ((d?.["masterClockFrame"] as number) ?? 0) > 0) {
        clockMovedAtMs = Date.now() - T0;
      }
      const peaks = d?.["busPeakDbfs"] as Record<string, number> | undefined;
      if (clipAudibleAtMs === null && (peaks?.["clip"] ?? -120) > -60) {
        clipAudibleAtMs = Date.now() - T0;
      }
    }
    // §10.1's telemetry tick does NOT carry showState — it rides on the
    // §5.4.1 stateChange frame instead. Observed on the wire, and recorded as
    // an [RI-3] finding: a dashboard holding only telemetry cannot say whether
    // the show is running.
    if (frame["kind"] === "stateChange") {
      const st = (frame["state"] as Record<string, unknown> | undefined)?.["showState"];
      if (typeof st === "string") showStates.push(st);
    }
    if (frame["kind"]) pushes.push(frame);
  });
  await new Promise<void>((r, reject) => {
    const timer = setTimeout(() => reject(new Error("operator connect timed out")), COMMAND_MS);
    ws.once("open", () => {
      clearTimeout(timer);
      r();
    });
    ws.once("error", reject);
  });

  // Telemetry is opt-in. Nothing pushes ticks to a connection that has not
  // asked for them, so without this subscribe every wire-level assertion in
  // this file starves with "last telemetry was null".
  await new Promise<void>((r) => {
    ws.send(
      JSON.stringify({
        v: "0.3",
        id: randomUUID(),
        command: "system.telemetry.subscribe",
        payload: { intervalMs: TICK_MS },
      }),
    );
    setTimeout(r, 200);
  });

  // Wait for the engine to register as a render session. Nothing in the show
  // is meaningful until a render node is attached.
  const deadline = Date.now() + ENGINE_READY_MS;
  while (Date.now() < deadline) {
    if (server.wsBridge.renderNodeCount() > 0) return;
    if (engine.exitCode !== null) {
      throw new Error(
        `the engine exited before connecting (code ${engine.exitCode}):\n${engineLog.join("")}`,
      );
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(
    `no render node registered within ${ENGINE_READY_MS} ms:\n${engineLog.join("")}`,
  );
});

after(async () => {
  // Failure artifacts, per the charter: a real-time gate that fails without
  // evidence costs a re-run to learn anything. Written unconditionally and
  // cheap; CI uploads the directory.
  try {
    const dir = resolve(import.meta.dirname, "../../../target/dress-rehearsal");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "engine.log"), engineLog.join(""));
    writeFileSync(join(dir, "telemetry.jsonl"), ticks.map((t) => JSON.stringify(t)).join("\n"));
    writeFileSync(join(dir, "pushes.jsonl"), pushes.map((f) => JSON.stringify(f)).join("\n"));
    writeFileSync(join(dir, "show-states.json"), JSON.stringify(showStates, null, 2));
    writeFileSync(
      join(dir, "timings.json"),
      JSON.stringify({ timings, clockMovedAtMs, clipAudibleAtMs, recordSpan }, null, 2),
    );
  } catch {
    // Artifacts are diagnostics, never a reason to fail the gate.
  }
  engine?.kill("SIGKILL");
  ws?.close();
  if (server) await server.close();
});

test("[RI-1] step 1: preflight passes on the dress package with a populated report", () => {
  // RUNS preflight. The first version only read `preflight_report.json` off
  // disk — a gitignored file written as a side effect of step 2's `show.load`,
  // which runs AFTER this step. On a clean checkout it failed ENOENT; on any
  // later run it passed against whatever the previous run left behind. It
  // proved a JSON file existed and said `airReady`, which a hand-written file
  // satisfies equally well.
  const bin = resolve(import.meta.dirname, "../../../target/debug/nbe-preflight");
  assert.ok(existsSync(bin), `nbe-preflight must be built: ${bin}`);
  const run = spawnSync(bin, ["--package-path", PKG], { encoding: "utf8" });
  assert.equal(
    run.status,
    0,
    `preflight must exit 0 on the dress package; got ${run.status}: ${run.stderr}`,
  );

  const report = JSON.parse(
    readFileSync(join(PKG, "preflight_report.json"), "utf8"),
  ) as Record<string, unknown>;
  assert.equal(report["manifestValid"], true);
  assert.equal(report["airReady"], true);
  assert.deepEqual(report["errors"], []);
  // Populated, not merely present: a report of empty arrays proves nothing.
  const assets = report["assets"] as unknown[];
  assert.ok(assets.length >= 4, `report must cover every asset, saw ${assets.length}`);
});

test("[RI-1] step 2: show.load, and a render node acknowledges it", async () => {
  const reply = await ok("show.load", { packagePath: PKG }, LOAD_MS);
  const version = (reply["stateVersion"] ?? 0) as number;
  assert.ok(version > 0, "an accepted command must bump stateVersion");

  // §5.9.5: the engine confirms with appliedStateVersion. Without a render
  // node applying directives, everything after this step is theatre.
  await untilTelemetry(
    "the engine to report a fresh connection",
    (t) => ((t["data"] as Record<string, unknown>)?.["engineConnected"] ?? false) === true,
    ENGINE_READY_MS,
  );

  // And wait for the engine to have actually APPLIED the load, which is what the
  // comment above has always claimed this step does.
  //
  // It did not, and that was finding R5 in its entirety. The engine applies
  // directives in arrival order (§5.9) and `show.load` decodes every asset —
  // 10.7 s for this package on the reference machine. `show.start` therefore sat
  // queued behind it while the control plane had already recorded RUNNING, so
  // `masterClockFrame` read 0 for ~5.8 s and step 3's window expired against
  // decode time rather than against clock-start latency. Measured 2026-09-11:
  // showState RUNNING at t=3005 ms, first non-zero frame at t=10014 ms, then
  // exactly 30 frames a second thereafter — the clock was never the defect.
  //
  // Pressing START while the package is still loading is not something an
  // operator does, and a rehearsal that does it is not playing the show.
  const applied = await server.awaitApplied(version, LOAD_MS);
  assert.ok(
    applied,
    `the engine must confirm it applied show.load (stateVersion ${version}) ` +
      `within ${LOAD_MS} ms; §5.9.5 is the contract`,
  );
});

test("[RI-1] the engine binary is actually running its audio driver", async () => {
  // This is the wiring gate that three Rust tests failed to be. It observes an
  // EFFECT, not text: `EngineState::bus_peaks` is empty at construction and is
  // only ever written by `AudioDriver::publish`, reachable solely from
  // `cycle()`. So bus keys arriving in telemetry from a separately-spawned
  // binary, over the real protocol, cannot be forged by printing a line.
  const tick = await untilTelemetry("a telemetry tick carrying bus peaks", (t) => {
    const peaks = (t["data"] as Record<string, unknown> | undefined)?.["busPeakDbfs"] as
      | Record<string, number>
      | undefined;
    return peaks !== undefined && Object.keys(peaks).length > 0;
  });
  const peaks = (tick["data"] as Record<string, unknown>)["busPeakDbfs"] as Record<
    string,
    number
  >;
  assert.ok(
    Object.keys(peaks).length >= 8,
    `the driver must publish every §8.1 bus, saw ${JSON.stringify(Object.keys(peaks))}`,
  );
});

test("[RI-1] step 3: show.start runs the clock", async () => {
  await ok("show.start", { startClock: true });
  assert.ok(
    showStates.includes("RUNNING"),
    `show.start must announce RUNNING on a stateChange frame; saw ${JSON.stringify(showStates)}`,
  );
  const running = await untilTelemetry("a tick after show.start", () => true);
  const first = ((running["data"] as Record<string, unknown>)["masterClockFrame"] ?? 0) as number;

  // Advancing, not merely present. A clock stuck at its start value satisfies
  // every "field exists" assertion ever written.
  const advanced = await untilTelemetry(
    "masterClockFrame to advance",
    (t) => (((t["data"] as Record<string, unknown>)["masterClockFrame"] ?? 0) as number) > first,
  );
  const second = ((advanced["data"] as Record<string, unknown>)["masterClockFrame"] ??
    0) as number;
  assert.ok(second > first, `clock must advance: ${first} -> ${second}`);
});

test("[RI-1] step 4: a take with audio follow raises the clip bus on the wire", async () => {
  await ok("preview.set", { itemRef: "A1" });
  await ok("view.take", { transition: "cut", audio: { transition: "follow" } });

  // The wire-visible proof that audio follows video: the control plane never
  // computes this number — it comes from the engine's own graph, through
  // telemetry, because a real clip with a real AAC track is being decoded.
  const tick = await untilTelemetry(
    "busPeakDbfs.clip to rise",
    (t) => {
      const peaks = (t["data"] as Record<string, unknown> | undefined)?.["busPeakDbfs"] as
        | Record<string, number>
        | undefined;
      return (peaks?.["clip"] ?? -120) > -60;
    },
    RISE_MS + TICK_MS,
  );
  const peaks = (tick["data"] as Record<string, unknown>)["busPeakDbfs"] as Record<
    string,
    number
  >;
  const clip = peaks["clip"] ?? -120;
  assert.ok(clip > -60, `clip bus should carry the take: ${clip} dBFS`);
});

test("[RI-1] step 5: a 15-frame mix drops no frames", async () => {
  const before = droppedNow();
  await ok("preview.set", { itemRef: "A2" });
  await ok("view.take", { transition: "mix", durationFrames: 15 });
  // Two ticks so the whole transition is inside the measured window.
  await new Promise((r) => setTimeout(r, RISE_MS));
  const after = droppedNow();
  assert.equal(after, before, `a mix must not drop frames: ${before} -> ${after}`);
});

test("[RI-1] step 6: a soundboard stab raises the sfx bus and drops nothing", async () => {
  const before = droppedNow();
  await ok("soundboard.play", { assetId: "stab_sfx" });
  const tick = await untilTelemetry(
    "busPeakDbfs.sfx to rise",
    (t) => {
      const peaks = (t["data"] as Record<string, unknown> | undefined)?.["busPeakDbfs"] as
        | Record<string, number>
        | undefined;
      return (peaks?.["sfx"] ?? -120) > -60;
    },
    RISE_MS + TICK_MS,
  );
  const peaks = (tick["data"] as Record<string, unknown>)["busPeakDbfs"] as Record<
    string,
    number
  >;
  const sfx = peaks["sfx"] ?? -120;
  assert.ok(sfx > -60, `sfx bus should carry the stab: ${sfx} dBFS`);
  assert.equal(droppedNow(), before, "a soundboard trigger must not cost a frame");
});

test("[RI-1] step 7: an audio.bus.set is accepted and the graph survives it", async () => {
  // The predicate here used to be `() => true`, which asserts nothing at all:
  // the step passed with `audio.bus.set` a complete no-op, while its name
  // claimed the change was "reflected on the next tick".
  //
  // What is actually observable on the wire is that the command is accepted,
  // that the engine keeps publishing bus meters afterwards (so the graph
  // survived the change rather than wedging), and that the show keeps running.
  // The gain value itself is not in telemetry — §10.1 carries peaks, not
  // per-bus gain — so asserting a level here would be asserting a number the
  // protocol does not send. The graph-level behaviour is gated by the engine
  // suite (`audio_directives_parse_into_intents_and_apply_to_the_graph`).
  // `untilTelemetry` already only considers ticks that arrive AFTER the
  // awaited command, so the index guard an earlier version used here was
  // unreachable. Removed rather than left as decoration.
  await ok("audio.bus.set", { bus: "music", gainDb: -20 });
  const after = await untilTelemetry(
    "a tick carrying live bus meters after the bus change",
    (t) => {
      const peaks = (t["data"] as Record<string, unknown> | undefined)?.["busPeakDbfs"] as
        | Record<string, number>
        | undefined;
      return peaks !== undefined && Object.keys(peaks).length >= 8;
    },
  );
  assert.ok(
    showStates.includes("RUNNING"),
    "the show must still be running after an audio.bus.set",
  );
  assert.ok(
    (after["data"] as Record<string, unknown>)["engineConnected"] === true,
    "the engine must still be connected after an audio.bus.set",
  );
});

test("[RI-1] a non-house-rate clip takes cleanly and costs no frames", async () => {
  // NOT the AC-4 cadence gate. The first version of this step asserted that no
  // `itemEvent: end` arrives for a 12 fps take, and it could not fail:
  // `ItemEvent::End` is emitted only from `schedule_done`, which is spawned
  // only when the take payload carries `durationFrames` — and this take
  // carries none. Nothing on the wire moves when a clip is exhausted:
  // `viewItem` does not clear and no event fires. So the assertion was green
  // whether or not cadence conversion existed, which is exactly the defeatable
  // gate this review spent three rounds removing elsewhere (report §3.10).
  //
  // The cadence mapping is gated where the observable actually lives: in
  // pixels, by `a_12_fps_source_spans_30_house_frames_in_the_rendered_picture`
  // (crates/nbe-engine/tests/prompt05.rs), which reads back the View and fails
  // when the mapping reverts to 1:1. Two nbe-decode unit tests gate the
  // arithmetic itself.
  //
  // What this step honestly proves is narrower and still worth having: a
  // non-house-rate asset is in the package the rehearsal plays, it takes
  // without error, and it costs no dropped frames. Before this, no end-to-end
  // path touched a non-house-rate clip at all.
  const before = droppedNow();
  await ok("preview.set", { itemRef: "A3" });
  await ok("view.take", { transition: "cut" });
  const tick = await untilTelemetry(
    "A3 on air",
    (t) => (t["data"] as Record<string, unknown>)?.["viewItem"] === "A3",
  );
  assert.equal((tick["data"] as Record<string, unknown>)["viewItem"], "A3");
  assert.equal(droppedNow(), before, "taking a 12 fps source must not drop frames");
});

test("[RI-1] step 8: preview.set is visible in telemetry", async () => {
  await ok("preview.set", { itemRef: "A1" });
  const tick = await untilTelemetry(
    "previewItem to appear",
    (t) => (t["data"] as Record<string, unknown>)["previewItem"] === "A1",
  );
  assert.equal((tick["data"] as Record<string, unknown>)["previewItem"], "A1");
});

test("[RI-1] gate: no drops, no underruns, no fallback, and the profile is real", async () => {
  const fields = (ticks.at(-1)?.["data"] ?? {}) as Record<string, unknown>;
  assert.equal(fields["droppedFramesTotal"], 0, "zero-drop across the whole show");
  assert.equal(fields["audioUnderrunsTotal"], 0, "no audio underruns across the show");

  // fallbackActive false THROUGHOUT, not merely at the end.
  const everFell = ticks.some(
    (t) => ((t["data"] as Record<string, unknown>)?.["fallbackActive"] ?? false) === true,
  );
  assert.equal(everFell, false, "the fallback slate must never have gone to air");

  assert.ok(
    (fields["decodeSessions"] as number) >= 1,
    `a show playing real clips must hold a decode session, saw ${fields["decodeSessions"]}`,
  );
  // A stub would report the manifest's declared value; the engine reports what
  // the hardware probe actually allowed.
  assert.ok(
    typeof fields["qualityProfile"] === "string" && fields["qualityProfile"] !== "",
    `qualityProfile must be a real capped value, saw ${JSON.stringify(fields["qualityProfile"])}`,
  );
});

test("[RI-1] step 10: show.stop acknowledges inside the grace window and the clock stops", async () => {
  const started = Date.now();
  await ok("show.stop", {});
  const elapsed = Date.now() - started;
  assert.ok(
    elapsed <= STOP_MS,
    `show.stop must acknowledge within ${STOP_MS} ms, took ${elapsed} ms`,
  );

  assert.ok(
    showStates.includes("STOPPED"),
    `show.stop must announce STOPPED; saw ${JSON.stringify(showStates)}`,
  );
  const stopped = await untilTelemetry("a tick after show.stop", () => true);
  const frozen = ((stopped["data"] as Record<string, unknown>)["masterClockFrame"] ??
    0) as number;
  await new Promise((r) => setTimeout(r, RISE_MS));
  const later = ((ticks.at(-1)?.["data"] as Record<string, unknown>)?.["masterClockFrame"] ??
    0) as number;
  assert.equal(later, frozen, `a stopped clock must not advance: ${frozen} -> ${later}`);
});

function droppedNow(): number {
  return ((ticks.at(-1)?.["data"] as Record<string, unknown>)?.["droppedFramesTotal"] ??
    0) as number;
}

// --- Record extension (Prompt 09): structure, never performance --------------
//
// The dress package declares no `show.outputs.record.directory` (proven RED:
// the engine refused `record.start` with "no record directory"), so these
// steps stage a record-enabled COPY of the package in a temp dir at runtime:
// same assets, plus `show.outputs.record.directory` pointing at a temp record
// out dir. No schema edit, no fixture edit, no engine seam — the schema
// already allows `outputs.record.directory`, preflight does not gate on it,
// and the take path is discovered by scanning the record dir for `*.mp4`
// (the control-plane ack carries no path).
//
// CI runner is 3 arm64 cores with zero-drop thresholds already failing by
// design there. Every assertion below is STRUCTURE (bytes on disk,
// parseability, alignment) — never a frame budget or callback cadence.

/** ffprobe fallback locations when PATH lookup misses (Intel vs arm64 Homebrew). */
const FFPROBE_FALLBACKS = ["/usr/local/bin/ffprobe", "/opt/homebrew/bin/ffprobe"];
/** Nominal house rate the engine is spawned with (`NBE_HOUSE_RATE: "30"`). */
const HOUSE_RATE = 30;
/** Seconds of show recorded in the clean-stop take. */
const RECORD_SECS = 4;
/** `record.stop` finalize is a real mux finish: generous, not inherited. */
const RECORD_STOP_MS = 30_000;

/** The clean-stop take's artifact, shared from step 11 to step 12. */
let recordFile: string | null = null;
/** Wall clock at `record.start` applied (content begins) and at `marker.add`. */
let recordWallStartMs = 0;
let markerWallOffsetSecs = 0;
/** Record-path pressure across the step-11 span (fork condition #2). */
let recordSpan: Record<string, unknown> | null = null;

/**
 * Resolve ffprobe via PATH lookup first, then the two Homebrew prefixes.
 * Returns null when absent; callers SKIP the 3 record steps with a logged
 * line (documented skip, never false-green — the steps return early without
 * asserting anything).
 */
function resolveFfprobe(): string | null {
  const pathEnv = process.env["PATH"] ?? "";
  for (const dir of pathEnv.split(":")) {
    if (!dir) continue;
    const cand = join(dir, "ffprobe");
    try {
      if (existsSync(cand)) return cand;
    } catch {
      // Ignore bad PATH entries and keep scanning.
    }
  }
  for (const fb of FFPROBE_FALLBACKS) {
    try {
      if (existsSync(fb)) return fb;
    } catch {
      // Ignore and keep scanning.
    }
  }
  // Last resort: a bare `ffprobe` resolvable by exec even if the file probe
  // above missed (unusual PATH layouts). Only accept when it runs.
  try {
    const probe = spawnSync("ffprobe", ["-version"], { encoding: "utf8" });
    if (probe.status === 0) return "ffprobe";
  } catch {
    // Absent — fall through to null.
  }
  return null;
}

/** ffprobe path, or null with a skip message when absent (the ONLY skip). */
function ffprobeOrSkip(): string | null {
  const found = resolveFfprobe();
  if (found !== null) return found;
  console.log(
    `SKIP: ffprobe absent (PATH + ${FFPROBE_FALLBACKS.join(", ")} checked) — record steps need ffprobe 9.0.1`,
  );
  return null;
}

function ffprobeJson(ffprobe: string, file: string, extra: string[]): Record<string, unknown> {
  const run = spawnSync(ffprobe, ["-v", "error", ...extra, "-of", "json", file], {
    encoding: "utf8",
  });
  assert.equal(
    run.status,
    0,
    `ffprobe must parse the recording; stderr: ${run.stderr}`,
  );
  return JSON.parse(run.stdout) as Record<string, unknown>;
}

/**
 * Stage a record-enabled copy of the dress package: same assets, plus
 * `show.outputs.record.directory` (absolute) into a fresh record out dir.
 * Returns the staged package dir and the record out dir.
 */
function stageRecordPackage(): { pkgDir: string; recordDir: string } {
  const tmp = mkdtempSync(join(tmpdir(), "nbe-dress-record-"));
  const pkgDir = join(tmp, "dress_show");
  mkdirSync(pkgDir, { recursive: true });
  const recordDir = join(tmp, "record-out");
  mkdirSync(recordDir, { recursive: true });
  // Copy the whole fixture (136 K: manifest + media + preflight sidecar).
  cpSync(PKG, pkgDir, { recursive: true });
  const manifestPath = join(pkgDir, "manifest.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as Record<
    string,
    unknown
  >;
  const show = manifest["show"] as Record<string, unknown>;
  show["outputs"] = { record: { directory: recordDir } };
  writeFileSync(manifestPath, JSON.stringify(manifest));
  return { pkgDir, recordDir };
}

/** The single `*.mp4` in `dir`, or fail listing what was there. */
function soleRecording(dir: string): string {
  const mp4s = readdirSync(dir).filter((f) => f.endsWith(".mp4"));
  assert.equal(mp4s.length, 1, `expected one recording in ${dir}, saw ${JSON.stringify(mp4s)}`);
  const name = mp4s[0];
  assert.ok(name !== undefined);
  return join(dir, name);
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

test("[RI-1] step 11: record the running show, mark it, stop cleanly", async () => {
  const ffprobe = ffprobeOrSkip();
  if (ffprobe === null) return;
  const { pkgDir, recordDir } = stageRecordPackage();

  const loadReply = await ok("show.load", { packagePath: pkgDir, mode: "reload" }, LOAD_MS);
  const loadApplied = await server.awaitApplied(
    (loadReply["stateVersion"] ?? 0) as number,
    LOAD_MS,
  );
  assert.ok(loadApplied, "the engine must apply the record-enabled package load (§5.9.5)");
  const startReply = await ok("show.start", { startClock: true });
  assert.ok(
    await server.awaitApplied((startReply["stateVersion"] ?? 0) as number, LOAD_MS),
    "the engine must apply show.start before recording",
  );

  // Real A/V content under the take: A1 carries the fixture's AAC track.
  await ok("preview.set", { itemRef: "A1" });
  await ok("view.take", { transition: "cut", audio: { transition: "follow" } });
  await untilTelemetry(
    "A1 on air before recording",
    (t) => (t["data"] as Record<string, unknown>)?.["viewItem"] === "A1",
  );

  const recReply = await ok("record.start", { outputId: "rehearsal" });
  // Capability gate: a machine without a hardware H.264 encoder (headless CI
  // runners have no GPU) refuses record.start engine-side with
  // E_NO_HARDWARE_ENCODER — but the control plane already acked ok (dispatch
  // does not wait for engine application), so no ack ever arrives and the
  // wait below times out identically for refusal and for a wedged pipeline.
  // Distinguish via the engine's own log (captured in engineLog): a refusal
  // logs the E_NO_HARDWARE_ENCODER token; an absent token with no ack means
  // the pipeline is wedged → fail, never skip.
  const recApplied = await server.awaitApplied(
    (recReply["stateVersion"] ?? 0) as number,
    COMMAND_MS,
  );
  if (!recApplied) {
    const refused = engineLog.some((line) => line.includes("E_NO_HARDWARE_ENCODER"));
    assert.ok(
      refused,
      `record.start produced no engine ack and no encoder refusal in the engine log — pipeline wedged, not a missing encoder (last log lines: ${JSON.stringify(engineLog.slice(-3))})`,
    );
    console.log("SKIP record steps: engine refused record.start (no hardware H.264 encoder on this machine)");
    return;
  }
  recordWallStartMs = Date.now();
  // Record-path pressure snapshot (fork condition #2): the wire-visible
  // counters at span start. record_tap_ms / skipped_record_frames live in
  // EngineState only — NOT on the §10.1 tick — so operators cannot see
  // record-path pressure today (recorded finding, v0.5 candidate); the tick
  // carries drops + underruns, which is what lands in timings.json.
  const spanStart = {
    droppedFramesTotal: droppedNow(),
    audioUnderrunsTotal:
      ((ticks.at(-1)?.["data"] as Record<string, unknown>)?.["audioUnderrunsTotal"] ?? 0) as number,
  };

  await sleep((RECORD_SECS * 1000) / 2);
  await ok("marker.add", { name: "midtake", timecode: "00:00:02:00" });
  markerWallOffsetSecs = (Date.now() - recordWallStartMs) / 1000;

  // Step-2 record-through-mix probe (transitions audit): mid-recording, drive
  // a real 15-frame mix between two real dress scenes (A1→A2, both video —
  // clip and loop) so the span covers a blend under record load. The mix and
  // the cut back below both sit inside the spanStart→spanEnd window, so the
  // whole-span no-drop assert below covers them; the mix-local delta assert
  // beside recordSpan names the blend itself.
  //
  // record_tap_ms / skipped_record_frames re-verified Step 2 against
  // `telemetry.rs`'s `build_tick_for_dir`: still EngineState-only, still
  // absent from the §10.1 tick — the reachability gap stands (v0.5
  // candidate), so only the wire counters (drops + underruns) land in
  // timings.json. No wire fields added.
  const mixDropsBefore = droppedNow();
  const mixUnderrunsBefore =
    ((ticks.at(-1)?.["data"] as Record<string, unknown>)?.["audioUnderrunsTotal"] ?? 0) as number;
  await ok("preview.set", { itemRef: "A2" });
  await ok("view.take", { transition: "mix", durationFrames: 15 });
  await untilTelemetry(
    "A2 on air after the record-through-mix",
    (t) => (t["data"] as Record<string, unknown>)?.["viewItem"] === "A2",
  );
  // 15 frames is 0.5 s; this outlasts the blend plus a telemetry tick so the
  // whole transition is inside the measured span.
  await sleep(1500);
  const mixDropsAfter = droppedNow();
  const mixUnderrunsAfter =
    ((ticks.at(-1)?.["data"] as Record<string, unknown>)?.["audioUnderrunsTotal"] ?? 0) as number;
  // Back to A1 on a cut: step 13's kill-take assumes A1 is the on-air item
  // (an exhausted clip holds its viewItem), and this step must not move it.
  await ok("preview.set", { itemRef: "A1" });
  await ok("view.take", { transition: "cut" });
  await untilTelemetry(
    "A1 back on air before record.stop",
    (t) => (t["data"] as Record<string, unknown>)?.["viewItem"] === "A1",
  );

  await sleep(RECORD_SECS * 1000 - (RECORD_SECS * 1000) / 2);

  const stopSentMs = Date.now();
  const stopReply = await ok("record.stop", {});
  // The file + sidecar are complete BEFORE the engine's applied ack lands
  // (`stop_and_finish` waits for the thread's terminal report) — so this
  // wait, not the control-plane ack, is what the assertions below stand on.
  assert.ok(
    await server.awaitApplied((stopReply["stateVersion"] ?? 0) as number, RECORD_STOP_MS),
    "the engine must finalize the take before record.stop applies",
  );

  const file = soleRecording(recordDir);
  recordFile = file;
  // Span end: no View drop may occur while recording on the normative
  // machine (CI-gated: the 3-core runner drops by design, so this asserts
  // only where frame budgets mean something).
  const spanEnd = {
    droppedFramesTotal: droppedNow(),
    audioUnderrunsTotal:
      ((ticks.at(-1)?.["data"] as Record<string, unknown>)?.["audioUnderrunsTotal"] ?? 0) as number,
  };
  recordSpan = { ...spanStart, endDroppedFramesTotal: spanEnd.droppedFramesTotal, endAudioUnderrunsTotal: spanEnd.audioUnderrunsTotal, mixDroppedBefore: mixDropsBefore, mixDroppedAfter: mixDropsAfter, mixUnderrunsBefore, mixUnderrunsAfter };
  if (process.env["CI"] === undefined || process.env["CI"] === "") {
    assert.equal(
      spanEnd.droppedFramesTotal,
      spanStart.droppedFramesTotal,
      `no View drops while recording on the normative machine: ${spanStart.droppedFramesTotal} -> ${spanEnd.droppedFramesTotal}`,
    );
    assert.equal(
      mixDropsAfter,
      mixDropsBefore,
      `no View drops through the record-through-mix on the normative machine: ${mixDropsBefore} -> ${mixDropsAfter}`,
    );
  }
  const size = statSync(file).size;
  // MEASURED 2026-09-15 on the Intel local machine: 39-41 KB for the 4 s
  // take. Video is sparse by design there (the feed.rs ladder sheds record
  // frames whenever render meets-or-exceeds budget while View never waits —
  // ~34 of 120 frames landed), so the bytes are mostly AAC + boxes. The
  // floor sits 2.4x below measured and infinitely above an empty file
  // (which the engine refuses to materialize at all: E_RECORD_INPUT): it
  // proves substance, not a bitrate.
  assert.ok(size > 16 * 1024, `a ${RECORD_SECS} s take must be non-trivially sized, saw ${size} bytes`);

  const bytes = readFileSync(file);
  assert.ok(bytes.includes("ftyp"), "the recording must open with ftyp (init safe)");
  // Structural only: the moov here is written upfront, so its presence is not
  // finalization proof — the finalize proof is ffprobe-parse + duration match
  // below. Kept as a structural box check, nothing more.
  assert.ok(bytes.includes("moov"), "moov box present (structural; upfront moov, not finalize proof)");

  const probed = ffprobeJson(ffprobe, file, ["-show_streams", "-show_format"]);
  const streams = probed["streams"] as Array<Record<string, unknown>>;
  assert.equal(streams.length, 2, "exactly 1 video + 1 audio stream");
  const video = streams.find((s) => s["codec_type"] === "video");
  const audio = streams.find((s) => s["codec_type"] === "audio");
  assert.equal(video?.["codec_name"], "h264");
  assert.equal(audio?.["codec_name"], "aac", "audio must be AAC frames, never PCM-in-MP4");

  // Duration bound is honestly ~107ms = 1 video frame + 3 AAC frames + 10ms
  // container rounding, audio-driven (64ms of the ~107ms is the 3x1024-sample
  // AAC packets), video exempted (sparse by design — see the size comment
  // above; video duration is asserted as presence-only below). The AAC side
  // is named, not guessed: the writer trims 2 priming packets at mux
  // time leaving a 64-sample / 1.33 ms residual, and the audio track holds
  // whole 1024-sample packets against exact video. Expected is measured
  // wall (stop signal sent minus start applied), NOT the commanded sleep:
  // sleep overshoot moves content and wall together, while mux finalize
  // happens after the stop signal and is in neither — so the bound holds
  // under scheduling slop instead of flaking on it.
  const expectedSecs = (stopSentMs - recordWallStartMs) / 1000;
  const dur = parseFloat((probed["format"] as Record<string, unknown>)["duration"] as string);
  const durTol = 1 / HOUSE_RATE + (3 * 1024) / 48000 + 0.01;
  assert.ok(
    Math.abs(dur - expectedSecs) <= durTol,
    `duration ${dur}s must match the ${expectedSecs.toFixed(2)}s wall take within ~107ms = 1 video frame + 3 AAC + 10ms, audio-driven, video exempted (tol ${durTol.toFixed(3)}s)`,
  );
  // Video is sparse by design (see the size comment above), so its only
  // honest claim is presence: the h264 path wrote real frames.
  const fileVideo = streams.find((s) => s["codec_type"] === "video");
  assert.ok(
    fileVideo !== undefined && parseFloat(fileVideo["duration"] as string) > 0,
    "the take must contain video content (sparse is by design, absent is not)",
  );

  // Marker chapter/sidecar: fMP4 carries chapters poorly, so chapters ride
  // the always-sidecar `<stem>.markers.json` beside the recording for every
  // container (crates/nbe-engine/src/record/markers.rs: honest container
  // story; in-container chapters deferred). The sidecar IS the chapter
  // record the suite asserts — nothing here claims the MP4 carries chapters.
  const sidecar = file.replace(/\.mp4$/, ".markers.json");
  assert.ok(existsSync(sidecar), `marker sidecar must sit beside the recording: ${sidecar}`);
  const markers = (
    JSON.parse(readFileSync(sidecar, "utf8")) as Record<string, unknown>
  )["markers"] as Array<Record<string, unknown>>;
  assert.ok(
    markers.some((m) => m["name"] === "midtake"),
    `sidecar must carry the mid-take chapter, saw ${JSON.stringify(markers)}`,
  );
});

test("[RI-1] step 12: the take lands at file zero in sync within 20 ms", async () => {
  const ffprobe = ffprobeOrSkip();
  if (ffprobe === null) return;
  if (recordFile === null) {
    console.log("SKIP sync step: step 11 skipped (no recording — capability gate)");
    return;
  }
  const file = recordFile as string;

  // AC-13 bound, borrowed — with the nuance on the record. AC-13 is the
  // SOUNDBOARD-trigger latency bound (a live-path number), the closest
  // normative latency figure to hand; it is NOT a file-sync spec, and no
  // normative file A/V-sync bound exists. This assertion borrows AC-13's
  // 20 ms for the one file-observable sync claim that survives the feed.rs
  // shed ladder: the KNOWN EVENT is the take itself (A1 on air when the
  // take opened), its EXPECTED FILE TIMECODE is zero, and both tracks must
  // start there. Track DURATION equality is deliberately NOT asserted —
  // video is sparse by design while audio runs whole, so durations disagree
  // on a healthy file; samples are timestamped, and start alignment is what
  // "in sync" means for the artifact. Structure, never performance.
  const probed = ffprobeJson(ffprobe, file, ["-show_streams"]);
  const streams = probed["streams"] as Array<Record<string, unknown>>;
  const video = streams.find((s) => s["codec_type"] === "video");
  const audio = streams.find((s) => s["codec_type"] === "audio");
  assert.ok(video !== undefined && audio !== undefined, "the take must carry video + audio streams");
  const vStart = parseFloat(video["start_time"] as string);
  const aStart = parseFloat(audio["start_time"] as string);
  assert.ok(
    vStart <= 0.02,
    `video must start at file zero within 20 ms, started at ${vStart}s`,
  );
  assert.ok(
    aStart <= 0.02,
    `audio must start at file zero within 20 ms, started at ${aStart}s (priming trim leaves 1.33 ms)`,
  );
  assert.ok(
    Math.abs(vStart - aStart) <= 0.02,
    `tracks must start together within 20 ms (v ${vStart}s vs a ${aStart}s)`,
  );

  // The known event (the mid-take marker, sent at a measured wall offset)
  // lands inside the take: 0 <= marker offset <= file duration.
  const full = ffprobeJson(ffprobe, file, ["-show_format"]);
  const dur = parseFloat((full["format"] as Record<string, unknown>)["duration"] as string);
  assert.ok(
    markerWallOffsetSecs >= 0 && markerWallOffsetSecs <= dur,
    `marker at wall offset ${markerWallOffsetSecs.toFixed(2)}s must land inside the ${dur}s take`,
  );
});

test("[RI-1] step 13 (AC-6): kill -9 mid-record leaves prior fragments playable", async () => {
  const ffprobe = ffprobeOrSkip();
  if (ffprobe === null) return;
  if (recordFile === null) {
    console.log("SKIP kill step: step 11 skipped (no recording — capability gate)");
    return;
  }
  // The show is still RUNNING from step 11 (no show.stop since): open a
  // second take under a distinct episode name so its file is unambiguous.
  // No re-take first: A1 is still the on-air item (an exhausted clip holds
  // its last viewItem rather than clearing it), and this step asserts
  // parseability only, so any rendered picture — even a held frame — plus
  // the live master mix is the content it needs.
  const recReply = await ok("record.start", { outputId: "killtake" });
  assert.ok(
    await server.awaitApplied((recReply["stateVersion"] ?? 0) as number, COMMAND_MS),
    "the engine must apply the kill-take record.start",
  );
  // Fragments complete on the RECEIVED-video timeline (1 s of handed-off
  // video per moof), and the feed.rs ladder sheds most frames on slow
  // render hardware — so 2.5 s of wall holds zero complete fragments here
  // (measured: init-only 1122-byte file, ftyp+moov, no moof). Six seconds
  // of wall buys ~1.7 received-video seconds on the local Intel machine:
  // one flushed moof plus a partial. The wait is wall, the proof is bytes.
  await sleep(6000);
  engine.kill("SIGKILL");
  await new Promise<void>((r) => {
    if (engine.exitCode !== null || engine.signalCode !== null) return r();
    const timer = setTimeout(r, 5000);
    engine.once("exit", () => {
      clearTimeout(timer);
      r();
    });
  });

  // Parseability plus a flushed-fragment floor — deliberately no other timing
  // assertions here. The file shape differs by runner speed (fragment
  // completion is wall-driven), so the content proof is gated on what this
  // runner actually flushed. Both paths require hasMoof (≥1 flushed prior
  // fragment): streams-parse alone passes an init-only file (ftyp+moov, no
  // moof), so moof presence is the playability floor on CI too, not just on
  // the normative machine. The fixed-byte size floor below is likewise gated
  // on hasMoof — with no moof the file is bare init by wall timing, and that
  // wall-driven flake must log-skip, never red CI. The branch is on `CI` and
  // is logged either way.
  const recordDir = dirname(recordFile as string);
  const mp4s = readdirSync(recordDir).filter((f) => f.endsWith(".mp4"));
  const killed = mp4s.find((f) => f.includes("killtake"));
  assert.ok(killed !== undefined, `the kill take must leave a file, saw ${JSON.stringify(mp4s)}`);
  const file = join(recordDir, killed);
  const bytes = readFileSync(file);
  assert.ok(bytes.includes("ftyp"), "even the partial file must open with ftyp");
  // Structural only: the upfront moov ships with init, so its presence is not
  // finalization proof — the finalize proof is ffprobe-parse + moof below.
  assert.ok(bytes.includes("moov"), "even the partial file must carry the upfront moov (structural; not finalize proof)");
  const hasMoof = bytes.includes("moof");
  const isCI = (process.env["CI"] ?? "") !== "";
  if (!isCI) {
    assert.ok(hasMoof, "normative: 6 s of wall must flush ≥1 prior fragment (moof)");
  } else {
    console.log(`CI timing note: partial file ${hasMoof ? "has" : "has no"} flushed moof; requiring moof as the playability floor`);
    assert.ok(hasMoof, "CI: partial file must still carry ≥1 flushed prior fragment (moof) — streams-parse alone passes an init-only file");
  }
  if (hasMoof) {
    assert.ok(bytes.length > 1122, `the partial take must exceed bare init, saw ${bytes.length} bytes`);
  } else {
    console.log(`SKIP size floor: no moof flushed (wall-driven), saw ${bytes.length} bytes — size assert skipped, not failed`);
  }
  const probed = ffprobeJson(ffprobe, file, ["-show_streams", "-show_format"]);
  const streams = probed["streams"] as Array<Record<string, unknown>>;
  assert.ok(streams.some((s) => s["codec_name"] === "h264"), "prior video fragments must still parse");
  assert.ok(streams.some((s) => s["codec_name"] === "aac"), "prior audio fragments must still parse");
  if (hasMoof) {
    const dur = parseFloat((probed["format"] as Record<string, unknown>)["duration"] as string);
    assert.ok(dur > 0, `the partial file must play (duration ${dur}s)`);
  }
});
