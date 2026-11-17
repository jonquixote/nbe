//! Review4 step 5, "disk-full does not stop control", as the user ruled on
//! 2026-11-16 23:45: a failed audit or journal write does not reject an
//! on-air command (take, cut, stop, record, stream, guest). It raises the
//! existing `audit.sinkFailed` alert and the command proceeds. Other commands
//! keep R18's behaviour, and an auth is refused while the audit cannot be
//! written. Rows with no command to refuse (an engine item event, a belief
//! correction, a resync, a rule's bookkeeping) never throw.
//!
//! The audit file is made read-only (chmod 444), and the journal's directory
//! read-only (chmod 555), standing in for a full disk: both fail the write.

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog, ON_AIR_COMMANDS } from "./audit.js";
import { StatePersistence } from "./persistence.js";
import { CommandNames } from "./protocol.js";
import { MockRenderBridge } from "./render-bridge.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState, type PackageInfo } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";

function pkg(): PackageInfo {
  return {
    packagePath: "/tmp/none",
    showId: "s",
    houseRate: 30,
    qualityProfile: undefined,
    items: new Map([
      ["A1", { id: "A1", kind: "sceneRef", durationFrames: 60, autoFollow: true }],
      ["A2", { id: "A2", kind: "sceneRef", durationFrames: 60 }],
    ]),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(),
    templates: new Set(),
    breakingTemplates: new Set(),
    tickerExists: false,
    clockElements: new Set(),
    plugins: new Set(),
    audioAssets: new Set(),
    cameras: new Set(),
    guests: new Set(),
    fallbackAssetId: undefined,
    automationRules: [],
    automation: [],
    bindings: new Map(),
  } as unknown as PackageInfo;
}

interface Rig {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  audit: AuditLog;
  auditPath: string;
  conn: (role: string, token: string) => WebSocket;
  status: () => Promise<Record<string, unknown>>;
}

async function rig(
  t: { after: (fn: () => void | Promise<void>) => void },
  opts: { persistence?: StatePersistence; bridge?: MockRenderBridge; state?: ControlPlaneState } = {},
): Promise<Rig> {
  const dir = tempDir("nbe-disk-full-");
  const auditPath = join(dir, "audit.jsonl");
  writeFileSync(auditPath, "");
  t.after(() => chmodSync(auditPath, 0o644));
  const audit = new AuditLog(auditPath);
  const state = opts.state ?? new ControlPlaneState();
  state.loadPackage(pkg());
  state.showState = "RUNNING";
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit,
    state,
    persistence: opts.persistence ?? { onDirty: () => {}, flushNow: () => {} },
    ...(opts.bridge ? { bridge: opts.bridge } : {}),
  });
  t.after(() => server.close());
  const conn = (role: string, token: string): WebSocket => {
    const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
    });
    t.after(() => ws.close());
    return ws;
  };
  const status = async () =>
    (await (await fetch(`http://127.0.0.1:${server.port}/nbe/v0.3/status`)).json()) as Record<string, unknown>;
  return { server, state, audit, auditPath, conn, status };
}

const open = (ws: WebSocket): Promise<void> =>
  new Promise((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });

/** One command; resolves with the frame carrying its own requestId, or
 * null when none arrives within `ms` (R18's reply goes under a fresh id). */
function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}, ms = 5000): Promise<Record<string, unknown> | null> {
  const id = randomUUID();
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      ws.off("message", onMsg);
      resolve(null);
    }, ms);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      clearTimeout(timer);
      ws.off("message", onMsg);
      resolve(msg);
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function until(check: () => boolean, ms = 3000): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 10));
  }
  return check();
}

const alertIds = (s: ControlPlaneState): string[] => s.alerts.map((a) => a.id);

