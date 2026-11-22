// Review4 step 7c: an engine restarted mid-show is re-sent the take and the
// stream the control plane believes on, once it has applied its resync.
//
// Before: a new engine process (a new `engineInstanceId`) got the resync, which
// carries no output state, and nothing else. The recording and the stream were
// gone while the control plane still believed them on; #195 made that loud
// (record.failed), this makes it recover.

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { indexPackage } from "./package.js";
import { PROTOCOL_VERSION } from "./protocol.js";
import { createControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const STREAM_URL = "rtmp://127.0.0.1:1935/live/secret-stream-key";

function makePackage(): string {
  const dir = tempDir("nbe-reissue-");
  mkdirSync(join(dir, "media"), { recursive: true });
  for (const f of ["fallback", "A1"]) writeFileSync(join(dir, "media", `${f}.png`), "png");
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
      assets: ["fallback", "A1"].map((id) => ({ id, kind: "image", source: `media/${id}.png` })),
      scenes: [{ id: "SCN_A1", elements: [{ id: "el_A1", kind: "clip", z: 1, assetId: "A1" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN_A1", durationFrames: 900 }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

/** A fresh engine process's hello: nothing running, nothing on air. */
const hello = (engineInstanceId: string) => ({
  v: PROTOCOL_VERSION,
  kind: "engineHello",
  engineInstanceId,
  startedAtMs: Date.now(),
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

async function rig(t: { after: (fn: () => void | Promise<void>) => void }) {
  const pkgPath = makePackage();
  const auditPath = join(tempDir("nbe-reissue-audit-"), "audit.jsonl");
  const state = new ControlPlaneState();
  state.loadPackage(indexPackage(pkgPath));
  state.showState = "RUNNING";
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
  t.after(() => server.close());
  const open = (
    role: string,
    token: string,
    onMessage?: (ws: WebSocket, msg: Record<string, unknown>) => void,
    onOpen?: (ws: WebSocket) => void,
  ): Promise<WebSocket> =>
    new Promise((resolve, reject) => {
      const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
        headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
      });
      if (onMessage) ws.on("message", (buf: Buffer) => onMessage(ws, JSON.parse(buf.toString("utf8")) as Record<string, unknown>));
      t.after(() => ws.close());
      ws.once("open", () => {
        onOpen?.(ws);
        resolve(ws);
      });
      ws.once("error", reject);
    });
  return {
    state,
    rows: (): Array<Record<string, unknown>> =>
      existsSync(auditPath)
        ? readFileSync(auditPath, "utf8")
            .split("\n")
            .filter((l) => l.length > 0)
            .map((l) => JSON.parse(l) as Record<string, unknown>)
        : [],
    auditText: (): string => (existsSync(auditPath) ? readFileSync(auditPath, "utf8") : ""),
    /** A scripted engine that applies every directive at once. Like the real
     * one it says hello first, on open; `lateHello` sends it only after the
     * resync was applied (the order a control plane must also survive). */
    engine: async (instanceId: string, lateHello = false) => {
      const directives: Array<Record<string, unknown>> = [];
      let greeted = false;
      const greet = (sock: WebSocket) => {
        if (greeted) return;
        greeted = true;
        sock.send(JSON.stringify(hello(instanceId)));
      };
      const ws = await open(
        "render",
        RENDER,
        (sock, msg) => {
          if (msg.kind !== "directive") return;
          directives.push(msg);
          sock.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "appliedStateVersion", stateVersion: msg.stateVersion }));
          if (lateHello && msg.command === "show.resync") setTimeout(() => greet(sock), 50);
        },
        (sock) => {
          if (!lateHello) greet(sock);
        },
      );
      return { ws, directives };
    },
    admin: () => open("admin", ADMIN),
  };
}

function send(ws: WebSocket, command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`no response to ${command}`)), 5000);
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

const commands = (ds: Array<Record<string, unknown>>) => ds.map((d) => d.command);

async function closed(ws: WebSocket): Promise<void> {
  if (ws.readyState === WebSocket.CLOSED) return;
  await new Promise<void>((r) => {
    ws.once("close", () => r());
    ws.close();
  });
}

test("7c: an engine restarted mid-show is re-sent the take and the stream, after it applied its resync", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")), "the first engine is resynced");
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");
  assert.equal((await send(admin, "stream.start", { url: STREAM_URL })).status, "ok");

  await closed(e1.ws);
  const e2 = await r.engine("engine-2");
  assert.ok(
    await until(() => commands(e2.directives).includes("record.start") && commands(e2.directives).includes("stream.start")),
    `the restarted engine is re-sent both starts: ${JSON.stringify(commands(e2.directives))}`,
  );
  const resync = e2.directives.find((d) => d.command === "show.resync")!;
  const record = e2.directives.find((d) => d.command === "record.start")!;
  const stream = e2.directives.find((d) => d.command === "stream.start")!;
  assert.ok(e2.directives.indexOf(resync) < e2.directives.indexOf(record), "the starts follow the resync");
  assert.ok((record.stateVersion as number) > (resync.stateVersion as number), "each re-issue has its own, later version");
  assert.deepEqual(record.payload, { outputId: "ep1" }, "the take is re-sent as it was started");
  assert.deepEqual(stream.payload, { url: STREAM_URL }, "the stream is re-sent to the same endpoint");
  assert.equal(r.state.recordState, "recording", "the belief does not change");
  assert.equal(r.state.streamState, "live");

  const rows = r.rows();
  assert.ok(rows.some((x) => x.event === "record.reissued"), "the take's re-issue is audited");
  assert.ok(rows.some((x) => x.event === "stream.reissued"), "the stream's re-issue is audited");
  assert.ok(!r.auditText().includes("secret-stream-key"), "the stream key never reaches the audit log");
});

test("7c: a reconnect of the same engine is re-sent nothing (it still holds its take)", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");

  await closed(e1.ws);
  const again = await r.engine("engine-1");
  assert.ok(await until(() => commands(again.directives).includes("show.resync")), "the reconnect is resynced");
  await new Promise((res) => setTimeout(res, 300));
  assert.deepEqual(commands(again.directives), ["show.resync"], "nothing is re-sent to the same process");
  assert.ok(!r.rows().some((x) => x.event === "record.reissued"));
});

test("7c: a take stopped before the restart is not re-sent", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");
  assert.equal((await send(admin, "record.stop")).status, "ok");

  await closed(e1.ws);
  const e2 = await r.engine("engine-2");
  assert.ok(await until(() => commands(e2.directives).includes("show.resync")));
  await new Promise((res) => setTimeout(res, 300));
  assert.deepEqual(commands(e2.directives), ["show.resync"], "a stopped take is not restarted");
});

test("7c: a hello that arrives after the resync was applied still gets the starts re-sent", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");

  await closed(e1.ws);
  const e2 = await r.engine("engine-2", true);
  assert.ok(
    await until(() => commands(e2.directives).includes("record.start")),
    `re-sent after a late hello: ${JSON.stringify(commands(e2.directives))}`,
  );
  assert.deepEqual(commands(e2.directives), ["show.resync", "record.start"]);
});
