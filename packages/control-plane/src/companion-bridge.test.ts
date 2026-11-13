//! The Companion bridge (sprint item F), driven the way Companion drives it:
//! the bridge runs as its own process (`node --import tsx
//! src/companion-bridge.ts`), and a scripted Companion-like client presses
//! buttons over HTTP and over the bridge's WebSocket. The control plane is the
//! real server, unchanged, with an operator token the bridge holds and an
//! admin session the test uses to load the show.
//!
//! What is checked is behaviour on the wire and in the control plane: the
//! state a press changes, the audit row it leaves (role, token, intentSource),
//! the HTTP status a refusal returns, and the requests the bridge must refuse
//! (a browser page's cross-site POST, a DNS-rebound Host, a GET, a cross-site
//! WebSocket, a peer that is not loopback).
//!
//! The tests share one control plane and one loaded show and run in order;
//! each test that needs an item previewed sets that up itself, so a failure in
//! one does not cascade into the next.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { request } from "node:http";
import { networkInterfaces } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

import { AuditLog, type AuditRecord } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";
import { NO_PREFLIGHT } from "./test-preflight.js";

const OP_TOKEN = `op-${randomUUID()}`;
const ADMIN_TOKEN = `admin-${randomUUID()}`;
const BRIDGE = fileURLToPath(new URL("./companion-bridge.ts", import.meta.url));
const PKG_ROOT = fileURLToPath(new URL("..", import.meta.url));
const PROFILE = "deck-a";

function tokenId(token: string): string {
  return createHash("sha256").update(token).digest("hex").slice(0, 16);
}

function makePackage(): string {
  const dir = tempDir("nbe-bridge-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
  writeFileSync(join(dir, "media", "A2.png"), "png");
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
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
        { id: "A1_clip", kind: "image", source: "media/A1.png" },
        { id: "A2_clip", kind: "image", source: "media/A2.png" },
      ],
      scenes: [
        { id: "SCN_A1", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_clip" }] },
        { id: "SCN_A2", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A2_clip" }] },
      ],
      rundown: {
        id: "R",
        items: [
          { id: "A1", kind: "sceneRef", sceneRef: "SCN_A1" },
          { id: "A2", kind: "sceneRef", sceneRef: "SCN_A2" },
        ],
      },
      control: {
        bindings: [
          { id: "take-1", description: "TAKE", trigger: { kind: "companionKey", page: 1, bank: 1, key: "0" }, action: "view.take", payload: {} },
          { id: "arm-a2", description: "Preview A2", trigger: { kind: "companionKey", page: 1, bank: 1, key: "1" }, action: "preview.set", payload: { itemRef: "A2" } },
        ],
      },
    }),
  );
  return dir;
}

function tokenFile(token: string, mode = 0o600): string {
  const p = join(tempDir("nbe-bridge-token-"), "token");
  writeFileSync(p, `${token}\n`, { mode });
  return p;
}

interface BridgeProc {
  proc: ChildProcess;
  port: number;
  out: string[];
}

/** Spawn the bridge CLI; resolve once it prints its listening line. */
function spawnBridge(args: string[]): Promise<BridgeProc> {
  return new Promise((resolve, reject) => {
    const proc = spawn(process.execPath, ["--import", "tsx", BRIDGE, ...args], { cwd: PKG_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    const out: string[] = [];
    const timer = setTimeout(() => {
      proc.kill("SIGKILL");
      reject(new Error(`bridge never listened:\n${out.join("\n")}`));
    }, 15000);
    const onLine = (chunk: Buffer): void => {
      for (const line of chunk.toString("utf8").split("\n").filter((l) => l !== "")) {
        out.push(line);
        const m = /listening on http:\/\/127\.0\.0\.1:(\d+)/.exec(line);
        if (m) {
          clearTimeout(timer);
          resolve({ proc, port: Number(m[1]), out });
        }
      }
    };
    proc.stdout!.on("data", onLine);
    proc.stderr!.on("data", onLine);
    proc.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`bridge exited ${code} before listening:\n${out.join("\n")}`));
    });
  });
}