test("the on-air set names real commands, and only the ruling's families", () => {
  const known = new Set<string>(CommandNames);
  for (const c of ON_AIR_COMMANDS) assert.ok(known.has(c), `${c} is a registered command`);
  const families = new Set([...ON_AIR_COMMANDS].map((c) => (c.startsWith("guest.") ? "guest" : c)));
  assert.deepEqual(
    [...families].sort(),
    ["guest", "item.stop", "record.start", "record.stop", "show.stop", "stream.start", "stream.stop", "view.cut", "view.take"],
  );
  const guests = CommandNames.filter((c) => c.startsWith("guest."));
  assert.deepEqual(guests.filter((c) => !ON_AIR_COMMANDS.has(c)), [], "every guest command");
});

test("with the audit file unwritable, view.take is accepted under its own id and audit.sinkFailed is raised", async (t) => {
  const r = await rig(t, { bridge: new MockRenderBridge() });
  const admin = r.conn("admin", ADMIN);
  await open(admin);
  assert.equal((await send(admin, "preview.set", { itemRef: "A1" }))?.status, "ok", "armed while the sink works");
  chmodSync(r.auditPath, 0o444);
  const reply = await send(admin, "view.take");
  assert.equal(reply?.status, "ok", `the take is accepted: ${JSON.stringify(reply)}`);
  assert.equal(r.state.viewItem, "A1", "and it happened");
  assert.equal(r.audit.sinkOk, false, "its row failed, as arranged");
  assert.ok(await until(() => alertIds(r.state).includes("audit.sinkFailed")), "the alert raises");
  assert.deepEqual((await r.status()).health, { auditSinkOk: false });
});

test("with the audit file unwritable, a refused on-air command replies its own error under its own id", async (t) => {
  const r = await rig(t, { bridge: new MockRenderBridge() });
  const admin = r.conn("admin", ADMIN);
  await open(admin);
  chmodSync(r.auditPath, 0o444);
  const reply = await send(admin, "view.take"); // nothing armed
  assert.equal(reply?.status, "error");
  assert.equal((reply?.error as { code?: string } | undefined)?.code, "E_FORBIDDEN_STATE", JSON.stringify(reply));
});

test("a command outside the on-air set keeps R18's behaviour: no reply under its id, an E_ENGINE frame instead", async (t) => {
  const r = await rig(t, { bridge: new MockRenderBridge() });
  const admin = r.conn("admin", ADMIN);
  await open(admin);
  const frames: Array<Record<string, unknown>> = [];
  admin.on("message", (buf: Buffer) => frames.push(JSON.parse(buf.toString("utf8")) as Record<string, unknown>));
  chmodSync(r.auditPath, 0o444);
  assert.equal(await send(admin, "automation.hold", { hold: true }, 1000), null, "no reply under its own id");
  assert.ok(
    frames.some((f) => f.status === "error" && (f.error as { code?: string } | undefined)?.code === "E_ENGINE"),
    "R18's E_ENGINE frame, under a fresh id",
  );
  // Pinned as it is today, for the user's ruling: the command's change was
  // applied before its audit row failed (server.ts runCommand: dispatch,
  // then the row). "Refused" here means the reply, not the change.
  assert.equal(r.state.automationHold, true, "today: applied, then refused in its reply");
});

test("with the audit file unwritable, a new connection is refused with 503 E_DISK and the control plane lives; it is accepted once the file can be written", async (t) => {
  const r = await rig(t);
  chmodSync(r.auditPath, 0o444);
  const refused = r.conn("admin", ADMIN);
  // A timeout of its own: node:test catches an uncaught throw and blames the
  // running test, so without this a throw in the upgrade listener (which ends
  // a real control plane) would leave this wait hanging, not failing.
  const err = await Promise.race([
    new Promise<Error>((resolve) => refused.once("error", resolve)),
    new Promise<Error>((resolve) => setTimeout(() => resolve(new Error("no response within 3 s: the upgrade listener threw")), 3000)),
  ]);
  assert.match(err.message, /503/, err.message);
  assert.equal((await r.status()).ok, true, "the control plane still answers");
  assert.ok(await until(() => alertIds(r.state).includes("audit.sinkFailed")), "the alert raises");
  chmodSync(r.auditPath, 0o644);
  const ok = r.conn("admin", ADMIN);
  await open(ok);
  assert.ok(await until(() => !alertIds(r.state).includes("audit.sinkFailed")), "the alert clears");
});

