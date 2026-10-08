//! Prompt 11 — the automation engine runtime (SPEC §13, AC-25).
//!
//! Rule 7 throughout: every rule arrives the way production gets it — a
//! manifest, `show.load` over the WebSocket, `nbe-preflight` — and every
//! firing runs through the server's one command path and is asserted on the
//! AUDIT LOG, the artifact AC-25 #4 names. Nothing here calls the evaluator's
//! matching internals and asserts a return value.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { WebSocket } from "ws";

import { AuditLog, type AuditRecord } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { preflightBin } from "./package.js";
import { tempDir } from "./test-tmp.js";

process.env.NBE_PREFLIGHT_TIMEOUT_MS ??= "5000";

const ADMIN = "admin-token";
const OPERATOR = "operator-token";
const RENDER = "render-token";

let server: ControlPlaneServer;
let state: ControlPlaneState;
let auditPath: string;
/**
 * The evaluator's clock, frozen unless a test moves it: the limiter keys on
 * a frame index read from this clock, and two triggers a millisecond apart
 * can otherwise straddle a 33 ms boundary.
 */
let clockMs = 0;
const FRAME_MS = 1000 / 30;
/**
 * Move the frozen clock into the NEXT frame — to its middle, not its edge.
 * `clockMs += FRAME_MS` drifted: 1000/30 is not exact, so the tenth step
 * floored into the ninth step's frame index (333.33…/33.33… → 9), and the
 * limiter read two frames as one. A frame's middle is half a frame from
 * either edge, far outside any rounding.
 */
const nextFrame = (): void => {
  clockMs = (Math.floor(clockMs / FRAME_MS) + 1.5) * FRAME_MS;
};

beforeEach(async () => {
  clockMs = 0;
  state = new ControlPlaneState();
  auditPath = join(tempDir("nbe-auto-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [OPERATOR]: "operator", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    showStopGraceMs: 150,
    warn: () => {},
    automationClock: () => clockMs,
  });
});

afterEach(async () => {
  if (server) await server.close();
});

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

export function automationPackage(automation: unknown[], extra: Record<string, unknown> = {}): string {
  const dir = tempDir("nbe-auto-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "a.png"), "png");
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Test" },
      show: {
        id: "show-auto",
        title: "Automation",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        { id: "a_img", kind: "image", source: "media/a.png" },
      ],
      scenes: [{ id: "SCN", elements: [{ id: "main", kind: "clip", z: 1, assetId: "a_img" }] }],
      rundown: {
        id: "R",
        items: [
          { id: "A1", kind: "sceneRef", sceneRef: "SCN" },
          { id: "A2", kind: "sceneRef", sceneRef: "SCN" },
          { id: "A3", kind: "sceneRef", sceneRef: "SCN" },
          // Timed: a take puts it PLAYING, and the engine's `end` makes it DONE.
          { id: "AT", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60 },
          // WU4: an autoFollow item and the item after it; and an autoFollow
          // item that is the last in the rundown.
          { id: "AF", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
          { id: "AN", kind: "sceneRef", sceneRef: "SCN" },
          { id: "AZ", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
        ],
      },
      control: {
        bindings: [
          { id: "take-key", action: "view.take", trigger: { kind: "hotkey", key: "F1" } },
          { id: "cmp-1", action: "view.take", trigger: { kind: "companionKey", page: 1, bank: 1, key: "1/1" } },
        ],
      },
      automation,
      ...extra,
    }),
  );
  return dir;
}

export function conn(role: string, token: string): WebSocket {
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

function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}, extra: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.off("message", onMsg);
      reject(new Error(`no response to "${command}" within 15000 ms`));
    }, 15_000);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId === id) {
        clearTimeout(timer);
        ws.off("message", onMsg);
        resolve(msg);
      }
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload, ...extra }));
  });
}

function audit(): AuditRecord[] {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as AuditRecord);
}

function automationRows(): AuditRecord[] {
  return audit().filter((r) => r.kind === "automation");
}

function preflightAvailable(): boolean {
  return existsSync(preflightBin());
}

/** Load a package over the wire and start the show; returns the admin socket. */
async function loadAndStart(automation: unknown[], start = true): Promise<WebSocket> {
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const load = await send(ws, "show.load", { packagePath: automationPackage(automation) });
  assert.equal(load.status, "ok", `show.load must succeed: ${JSON.stringify(load)}`);
  if (start) {
    const s = await send(ws, "show.start", {});
    assert.equal(s.status, "ok", `show.start must succeed: ${JSON.stringify(s)}`);
  }
  await server.automation.settled();
  return ws;
}

const markerRule = (id: string, trigger: Record<string, unknown>, extra: Record<string, unknown> = {}) => ({
  id,
  trigger,
  action: { command: "marker.add", payload: { name: `by-${id}` } },
  ...extra,
});

// ---------------------------------------------------------------------------
// WU1 — the audit kind, the one command path, enabled/disable, refusal
// ---------------------------------------------------------------------------

test("the audit kind union carries automation (WU1: the type accepted no automation writes)", () => {
  // Exhaustive over the union: adding a kind without handling it here, or
  // removing "automation", fails to compile (`never`) — the pin the draft
  // asked for, since nothing else enumerates the kinds.
  const label = (k: AuditRecord["kind"]): string => {
    switch (k) {
      case "command":
      case "auth":
      case "preflight":
      case "automation":
        return k;
      default: {
        const unreachable: never = k;
        return unreachable;
      }
    }
  };
  assert.deepEqual(
    (["command", "auth", "preflight", "automation"] as const).map(label),
    ["command", "auth", "preflight", "automation"],
  );
  const log = new AuditLog(join(tempDir("nbe-auto-kind-"), "a.jsonl"));
  const rec = log.record({ kind: "automation", actor: "automation:r", event: "automation.action", outcome: "ok" });
  assert.equal(rec.kind, "automation");
  assert.equal(rec.actor, "automation:r");
});

test("a rule fires through the real command path and its action is audited as automation", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([
    markerRule("on-air", { kind: "stateChange", params: { field: "showState", to: "RUNNING" } }),
  ]);
  const rows = automationRows().filter((r) => r.event === "automation.action");
  assert.equal(rows.length, 1, `one action row: ${JSON.stringify(automationRows())}`);
  const row = rows[0]!;
  assert.equal(row.outcome, "ok");
  assert.equal(row.command, "marker.add");
  assert.equal(row.actor, "automation:on-air");
  assert.equal(row.role, "operator", "an automation action faces an operator's preconditions (§13.1)");
  assert.equal(row.detail?.["ruleId"], "on-air");
  assert.deepEqual(row.detail?.["trigger"], { kind: "stateChange", field: "showState", from: "LOADED", to: "RUNNING" });
  assert.equal(typeof row.detail?.["latencyMs"], "number");
  // WU7: the span's three instants, in order — observed, queued, dispatched.
  const [observedAt, queuedAt, dispatchedAt] = ["observedAt", "queuedAt", "dispatchedAt"].map((k) => row.detail?.[k]);
  assert.ok(
    typeof observedAt === "number" && typeof queuedAt === "number" && typeof dispatchedAt === "number",
    `the row carries the span's instants: ${JSON.stringify(row.detail)}`,
  );
  assert.ok(observedAt <= queuedAt && queuedAt <= dispatchedAt, "observed ≤ queued ≤ dispatched");
  assert.ok((row.stateVersionAfter ?? 0) > (row.stateVersionBefore ?? 0), "the action bumped the version once");
  assert.ok(state.markers.some((m) => m.name === "by-on-air"), "the action's effect is real state");
  ws.close();
});

