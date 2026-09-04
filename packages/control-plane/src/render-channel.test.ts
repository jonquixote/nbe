//! Prompt 02c: SPEC v0.3.2 conformance for the render channel and push frames.
//! §5.4.1 stateChange, §5.9.4 show.resync, §5.9.5 appliedStateVersion and the
//! show.stop grace window, §16.0 authorization, §16.4 item.reset,
//! §16 E_RATE_LIMITED, §10.4 status completeness.
//!
//! Every test here fails if its behaviour is removed — that is the point of
//! writing them: the previous round shipped a directive path that delivered
//! nothing and a grace window that waited for nothing, and both suites passed.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { MockRenderBridge } from "./render-bridge.js";
import { CpError, RESYNC_COMMAND } from "./protocol.js";
import { preflightBin } from "./package.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const OPERATOR = "operator-token";
const MONITOR = "monitor-token";

let server: ControlPlaneServer;
let state: ControlPlaneState;
let pkgPath: string;
let warnings: string[];

function makePackage(): string {
  const dir = mkdtempSync(join(tmpdir(), "nbe-02c-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.3",
      network: { id: "nbe", name: "Test" },
      show: {
        id: "show-1",
        title: "Test",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        // An image, not video: these packages exercise the command bus, not
        // media. Prompt 05 gave preflight real decode, so a placeholder file
        // declared as video is now correctly rejected.
        { id: "A1_clip", kind: "image", source: "media/A1.png" },
      ],
      scenes: [{ id: "SCN_A1", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_clip" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN_A1" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

/** show.load shells out to nbe-preflight; skip cleanly when it is not built. */
function preflightAvailable(): boolean {
  return existsSync(preflightBin());
}

function conn(role: string, token: string): WebSocket {
  return new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
  });
}

function connect(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
}

function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((resolve) => {
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId === id) {
        ws.off("message", onMsg);
        resolve(msg);
      }
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

/** Collects every frame of one `kind` as it arrives. */
function collect(ws: WebSocket, kind: string): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = [];
  ws.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === kind) out.push(msg);
  });
  return out;
}

async function until(check: () => boolean, ms = 1000): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error("condition never became true");
}

