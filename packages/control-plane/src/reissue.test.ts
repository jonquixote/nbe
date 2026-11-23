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

async function rig(t: { after: (fn: () => void | Promise<void>) => void }, extra: { reissueWaitMs?: number } = {}) {
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
    ...extra,
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
     * resync was applied (the order a control plane must also survive).
     * `holdResync` holds the resync's acknowledgement, for that many ms or
     * (true) until `releaseResync()`, and every later one behind it: the real
     * engine applies in arrival order, and a new engine's resync reloads the
     * show. `reports` sends a report every 50 ms from the open, as the real
     * engine does whether or not its resync is applied: its stream and take
     * are what the directives it applied started. */
    engine: async (instanceId: string, lateHello = false, opts: { holdResync?: number | true; reports?: boolean } = {}) => {
      const directives: Array<Record<string, unknown>> = [];
      let greeted = false;
      let held: number[] | null = null;
      let greetedResync = false;
      let stream = "none";
      let take = "none";
      let sock: WebSocket | null = null;
      const ack = (v: number) => sock?.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "appliedStateVersion", stateVersion: v }));
      const releaseResync = () => {
        const queue = held ?? [];
        held = null;
        for (const v of queue) ack(v);
      };
      const greet = (s: WebSocket) => {
        if (greeted) return;
        greeted = true;
        s.send(JSON.stringify(hello(instanceId)));
      };
      const ws = await open(
        "render",
        RENDER,
        (s, msg) => {
          if (msg.kind !== "directive") return;
          directives.push(msg);
          if (msg.command === "stream.start") stream = "live";
          if (msg.command === "stream.stop") stream = "none";
          if (msg.command === "record.start") take = "recording";
          if (msg.command === "record.stop") take = "none";
          const v = msg.stateVersion as number;
          if (msg.command === "show.resync" && opts.holdResync !== undefined && held === null && !greetedResync) {
            greetedResync = true;
            held = [v];
            if (typeof opts.holdResync === "number") setTimeout(releaseResync, opts.holdResync);
          } else if (held !== null) held.push(v);
          else ack(v);
          if (lateHello && msg.command === "show.resync") setTimeout(() => greet(s), 50);
        },
        (s) => {
          sock = s;
          if (!lateHello) greet(s);
        },
      );
      let timer: ReturnType<typeof setInterval> | null = null;
      if (opts.reports) {
        const report = () =>
          ws.readyState === WebSocket.OPEN &&
          ws.send(
            JSON.stringify({
              v: PROTOCOL_VERSION,
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
              streamTransportState: stream,
              recordTake: { state: take, reason: null },
            }),
          );
        report();
        timer = setInterval(report, 50);
        t.after(() => {
          if (timer) clearInterval(timer);
        });
      }
      const stop = () => {
        if (timer) clearInterval(timer);
      };
      return { ws, directives, releaseResync, stop };
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

test("case E: a restarted engine is re-sent each connected guest's connect (and its mute); a guest that left is not; the WHIP URL never reaches the audit log", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  const whip = (g: string) => `https://media.example.test/whip/${g}?token=whip-secret-${g}`;
  for (const g of ["g1", "g2", "g3"]) {
    assert.equal((await send(admin, "guest.connect", { guestId: g, whipUrl: whip(g) })).status, "ok", `connect ${g}`);
  }
  assert.equal((await send(admin, "guest.mute", { guestId: "g2", muted: true })).status, "ok");
  assert.equal((await send(admin, "guest.disconnect", { guestId: "g3" })).status, "ok");

  await closed(e1.ws);
  const e2 = await r.engine("engine-2");
  assert.ok(
    await until(() => e2.directives.filter((d) => d.command === "guest.connect").length === 2 && commands(e2.directives).includes("guest.mute")),
    `the restarted engine is re-sent the guests: ${JSON.stringify(commands(e2.directives))}`,
  );
  await new Promise((res) => setTimeout(res, 200));
  const connects = e2.directives.filter((d) => d.command === "guest.connect").map((d) => d.payload);
  assert.deepEqual(
    connects,
    [
      { guestId: "g1", whipUrl: whip("g1") },
      { guestId: "g2", whipUrl: whip("g2") },
    ],
    "each connected guest, as it was connected; the guest that left is not",
  );
  const mutes = e2.directives.filter((d) => d.command === "guest.mute").map((d) => d.payload);
  assert.deepEqual(mutes, [{ guestId: "g2", muted: true }], "a muted guest is muted again");
  const resync = e2.directives.findIndex((d) => d.command === "show.resync");
  assert.ok(resync >= 0 && resync < e2.directives.findIndex((d) => d.command === "guest.connect"), "after the resync");
  assert.equal(r.rows().filter((x) => x.event === "guest.reissued").length, 2, "each guest's re-issue is audited");
  assert.ok(!r.auditText().includes("whip-secret"), "the WHIP URL never reaches the audit log");
});

