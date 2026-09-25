//! Server-level integration: WS auth, command round trip, alias deprecation
//! warning visible in a telemetry tick, mock-bridge ordering/stateVersion.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

let server: ControlPlaneServer;
let state: ControlPlaneState;
let pkgPath: string;
const TOKEN = "op-token-1";

function makePackage(): string {
  const dir = tempDir("nbe-srv-");
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
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
        // An image, not video: these packages exercise the command bus, not
        // media. Prompt 05 gave preflight real decode, so a placeholder file
        // declared as video is now correctly rejected.
        { id: "A1_clip", kind: "image", source: "media/A1.png" },
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
  const tmp = tempDir("nbe-audit-");
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

  // Free-standing form, usable before the per-test closure below exists.
  const waitForCommandIn = async (
    list: Record<string, unknown>[],
    command: string,
  ): Promise<Record<string, unknown>> => {
    for (let i = 0; i < 400; i++) {
      const found = list.find((d) => d.command === command);
      if (found) return found;
      await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error(`directive '${command}' never arrived; have ${JSON.stringify(list.map((d) => d.command))}`);
  };

  const render = conn("render", "render-token-1");

  // The collector is attached BEFORE the handshake completes, and that ordering
  // is the whole of R7.
  //
  // §5.9.4 makes the server send `show.resync` the moment a render session
  // registers — "before any other directive on this connection". This test used
  // to attach its listener after `await connect(render)`, so whether that frame
  // was observed was a race between the handshake resolving and the listener
  // binding. Locally the listener lost and the test saw three directives;
  // on a loaded 1-3 core runner it sometimes won and the test saw four. Five
  // sightings across a year of CI, always `got 4`, always green on rerun, twice
  // on branches whose diff was docs only — every property explained by a frame
  // that is always sent and only sometimes seen.
  //
  // The earlier diagnosis in this register — `directives.at(-1)` aliasing a
  // previous command's directive — was a real defect in the waits and is fixed
  // below, but it was NOT this. The payload dump added for exactly this purpose
  // is what settled it, on the fifth sighting:
  //
  //   received: [{"command":"show.resync","seq":0,"stateVersion":0},
  //              {"command":"show.load","seq":1,...}, ...]
  //
  // Attaching first makes the resync deterministic rather than racy, so it is
  // asserted here as the contract §5.9.4 says it is.
  //
  // NOT this test's first guard, and an earlier version of this comment claimed
  // it was (§2c). render-channel.test.ts already covered the sentence from the
  // channel's side — "show.resync is the first directive on a render
  // connection", the mid-show reconnect case, and `resyncRequest` — and deleting
  // `sendResync` from the connect path fails four tests, three of them those.
  // This assertion is a duplicate at a different layer: it is what stops THIS
  // test from counting a frame it never meant to observe.
  const directives: Record<string, unknown>[] = [];
  render.on("message", (buf: Buffer) => {
    const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
    if (msg.kind === "directive") directives.push(msg);
  });
  await connect(render);
  const resync = await waitForCommandIn(directives, "show.resync");
  // What §5.9.4 actually says, and what is merely true of THIS fixture, are
  // different things, so they are labelled differently.
  //
  // The contract: "On every render-role connection — initial connect and every
  // reconnect — the control plane MUST send a `show.resync` directive before any
  // other directive on that connection." That is the length-1 assertion below.
  assert.equal(
    directives.length,
    1,
    "§5.9.4: show.resync goes out BEFORE any other directive on this connection",
  );
  // FIXTURE FACTS, not §5.9.4 properties. seq 0 and stateVersion 0 hold because
  // this is an INITIAL connect against a server that has accepted no commands; a
  // reconnect mid-show carries the current stateVersion, and the snapshot is
  // still correct. The reconnect half of the sentence is covered by
  // "a render node connecting mid-show is resynced, and its seq starts at 0" in
  // render-channel.test.ts, which is where that case lives — not here.
  assert.equal(resync.seq, 0, "initial connect: the render channel's seq starts at 0");

  // A helper that returns the directive recorded for a given seq once it
  // appears (guards the delivery/ordering assertion below by waiting for it).
  const waitForSeq = async (seq: number): Promise<Record<string, unknown>> => {
    for (let i = 0; i < 200; i++) {
      const found = directives.find((d) => d.seq === seq);
      if (found) return found;
      await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error(`directive seq ${seq} never arrived; have ${directives.length}`);
  };

  // Wait for the directive a specific command produced, BY NAME. `at(-1)` reads
  // whatever arrived last, which is not necessarily the directive the command
  // just issued: the ok-response travels on `admin` and the directive on
  // `render`, so a response can be observed before its own directive lands. When
  // that happened, `at(-1)` aliased the PREVIOUS command's directive, and the
  // `waitForSeq` built from it found that already-present directive and returned
  // immediately — so the test proceeded without ever waiting for the directive
  // it meant to wait for. Waiting by command name cannot alias.
  const waitForCommand = async (command: string): Promise<Record<string, unknown>> => {
    for (let i = 0; i < 400; i++) {
      const found = directives.find((d) => d.command === command);
      if (found) return found;
      await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error(`directive '${command}' never arrived; have ${JSON.stringify(directives.map((d) => d.command))}`);
  };

  const admin = conn("admin", TOKEN);
  await connect(admin);
  const ev = (cmd: string, payload: Record<string, unknown>, seq: number) => ({ cmd, payload, seq });

  // show.load -> forward:true, one directive
  const load = await send(admin, { v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: pkgPath } });
  assert.equal(load.status, "ok");
  const loadSeq = (await waitForCommand("show.load")).seq as number;

  // preview.set -> forward:true, one directive
  const prev = await send(admin, { v: "0.3", id: randomUUID(), command: "preview.set", payload: { itemRef: "A1" } });
  assert.equal(prev.status, "ok");
  const prevSeq = (await waitForCommand("preview.set")).seq as number;

  // view.take -> forward:false + extraDirective (resolved), one directive
  const take = await send(admin, { v: "0.3", id: randomUUID(), command: "view.take", payload: {} });
  assert.equal(take.status, "ok");
  const takeSeq = (await waitForCommand("view.take")).seq as number;

  // All three have now been observed BY NAME, so the count below cannot be taken
  // early. What remains is the opposite risk — a fourth arriving late — and the
  // settle window below is what catches it. The 30 ms fixed sleep this replaced
  // did neither job: it could expire before a slow directive landed, and a
  // fourth arriving after it was never noticed at all.
  const SETTLE_MS = 150;
  await new Promise((r) => setTimeout(r, SETTLE_MS));

  const expect = [0, loadSeq, prevSeq, takeSeq]; // resync opens the stream
  // Three directives, in command order.
  //
  // R7's assertion. Four sightings now — 2026-09-08, and twice on branches
  // whose diff was docs+CI only (PR #17 run 34745014794, PR #19 run
  // 35314193753) — always `got 4`, always green on rerun. Every sighting
  // produced a COUNT and nothing else, because the message carried only
  // `directives.length`. So four sightings in, the open question (a redelivered
  // directive, or a fourth from an extra stateVersion bump) is still
  // unanswered. The message now dumps the directives so the next sighting
  // answers it instead of adding a tally mark. Diagnostics only — the
  // assertion itself is unchanged.
  assert.equal(
    directives.length,
    4,
    `expected 4 directives (resync + three commands), got ${directives.length}\n` +
      `expected seqs ${JSON.stringify(expect)}\n` +
      `received: ${JSON.stringify(
        directives.map((d) => ({
          command: d.command,
          seq: d.seq,
          stateVersion: (d as Record<string, unknown>)["stateVersion"],
        })),
      )}`,
  );
  assert.deepEqual(directives.map((d) => d.command), [
    "show.resync",
    "show.load",
    "preview.set",
    "view.take",
  ]);
  // A second settle window, asserted again: a duplicate or extra directive that
  // arrives late must still fail this test rather than slip past the first
  // count. This is the 4-detection R7 was reporting, kept and made deterministic
  // rather than removed — the fix changes WHEN the count is taken, never what
  // counts as wrong.
  await new Promise((r) => setTimeout(r, SETTLE_MS));
  assert.equal(
    directives.length,
    4,
    `an extra directive arrived ${SETTLE_MS} ms after the first count settled: ` +
      `${JSON.stringify(directives.map((d) => ({ command: d.command, seq: d.seq })))}`,
  );
  // seq strictly increasing across all three.
  const seqs = directives.map((d) => d.seq as number);
  for (let i = 1; i < seqs.length; i++) assert.ok(seqs[i]! > seqs[i - 1]!, `seq not increasing: ${seqs}`);
  // Pinned shape + stateVersion matches the ok-response's stateVersion.
  for (const d of directives) {
    assert.equal(d.v, "0.3");
    assert.equal(d.kind, "directive");
    assert.equal(typeof d.seq, "number");
    assert.equal(typeof d.stateVersion, "number");
  }
  void ev;
  // Each directive's stateVersion matches the stateVersion its ack returned.
  // Indices are offset by one: the §5.9.4 resync occupies slot 0 at
  // stateVersion 0, and the three commands follow it.
  assert.equal(directives[0]!.stateVersion, 0, "the resync snapshot carries stateVersion 0");
  assert.equal(directives[1]!.stateVersion, load.stateVersion);
  assert.equal(directives[2]!.stateVersion, prev.stateVersion);
  assert.equal(directives[3]!.stateVersion, take.stateVersion);

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