test("a rule disabled in the manifest never fires; automation.enable re-arms it", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([
    markerRule("pv", { kind: "stateChange", params: { field: "previewItem" } }, { enabled: false }),
  ]);
  assert.equal(state.automationRules.get("pv"), false, "the manifest's enabled:false is honoured at load");
  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  await server.automation.settled();
  assert.equal(automationRows().length, 0, "a disabled rule is not an attempt: nothing fires, nothing is audited");

  assert.equal((await send(ws, "automation.enable", { ruleId: "pv" })).status, "ok");
  assert.equal((await send(ws, "preview.set", { itemRef: "A2" })).status, "ok");
  await server.automation.settled();
  const rows = automationRows().filter((r) => r.event === "automation.action" && r.outcome === "ok");
  assert.equal(rows.length, 1, "re-armed, it fires on the next change");
  assert.deepEqual(rows[0]!.detail?.["trigger"], { kind: "stateChange", field: "previewItem", from: "A1", to: "A2" });
  ws.close();
});

test("an action refused by a precondition is refused exactly as an operator's, and audited as refused", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([
    {
      id: "stopper",
      trigger: { kind: "stateChange", params: { field: "previewItem", to: "A1" } },
      action: { command: "item.stop", payload: { itemId: "A1" } },
    },
  ]);
  // The operator's own attempt, for comparison.
  const human = await send(ws, "item.stop", { itemId: "A1" });
  assert.equal(human.status, "error");
  const humanCode = (human.error as { code: string }).code;

  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  await server.automation.settled();
  const row = automationRows().find((r) => r.event === "automation.action");
  assert.ok(row, "the refused attempt is in the audit log");
  assert.equal(row.outcome, "rejected");
  assert.equal(row.errorCode, humanCode, "the same refusal a human gets");
  assert.equal(row.actor, "automation:stopper");
  ws.close();
});

test("a rule that cannot be evaluated refuses the load, by name — never accepted and inert", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const r = await send(ws, "show.load", {
    packagePath: automationPackage([
      markerRule("rss", { kind: "rssKeyword", params: { keyword: "breaking" } }),
    ]),
  });
  assert.equal(r.status, "error");
  const err = r.error as { code: string; message: string };
  assert.equal(err.code, "E_PREFLIGHT_FAILED");
  assert.match(err.message, /automation rule `rss`/);
  assert.match(err.message, /rssKeyword/);
  assert.equal(state.pkg, null, "nothing was loaded");
  ws.close();
});

// ---------------------------------------------------------------------------
// The params contract: one verdict per fixture, shared with the Rust reading
// (crates/nbe-core/tests/automation_rules.rs reads the same file).
// ---------------------------------------------------------------------------

test("every fixture rule gets its verdict from the control plane's reading, the same as preflight's", async () => {
  const { parseRule } = await import("./automation.js");
  const fixture = JSON.parse(
    readFileSync(new URL("../../../crates/nbe-core/tests/fixtures/automation_rules.json", import.meta.url), "utf8"),
  ) as {
    refs: { items: string[]; bindings: Record<string, string | null> };
    cases: Array<{ valid: boolean; trigger: Record<string, unknown>; conditions?: Record<string, unknown>[] }>;
  };
  const ctx = {
    items: new Set(fixture.refs.items),
    bindings: new Map(Object.entries(fixture.refs.bindings).map(([k, v]) => [k, v ?? undefined])),
  };
  assert.ok(fixture.cases.length >= 30, `the fixture must exercise every kind; has ${fixture.cases.length}`);
  const wrong: string[] = [];
  fixture.cases.forEach((c, i) => {
    const id = `case${i}`;
    const verdict = parseRule(
      {
        id,
        trigger: c.trigger as never,
        ...(c.conditions ? { conditions: c.conditions } : {}),
        action: { command: "marker.add", payload: { name: "m" } },
      },
      ctx,
    );
    const valid = typeof verdict !== "string";
    if (valid !== c.valid) wrong.push(`${id} ${JSON.stringify(c.trigger)}: expected valid=${c.valid}, got ${JSON.stringify(verdict)}`);
    else if (!valid) assert.ok((verdict as string).startsWith(`automation rule \`${id}\`: `), `a refusal names the rule: ${verdict}`);
  });
  assert.deepEqual(wrong, [], "verdicts that disagree with the fixture");
});

test("an action payload preflight cannot judge refuses the load at the control plane's own read", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // Preflight checks an action's command name and required KEYS, as it does
  // for a control binding; the payload's full §16 validation is the control
  // plane's (the command schemas live here). `itemRef: 5` has the key and the
  // wrong type: preflight passes it, the control plane's read refuses it —
  // this layer, and only this layer.
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const r = await send(ws, "show.load", {
    packagePath: automationPackage([
      // A hotkey trigger: a cut cannot cause one, so the rule is no cycle
      // (WU5's check, §13.4) and preflight passes it to this layer.
      { id: "badcut", trigger: { kind: "hotkey", params: { bindingId: "take-key" } }, action: { command: "view.cut", payload: { itemRef: 5 } } },
    ]),
  });
  assert.equal(r.status, "error");
  const err = r.error as { code: string; message: string };
  assert.equal(err.code, "E_PREFLIGHT_FAILED");
  assert.match(err.message, /^automationRule: automation rule `badcut`: action payload is not a valid view\.cut payload/);
  assert.equal(state.pkg, null);
  ws.close();
});

test("an automation action faces an operator's role check, not an admin's", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // plugin.reload is admin-only (§16.0). A rule acts as operator (§13.1), so
  // its plugin.reload is refused E_AUTH — where an admin would have reached
  // the handler and been told E_NOT_FOUND.
  const ws = await loadAndStart([
    {
      id: "reloader",
      trigger: { kind: "stateChange", params: { field: "previewItem", to: "A1" } },
      action: { command: "plugin.reload", payload: { pluginId: "nope" } },
    },
  ]);
  const admin = await send(ws, "plugin.reload", { pluginId: "nope" });
  assert.notEqual((admin.error as { code: string }).code, "E_AUTH", "an admin gets past the role check");
  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  await server.automation.settled();
  const row = automationRows().find((r) => r.event === "automation.action");
  assert.ok(row);
  assert.equal(row.outcome, "rejected");
  assert.equal(row.errorCode, "E_AUTH", "the rule is refused as an operator is");
  ws.close();
});