/** Run the bridge CLI to its exit (for configurations it must refuse). */
function runBridgeToExit(args: string[]): Promise<{ code: number | null; out: string }> {
  return new Promise((resolve) => {
    const proc = spawn(process.execPath, ["--import", "tsx", BRIDGE, ...args], { cwd: PKG_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    const timer = setTimeout(() => proc.kill("SIGKILL"), 15000);
    proc.stdout!.on("data", (c: Buffer) => (out += c.toString("utf8")));
    proc.stderr!.on("data", (c: Buffer) => (out += c.toString("utf8")));
    proc.once("exit", (code) => {
      clearTimeout(timer);
      resolve({ code, out });
    });
  });
}

function stopProc(p: ChildProcess | undefined): Promise<void> {
  if (!p || p.exitCode !== null || p.signalCode !== null) return Promise.resolve();
  return new Promise((resolve) => {
    const kill = setTimeout(() => p.kill("SIGKILL"), 3000);
    p.once("exit", () => {
      clearTimeout(kill);
      resolve();
    });
    p.kill("SIGTERM");
  });
}

interface HttpResult {
  status: number;
  body: Record<string, unknown>;
}

/** A raw HTTP request, so the test can send the headers a browser would. */
function http(
  port: number,
  method: string,
  path: string,
  headers: Record<string, string> = {},
  host = "127.0.0.1",
): Promise<HttpResult> {
  return new Promise((resolve, reject) => {
    const req = request({ host, port, method, path, headers, timeout: 10000 }, (res) => {
      let text = "";
      res.on("data", (c: Buffer) => (text += c.toString("utf8")));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body: text ? (JSON.parse(text) as Record<string, unknown>) : {} }));
    });
    req.on("timeout", () => req.destroy(new Error("request timed out")));
    req.on("error", reject);
    req.end();
  });
}

const press = (port: number, page: number, bank: number, key: string, headers: Record<string, string> = {}): Promise<HttpResult> =>
  http(port, "POST", `/press/${page}/${bank}/${encodeURIComponent(key)}`, headers);

function connect(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
    ws.once("unexpected-response", (_req, res) => reject(new Error(`unexpected-response ${res.statusCode}`)));
  });
}

function nextMessage(ws: WebSocket): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("no reply")), 10000);
    ws.once("message", (buf: Buffer) => {
      clearTimeout(timer);
      resolve(JSON.parse(buf.toString("utf8")) as Record<string, unknown>);
    });
  });
}

/** Resolve with the next command response on a control-plane session. */
function send(ws: WebSocket, command: string, payload: Record<string, unknown>): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    const id = randomUUID();
    const onMsg = (buf: Buffer): void => {
      const m = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (m.requestId === id) {
        ws.off("message", onMsg);
        resolve(m);
      }
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function until(what: string, f: () => Promise<boolean> | boolean, ms = 10000): Promise<void> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await f()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`timed out waiting for ${what}`);
}

let state: ControlPlaneState;
let server: ControlPlaneServer;
let audit: AuditRecord[];
let admin: WebSocket;
let bridge: BridgeProc;