beforeEach(async () => {
  pkgPath = makePackage();
  state = new ControlPlaneState();
  warnings = [];
  const tmp = mkdtempSync(join(tmpdir(), "nbe-audit-"));
  server = await createControlPlaneServer({
    port: 0,
    auth: {
      tokens: { [ADMIN]: "admin", [RENDER]: "render", [OPERATOR]: "operator", [MONITOR]: "monitor" },
    },
    audit: new AuditLog(join(tmp, "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    showStopGraceMs: 150,
    warn: (m) => warnings.push(m),
  });
});

afterEach(async () => {
  if (server) await server.close();
});

// ---------------------------------------------------------------------------
// §5.4.1 server-push frames
// ---------------------------------------------------------------------------

test("stateChange: one frame per accepted command, carrying the response stateVersion", async () => {
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const frames = collect(ws, "stateChange");

  const r = await send(ws, "automation.hold", { hold: true });
  assert.equal(r.status, "ok");
  await until(() => frames.length === 1);
  assert.equal(frames.length, 1, "exactly one stateChange per accepted command");
  assert.equal(frames[0]!.stateVersion, r.stateVersion, "frame carries the response's stateVersion");
  assert.deepEqual(frames[0]!.changed, ["automation.hold"]);
  ws.close();
});

test("stateChange: a rejected command emits no frame", async () => {
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const frames = collect(ws, "stateChange");

  const r = await send(ws, "preview.set", { itemRef: "nope" });
  assert.equal(r.status, "error");
  await new Promise((res) => setTimeout(res, 60));
  assert.equal(frames.length, 0, "a rejection must not announce a state change");
  ws.close();
});

test("stateChange frames do not go to render sessions", async () => {
  const render = conn("render", RENDER);
  await connect(render);
  const stateFrames = collect(render, "stateChange");
  const admin = conn("admin", ADMIN);
  await connect(admin);
  await send(admin, "automation.hold", { hold: true });
  await new Promise((res) => setTimeout(res, 60));
  assert.equal(stateFrames.length, 0, "render nodes receive directives, not state frames");
  admin.close();
  render.close();
});

// ---------------------------------------------------------------------------
// §5.9.4 show.resync
// ---------------------------------------------------------------------------

test("show.resync is the first directive on a render connection", async () => {
  const render = conn("render", RENDER);
  const directives = collect(render, "directive");
  await connect(render);
  await until(() => directives.length >= 1);
  assert.equal(directives[0]!.command, RESYNC_COMMAND, "resync must precede every other directive");
  const payload = directives[0]!.payload as Record<string, unknown>;
  for (const key of ["showState", "viewItem", "previewItem", "itemStates", "sceneStates", "visibleOverlays", "automationHold", "stateVersion"]) {
    assert.ok(key in payload, `resync snapshot missing ${key}`);
  }
  render.close();
});

test("a render node connecting mid-show is resynced, and its seq starts at 0", async () => {
  const admin = conn("admin", ADMIN);
  await connect(admin);
  await send(admin, "automation.hold", { hold: true });
  const versionAtJoin = state.stateVersion;

  // Node joins after the state has already moved.
  const render = conn("render", RENDER);
  const directives = collect(render, "directive");
  await connect(render);
  await until(() => directives.length >= 1);

  assert.equal(directives[0]!.command, RESYNC_COMMAND);
  assert.equal(directives[0]!.seq, 0, "a fresh connection starts at seq 0 (§5.9.2)");
  const payload = directives[0]!.payload as Record<string, unknown>;
  assert.equal(payload.stateVersion, versionAtJoin, "snapshot carries the current version");
  assert.equal(payload.automationHold, true, "snapshot reflects state the node never saw");
  admin.close();
  render.close();
});

test("resyncRequest gets a fresh snapshot on that connection", async () => {
  const render = conn("render", RENDER);
  const directives = collect(render, "directive");
  await connect(render);
  await until(() => directives.length >= 1);

  render.send(JSON.stringify({ v: "0.3", kind: "resyncRequest", reason: "seqGap" }));
  await until(() => directives.length >= 2);
  assert.equal(directives[1]!.command, RESYNC_COMMAND);
  render.close();
});

// ---------------------------------------------------------------------------
// §5.9.5 appliedStateVersion and the show.stop grace window
// ---------------------------------------------------------------------------

test("show.stop: acknowledged within the window is graceful, with no warning", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  const admin = conn("admin", ADMIN);
  await connect(admin);
  const render = conn("render", RENDER);
  const directives = collect(render, "directive");
  await connect(render);

  await send(admin, "show.load", { packagePath: pkgPath });
  await send(admin, "show.start", {});
  state.recordState = "recording"; // an active output to quiesce

  // The engine acknowledges as soon as it sees the stop directives.
  render.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === "directive" && msg.command === "record.stop") {
      render.send(
        JSON.stringify({ v: "0.3", kind: "appliedStateVersion", stateVersion: msg.stateVersion }),
      );
    }
  });

  const r = await send(admin, "show.stop", { quiesceOutputs: true, force: false });
  assert.equal(r.status, "ok");
  assert.equal(state.showState, "STOPPED");
  assert.deepEqual(warnings, [], "an acknowledged stop logs no force warning");
  assert.ok(
    directives.some((d) => d.command === "record.stop"),
    "the stop directives must reach the engine before the wait",
  );
  admin.close();
  render.close();
});

test("show.stop: unacknowledged times out, forces, and logs the exact warning", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  const admin = conn("admin", ADMIN);
  await connect(admin);
  const render = conn("render", RENDER); // connected but deliberately silent
  await connect(render);

  await send(admin, "show.load", { packagePath: pkgPath });
  await send(admin, "show.start", {});
  state.recordState = "recording";

  const r = await send(admin, "show.stop", { quiesceOutputs: true, force: false });
  assert.equal(r.status, "ok");
  assert.deepEqual(
    warnings,
    ["show.stop: graceful output shutdown exceeded 2 s; force-stopping outputs"],
    "the timeout branch must be reachable in production, with the exact string",
  );
  admin.close();
  render.close();
});

