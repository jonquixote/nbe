//! Queue position 4 (SPEC v0.4.8 row 7): a failed re-resync on the engine's
//! `resyncRequest`, on the real engine. The unit guards in
//! `resync-handshake.test.ts` (G1, G2 and G4, with G3 their falsification, and
//! the teardown-order pair) pin the control plane's side on a real server and
//! a real socket, the engine scripted by the test. ~~G1–G4 … on a fake
//! socket~~ (§2c, the fresh pass on PR #45.) These pin what the real engine
//! does with it.
//!
//! The engine asks again only after a sequence gap. Each leg makes one: the
//! harness drops one directive on its way to the engine, so the next directive
//! meets the gap and the engine sends `resyncRequest` (`seqGap`). Then:
//! - **snapshot**: the re-resync's snapshot cannot be built. The control plane
//!   closes 1011; the engine reconnects at its backoff and its handshake
//!   resyncs it against a clean slate; directives apply again.
//! - **send**: the re-resync's send is refused. The session is kept; the engine
//!   stays frozen until the next directive's gap re-requests, and then it
//!   resyncs and directives apply again.
//! - **persistent**: every snapshot throws. The engine's reconnect loop paces
//!   the failures: one audit row per attempt, at the engine's backoff, never a
//!   row per directive.
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/resync-request.e2e.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { WsRenderBridge, type RenderSender } from "./render-bridge.js";
import { RESYNC_COMMAND } from "./protocol.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const APPLY_MS = 10_000;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
const stripAnsi = (s: string): string => s.replace(/\x1b\[[0-9;]*m/g, "");

/** A state whose snapshot throws `failNext` times (`Infinity`: always). */
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

interface Rig {
  server: ControlPlaneServer;
  state: FailingSnapshotState;
  engine: ChildProcess;
  log: string[];
  ws: WebSocket;
  warnings: string[];
  auditRows: (event: string) => Array<Record<string, unknown>>;
  /** Drop the next non-resync directive on its way to the engine: the gap. */
  dropNext: () => void;
  /** Refuse the Nth `sendDirect` on each registration (1 is the handshake's). */
  refuseDirect: Set<number>;
  close: () => Promise<void>;
}

async function rig(): Promise<Rig> {
  assert.ok(existsSync(ENGINE_BIN), `the engine binary must be built: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  holdOn = false;
  const state = new FailingSnapshotState();
  const warnings: string[] = [];
  const auditPath = join(tempDir("nbe-rr-audit-"), "audit.jsonl");
  let drop = false;
  const refuseDirect = new Set<number>();
  // The injectors ride the bridge's registration, the same seam the handshake
  // tests use: the dropped directive never reaches the socket, and a refused
  // direct send reports `false` as backpressure does.
  const register = WsRenderBridge.prototype.register;
  WsRenderBridge.prototype.register = function (this: WsRenderBridge, send: RenderSender) {
    const wrapped: RenderSender = (frame) => {
      if (drop && frame.command !== RESYNC_COMMAND) {
        drop = false;
        return true;
      }
      return send(frame);
    };
    const registration = register.call(this, wrapped);
    const sendDirect = registration.sendDirect;
    let calls = 0;
    registration.sendDirect = (directive) => (refuseDirect.has(++calls) ? false : sendDirect(directive));
    return registration;
  };
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: (m) => warnings.push(m),
  });
  const log: string[] = [];
  const engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      // The silent sink unless the environment asks for the device (the
      // device-audio design record, D7): a test never plays through, or
      // re-rates, the machine's speakers. A soak may set `device`.
      NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
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
  assert.ok(await server.awaitApplied(state.stateVersion, APPLY_MS), "the engine applies its handshake resync");
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
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
    warnings,
    auditRows: (event) =>
      existsSync(auditPath)
        ? readFileSync(auditPath, "utf8")
            .split("\n")
            .filter((l) => l.length > 0)
            .map((l) => JSON.parse(l) as Record<string, unknown>)
            .filter((r) => r["event"] === event)
        : [],
    dropNext: () => {
      drop = true;
    },
    refuseDirect,
    close: async () => {
      engine.kill("SIGKILL");
      ws.close();
      await server.close();
      WsRenderBridge.prototype.register = register;
    },
  };
}

/** One engine-applied directive (`automation.hold` toggles, forwarded and acknowledged). */
let holdOn = false;
async function directive(r: Rig): Promise<number> {
  holdOn = !holdOn;
  const id = randomUUID();
  const reply = await new Promise<Record<string, unknown>>((res) => {
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      r.ws.off("message", onMsg);
      res(msg);
    };
    r.ws.on("message", onMsg);
    r.ws.send(JSON.stringify({ v: "0.3", id, command: "automation.hold", payload: { hold: holdOn } }));
  });
  assert.equal(reply["status"], "ok", JSON.stringify(reply));
  return reply["stateVersion"] as number;
}

const linesOf = (r: Rig, needle: string): number => r.log.filter((l) => l.includes(needle)).length;
const rows = (r: Rig) =>
  r.auditRows("resync.requestFailed").map((row) => {
    const d = (row["detail"] ?? {}) as Record<string, unknown>;
    return { cause: d["cause"], reason: d["reason"], sessionKept: d["sessionKept"] };
  });

test("[queue 4] snapshot: a re-resync that cannot be built closes 1011; the real engine reconnects to a clean slate and directives apply", async () => {
  const r = await rig();
  try {
    r.dropNext();
    await directive(r); // dropped: the engine never sees it
    r.state.failNext = 1;
    const gapped = await directive(r); // the gap: the engine asks again, and the snapshot throws
    assert.ok(await waitFor(() => rows(r).length === 1, APPLY_MS), "the failed re-resync is audited");
    // The engine reconnects at its backoff; its handshake resyncs it.
    assert.ok(await waitFor(() => linesOf(r, "control plane connection dropped; reconnecting") >= 1, APPLY_MS), "the engine saw the close");
    // `awaitApplied` answers false at once while no render node is registered,
    // so wait for the reconnect's registration first.
    assert.ok(await waitFor(() => r.server.wsBridge.renderNodeCount() === 1, APPLY_MS), "the engine reconnected");
    assert.ok(await r.server.awaitApplied(gapped, APPLY_MS), "the reconnect's handshake resync applies (it carries the gapped directive's state)");
    const after = await directive(r);
    assert.ok(await r.server.awaitApplied(after, APPLY_MS), "directives apply again");
    const observed = {
      audited: rows(r),
      warned: r.warnings.some((w) => w.includes("show.resync on the engine's request failed")),
      gapRequests: linesOf(r, "seq gap; requesting resync"),
      reconnects: linesOf(r, "control plane connection dropped; reconnecting"),
      handshakeFailures: r.auditRows("resync.handshakeFailed").length,
      registrations: r.server.wsBridge.renderNodeCount(),
    };
    console.log(`RR-LEG snapshot ${JSON.stringify(observed)}`);
    assert.deepEqual(observed, {
      audited: [{ cause: "snapshot", reason: "seqGap", sessionKept: false }],
      warned: true,
      gapRequests: 1,
      reconnects: 1,
      handshakeFailures: 0,
      registrations: 1,
    });
  } finally {
    await r.close();
  }
});

test("[queue 4] send: a re-resync whose send is refused keeps the session; the engine stays frozen until the next gap re-requests, then directives apply", async () => {
  const r = await rig();
  try {
    r.refuseDirect.add(2); // 1 is the handshake's resync; 2 is the first re-request's
    r.dropNext();
    await directive(r); // dropped
    const gapped = await directive(r); // the gap: the engine asks again, and the send is refused
    assert.ok(await waitFor(() => rows(r).length === 1, APPLY_MS), "the refused re-resync is audited");
    const frozen = !(await r.server.awaitApplied(gapped, 1_000));
    const unfreezing = await directive(r); // the next gap re-requests, and this resync goes out
    assert.ok(await r.server.awaitApplied(unfreezing, APPLY_MS), "the next re-request's resync applies");
    const after = await directive(r);
    assert.ok(await r.server.awaitApplied(after, APPLY_MS), "directives apply again");
    const observed = {
      audited: rows(r),
      warned: r.warnings.some((w) => w.includes("show.resync on the engine's request was not sent")),
      frozenUntilTheNextRequest: frozen,
      gapRequests: linesOf(r, "seq gap; requesting resync"),
      reconnects: linesOf(r, "control plane connection dropped; reconnecting"),
      registrations: r.server.wsBridge.renderNodeCount(),
    };
    console.log(`RR-LEG send ${JSON.stringify(observed)}`);
    assert.deepEqual(observed, {
      audited: [{ cause: "send", reason: "seqGap", sessionKept: true }],
      warned: true,
      frozenUntilTheNextRequest: true,
      gapRequests: 2,
      reconnects: 0,
      registrations: 1,
    });
  } finally {
    await r.close();
  }
});

test("[queue 4] persistent: every snapshot throws; the failures are paced by the engine's reconnect backoff, one audit row per attempt", async () => {
  const r = await rig();
  const WATCH_MS = 25_000;
  try {
    r.dropNext();
    await directive(r);
    r.state.failNext = Number.POSITIVE_INFINITY;
    const t0 = Date.now();
    await directive(r); // the gap; from here every snapshot throws
    await sleep(WATCH_MS);
    // Directives sent meanwhile reach no engine (it holds no registration
    // between attempts), so they add no rows: the pace is the engine's.
    for (let i = 0; i < 5; i += 1) await directive(r);
    const handshakeRows = r.auditRows("resync.handshakeFailed").map((row) => (row["ts"] as number) - t0);
    const observed = {
      requestFailed: rows(r),
      attempts: linesOf(r, "control plane connection dropped; reconnecting"),
      handshakeFailedRows: handshakeRows.length,
      warnedEveryRow: r.warnings.filter((w) => w.includes("show.resync")).length === rows(r).length + handshakeRows.length,
    };
    console.log(`RR-LEG persistent ${JSON.stringify({ ...observed, handshakeFailedAtMs: handshakeRows })}`);
    assert.deepEqual(observed.requestFailed, [{ cause: "snapshot", reason: "seqGap", sessionKept: false }]);
    assert.ok(observed.warnedEveryRow, "every failure is warned");
    // One row per failure, never coalesced. The engine logs one "dropped" line
    // per connection that ends. The first is the request failure's own 1011,
    // whose row is the `resync.requestFailed` above; each later one is a
    // failed reconnect handshake with its `resync.handshakeFailed` row. So the
    // handshake rows are the attempts less one, or equal to the attempts if
    // the cut-off falls between a handshake's audit and the engine's log line.
    // ~~"The connection still open at the cut-off may not have closed yet, so
    // the rows trail the attempts by at most one."~~ (§2c, the fresh pass on
    // PR #45: the gap is structural, not the cut-off.)
    assert.ok(
      observed.handshakeFailedRows === observed.attempts || observed.handshakeFailedRows === observed.attempts - 1,
      `one audit row per attempt: ${observed.handshakeFailedRows} rows, ${observed.attempts} attempts`,
    );
    // The backoff caps at 10 s: in 25 s there are at most a dozen attempts,
    // never one per directive.
    assert.ok(observed.attempts >= 4 && observed.attempts <= 12, `paced by the backoff: ${observed.attempts} attempts in ${WATCH_MS} ms`);
  } finally {
    await r.close();
  }
});

async function waitFor(check: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await sleep(20);
  }
  return check();
}