// ---------------------------------------------------------------------------
// WU2 — the once-per-frame limiter, hold, and the pending queue (B5)
// ---------------------------------------------------------------------------

test("a rule that would fire twice in one frame fires once; the second is audited rate-limited", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([markerRule("pv", { kind: "stateChange", params: { field: "previewItem" } })]);
  // Two changes in the same frame (the clock is not moved between them).
  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  assert.equal((await send(ws, "preview.set", { itemRef: "A2" })).status, "ok");
  await server.automation.settled();
  let rows = automationRows();
  assert.equal(rows.filter((r) => r.event === "automation.action").length, 1, "fired once in the frame");
  const limited = rows.filter((r) => r.event === "automation.rateLimited");
  assert.equal(limited.length, 1, `the second is audited as rate-limited: ${JSON.stringify(rows)}`);
  assert.equal(limited[0]!.actor, "automation:pv");
  assert.equal(limited[0]!.outcome, "rejected");

  // The next frame, it fires again: a limit per frame, not a latch.
  nextFrame();
  assert.equal((await send(ws, "preview.set", { itemRef: "A3" })).status, "ok");
  await server.automation.settled();
  rows = automationRows();
  assert.equal(rows.filter((r) => r.event === "automation.action").length, 2, "a new frame fires again");
  ws.close();
});

test("a held engine fires nothing — each suppression audited — and release resumes", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([markerRule("pv", { kind: "stateChange", params: { field: "previewItem" } })]);
  assert.equal((await send(ws, "automation.hold", { hold: true })).status, "ok");
  nextFrame();
  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  await server.automation.settled();
  let rows = automationRows();
  assert.equal(rows.filter((r) => r.event === "automation.action").length, 0, "held: nothing dispatched");
  const suppressed = rows.filter((r) => r.event === "automation.suppressedByHold");
  assert.equal(suppressed.length, 1, "the held trigger is audited");
  assert.equal(suppressed[0]!.actor, "automation:pv");

  assert.equal((await send(ws, "automation.hold", { hold: false })).status, "ok");
  nextFrame();
  assert.equal((await send(ws, "preview.set", { itemRef: "A2" })).status, "ok");
  await server.automation.settled();
  rows = automationRows();
  assert.equal(rows.filter((r) => r.event === "automation.action").length, 1, "released: it fires");
  ws.close();
});

test("a hold cancels every pending action before it dispatches (B5, AC-25 #2)", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // One trigger fires three rules, queued in manifest order: the first
  // engages the hold, so the other two are PENDING — fired, not yet
  // dispatched — when the hold lands. They must be cancelled, not run.
  const ws = await loadAndStart([
    {
      id: "holder",
      trigger: { kind: "stateChange", params: { field: "previewItem", to: "A1" } },
      action: { command: "automation.hold", payload: { hold: true } },
    },
    markerRule("b", { kind: "stateChange", params: { field: "previewItem", to: "A1" } }),
    markerRule("c", { kind: "stateChange", params: { field: "previewItem", to: "A1" } }),
  ]);
  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  await server.automation.settled();
  const rows = automationRows();
  const actions = rows.filter((r) => r.event === "automation.action");
  assert.deepEqual(actions.map((r) => r.actor), ["automation:holder"], "only the hold itself dispatched");
  const cancelled = rows.filter((r) => r.event === "automation.cancelledByHold");
  assert.deepEqual(cancelled.map((r) => r.actor), ["automation:b", "automation:c"], "both pending actions cancelled, audited");
  // WU7: AC-25 #2's number — hold accepted → pending cancelled — on each row.
  for (const r of cancelled) {
    const { heldAt, cancelledAt, latencyMs } = r.detail as Record<string, unknown>;
    assert.ok(
      typeof heldAt === "number" && typeof cancelledAt === "number" && latencyMs === cancelledAt - heldAt,
      `the cancellation carries its latency: ${JSON.stringify(r.detail)}`,
    );
  }
  assert.equal(state.markers.length, 0, "no cancelled action reached state");
  assert.equal(state.automationHold, true);
  ws.close();
});

test("a snapshot.recall that restores a held snapshot cancels pending actions too (§13.5: hold is a state)", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // The hold is engaged by whichever accepted command sets `automationHold`.
  // Recalling a snapshot saved while held sets it with no `automation.hold` —
  // the path WU2 found missed (e985e40). The recall is the first of three
  // actions one trigger queues; the other two are pending when it lands.
  // The trigger is A2 going on air: a recall changes `previewItem`, so a
  // `previewItem` trigger would make the recaller a cycle of one, which
  // preflight refuses since WU5 (§13.4) — a recall is not a take.
  const trig = { kind: "mediaStart", params: { itemRef: "A2" } };
  const ws = await loadAndStart([
    { id: "recaller", trigger: trig, action: { command: "snapshot.recall", payload: { name: "held" } } },
    markerRule("b", trig),
    markerRule("c", trig),
  ]);
  assert.equal((await send(ws, "automation.hold", { hold: true })).status, "ok");
  assert.equal((await send(ws, "snapshot.save", { name: "held" })).status, "ok");
  assert.equal((await send(ws, "automation.hold", { hold: false })).status, "ok");
  nextFrame();
  assert.equal((await send(ws, "view.cut", { itemRef: "A2" })).status, "ok");
  await server.automation.settled();
  const rows = automationRows();
  assert.deepEqual(
    rows.filter((r) => r.event === "automation.action").map((r) => r.actor),
    ["automation:recaller"],
    "only the recall dispatched",
  );
  assert.deepEqual(
    rows.filter((r) => r.event === "automation.cancelledByHold").map((r) => r.actor),
    ["automation:b", "automation:c"],
    "the hold the recall restored cancelled both pending actions, audited",
  );
  assert.equal(state.markers.length, 0, "no cancelled action reached state");
  assert.equal(state.automationHold, true, "the recall restored the hold");
  ws.close();
});

// ---------------------------------------------------------------------------
// WU3 — the trigger adapters, each through its real source
// ---------------------------------------------------------------------------

/** A render-role session, for the engine frames that are trigger sources. */
async function renderSession(): Promise<WebSocket> {
  const r = conn("render", RENDER);
  await connect(r);
  return r;
}

function engineTelemetry(extra: Record<string, unknown>): string {
  return JSON.stringify({
    v: "0.3",
    kind: "engineTelemetry",
    ts: Date.now(),
    masterClockFrame: 10,
    droppedFramesTotal: 0,
    renderGpuTimeMs: 1,
    decodeSessions: 0,
    vramUsedMib: 0,
    textureCacheUsedMib: 0,
    streamBufferMs: -1,
    recordSpaceMib: 0,
    masterClockDriftMs: 0,
    fallbackActive: false,
    degradationRung: 0,
    ...extra,
  });
}

/** Wait for the server to have processed everything sent on `ws` so far. */
async function flushed(ws: WebSocket): Promise<void> {
  await send(ws, "system.status", {}); // answered after every earlier frame on this socket
  await server.automation.settled();
}

