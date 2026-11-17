//! Review4 step 7a (the user's ruling of 2026-11-16 23:45): the control plane
//! keeps each connection's `engineHello` and tells a restarted engine (a new
//! `engineInstanceId`, review4 case E) from a reconnect of the same one. A
//! restart is one `engine.restarted` audit row; a reconnect is none. Nothing
//! else changes here: adoption is step 7b, the re-issue on a restart 7c.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { EngineFrameSchema, PROTOCOL_VERSION } from "./protocol.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const RENDER = "render-token";

const hello = (engineInstanceId: string) => ({
  v: PROTOCOL_VERSION,
  kind: "engineHello",
  engineInstanceId,
  startedAtMs: 1_768_000_000_123.5,
  showRunning: false,
  masterFrame: null,
  lastAppliedStateVersion: 0,
  package: null,
  viewItem: null,
  viewItemStartFrame: null,
  previewItem: null,
  visibleOverlays: [],
  fallbackActive: false,
  recording: false,
  streamTransportState: "none",
});

test("the schema takes a hello with every fact null, and refuses one with an unknown key", () => {
  assert.equal(EngineFrameSchema.safeParse(hello("a")).success, true);
  assert.equal(
    EngineFrameSchema.safeParse({ ...hello("a"), package: { path: "/shows/x", loadStateVersion: 3 }, masterFrame: 9 }).success,
    true,
  );
  assert.equal(EngineFrameSchema.safeParse({ ...hello("a"), extra: 1 }).success, false, ".strict()");
});

test("a reconnect of the same engine is not a restart; a new instance id is one engine.restarted row", async (t) => {
  const auditPath = join(tempDir("nbe-engine-hello-"), "audit.jsonl");
  const state = new ControlPlaneState();
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
  t.after(() => server.close());
  const rows = (): Array<Record<string, unknown>> =>
    existsSync(auditPath)
      ? readFileSync(auditPath, "utf8")
          .split("\n")
          .filter((l) => l.length > 0)
          .map((l) => JSON.parse(l) as Record<string, unknown>)
      : [];
  const restarted = () => rows().filter((r) => r["event"] === "engine.restarted");

  /** One render connection that says hello as `id`, then goes away. */
  const session = async (id: string): Promise<void> => {
    const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: `Bearer ${RENDER}`, "x-nbe-role": "render" },
    });
    await new Promise<void>((resolve, reject) => {
      ws.once("open", () => resolve());
      ws.once("error", reject);
    });
    ws.send(JSON.stringify(hello(id)));
    // The hello is handled before the next frame on this connection; a
    // second, harmless frame proves it was read.
    ws.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "appliedStateVersion", stateVersion: 0 }));
    await new Promise((r) => setTimeout(r, 100));
    ws.close();
    await new Promise<void>((resolve) => ws.once("close", () => resolve()));
    await new Promise((r) => setTimeout(r, 50));
  };

  await session("engine-a");
  assert.deepEqual(restarted(), [], "the first hello is not a restart");
  await session("engine-a");
  assert.deepEqual(restarted(), [], "a reconnect of the same process is not a restart");
  await session("engine-b");
  const rowsB = restarted();
  assert.equal(rowsB.length, 1, "one row for the new process");
  const detail = rowsB[0]!["detail"] as Record<string, unknown>;
  assert.equal(detail["previousInstanceId"], "engine-a");
  assert.equal(detail["instanceId"], "engine-b");
  assert.equal(typeof detail["downMs"], "number");
  assert.ok((detail["downMs"] as number) >= 0);
  assert.equal(rowsB[0]!["command"], "engine:hello");
  await session("engine-b");
  assert.equal(restarted().length, 1, "and its reconnect adds none");
});