async function startCp(port = 0): Promise<void> {
  const log = new AuditLog(join(tempDir("nbe-bridge-audit-"), "audit.jsonl"));
  const orig = log.record.bind(log);
  log.record = (rec) => {
    const full = orig(rec);
    audit.push(full);
    return full;
  };
  server = await createControlPlaneServer({
    port,
    auth: { tokens: { [OP_TOKEN]: "operator", [ADMIN_TOKEN]: "admin" } },
    audit: log,
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
}

const cpUrl = (): string => `ws://127.0.0.1:${server.port}/nbe/v0.3`;
const commandRows = (): AuditRecord[] => audit.filter((r) => r.kind === "command");

before(async () => {
  state = new ControlPlaneState();
  audit = [];
  await startCp();
  admin = new WebSocket(cpUrl(), { headers: { authorization: `Bearer ${ADMIN_TOKEN}`, "x-nbe-role": "admin" } });
  await connect(admin);
  if (!NO_PREFLIGHT) {
    const load = await send(admin, "show.load", { packagePath: makePackage() });
    assert.equal(load.status, "ok", JSON.stringify(load));
    const arm = await send(admin, "preview.set", { itemRef: "A1" });
    assert.equal(arm.status, "ok", JSON.stringify(arm));
  }
  bridge = await spawnBridge([
    "--token-file", tokenFile(OP_TOKEN),
    "--port", "0",
    "--cp", cpUrl(),
    "--profile", PROFILE,
    ...(NO_PREFLIGHT ? [] : ["--package", makePackage()]),
  ]);
  await until("the bridge's control-plane link", async () => {
    const h = await http(bridge.port, "GET", "/health");
    return (h.body.controlPlane as { connected: boolean }).connected;
  });
});

after(async () => {
  await stopProc(bridge?.proc);
  admin?.terminate();
  if (server) await server.close();
});

test("a manifest button pressed over HTTP runs its command on the bridge's operator session, audited as companion/<profile>:<binding>", { skip: NO_PREFLIGHT, timeout: 30000 }, async () => {
  const before = state.stateVersion;
  const r = await press(bridge.port, 1, 1, "0");
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.ok, true);
  assert.equal(r.body.command, "view.take");
  assert.equal(r.body.intentSource, `companion/${PROFILE}:take-1`);
  assert.equal(r.body.stateVersion, before + 1);
  assert.equal(state.viewItem, "A1", "the take put A1 on air");

  const row = commandRows().at(-1)!;
  assert.equal(row.command, "view.take");
  assert.equal(row.outcome, "ok");
  assert.equal(row.role, "operator", "the bridge's token, not the admin's");
  assert.equal(row.tokenId, tokenId(OP_TOKEN));
  assert.equal(row.intentSource, `companion/${PROFILE}:take-1`);

  // A second manifest key on the same bank: the press reaches the right binding.
  const arm = await press(bridge.port, 1, 1, "1");
  assert.equal(arm.status, 200, JSON.stringify(arm.body));
  assert.equal(arm.body.command, "preview.set");
  assert.equal(state.previewItem, "A2");
  assert.equal(commandRows().at(-1)!.intentSource, `companion/${PROFILE}:arm-a2`);
});

test("the bridge's WebSocket takes Companion's message form, with the Origin Companion's module sends", { skip: NO_PREFLIGHT, timeout: 30000 }, async () => {
  const ws = new WebSocket(`ws://127.0.0.1:${bridge.port}/ws`, { headers: { origin: "http://127.0.0.1" } });
  await connect(ws);
  try {
    // Set up this test's own preview (the item not on air), so it does not
    // depend on what earlier tests left on air.
    const target = state.viewItem === "A2" ? "A1" : "A2";
    const arm = await send(admin, "preview.set", { itemRef: target });
    assert.equal(arm.status, "ok", JSON.stringify(arm));

    // The default deck's page (0: the manifest uses page 1 only), in the
    // text form Companion's module sends (value plus a line terminator).
    ws.send("0/1/take\r\n");
    const a = await nextMessage(ws);
    assert.equal(a.httpStatus, 200, JSON.stringify(a));
    assert.equal(a.command, "view.take");
    assert.equal(a.intentSource, `companion/${PROFILE}:default-take`);
    assert.equal(state.viewItem, target);
    assert.equal(commandRows().at(-1)!.intentSource, `companion/${PROFILE}:default-take`);

    // The JSON form, and a refusal over the WebSocket: the default SFX 1
    // button's placeholder asset is not in this package.
    ws.send(JSON.stringify({ page: 0, bank: 1, key: "sfx-1" }));
    const b = await nextMessage(ws);
    assert.equal(b.httpStatus, 409, JSON.stringify(b));
    assert.equal(b.command, "soundboard.play");
    assert.equal(commandRows().at(-1)!.intentSource, `companion/${PROFILE}:default-sfx-1`);

    ws.send("not a press");
    const c = await nextMessage(ws);
    assert.equal(c.httpStatus, 400);
  } finally {
    ws.terminate();
  }
});

test("a control-plane refusal comes back with the control plane's own code, and is audited with the button's identity", { skip: NO_PREFLIGHT, timeout: 30000 }, async () => {
  // The default SFX 1 button names the placeholder asset `sfx-1`, which this
  // package does not have.
  const rows = commandRows().length;
  const r = await press(bridge.port, 0, 1, "sfx-1");
  assert.equal(r.status, 409, JSON.stringify(r.body));
  assert.equal(r.body.ok, false);
  assert.equal(r.body.command, "soundboard.play");
  const code = (r.body.error as { code: string }).code;
  assert.match(code, /^E_/, "a §16 code, passed through");
  const row = commandRows().at(-1)!;
  assert.equal(commandRows().length, rows + 1);
  assert.equal(row.outcome, "rejected");
  assert.equal(row.errorCode, code);
  assert.equal(row.intentSource, `companion/${PROFILE}:default-sfx-1`);
});

test("an unknown button and a malformed press reach nothing", { timeout: 30000 }, async () => {
  const rows = commandRows().length;
  const r = await press(bridge.port, 9, 9, "nope");
  assert.equal(r.status, 404);
  assert.equal((r.body.error as { code: string }).code, "BRIDGE_NO_BUTTON");
  const bad = await http(bridge.port, "POST", "/press/x/1/take");
  assert.equal(bad.status, 400);
  assert.equal(commandRows().length, rows, "no command reached the control plane");
});

test("browser-shaped requests are refused: a cross-site POST, a DNS-rebound Host, a GET press, a cross-site WebSocket", { timeout: 30000 }, async () => {
  const rows = commandRows().length;
  const versionBefore = state.stateVersion;

  const crossSite = await press(bridge.port, 0, 1, "take", { origin: "https://evil.example" });
  assert.equal(crossSite.status, 403);
  assert.equal((crossSite.body.error as { code: string }).code, "BRIDGE_FORBIDDEN");

  const rebound = await press(bridge.port, 0, 1, "take", { host: `evil.example:${bridge.port}` });
  assert.equal(rebound.status, 403);

  const nullOrigin = await press(bridge.port, 0, 1, "take", { origin: "null" });
  assert.equal(nullOrigin.status, 403, "a sandboxed page's Origin: null");

  const get = await http(bridge.port, "GET", "/press/0/1/take");
  assert.equal(get.status, 405);

  const ws = new WebSocket(`ws://127.0.0.1:${bridge.port}/ws`, { headers: { origin: "https://evil.example" } });
  await assert.rejects(connect(ws), /unexpected-response 403/);

  assert.equal(commandRows().length, rows, "no refused request reached the control plane");
  assert.equal(state.stateVersion, versionBefore);

  // A loopback Origin with a port (a page served from this machine) and no
  // Origin at all (Companion's HTTP module) are both allowed through to the
  // button lookup.
  const local = await press(bridge.port, 9, 9, "nope", { origin: "http://localhost:8000" });
  assert.equal(local.status, 404);
});

test("the bridge listens on loopback only", { timeout: 30000 }, async (t) => {
  const lan = Object.values(networkInterfaces())
    .flat()
    .find((a) => a && a.family === "IPv4" && !a.internal);
  if (!lan) {
    t.skip("this machine has no non-loopback IPv4 address");
    return;
  }
  await assert.rejects(http(bridge.port, "GET", "/health", {}, lan.address), /ECONNREFUSED/);
});

test("control-plane auth is unchanged: a header-less client is refused, and a bridge with a wrong token presses nothing", { timeout: 30000 }, async () => {
  // Companion's own modules send no Authorization header.
  const bare = new WebSocket(cpUrl(), { headers: { origin: "http://127.0.0.1" } });
  await assert.rejects(connect(bare), /unexpected-response 401/);

  const wrong = await spawnBridge(["--token-file", tokenFile(`wrong-${randomUUID()}`), "--port", "0", "--cp", cpUrl()]);
  try {
    await until("the wrong token's refusal", async () => {
      const h = await http(wrong.port, "GET", "/health");
      return /401/.test(String((h.body.controlPlane as { lastError: string | null }).lastError));
    });
    const rows = commandRows().length;
    const r = await press(wrong.port, 0, 1, "take");
    assert.equal(r.status, 503);
    assert.equal((r.body.error as { code: string }).code, "BRIDGE_NOT_CONNECTED");
    assert.equal(commandRows().length, rows);
    assert.ok(audit.some((a) => a.kind === "auth" && a.outcome === "rejected"), "the control plane audited the refused token");
  } finally {
    await stopProc(wrong.proc);
  }
});

test("the token file must be owner-only, and a token on the command line is not accepted", { timeout: 30000 }, async () => {
  const open = await runBridgeToExit(["--token-file", tokenFile(OP_TOKEN, 0o644), "--port", "0", "--cp", cpUrl()]);
  assert.equal(open.code, 2, open.out);
  assert.match(open.out, /readable by group or others/);
  assert.doesNotMatch(open.out, new RegExp(OP_TOKEN), "the token is never printed");

  const none = await runBridgeToExit(["--port", "0", "--cp", cpUrl()]);
  assert.equal(none.code, 2, none.out);
  assert.match(none.out, /--token-file/);
});

test("the bridge reconnects when the control plane restarts", { skip: NO_PREFLIGHT, timeout: 60000 }, async () => {
  const port = server.port;
  admin.terminate();
  await server.close();
  await until("the bridge to see the control plane go", async () => {
    const h = await http(bridge.port, "GET", "/health");
    return !(h.body.controlPlane as { connected: boolean }).connected;
  });
  const down = await press(bridge.port, 1, 1, "0");
  assert.equal(down.status, 503);

  await startCp(port);
  admin = new WebSocket(cpUrl(), { headers: { authorization: `Bearer ${ADMIN_TOKEN}`, "x-nbe-role": "admin" } });
  await connect(admin);
  await until("the bridge to reconnect", async () => {
    const h = await http(bridge.port, "GET", "/health");
    return (h.body.controlPlane as { connected: boolean }).connected;
  });
  // The press reaches the restarted control plane: it answers (accepted or
  // refused, depending on what earlier tests left on air), and audits the
  // press with the button's identity.
  const rows = commandRows().length;
  const up = await press(bridge.port, 1, 1, "0");
  assert.ok(up.status === 200 || up.status === 409, JSON.stringify(up.body));
  if (up.status === 409) assert.match((up.body.error as { code: string }).code, /^E_/, "the control plane's own code");
  assert.equal(commandRows().length, rows + 1);
  assert.equal(commandRows().at(-1)!.intentSource, `companion/${PROFILE}:take-1`);
});