test("appliedStateVersion: recorded per session; a stale value is ignored", async () => {
  const render = conn("render", RENDER);
  await connect(render);
  await new Promise((r) => setTimeout(r, 30));

  render.send(JSON.stringify({ v: "0.3", kind: "appliedStateVersion", stateVersion: 10 }));
  await new Promise((r) => setTimeout(r, 30));
  const statusAfter10 = await fetchStatus();
  assert.equal((statusAfter10.renderNode as Record<string, unknown>).lastAppliedStateVersion, 10);

  render.send(JSON.stringify({ v: "0.3", kind: "appliedStateVersion", stateVersion: 4 }));
  await new Promise((r) => setTimeout(r, 30));
  const statusAfterStale = await fetchStatus();
  assert.equal(
    (statusAfterStale.renderNode as Record<string, unknown>).lastAppliedStateVersion,
    10,
    "a lower stateVersion must not move the applied marker backwards",
  );
  render.close();
});

async function fetchStatus(): Promise<Record<string, unknown>> {
  const res = await fetch(`http://127.0.0.1:${server.port}/nbe/v0.3/status`);
  return (await res.json()) as Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// §10.1.1 qualityProfile: effective (engine) over requested (manifest)
// ---------------------------------------------------------------------------

test("qualityProfile reports the engine's effective profile, falling back to the manifest's", async () => {
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const ticks = collect(ws, "telemetry");

  // Requested profile, with no engine report yet.
  state.qualityProfile = "pro";
  const sub = await send(ws, "system.telemetry.subscribe", { intervalMs: 100 });
  assert.equal(sub.status, "ok");
  await until(() => ticks.length >= 1, 2000);
  assert.equal(
    (ticks.at(-1)!.data as Record<string, unknown>).qualityProfile,
    "pro",
    "with no engine report the control plane reports what the show asked for",
  );

  // A render node reports a lower effective profile; it wins.
  const render = conn("render", RENDER);
  await connect(render);
  render.send(
    JSON.stringify({
      v: "0.3",
      kind: "engineTelemetry",
      ts: Date.now(),
      masterClockFrame: 10,
      droppedFramesTotal: 0,
      renderGpuTimeMs: 4.2,
      decodeSessions: 0,
      vramUsedMib: 100,
      textureCacheUsedMib: 0,
      streamBufferMs: 0,
      recordSpaceMib: 0,
      masterClockDriftMs: 0,
      fallbackActive: false,
      degradationRung: 0,
      qualityProfile: "consumer",
    }),
  );
  const before = ticks.length;
  await until(
    () =>
      ticks.length > before &&
      (ticks.at(-1)!.data as Record<string, unknown>).qualityProfile === "consumer",
    2000,
  );
  ws.close();
  render.close();
});

// ---------------------------------------------------------------------------
// §10.4 status completeness
// ---------------------------------------------------------------------------

test("GET /status carries all seven Section 10.4 fields", async () => {
  const status = await fetchStatus();
  for (const key of [
    "showState", // show load state
    "masterClockState", // master clock state
    "renderNode", // render node health
    "streamState", // stream health
    "recordState", // recording health
    "preflightPassed", // preflight state
    "lastError", // last error
  ]) {
    assert.ok(key in status, `status is missing the §10.4 field ${key}`);
  }
});

// ---------------------------------------------------------------------------
// §16.0 authorization matrix, §16 E_RATE_LIMITED, §16.4 item.reset
// ---------------------------------------------------------------------------

test("authorization matrix: monitor is read-only, operator is live, plugin.reload is admin-only", async () => {
  const monitor = conn("monitor", MONITOR);
  const operator = conn("operator", OPERATOR);
  await Promise.all([connect(monitor), connect(operator)]);

  const cases: Array<[WebSocket, string, "ok" | "error"]> = [
    [monitor, "system.status", "ok"],
    [monitor, "view.fallback", "error"],
    [monitor, "show.load", "error"],
    [operator, "view.fallback", "ok"],
    [operator, "system.status", "ok"],
    [operator, "show.load", "error"], // producer/admin only
    [operator, "show.start", "error"], // admin only — going to air is not an operator call
    [operator, "plugin.reload", "error"], // admin only — it loads code
  ];
  for (const [ws, command, expected] of cases) {
    const payload =
      command === "show.load" ? { packagePath: pkgPath } : command === "plugin.reload" ? { pluginId: "p" } : {};
    const r = await send(ws, command, payload);
    assert.equal(r.status, expected, `${command} for this role should be ${expected}`);
    if (expected === "error" && command !== "show.load") {
      assert.equal((r.error as Record<string, unknown>).code, "E_AUTH", `${command} must fail with E_AUTH`);
    }
  }
  monitor.close();
  operator.close();
});

test("rate limiting returns E_RATE_LIMITED and does not bump stateVersion", async () => {
  const deps: DispatchDeps = {
    state,
    bridge: new MockRenderBridge(),
    persistence: { onDirty: () => {}, flushNow: () => {} },
    rateLimiter: { allow: () => false },
  };
  const registry = buildRegistry(deps);
  const before = state.stateVersion;
  await assert.rejects(
    () =>
      dispatch(deps, registry, {
        connectionId: "c1",
        role: "admin",
        envelope: { v: "0.3", id: randomUUID(), command: "automation.hold", payload: { hold: true } },
      }),
    (e: unknown) => e instanceof CpError && e.code === "E_RATE_LIMITED",
  );
  assert.equal(state.stateVersion, before, "a rate-limited command mutates nothing");
});

test("item.reset clears DONE/MISSING/ERROR and refuses anything else", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight binary not available");
  const admin = conn("admin", ADMIN);
  await connect(admin);
  await send(admin, "show.load", { packagePath: pkgPath });

  // READY: nothing to reset.
  let r = await send(admin, "item.reset", { itemId: "A1" });
  assert.equal(r.status, "error");
  assert.equal((r.error as Record<string, unknown>).code, "E_FORBIDDEN_STATE");

  // ERROR -> READY.
  state.markError("A1");
  r = await send(admin, "item.reset", { itemId: "A1" });
  assert.equal(r.status, "ok");
  assert.equal(state.itemStateOf("A1"), "READY");

  // Unrecoverable stays ERROR (Section 17.3's terminal row).
  state.markError("A1");
  state.unrecoverableItems.add("A1");
  r = await send(admin, "item.reset", { itemId: "A1" });
  assert.equal(r.status, "error");
  assert.equal(state.itemStateOf("A1"), "ERROR");
  admin.close();
});