function firedTriggers(): unknown[] {
  return automationRows()
    .filter((r) => r.event === "automation.action" && r.outcome === "ok")
    .map((r) => r.detail?.["trigger"]);
}

test("mediaStart fires when a take puts its item on air (B3: control-plane-side)", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([markerRule("start-a2", { kind: "mediaStart", params: { itemRef: "A2" } })]);
  assert.equal((await send(ws, "view.cut", { itemRef: "A1" })).status, "ok");
  nextFrame();
  assert.equal((await send(ws, "preview.set", { itemRef: "A2" })).status, "ok");
  assert.equal((await send(ws, "view.take", {})).status, "ok");
  await server.automation.settled();
  assert.deepEqual(firedTriggers(), [{ kind: "mediaStart", itemRef: "A2" }], "only A2's start, once");
  ws.close();
});

test("mediaEnd fires on the engine's end of a PLAYING item — and not for a stopped one", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([markerRule("end-at", { kind: "mediaEnd", params: { itemRef: "AT" } })]);
  const render = await renderSession();
  const end = JSON.stringify({ v: "0.3", kind: "itemEvent", itemRef: "AT", event: "end" });

  // Stopped, then the engine's late end: PLAYING -> READY -> (end dropped).
  assert.equal((await send(ws, "view.cut", { itemRef: "AT" })).status, "ok");
  assert.equal(state.itemStates.get("AT"), "PLAYING");
  assert.equal((await send(ws, "item.stop", { itemId: "AT" })).status, "ok");
  render.send(end);
  await flushed(render);
  assert.deepEqual(firedTriggers(), [], "a stop is not a completion (§13.4.1's item.stop row)");

  // Taken again, and completed: PLAYING -> DONE is mediaEnd.
  nextFrame();
  assert.equal((await send(ws, "view.cut", { itemRef: "AT" })).status, "ok");
  render.send(end);
  await flushed(render);
  assert.equal(state.itemStates.get("AT"), "DONE");
  assert.deepEqual(firedTriggers(), [{ kind: "mediaEnd", itemRef: "AT" }]);
  render.close();
  ws.close();
});

test("timer fires atMs of show clock after show.start", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([markerRule("t50", { kind: "timer", params: { atMs: 50 } })], false);
  await new Promise((r) => setTimeout(r, 120));
  assert.deepEqual(firedTriggers(), [], "the show clock has not started: no timer");
  assert.equal((await send(ws, "show.start", {})).status, "ok");
  const deadline = Date.now() + 3000;
  while (firedTriggers().length === 0 && Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 20));
    await server.automation.settled();
  }
  assert.deepEqual(firedTriggers(), [{ kind: "timer", ruleId: "t50" }]);
  ws.close();
});

test("timeOfDay fires when the local wall clock reaches `at`", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const at = new Date(Date.now() + 2000);
  const hhmmss = [at.getHours(), at.getMinutes(), at.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
  const ws = await loadAndStart([markerRule("tod", { kind: "timeOfDay", params: { at: hhmmss } })]);
  const deadline = Date.now() + 5000;
  while (firedTriggers().length === 0 && Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 50));
    await server.automation.settled();
  }
  assert.deepEqual(firedTriggers(), [{ kind: "timeOfDay", ruleId: "tod" }], `at ${hhmmss}`);
  ws.close();
});

test("hotkey fires when its binding fires, whatever the carried command's outcome", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([markerRule("hk", { kind: "hotkey", params: { bindingId: "take-key" } })]);
  // The binding's own command is refused (no preview armed) — the key was
  // still pressed.
  const r = await send(ws, "view.take", {}, { intentSource: "keyboard/desk:take-key" });
  assert.equal(r.status, "error");
  await server.automation.settled();
  assert.deepEqual(firedTriggers(), [{ kind: "hotkey", bindingId: "take-key" }]);
  // Another binding does not fire this rule.
  nextFrame();
  await send(ws, "view.take", {}, { intentSource: "companion/xl:cmp-1" });
  await server.automation.settled();
  assert.equal(firedTriggers().length, 1);
  ws.close();
});

test("streamHealth fires on a change TO its transport state; the first observation is a baseline", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([
    markerRule("sh-live", { kind: "streamHealth", params: { state: "live" } }),
    markerRule("sh-redial", { kind: "streamHealth", params: { state: "reconnecting" } }),
  ]);
  const render = await renderSession();
  for (const [token, expect] of [
    ["live", []], // baseline: the control plane has observed nothing before
    ["live", []], // no change
    ["reconnecting", [{ kind: "streamHealth", state: "reconnecting", from: "live" }]], // the redial
    ["live", [{ kind: "streamHealth", state: "live", from: "reconnecting" }]],
    ["none", []], // a stub, never a state
  ] as const) {
    const before = firedTriggers().length;
    nextFrame();
    render.send(engineTelemetry({ streamTransportState: token }));
    await flushed(render);
    assert.deepEqual(firedTriggers().slice(before), expect, `after ${token}`);
  }
  render.close();
  ws.close();
});

test("audioLevel fires on the engine's matching crossing (v0.4.7) and not on another", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([
    markerRule("hot-mic", { kind: "audioLevel", params: { bus: "mic", thresholdDbfs: -12 } }),
  ]);
  const render = await renderSession();
  const crossing = (threshold: number, direction: string) =>
    JSON.stringify({ v: "0.3", kind: "audioLevelCrossing", ts: 1768000000000.5, bus: "mic", thresholdDbfs: threshold, direction, levelDbfs: -3, masterFrame: 900 });
  render.send(crossing(-12, "falling")); // the other direction
  render.send(crossing(-20, "rising")); // another threshold
  render.send(crossing(-12, "rising"));
  await flushed(render);
  assert.deepEqual(firedTriggers(), [
    { kind: "audioLevel", bus: "mic", thresholdDbfs: -12, direction: "rising", levelDbfs: -3, masterFrame: 900, ts: 1768000000000.5 },
  ]);
  render.close();
  ws.close();
});

// ---------------------------------------------------------------------------
// WU4 — autoFollow: advance on media end, through the same queue, held by hold
// ---------------------------------------------------------------------------

const END = (itemRef: string) => JSON.stringify({ v: "0.3", kind: "itemEvent", itemRef, event: "end" });

function followRows(): AuditRecord[] {
  return automationRows().filter((r) => (r.event ?? "").startsWith("autoFollow."));
}

test("autoFollow advances to the next rundown item when its item completes", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([]);
  const render = await renderSession();
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  render.send(END("AF"));
  await flushed(render);
  assert.equal(state.viewItem, "AN", "the next item is on air");
  const rows = followRows();
  assert.equal(rows.length, 1, JSON.stringify(rows));
  assert.equal(rows[0]!.event, "autoFollow.advance");
  assert.equal(rows[0]!.outcome, "ok");
  assert.equal(rows[0]!.actor, "autoFollow:AF");
  assert.equal(rows[0]!.command, "view.cut");
  render.close();
  ws.close();
});

