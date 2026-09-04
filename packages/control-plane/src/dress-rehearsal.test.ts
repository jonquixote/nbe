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
import { mkdtempSync, existsSync, readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
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
  const reply = await send(command, payload, timeoutMs);
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
  pred: (t: Record<string, unknown>, index: number) => boolean,
  ms: number = TELEMETRY_MS,
): Promise<Record<string, unknown>> {
  const deadline = Date.now() + ms;
  const seen = ticks.length;
  while (Date.now() < deadline) {
    const hit = ticks.slice(seen).find((t, i) => pred(t, seen + i));
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
    if (frame["kind"] === "telemetry") ticks.push(frame);
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

test("[RI-1] step 7: an audio.bus.set is reflected on the next tick", async () => {
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
  const before = ticks.length;
  await ok("audio.bus.set", { bus: "music", gainDb: -20 });
  const after = await untilTelemetry(
    "a tick carrying live bus meters after the bus change",
    (t, i) => {
      if (i < before) return false;
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
