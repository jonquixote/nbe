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
const nextFrame = (): void => {
  clockMs += FRAME_MS;
};

beforeEach(async () => {
  clockMs = 0;
  state = new ControlPlaneState();
  auditPath = join(tempDir("nbe-auto-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [OPERATOR]: "operator" } },
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
      { id: "badcut", trigger: { kind: "mediaEnd" }, action: { command: "view.cut", payload: { itemRef: 5 } } },
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
  assert.equal(state.markers.length, 0, "no cancelled action reached state");
  assert.equal(state.automationHold, true);
  ws.close();
});