test("with the audit file unwritable, an engine's item end still drives autoFollow (a throw would have skipped it)", async (t) => {
  const r = await rig(t);
  const engine = r.conn("render", RENDER);
  engine.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === "directive") engine.send(JSON.stringify({ v: "0.3", kind: "appliedStateVersion", stateVersion: msg.stateVersion }));
  });
  await open(engine);
  const admin = r.conn("admin", ADMIN);
  await open(admin);
  assert.equal((await send(admin, "preview.set", { itemRef: "A1" }))?.status, "ok");
  assert.equal((await send(admin, "view.take"))?.status, "ok");
  assert.equal(r.state.itemStateOf("A1"), "PLAYING");
  chmodSync(r.auditPath, 0o444);
  engine.send(JSON.stringify({ v: "0.3", kind: "itemEvent", itemRef: "A1", event: "end" }));
  assert.ok(await until(() => r.state.viewItem === "A2"), `autoFollow cut to A2 (view: ${r.state.viewItem})`);
  assert.equal(r.audit.sinkOk, false, "its rows failed, as arranged");
});

test("a journal that cannot be written raises audit.sinkFailed with the journal's reason; the take proceeds; the alert clears once a write succeeds", async (t) => {
  const dir = join(tempDir("nbe-disk-full-journal-"), "state");
  mkdirSync(dir);
  const state = new ControlPlaneState();
  const persistence = new StatePersistence(state, join(dir, "control-plane.json"));
  const r = await rig(t, { persistence, state, bridge: new MockRenderBridge() });
  t.after(() => chmodSync(dir, 0o755));
  const admin = r.conn("admin", ADMIN);
  await open(admin);
  assert.equal((await send(admin, "preview.set", { itemRef: "A1" }))?.status, "ok");
  chmodSync(dir, 0o555);
  const reply = await send(admin, "view.take");
  assert.equal(reply?.status, "ok", `the take is accepted: ${JSON.stringify(reply)}`);
  assert.equal(persistence.writeOk(), false, "its journal write failed, as arranged");
  assert.ok(await until(() => alertIds(r.state).includes("audit.sinkFailed")), "the alert raises");
  const alert = r.state.alerts.find((a) => a.id === "audit.sinkFailed")!;
  assert.match(alert.reason, /state journal cannot be written/);
  assert.deepEqual((await r.status()).health, { auditSinkOk: true }, "the health flag stays the audit sink's");
  chmodSync(dir, 0o755);
  assert.equal((await send(admin, "view.cut", { itemRef: "A2" }))?.status, "ok");
  assert.equal(persistence.writeOk(), true);
  assert.ok(await until(() => !alertIds(r.state).includes("audit.sinkFailed")), "the alert clears");
});

test("with the audit file unwritable, a belief correction inside the 250 ms signal timer lands, and the process lives (a throw there would end it)", async (t) => {
  const r = await rig(t);
  const engine = r.conn("render", RENDER);
  await open(engine);
  r.state.streamState = "live"; // a belief with no live claim behind it
  chmodSync(r.auditPath, 0o444);
  const report = () =>
    engine.send(
      JSON.stringify({
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
        streamTransportState: "none",
      }),
    );
  report();
  const timer = setInterval(report, 100);
  t.after(() => clearInterval(timer));
  assert.ok(await until(() => r.state.streamState === "idle"), `corrected to idle (belief: ${r.state.streamState})`);
  await new Promise((res) => setTimeout(res, 600)); // two more signal ticks, alive
  assert.equal((await r.status()).ok, true, "the control plane still answers");
  assert.equal(r.audit.sinkOk, false, "its row failed, as arranged");
});