test("autoFollow does not advance for an item without it, or for a stopped item", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([]);
  const render = await renderSession();
  // AT carries no autoFollow: it completes, and the View stays on it.
  assert.equal((await send(ws, "view.cut", { itemRef: "AT" })).status, "ok");
  render.send(END("AT"));
  await flushed(render);
  assert.equal(state.viewItem, "AT");
  // AF stopped, then the engine's late end: no completion, no advance.
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  assert.equal((await send(ws, "item.stop", { itemId: "AF" })).status, "ok");
  render.send(END("AF"));
  await flushed(render);
  assert.equal(state.viewItem, "AF");
  assert.deepEqual(followRows(), [], "no advance, nothing audited");
  render.close();
  ws.close();
});

test("a hold suppresses autoFollow (§13.5 #2), audited", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([]);
  const render = await renderSession();
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  assert.equal((await send(ws, "automation.hold", { hold: true })).status, "ok");
  render.send(END("AF"));
  await flushed(render);
  assert.equal(state.viewItem, "AF", "held: the View does not advance");
  const rows = followRows();
  assert.deepEqual(rows.map((r) => r.event), ["autoFollow.suppressedByHold"]);
  assert.equal(rows[0]!.actor, "autoFollow:AF");
  render.close();
  ws.close();
});

test("a pending autoFollow is cancelled by a hold that lands before it dispatches (B5)", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // A rule on AF's end engages the hold; it is queued ahead of the advance,
  // so the advance is PENDING when the hold lands.
  const ws = await loadAndStart([
    {
      id: "holder",
      trigger: { kind: "mediaEnd", params: { itemRef: "AF" } },
      action: { command: "automation.hold", payload: { hold: true } },
    },
  ]);
  const render = await renderSession();
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  render.send(END("AF"));
  await flushed(render);
  assert.equal(state.viewItem, "AF", "the advance never dispatched");
  const cancelled = automationRows().filter((r) => r.event === "automation.cancelledByHold");
  assert.deepEqual(cancelled.map((r) => r.actor), ["autoFollow:AF"]);
  render.close();
  ws.close();
});

// ~~"autoFollow on the last rundown item goes nowhere, and says so"~~ — WU4's
// first landing audited `autoFollow.endOfRundown`. The user's word
// (2026-09-27): a completion with no next item is a no-op that audits
// nothing, because nothing was attempted (§2c).
test("autoFollow on the last rundown item is a no-op that audits nothing", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([]);
  const render = await renderSession();
  assert.equal((await send(ws, "view.cut", { itemRef: "AZ" })).status, "ok");
  render.send(END("AZ"));
  await flushed(render);
  assert.equal(state.viewItem, "AZ");
  assert.equal(state.itemStates.get("AZ"), "DONE", "the item did complete");
  assert.deepEqual(automationRows(), [], "nothing attempted, nothing audited");
  render.close();
  ws.close();
});

test("autoFollow advances once per completion, never twice, and the limiter counts it", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  const ws = await loadAndStart([]);
  const render = await renderSession();
  // A duplicate end for one completion: one advance.
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  render.send(END("AF"));
  render.send(END("AF"));
  await flushed(render);
  assert.equal(followRows().filter((r) => r.event === "autoFollow.advance").length, 1, "one advance per completion");
  // Taken and completed again in the SAME frame (the clock is frozen): the
  // once-per-frame limiter counts autoFollow like a rule's action.
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  render.send(END("AF"));
  await flushed(render);
  const rows = automationRows();
  assert.equal(rows.filter((r) => r.event === "autoFollow.advance").length, 1, "no second advance in the frame");
  const limited = rows.filter((r) => r.event === "automation.rateLimited");
  assert.deepEqual(limited.map((r) => r.actor), ["autoFollow:AF"], "the second is audited as rate-limited");
  assert.equal(state.viewItem, "AF", "the limited advance never ran");
  // The next frame, it advances again.
  nextFrame();
  assert.equal((await send(ws, "view.cut", { itemRef: "AF" })).status, "ok");
  render.send(END("AF"));
  await flushed(render);
  assert.equal(automationRows().filter((r) => r.event === "autoFollow.advance").length, 2);
  assert.equal(state.viewItem, "AN");
  render.close();
  ws.close();
});

// ---------------------------------------------------------------------------
// WU5 — self-triggers: the runtime's suppression, and §13.4.1 row by row
// ---------------------------------------------------------------------------

/** §13.4.1 as data — the same file preflight's cycle check reads. */
const EFFECTS = JSON.parse(
  readFileSync(new URL("../../../crates/nbe-core/src/automation_effects.json", import.meta.url), "utf8"),
) as { commands: Record<string, { stateChange?: string[]; mediaStart?: boolean; deferred?: string[]; item?: string }> };

test("a rule's action carries its chain into the triggers it raises in the same dispatch", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // r1's action changes previewItem, which r2 listens for. r2's firing names
  // r1 in its chain — the fact the runtime's self-trigger suppression reads.
  const ws = await loadAndStart([
    {
      id: "r1",
      trigger: { kind: "stateChange", params: { field: "showState", to: "RUNNING" } },
      action: { command: "preview.set", payload: { itemRef: "A1" } },
    },
    markerRule("r2", { kind: "stateChange", params: { field: "previewItem" } }),
  ]);
  const rows = automationRows().filter((r) => r.event === "automation.action");
  assert.deepEqual(
    rows.map((r) => [r.actor, r.detail?.["chain"]]),
    [
      ["automation:r1", []],
      ["automation:r2", ["r1"]],
    ],
  );
  ws.close();
});

test("the runtime suppresses a rule its own action re-triggered (§13.4), audited", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // No manifest preflight admits can do this through the command path: the
  // static check refuses every cycle §13.4.1's data shows, and the data
  // over-approximates. The suppression exists for an edge the data lacks, so
  // it is reached here the way such an edge would reach it — an event raised
  // with the rule in its chain, through the evaluator's one entry — and
  // asserted on the audit log.
  const ws = await loadAndStart([markerRule("self", { kind: "stateChange", params: { field: "previewItem" } })]);
  const before = automationRows().length;
  const event = { kind: "stateChange", field: "previewItem", from: null, to: "A1" } as const;
  nextFrame();
  server.automation.fire(event, { chain: ["self"] });
  await server.automation.settled();
  let rows = automationRows().slice(before);
  assert.deepEqual(
    rows.map((r) => [r.event, r.actor, r.detail?.["chain"]]),
    [["automation.suppressedSelfTrigger", "automation:self", ["self"]]],
  );
  assert.equal(state.markers.length, 0, "nothing dispatched");
  // Another rule's chain is not this rule's: it fires.
  nextFrame();
  server.automation.fire(event, { chain: ["other"] });
  await server.automation.settled();
  rows = automationRows().slice(before);
  assert.deepEqual(
    rows.map((r) => r.event),
    ["automation.suppressedSelfTrigger", "automation.action"],
  );
  ws.close();
});