// ---------------------------------------------------------------------------
// §16.1 show.start warnings policy
// ---------------------------------------------------------------------------

test("show.start refuses a warnings-only package unless allowWarnings is passed", async () => {
  // Drive the state directly: this is the policy gate, not the preflight run.
  state.showState = "LOADED";
  state.preflightPassed = false;
  state.preflightWarnings = ["loudness -15.2 LUFS approaching tolerance"];
  const admin = conn("admin", ADMIN);
  await connect(admin);

  let r = await send(admin, "show.start", {});
  assert.equal(r.status, "error");
  assert.equal((r.error as Record<string, unknown>).code, "E_FORBIDDEN_STATE");
  assert.match(String((r.error as Record<string, unknown>).message), /allowWarnings/);
  assert.equal(state.showState, "LOADED", "a refused start must not change the show state");

  r = await send(admin, "show.start", { allowWarnings: true });
  assert.equal(r.status, "ok");
  assert.equal(state.showState, "RUNNING");
  admin.close();
});

// ---------------------------------------------------------------------------
// Per-subscriber deprecation cursors
// ---------------------------------------------------------------------------

test("every telemetry subscriber sees the deprecation warning, not just the first", async () => {
  const a = conn("admin", ADMIN);
  const b = conn("admin", ADMIN);
  await Promise.all([connect(a), connect(b)]);
  const ticksA = collect(a, "telemetry");
  const ticksB = collect(b, "telemetry");

  // Assert the subscriptions took: an unchecked E_BAD_PAYLOAD here (the
  // schema floor is 100 ms) silently turns the rest of this test into a
  // three-second wait for ticks that were never going to arrive.
  for (const ws of [a, b]) {
    const sub = await send(ws, "system.telemetry.subscribe", { intervalMs: 100 });
    assert.equal(sub.status, "ok");
  }
  // `program.fallback` is a deprecated alias that succeeds unconditionally —
  // the warning only rides the tick when the aliased command was accepted.
  const r = await send(a, "program.fallback", {});
  assert.equal(r.status, "ok");

  const sawWarning = (ticks: Record<string, unknown>[]) =>
    ticks.some((t) => {
      const data = t.data as { deprecationWarnings?: Array<{ command: string }> };
      return (data.deprecationWarnings ?? []).some((w) => w.command === "program.fallback");
    });

  await until(() => sawWarning(ticksA) && sawWarning(ticksB), 3000);
  a.close();
  b.close();
});
