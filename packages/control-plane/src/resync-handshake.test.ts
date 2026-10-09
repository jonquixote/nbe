//! The render handshake (SPEC §5.9.4): a connect either completes its resync or
//! leaves NO registration. There is no third state.
//!
//! PR #38's falsification F3 showed the third state. `server.ts` registered a
//! render session, sent `show.resync`, and only then installed the socket's
//! listeners. A throw while the snapshot was built (there, the strict parse of
//! an unclamped end) skipped them: the session stayed registered but was never
//! resynced, so §5.9.4 had the engine hold and apply nothing, and the control
//! plane never processed its frames or noticed it go. A zombie. A send that
//! failed was worse in its own way: `sendDirect` reports it as `false`, and the
//! handshake ignored the result.
//!
//! These force each failure on a real server and a real socket, and pin the
//! teardown: the connection closes with 1011 and a reason, the session holds
//! no registration, the failure is in the audit and the warn log, and the
//! engine's next connect resyncs cleanly. One injector per test, each standing
//! alone: a state whose snapshot throws, and a bridge whose send refuses.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { WsRenderBridge } from "./render-bridge.js";
import { RESYNC_COMMAND } from "./protocol.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";

/** A state whose snapshot throws `failNext` times: the build-failure injector. */
class FailingSnapshotState extends ControlPlaneState {
  failNext = 0;
  override resyncSnapshot(nowMs?: number): Record<string, unknown> {
    if (this.failNext > 0) {
      this.failNext -= 1;
      throw new Error("injected: the resync snapshot could not be built");
    }
    return super.resyncSnapshot(nowMs);
  }
}

let server: ControlPlaneServer;
let state: FailingSnapshotState;
let auditPath: string;
let warnings: string[];

beforeEach(async () => {
  state = new FailingSnapshotState();
  warnings = [];
  auditPath = join(tempDir("nbe-handshake-audit-"), "audit.jsonl");
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: (m) => warnings.push(m),
  });
});

afterEach(async () => {
  if (server) await server.close();
});

function conn(role: string, token: string): WebSocket {
  return new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
  });
}

/** Every directive a socket receives, from its first frame. */
function directivesOf(ws: WebSocket): Array<Record<string, unknown>> {
  const out: Array<Record<string, unknown>> = [];
  ws.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === "directive") out.push(msg);
  });
  return out;
}

/** How the socket closed, or null if it was still open at the deadline. */
function closeOf(ws: WebSocket, ms = 2000): Promise<{ code: number; reason: string } | null> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(null), ms);
    ws.once("close", (code: number, reason: Buffer) => {
      clearTimeout(timer);
      resolve({ code, reason: reason.toString("utf8") });
    });
  });
}

async function until(check: () => boolean, ms = 2000): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 10));
  }
  return check();
}

function handshakeFailures(): Array<Record<string, unknown>> {
  if (!existsSync(auditPath)) return [];
  return readFileSync(auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>)
    .filter((r) => r["event"] === "resync.handshakeFailed");
}

/** The failure's whole picture, as one value: a failure shows everything at once. */
async function afterFailedConnect(): Promise<Record<string, unknown>> {
  const render = conn("render", RENDER);
  const directives = directivesOf(render);
  const closed = closeOf(render);
  const close = await closed;
  // The registration is released by the time the socket closes, or it is a zombie.
  await until(() => server.wsBridge.renderNodeCount() === 0, 500);
  const registrations = server.wsBridge.renderNodeCount();
  // F3's worst finding: the control plane never noticed the engine go. If the
  // server left the socket open, the engine side closes it, and the
  // registration must still be released.
  if (close === null) render.close();
  await until(() => server.wsBridge.renderNodeCount() === 0, 500);
  return {
    closed: close === null ? "never: the socket stayed open" : { code: close.code, reason: close.reason },
    registrations,
    registrationsAfterTheEngineGoes: server.wsBridge.renderNodeCount(),
    resyncsReceived: directives.filter((d) => d.command === RESYNC_COMMAND).length,
    audited: handshakeFailures().map((r) => ({ outcome: r["outcome"], command: r["command"] })),
    warned: warnings.some((w) => w.includes("show.resync handshake failed")),
  };
}