test("§13.4.1, row by row: every command raises only the same-dispatch triggers its row names", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // Watchers on every field a stateChange rule can name, and on mediaStart.
  // Each command runs from a fresh admin session (§10.7's limiter is per
  // connection), a frame apart, and the watchers it raises — fired, held or
  // limited, all audited — must be cells its §13.4.1 row names. A trigger
  // raised and NOT named is a missing edge: the cycle check's unsafe side.
  const FIELDS = ["showState", "viewItem", "previewItem", "streamState", "recordState", "automationHold", "fallbackActive"];
  const ITEMS = ["A1", "A2", "AT"];
  const pkg = automationPackage([
    ...FIELDS.map((f) => markerRule(`w-${f}`, { kind: "stateChange", params: { field: f } })),
    ...ITEMS.map((i) => markerRule(`w-itemState-${i}`, { kind: "stateChange", params: { field: "itemState", itemRef: i } })),
    markerRule("w-mediaStart", { kind: "mediaStart" }),
    markerRule("idle", { kind: "hotkey", params: { bindingId: "take-key" } }), // for automation.enable/disable to name
  ], {
    // Templates, so the graphics, breaking and ticker rows have commands that
    // can be accepted (a ticker is declared by a ticker template, package.ts).
    templates: [
      { id: "TPL", kind: "generic" },
      { id: "BRK", kind: "breakingBanner" },
      { id: "TCK", kind: "ticker" },
    ],
  });
  const render = await renderSession();
  const cmd = async (command: string, payload: Record<string, unknown>): Promise<Record<string, unknown>> => {
    nextFrame();
    const c = conn("admin", ADMIN);
    await connect(c);
    const r = await send(c, command, payload);
    c.close();
    await server.automation.settled();
    return r;
  };
  const setup = async (command: string, payload: Record<string, unknown> = {}): Promise<void> => {
    const r = await cmd(command, payload);
    assert.equal(r.status, "ok", `setup ${command}: ${JSON.stringify(r)}`);
  };
  const WATCHED = new Set(["automation.action", "automation.suppressedByHold", "automation.rateLimited", "automation.suppressedSelfTrigger"]);
  const raised = (from: number): Set<string> => {
    const out = new Set<string>();
    for (const r of automationRows().slice(from)) {
      const trig = r.detail?.["trigger"] as { kind: string; field?: string } | undefined;
      if (!WATCHED.has(r.event ?? "") || !r.actor?.startsWith("automation:w-") || !trig) continue;
      out.add(trig.kind === "stateChange" ? `stateChange:${trig.field}` : trig.kind);
    }
    return out;
  };
  const declared = (command: string): Set<string> => {
    const e = EFFECTS.commands[command];
    assert.ok(e, `${command} has a §13.4.1 row`);
    return new Set([...(e.stateChange ?? []).map((f) => `stateChange:${f}`), ...(e.mediaStart ? ["mediaStart"] : [])]);
  };
  const seen = new Map<string, Set<string>>();
  const accepted = new Map<string, boolean>();
  const unnamed: string[] = [];
  const probe = async (command: string, payload: Record<string, unknown> = {}): Promise<void> => {
    const from = automationRows().length;
    const r = await cmd(command, payload);
    accepted.set(command, (accepted.get(command) ?? false) || r.status === "ok");
    const got = raised(from);
    const want = declared(command);
    for (const k of got) if (!want.has(k)) unnamed.push(`${command} ${JSON.stringify(payload)} raised ${k}`);
    const u = seen.get(command) ?? new Set<string>();
    for (const k of got) u.add(k);
    seen.set(command, u);
  };

  await probe("show.load", { packagePath: pkg });
  await probe("show.preflight", {});
  await probe("show.start", {});
  await probe("preview.set", { itemRef: "A1" });
  await setup("view.fallback");
  await setup("preview.set", { itemRef: "A2" });
  await probe("view.take", {});
  await setup("view.fallback");
  await setup("preview.set", { itemRef: "A1" });
  await probe("view.cut", { itemRef: "A1" });
  await setup("view.cut", { itemRef: "AT" });
  await probe("item.stop", { itemId: "AT" });
  await probe("item.arm", { itemId: "A2" });
  await probe("item.unarm", { itemId: "A2" });
  await setup("view.cut", { itemRef: "AT" });
  render.send(END("AT"));
  await flushed(render);
  await probe("item.reset", { itemId: "AT" });
  await setup("view.cut", { itemRef: "A1" });
  await probe("view.fallback", {});
  await probe("automation.hold", { hold: true });
  await probe("automation.hold", { hold: false });
  // A held snapshot with A2 on air and A1 in preview, recalled over A1 on air.
  await setup("view.cut", { itemRef: "A2" });
  await setup("preview.set", { itemRef: "A1" });
  await setup("automation.hold", { hold: true });
  await setup("snapshot.save", { name: "held" });
  await setup("automation.hold", { hold: false });
  await setup("view.cut", { itemRef: "A1" });
  await probe("snapshot.recall", { name: "held" });
  await setup("automation.hold", { hold: false });
  await probe("record.start", {});
  await probe("record.stop", {});
  await probe("stream.start", {});
  await probe("stream.stop", {});
  // `scene.arm` writes `previewItem` only when the preview is empty
  // (`armScene`) — the path the two-key pass of 2026-09-29 found this probe
  // never took (A1 was still in preview). Empty it first: a cut of the
  // previewed item clears the preview.
  await setup("view.cut", { itemRef: "A1" });
  assert.equal(state.previewItem, null, "precondition: the preview is empty for scene.arm");
  // The rows whose cells change nothing a stateChange rule can name — and
  // `scene.arm`, first, whose one cell is conditional.
  for (const [command, payload] of [
    ["scene.arm", { sceneId: "SCN" }],
    ["scene.apply", { sceneId: "SCN", target: "preview" }],
    ["element.toggle", { elementId: "main", visible: false }],
    ["element.set", { elementId: "main", patch: {} }],
    ["graphic.show", { templateId: "TPL", fields: {} }],
    ["graphic.hide", {}],
    ["graphic.update", { elementId: "main", fields: {} }],
    ["breaking.show", { headline: "h" }],
    ["breaking.hide", {}],
    ["overlay.show", { overlayId: "o" }],
    ["overlay.hide", { overlayId: "o" }],
    ["clock.configure", { elementId: "main" }],
    ["ticker.setSource", { source: "manual" }],
    ["ticker.override", { items: [] }],
    ["ticker.clearOverride", {}],
    ["ticker.refreshRss", {}],
    ["soundboard.play", { assetId: "a_img" }],
    ["soundboard.stop", {}],
    ["soundboard.stopAll", {}],
    ["audio.bus.set", { bus: "music", gainDb: -6 }],
    ["audio.duck", { bus: "music", enabled: true }],
    ["guest.connect", { guestId: "g1", whipUrl: "https://guest.invalid/whip" }],
    ["guest.mute", { guestId: "g1", muted: true }],
    ["guest.setLayout", { guestId: "g1", layout: "pip" }],
    ["guest.placeholder", { guestId: "g1" }],
    ["guest.configureReturn", { guestId: "g1" }],
    ["guest.getTurn", { guestId: "g1" }],
    ["guest.disconnect", { guestId: "g1" }],
    ["automation.disable", { ruleId: "idle" }],
    ["automation.enable", { ruleId: "idle" }],
    ["snapshot.save", { name: "plain" }],
    ["marker.add", { name: "m" }],
    ["plugin.reload", { pluginId: "p" }],
    ["system.status", {}],
    ["system.telemetry.subscribe", {}],
    ["system.telemetry.unsubscribe", {}],
  ] as const) {
    await probe(command, payload as Record<string, unknown>);
  }
  // `show.stop` sets `streamState` and `recordState` idle unconditionally
  // (commands/show.ts); they change only if an output was active — the path
  // the pass found this probe never took. Start both first.
  await setup("record.start", {});
  await setup("stream.start", {});
  await probe("show.stop", {});
  await probe("show.unload", {});

  const refused = [...accepted].filter(([, ok]) => !ok).map(([c]) => c);
  // One command per §13.4.1 row (19 rows) must be ACCEPTED, or its row is
  // checked against nothing: a refused command raises nothing and passes.
  const ONE_PER_ROW = [
    "view.take", "item.stop", "show.start", "show.stop", "show.load", "preview.set", "scene.arm",
    "snapshot.recall", "view.fallback", "stream.start", "record.start", "soundboard.play", "element.toggle",
    "ticker.setSource", "ticker.refreshRss", "guest.connect", "automation.hold", "marker.add", "system.status",
  ];
  assert.deepEqual(ONE_PER_ROW.filter((c) => !accepted.get(c)), [], "a row with no accepted command is unchecked");
  console.log(`ROW CHECK: ${accepted.size} commands probed, ${accepted.size - refused.length} accepted; refused by their own preconditions: ${refused.join(", ") || "none"}`);
  assert.deepEqual(unnamed, [], "triggers raised that the command's §13.4.1 row does not name — missing edges");
  assert.deepEqual(
    Object.keys(EFFECTS.commands).filter((c) => !accepted.has(c)),
    [],
    "every §13.4.1 row's commands are probed",
  );
  // Each named same-dispatch cell is demonstrated, not only allowed — except
  // where this package cannot reach it, which is stated.
  const NOT_REACHED: Record<string, string[]> = {
    // Loaded from UNLOADED, nothing is on air to clear. A load over a loaded
    // show clears them (state.ts loadPackage), and the new rules see it: the
    // evaluator loads them before the diff (server.ts afterAccepted).
    "show.load": ["stateChange:viewItem", "stateChange:previewItem", "stateChange:fallbackActive", "stateChange:itemState"],
  };
  const undemonstrated: string[] = [];
  for (const [command] of Object.entries(EFFECTS.commands)) {
    const want = declared(command);
    if (want.size === 0) continue;
    assert.ok(accepted.get(command), `${command}, whose row names triggers, must be accepted to be checked`);
    for (const k of want) {
      if (!seen.get(command)?.has(k) && !(NOT_REACHED[command] ?? []).includes(k)) undemonstrated.push(`${command}: ${k}`);
    }
  }
  assert.deepEqual(undemonstrated, [], "cells a row names that no probe demonstrated");
  render.close();
});

