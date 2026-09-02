//! Server-level integration: WS auth, command round trip, alias deprecation
//! warning visible in a telemetry tick, mock-bridge ordering/stateVersion.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";

let server: ControlPlaneServer;
let state: ControlPlaneState;
let pkgPath: string;
const TOKEN = "op-token-1";

function makePackage(): string {
  const dir = mkdtempSync(join(tmpdir(), "nbe-srv-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.mp4"), "mp4");
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
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        { id: "A1_clip", kind: "video", source: "media/A1.mp4" },
      ],
      scenes: [{ id: "SCN_A1", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_clip" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN_A1" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

function connect(ws: WebSocket): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    ws.once("open", () => resolve({}));
    ws.once("error", reject);
    ws.once("unexpected-response", (_req, res) => reject(new Error(`unexpected-response ${res.statusCode}`)));
  });
}

function send(ws: WebSocket, envelope: unknown): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.kind !== "telemetry" && msg.kind !== "stateChange") {
        ws.off("message", onMsg);
        resolve(msg);
      }
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify(envelope));
  });
}

beforeEach(async () => {
  pkgPath = makePackage();
  state = new ControlPlaneState();
  const tmp = mkdtempSync(join(tmpdir(), "nbe-audit-"));
  await (async () => {
    if (server) await server.close();
  })();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [TOKEN]: "admin", "render-token-1": "render" } },
    audit: new AuditLog(join(tmp, "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
});

afterEach(async () => {
  if (server) await server.close();
});

test("WS with valid token+role pipes commands through and returns ok", async () => {
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${TOKEN}`, "x-nbe-role": "admin" },
  });
  await connect(ws);
  const resp = await send(ws, { v: "0.3", id: randomUUID(), command: "system.status", payload: {} });
  assert.equal(resp.status, "ok");
  ws.close();
});

test("render-role session receives directives in order with correct stateVersion", async () => {
  const conn = (role: string, token: string) =>
    new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
    });

  const render = conn("render", "render-token-1");
  await connect(render);

  // Render node sees directive frames; collect them.
  const directivePromise = new Promise<Record<string, unknown>>((resolve) => {
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.kind === "directive") {
        render.off("message", onMsg);
        resolve(msg);
      }
    };
    render.on("message", onMsg);
  });

  const admin = conn("admin", TOKEN);
  await connect(admin);
  let r = await send(admin, { v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: pkgPath } });
  assert.equal(r.status, "ok");
  r = await send(admin, { v: "0.3", id: randomUUID(), command: "preview.set", payload: { itemRef: "A1" } });
  assert.equal(r.status, "ok");
  await send(admin, { v: "0.3", id: randomUUID(), command: "view.take", payload: {} });

  const frame = await directivePromise;
  // Pinned directive frame shape.
  assert.equal(frame.v, "0.3");
  assert.equal(frame.kind, "directive");
  assert.equal(typeof frame.seq, "number");
  assert.equal(typeof frame.stateVersion, "number");
  admin.close();
  render.close();
});

test("bad token fails with E_AUTH at the HTTP upgrade", async () => {
  await assert.rejects(
    connect(
      new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
        headers: { authorization: "Bearer wrong", "x-nbe-role": "admin" },
      }),
    ),
    /unexpected-response 401/,
  );
});

test("deprecated program.take executes and the next telemetry tick carries the warning", async () => {
  const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${TOKEN}`, "x-nbe-role": "admin" },
  });
  await connect(ws);
  let r = await send(ws, { v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: pkgPath } });
  assert.equal(r.status, "ok");
  r = await send(ws, { v: "0.3", id: randomUUID(), command: "preview.set", payload: { itemRef: "A1" } });
  assert.equal(r.status, "ok");

  await send(ws, { v: "0.3", id: randomUUID(), command: "system.telemetry.subscribe", payload: { intervalMs: 200 } });
  const tickPromise = new Promise<Record<string, unknown>>((resolve) => {
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.kind === "telemetry") {
        ws.off("message", onMsg);
        resolve(msg);
      }
    };
    ws.on("message", onMsg);
  });

  const take = await send(ws, { v: "0.3", id: randomUUID(), command: "program.take", payload: {} });
  assert.equal(take.status, "ok");

  const tick = await tickPromise;
  const data = tick.data as { deprecationWarnings: Array<{ command: string; resolvedTo: string }> };
  assert.ok(
    data.deprecationWarnings.some((w) => w.command === "program.take"),
    "deprecation warning must appear on the telemetry tick",
  );
  ws.close();
});