/** The engine's next connect, against a clean slate: resynced first, registered once. */
async function nextConnectResyncs(): Promise<Record<string, unknown>> {
  const render = conn("render", RENDER);
  const directives = directivesOf(render);
  await new Promise<void>((res, rej) => {
    render.once("open", () => res());
    render.once("error", rej);
  });
  await until(() => directives.length >= 1);
  const out = {
    firstDirective: directives[0]?.command ?? null,
    registrations: server.wsBridge.renderNodeCount(),
  };
  render.close();
  return out;
}

const TORN_DOWN = {
  closed: { code: 1011, reason: "show.resync handshake failed" },
  registrations: 0,
  registrationsAfterTheEngineGoes: 0,
  resyncsReceived: 0,
  audited: [{ outcome: "rejected", command: RESYNC_COMMAND }],
  warned: true,
};

test("a resync that cannot be built: the connection closes (1011), holds no registration, is surfaced, and the next connect resyncs", async () => {
  // F3's shape: a throw while the snapshot is built, inside the handshake.
  state.failNext = 1;
  assert.deepEqual(
    await afterFailedConnect(),
    TORN_DOWN,
    "a failed handshake tears down, loudly: never registered-but-never-resynced",
  );
  assert.match(
    String((handshakeFailures()[0]?.["detail"] as Record<string, unknown> | undefined)?.["error"]),
    /injected: the resync snapshot could not be built/,
    "the audit names the cause",
  );
  assert.deepEqual(await nextConnectResyncs(), { firstDirective: RESYNC_COMMAND, registrations: 1 });
});

test("a resync that cannot be sent: the same teardown, and the next connect resyncs", async (t) => {
  // The second injector class: the snapshot builds, but the send fails.
  // `sendDirect` reports a failed or refused send as `false`; the handshake
  // used to ignore it, leaving a registration that never got its resync.
  const register = WsRenderBridge.prototype.register;
  let refuse = 1;
  WsRenderBridge.prototype.register = function (this: WsRenderBridge, send) {
    const registration = register.call(this, send);
    const sendDirect = registration.sendDirect;
    registration.sendDirect = (directive) => (refuse-- > 0 ? false : sendDirect(directive));
    return registration;
  };
  t.after(() => {
    WsRenderBridge.prototype.register = register;
  });
  assert.deepEqual(await afterFailedConnect(), TORN_DOWN, "an unsent resync tears down like an unbuilt one");
  assert.match(
    String((handshakeFailures()[0]?.["detail"] as Record<string, unknown> | undefined)?.["error"]),
    /did not accept show\.resync/,
    "the audit names the cause",
  );
  assert.deepEqual(await nextConnectResyncs(), { firstDirective: RESYNC_COMMAND, registrations: 1 });
});

test("a failed render handshake leaves an operator's connection untouched", async () => {
  // The teardown is the failing connection's own: an operator session open
  // beside it keeps working.
  const admin = conn("admin", ADMIN);
  await new Promise<void>((res, rej) => {
    admin.once("open", () => res());
    admin.once("error", rej);
  });
  state.failNext = 1;
  const failed = await afterFailedConnect();
  const id = "00000000-0000-0000-0000-000000000001";
  const reply = await new Promise<Record<string, unknown>>((res) => {
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return; // pushes (stateChange) arrive on this socket too
      admin.off("message", onMsg);
      res(msg);
    };
    admin.on("message", onMsg);
    admin.send(JSON.stringify({ v: "0.3", id, command: "automation.hold", payload: { hold: true } }));
  });
  assert.deepEqual(
    { renderClosed: (failed["closed"] as Record<string, unknown>)?.["code"], adminStatus: reply["status"], adminOpen: admin.readyState === WebSocket.OPEN },
    { renderClosed: 1011, adminStatus: "ok", adminOpen: true },
    "only the failing render connection was torn down",
  );
  admin.close();
});