// ---------------------------------------------------------------------------
// §13.3 (v0.4.7): a rule's actions are exempt from §10.7's command limiter
// ---------------------------------------------------------------------------

test("a rule firing on every frame, past 5 actions a second, is not throttled by the operator limiter (§13.3)", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // §10.7's limiter admits 10 per burst and refills 5/s, per connection per
  // command family. A rule dispatches on its own connection, so before the
  // exemption its 11th action in a burst was refused E_RATE_LIMITED — below
  // once per frame. The triggers are engine frames (audioLevelCrossing), which
  // no limiter sees, one per frame: only the rule's own actions could be
  // throttled.
  const ws = await loadAndStart([markerRule("fast", { kind: "audioLevel", params: { bus: "mic", thresholdDbfs: -12 } })]);
  const render = await renderSession();
  const N = 30;
  const started = performance.now();
  for (let i = 0; i < N; i++) {
    nextFrame(); // a new frame each time: §13.3 #3 admits every one
    render.send(
      JSON.stringify({ v: "0.3", kind: "audioLevelCrossing", ts: Date.now(), bus: "mic", thresholdDbfs: -12, direction: "rising", levelDbfs: -3, masterFrame: i }),
    );
    await flushed(render);
  }
  const seconds = (performance.now() - started) / 1000;
  // The check discriminates only if the operator limiter WOULD have refused:
  // it admits at most 10 + 5·seconds over the run.
  assert.ok(10 + 5 * seconds < N, `ran ${seconds.toFixed(2)} s: too slow for the limiter to have refused any of ${N}`);
  const rows = automationRows().filter((r) => r.event === "automation.action" && r.actor === "automation:fast");
  assert.equal(rows.length, N, "every firing dispatched");
  assert.deepEqual(
    rows.filter((r) => r.outcome !== "ok").map((r) => r.errorCode ?? r.outcome),
    [],
    "no action refused — in particular none E_RATE_LIMITED",
  );
  assert.equal(state.markers.filter((m) => m.name === "by-fast").length, N, "every action reached state");
  render.close();
  ws.close();
});

// ---------------------------------------------------------------------------
// The Prompt 13 re-plan's P1, the recall leg (SPEC v0.4.8 row 1): the engine
// applies `snapshot.recall`. Its engine half is `prompt13_recall.rs`; the two
// together, against the real binary, `recall.e2e.ts`.
// ---------------------------------------------------------------------------

/** Every directive a render session receives, in order (show.resync first). */
function directivesOn(render: WebSocket): Array<Record<string, unknown>> {
  const seen: Array<Record<string, unknown>> = [];
  render.on("message", (buf: Buffer) => {
    const frame = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (frame["kind"] === "directive") seen.push(frame);
  });
  return seen;
}