/** The audit rows that would show a flicker: a belief correction, or `record.failed` or the stream's divergence raised. */
const flickerRows = (rows: Array<Record<string, unknown>>) =>
  rows.filter((x) => {
    const detail = x.detail as Record<string, unknown> | undefined;
    const raised = x.command === "alert" && detail?.edge === "raised";
    return x.event === "belief.corrected" || (raised && (detail?.alert === "record.failed" || detail?.alert === "stream.divergence"));
  });

test("7c: the restarted engine's reports before its resync is applied correct nothing: the stream stays live and record.failed does not raise", async (t) => {
  // Seen on the real engine (operator-signals e2e, 2026-11-22): the belief
  // went live -> idle 144 ms after the restart's connect, then back to live
  // once the re-issued start landed. The new engine reports from its open;
  // its resync (which reloads the show) is acknowledged later, and only
  // then are the starts re-sent, so its first reports read no stream and no
  // take: outputs not yet re-sent, not outputs that failed.
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");
  assert.equal((await send(admin, "stream.start", { url: STREAM_URL })).status, "ok");

  await closed(e1.ws);
  const e2 = await r.engine("engine-2", false, { holdResync: 900, reports: true });
  const seen = new Set<string>();
  const sampler = setInterval(() => seen.add(`${r.state.streamState}/${r.state.recordState}`), 5);
  t.after(() => clearInterval(sampler));
  assert.ok(
    await until(() => commands(e2.directives).includes("record.start") && commands(e2.directives).includes("stream.start"), 4000),
    `the starts are re-sent once the resync is applied: ${JSON.stringify(commands(e2.directives))}`,
  );
  await new Promise((res) => setTimeout(res, 600)); // reports after the re-issue, read by the 250 ms evaluation
  clearInterval(sampler);
  e2.stop();
  assert.deepEqual([...seen], ["live/recording"], "the belief never left live and recording");
  assert.deepEqual(flickerRows(r.rows()), [], "no correction and no record.failed across the restart");
});

test("7c: a stop accepted while the restarted engine's resync is unapplied is not undone: only what is still believed on is re-sent", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");
  assert.equal((await send(admin, "stream.start", { url: STREAM_URL })).status, "ok");

  await closed(e1.ws);
  const e2 = await r.engine("engine-2", false, { holdResync: true });
  assert.ok(await until(() => commands(e2.directives).includes("show.resync")));
  await new Promise((res) => setTimeout(res, 100)); // the hello has landed: the re-issue is owed
  assert.equal((await send(admin, "stream.stop")).status, "ok");
  e2.releaseResync();
  assert.ok(await until(() => commands(e2.directives).includes("record.start")), JSON.stringify(commands(e2.directives)));
  await new Promise((res) => setTimeout(res, 300));
  assert.deepEqual(
    commands(e2.directives),
    ["show.resync", "stream.stop", "record.start"],
    "the take is re-sent; the stream the operator stopped is not started again",
  );
  assert.equal(r.state.streamState, "idle");
  assert.ok(!r.rows().some((x) => x.event === "stream.reissued"));
});

test("7c: a restarted engine whose connection drops before it applied its resync is still owed the starts when it reconnects", async (t) => {
  const r = await rig(t);
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "record.start", { outputId: "ep1" })).status, "ok");

  await closed(e1.ws);
  const first = await r.engine("engine-2", false, { holdResync: true });
  assert.ok(await until(() => commands(first.directives).includes("show.resync")));
  await new Promise((res) => setTimeout(res, 100)); // its hello has landed
  await closed(first.ws); // gone before it acknowledged the resync
  const again = await r.engine("engine-2");
  assert.ok(
    await until(() => commands(again.directives).includes("record.start")),
    `the same process, reconnected, is re-sent the take: ${JSON.stringify(commands(again.directives))}`,
  );
  assert.deepEqual(commands(again.directives), ["show.resync", "record.start"]);
});

test("7c: a re-issue still owed after reissueWaitMs ends its claim: the unacknowledged alert raises and a later report corrects the stream", async (t) => {
  const r = await rig(t, { reissueWaitMs: 500 });
  const e1 = await r.engine("engine-1");
  assert.ok(await until(() => commands(e1.directives).includes("show.resync")));
  const admin = await r.admin();
  assert.equal((await send(admin, "stream.start", { url: STREAM_URL })).status, "ok");

  await closed(e1.ws);
  const t0 = Date.now();
  const e2 = await r.engine("engine-2", false, { holdResync: true, reports: true });
  const raised = () =>
    r.rows().find((x) => x.event === "command.unacknowledged" && (x.detail as Record<string, unknown>).edge === "raised");
  assert.ok(await until(() => raised() !== undefined, 3000), "the owed claim's expiry raises");
  assert.ok(Date.now() - t0 >= 450, "not before reissueWaitMs");
  assert.match(String((raised()!.detail as Record<string, unknown>).reason), /has not applied its resync within 500 ms/);
  assert.ok(await until(() => r.state.streamState === "idle", 2000), "a report after the expiry corrects the belief");
  e2.stop();
});
