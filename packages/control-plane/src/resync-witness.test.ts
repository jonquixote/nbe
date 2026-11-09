// R32, reproduced deterministically, and the e2e's fix held to it (test-only).
//
// The candidate mechanism of R32's sightings 3 to 5: the control plane stamps
// `show.resync` with its version at send time (N); an item end the engine
// reports (`itemEvent`) moves the version to N+1 with no directive to apply;
// package-reconcile.e2e.ts's `reconnect()` read the current version and waited
// for the engine's last applied version to EQUAL it, so when the end landed
// between the resync's send and that read, it waited for N+1, which no
// directive carries. `awaitHandshakeResync` (resync-witness.ts), which the e2e
// now calls, waits for the version the resync was stamped with at send time.
//
// A fake render node on the real server: no engine, and no clock in any
// assertion. Every wait is a count of reads 10 ms apart (R39, R48's form): a
// stall lengthens a wait and never reddens it.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { RESYNC_COMMAND } from "./protocol.js";
import { tempDir } from "./test-tmp.js";
import { awaitHandshakeResync, witnessResyncs, type ResyncWitness } from "./resync-witness.js";

const RENDER = "render-token";
/** Reads for a wait that must come true (a hang guard: it measures nothing). */
const READS = 2000;
/** Reads given to a check that must NOT come true, before it is called stuck. */
const STUCK_READS = 40;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let witness: ResyncWitness;

beforeEach(async () => {
  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-r32-audit-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  witness = witnessResyncs(server.wsBridge);
});

afterEach(async () => {
  witness.restore();
  if (server) await server.close();
});

/** True once `check` holds, read at most `reads` times, 10 ms apart. */
async function within(reads: number, check: () => boolean | Promise<boolean>): Promise<boolean> {
  for (let i = 0; i < reads; i++) {
    if (await check()) return true;
    await new Promise((r) => setTimeout(r, 10));
  }
  return check();
}
const eventually = (check: () => boolean | Promise<boolean>) => within(READS, check);

async function lastApplied(): Promise<number | null> {
  const res = await fetch(`http://127.0.0.1:${server.port}/nbe/v0.3/status`);
  const body = (await res.json()) as { renderNode?: { lastAppliedStateVersion?: number | null } };
  return body.renderNode?.lastAppliedStateVersion ?? null;
}