test("snapshot.recall reaches the engine resolved: the recalled item as a cut, the overlays wholesale, never the name", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // Before the fix the control plane forwarded the recall's payload — the
  // snapshot's NAME — and the engine routed nothing for it. §5.9.1: the
  // engine never resolves, so the directive carries the resolved View: the
  // take's resolution of a cut (`resolveTransition`), `target.itemRef` only
  // when the recalled item is not the one on air (null for an empty View),
  // and the snapshot's overlays wholesale.
  const ws = await loadAndStart([]);
  const render = await renderSession();
  const directives = directivesOn(render);
  const recalls = () => directives.filter((d) => d["command"] === "snapshot.recall");
  const resolved = { transition: "cut", audio: { transition: "follow" }, visibleOverlays: [] };

  render.send(engineTelemetry({ masterClockFrame: 10 }));
  await flushed(render);
  assert.equal((await send(ws, "snapshot.save", { name: "empty" })).status, "ok");
  assert.equal((await send(ws, "view.cut", { itemRef: "A1" })).status, "ok");
  assert.equal((await send(ws, "snapshot.save", { name: "a1" })).status, "ok");
  assert.equal((await send(ws, "view.cut", { itemRef: "A2" })).status, "ok");
  render.send(engineTelemetry({ masterClockFrame: 500 }));
  await flushed(render);
  // The operator's slate is up when the recall lands.
  assert.equal((await send(ws, "view.fallback", {})).status, "ok");

  const r1 = await send(ws, "snapshot.recall", { name: "a1" });
  assert.equal(r1.status, "ok");
  await flushed(render);
  assert.deepEqual(
    recalls().map((d) => ({ stateVersion: d["stateVersion"], target: d["target"], payload: d["payload"] })),
    [{ stateVersion: r1["stateVersion"], target: { itemRef: "A1" }, payload: resolved }],
    "A2 → A1: the recalled item, as the take's resolved cut, at the recall's stateVersion",
  );
  assert.equal(state.viewItem, "A1");
  assert.equal(state.viewItemStartFrame, 500, "the recalled item starts now, as a take's does");
  // Release parity, the control plane's half: a recall does not clear the
  // fallback flag, so the engine must not release the slate either
  // (prompt13_recall.rs pins the engine's half).
  assert.equal(state.fallbackActive, true, "a recall is not a clear site");

  // The recalled item is already on air: nothing on the View moves.
  assert.equal((await send(ws, "snapshot.recall", { name: "a1" })).status, "ok");
  // A snapshot saved before anything was taken: the View empties.
  assert.equal((await send(ws, "snapshot.recall", { name: "empty" })).status, "ok");
  await flushed(render);
  assert.deepEqual(
    recalls().slice(1).map((d) => d["target"]),
    [{}, { itemRef: null }],
    "already on air: no itemRef; an empty snapshot: itemRef null",
  );
  assert.equal(state.viewItem, null);
  assert.equal(state.viewItemStartFrame, null);
  render.close();
  ws.close();
});

test("a recall is not a cut for autoFollow: the rundown goes on from the snapshot's position", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // autoFollow keeps no pointer: it advances from the item that ENDS
  // (server.ts, itemEvent → `order[indexOf(ended) + 1]`), and only through a
  // PLAYING → DONE completion. So the snapshot's position holds when the
  // recall restores the item states with the View: the item it took off air
  // cannot complete and advance past the snapshot — the engine drops that
  // item's pending end (prompt13_recall.rs), and here an end that arrived
  // anyway finds it no longer PLAYING. The recall itself fires neither
  // autoFollow nor mediaStart (B3: it is not a take).
  const ws = conn("admin", ADMIN);
  await connect(ws);
  const pkg = automationPackage([markerRule("w-start", { kind: "mediaStart" })], {
    rundown: {
      id: "R",
      items: [
        { id: "P1", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
        { id: "P2", kind: "sceneRef", sceneRef: "SCN" },
        { id: "Q1", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
        { id: "Q2", kind: "sceneRef", sceneRef: "SCN" },
      ],
    },
  });
  assert.equal((await send(ws, "show.load", { packagePath: pkg })).status, "ok");
  assert.equal((await send(ws, "show.start", {})).status, "ok");
  const render = await renderSession();

  assert.equal((await send(ws, "view.cut", { itemRef: "P1" })).status, "ok");
  assert.equal((await send(ws, "snapshot.save", { name: "at-p1" })).status, "ok");
  nextFrame();
  assert.equal((await send(ws, "view.cut", { itemRef: "Q1" })).status, "ok");
  nextFrame();
  assert.equal((await send(ws, "snapshot.recall", { name: "at-p1" })).status, "ok");
  await server.automation.settled();
  assert.equal(state.viewItem, "P1");
  assert.deepEqual(followRows(), [], "the recall fires no autoFollow");
  assert.deepEqual(
    firedTriggers(),
    [
      { kind: "mediaStart", itemRef: "P1" },
      { kind: "mediaStart", itemRef: "Q1" },
    ],
    "the two cuts started media; the recall did not",
  );

  // The item the recall took off air ends anyway: no advance to Q2.
  nextFrame();
  render.send(END("Q1"));
  await flushed(render);
  assert.equal(state.viewItem, "P1", "Q1's late end cannot advance the rundown past the snapshot");
  assert.deepEqual(followRows(), []);

  // The recalled item completes: the rundown goes on from P1, to P2.
  nextFrame();
  render.send(END("P1"));
  await flushed(render);
  assert.equal(state.viewItem, "P2");
  assert.deepEqual(
    followRows().map((r) => [r.event, r.actor]),
    [["autoFollow.advance", "autoFollow:P1"]],
    "one advance, by P1's completion",
  );
  render.close();
  ws.close();
});

test("a take directive carries the ITEM's duration for a timed item, apart from the transition's (SPEC v0.4.8 row 2)", async (t) => {
  if (!preflightAvailable()) return t.skip("nbe-preflight not built");
  // The engine schedules an item's end from `itemDurationFrames` alone. Before
  // v0.4.8 row 2 it read the transition's `durationFrames`, so a cut never
  // ended a timed item and a mix ended it at the mix's length. The control
  // plane resolves the item's duration from the package (§5.9.1): present for
  // a timed item, absent for an untimed one, on view.cut, view.take and
  // snapshot.recall alike.
  const ws = await loadAndStart([]);
  const render = await renderSession();
  const directives = directivesOn(render);
  const takes = () =>
    directives
      .filter((d) => d["command"] === "view.take" || d["command"] === "snapshot.recall")
      .map((d) => ({ command: d["command"], target: d["target"], payload: d["payload"] }));

  // AT is timed (60 frames); A1 is not.
  assert.equal((await send(ws, "view.cut", { itemRef: "AT" })).status, "ok");
  assert.equal((await send(ws, "snapshot.save", { name: "on-at" })).status, "ok");
  assert.equal((await send(ws, "preview.set", { itemRef: "A1" })).status, "ok");
  assert.equal((await send(ws, "view.take", { transition: "mix", durationFrames: 15 })).status, "ok");
  assert.equal((await send(ws, "snapshot.recall", { name: "on-at" })).status, "ok");
  await flushed(render);
  assert.deepEqual(takes(), [
    { command: "view.take", target: { itemRef: "AT" }, payload: { transition: "cut", itemDurationFrames: 60 } },
    {
      command: "view.take",
      target: { itemRef: "A1" },
      // No item duration: A1 is untimed. The mix's 15 frames are the transition's.
      payload: { transition: "mix", durationFrames: 15, audio: { transition: "follow", durationFrames: 15 } },
    },
    {
      command: "snapshot.recall",
      target: { itemRef: "AT" },
      payload: { transition: "cut", audio: { transition: "follow" }, itemDurationFrames: 60, visibleOverlays: [] },
    },
  ]);
  render.close();
  ws.close();
});