/** A fake render node: its handshake's resync received, not yet acknowledged. */
async function renderNode(): Promise<{ ws: WebSocket; directives: Array<Record<string, unknown>> }> {
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${RENDER}`, "x-nbe-role": "render" },
  });
  const directives: Array<Record<string, unknown>> = [];
  ws.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === "directive") directives.push(msg);
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  assert.ok(await eventually(() => directives.length >= 1), "the handshake's resync arrives");
  assert.equal(directives[0]!.command, RESYNC_COMMAND);
  return { ws, directives };
}

async function closed(ws: WebSocket): Promise<void> {
  ws.close();
  assert.ok(await eventually(() => server.wsBridge.renderNodeCount() === 0), "the node is gone");
}

const itemEnd = (itemRef: string) => JSON.stringify({ v: "0.3", kind: "itemEvent", itemRef, event: "end" });
const applied = (sv: unknown) => JSON.stringify({ v: "0.3", kind: "appliedStateVersion", stateVersion: sv });

test("R32 reproduced: an item end between the resync's send and the read; 'applied == current' never comes, the stamped version does", async () => {
  const since = witness.stamped.length;
  const { ws, directives } = await renderNode();
  const n = directives[0]!.stateVersion as number;
  // The engine reports an item end before it acknowledges the resync (an
  // overdue item ends on the resync's receipt, SPEC v0.4.8 row 3).
  ws.send(itemEnd("A1"));
  assert.ok(await eventually(() => state.stateVersion === n + 1), "the item end moves the version, with no directive");
  ws.send(applied(n));
  assert.ok(await eventually(async () => (await lastApplied()) === n), "the engine applied the resync, at N");
  // The check reconnect() made before the fix: the current version, by equality.
  const sv = state.stateVersion;
  const equalCurrent = await within(STUCK_READS, async () => (await lastApplied()) === sv);
  // The check it makes now.
  const r = await awaitHandshakeResync({ witness, since, lastApplied, current: () => state.stateVersion, polls: STUCK_READS });
  assert.deepEqual(
    {
      sv,
      lastApplied: await lastApplied(),
      equalCurrent,
      directives: directives.map((d) => `${String(d.command)}@${String(d.stateVersion)}`),
      fix: { applied: r.applied, resyncAt: r.resyncAt, last: r.last },
    },
    {
      sv: n + 1,
      lastApplied: n,
      equalCurrent: false,
      directives: [`${RESYNC_COMMAND}@${n}`],
      fix: { applied: true, resyncAt: n, last: n },
    },
    "R32's shape: equality with the current version waits for N+1, which no directive carries; the stamped N is reached",
  );
  await closed(ws);
});

test("both orders, and a reconnect: the stamped version is reached whether the item end comes before or after the acknowledgement", async () => {
  for (const endFirst of [true, false]) {
    const since = witness.stamped.length;
    const { ws, directives } = await renderNode();
    const resyncAt = directives[0]!.stateVersion as number;
    // Each message is waited for at the server before the next, so the order
    // is the server's, not the socket's.
    const end = async () => {
      ws.send(itemEnd("A1"));
      assert.ok(await eventually(() => state.stateVersion === resyncAt + 1), "the item end moved the version");
    };
    if (endFirst) await end();
    ws.send(applied(resyncAt));
    assert.ok(await eventually(async () => (await lastApplied()) === resyncAt), "the acknowledgement landed");
    if (!endFirst) await end();
    const r = await awaitHandshakeResync({ witness, since, lastApplied, current: () => state.stateVersion, polls: READS });
    assert.deepEqual(
      { applied: r.applied, resyncAt: r.resyncAt },
      { applied: true, resyncAt },
      `the engine applied the resync it was sent (end ${endFirst ? "before" : "after"} the acknowledgement)`,
    );
    await closed(ws);
  }
});

test("the witness records each resync's stamp as it is sent, and leaves the frame and the send alone", async () => {
  const since = witness.stamped.length;
  const first = await renderNode();
  first.ws.send(itemEnd("A1")); // move the version, so the next resync is stamped higher
  const moved = (first.directives[0]!.stateVersion as number) + 1;
  assert.ok(await eventually(() => state.stateVersion === moved));
  await closed(first.ws);
  const second = await renderNode();
  assert.deepEqual(
    witness.stamped.slice(since),
    [first.directives[0]!.stateVersion, second.directives[0]!.stateVersion],
    "one stamp per handshake resync, as the node received it",
  );
  assert.equal(second.directives[0]!.stateVersion, moved, "the second resync carries the moved version");
  await closed(second.ws);
});

test("a registration with no resync witnessed reads as not applied, never as applied", async () => {
  const since = witness.stamped.length;
  const r = await awaitHandshakeResync({ witness, since, lastApplied, current: () => state.stateVersion, polls: STUCK_READS });
  assert.deepEqual({ applied: r.applied, resyncAt: r.resyncAt }, { applied: false, resyncAt: undefined });
});

test("R32: the version moves even when the item end changes nothing (no item is PLAYING)", async () => {
  const { ws, directives } = await renderNode();
  const n = directives[0]!.stateVersion as number;
  ws.send(applied(n));
  assert.equal(state.itemStateOf("A1"), "READY");
  ws.send(itemEnd("A1"));
  assert.ok(await eventually(() => state.stateVersion === n + 1), "the end was ignored, and the version still moved");
  assert.equal(state.itemStateOf("A1"), "READY");
  await closed(ws);
});
