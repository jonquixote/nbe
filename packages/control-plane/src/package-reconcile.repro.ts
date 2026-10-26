//! R32's reproducer (docs/soak-protocol.md, R32; the user's ruling of
//! 2026-10-26, (b)).
//!
//! NOT A TEST, and not in `npm test`. Run it on the normative machine, on AC
//! power under `caffeinate`:
//!
//!   npm run repro:r32 -- --n 50 --load 12          # 50 runs, 12 busy processes
//!   npm run repro:r32 -- --n 10 --load 0 --no-reload   # the scenario as test 4 runs it
//!
//! R32: `package-reconcile.e2e.ts` test 4 failed twice on CI with "the engine
//! never applied its resync": its reconnect waited the 20 s bound and the
//! engine's apply never came. Both sightings were load-blind, and the log
//! the step printed could not say whether the reconnect reloaded the package.
//! Test 4's reconnect is meant to load nothing (the identity matches); a
//! reload there, slow under load, would take the apply past the bound.
//!
//! Each run here is test 4's scenario: `show.stop`; `show.load` (reload) with
//! another connection's command landing inside its preflight; `show.start`;
//! `view.cut A1`; the engine applies everything. Then the reconnect, with the
//! reload INDUCED (unless `--no-reload`): the control plane's load generation
//! is moved one past the one the engine holds, so the snapshot names an
//! identity the engine does not hold and the reconnect reloads the dress
//! package. The run is timed from the reconnect's registration to the
//! engine's apply of that resync, against test 4's 20 s bound.
//!
//! `--load N` runs N busy processes (`yes`) for the whole run: the inflated
//! load. load1 is sampled at each run's start and end.
//!
//! Kept, under `.git/nbe-staged/r32/repro-<UTC stamp>/` (or `--out`): every
//! run (`runs.jsonl`), the engine's log, the meta, the summary.
//!
//! A run past 20 s is a REPRODUCTION, falsified normally (the hunt). About 50
//! clean runs demote R32 to watch (the same ruling).

import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer, connect, type Server, type Socket } from "node:net";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
/** Test 4's bound for the reconnect's apply (`REPORT_MS`). */
const BOUND_MS = 20_000;
/** How long a run waits before giving up on an apply that never comes. */
const GIVE_UP_MS = 120_000;

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`);
  return i < 0 ? undefined : process.argv[i + 1];
}
const N = Number(arg("n") ?? 50);
const LOAD = Number(arg("load") ?? 0);
const INDUCE = !process.argv.includes("--no-reload");
const OUT = resolve(
  arg("out") ?? join(REPO, ".git/nbe-staged/r32", `repro-${new Date().toISOString().replace(/[:.]/g, "-")}`),
);

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess | undefined;
let proxy: Server;
const proxied = new Set<Socket>();
let ws: WebSocket;
let ws2: WebSocket;
const engineLog: string[] = [];
const busy: ChildProcess[] = [];

function send(command: string, payload: Record<string, unknown>, on: WebSocket): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error(`no response to ${command}`)), GIVE_UP_MS);
    const onMsg = (buf: Buffer): void => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg["requestId"] !== id) return;
      clearTimeout(timer);
      on.off("message", onMsg);
      res(msg);
    };
    on.on("message", onMsg);
    on.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function ok(command: string, payload: Record<string, unknown> = {}, on: WebSocket = ws): Promise<Record<string, unknown>> {
  const r = await send(command, payload, on);
  if (r["status"] !== "ok") throw new Error(`${command}: ${JSON.stringify(r)}`);
  return r;
}

async function lastApplied(): Promise<number | null> {
  const res = await fetch(`http://127.0.0.1:${server.port}/nbe/v0.3/status`);
  const body = (await res.json()) as { renderNode?: { lastAppliedStateVersion?: number | null } };
  return body.renderNode?.lastAppliedStateVersion ?? null;
}

async function until(pred: () => boolean | Promise<boolean>, ms: number): Promise<number | null> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await pred()) return Date.now() - t0;
    await sleep(25);
  }
  return null;
}

/** The control plane's audit rows (test 4 reads the race from them). */
function auditRecords(): Array<Record<string, unknown>> {
  const path = join(OUT, "audit.jsonl");
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

/** The engine's log since `mark` (a character offset), as lines. */
function logSince(mark: number): string[] {
  return engineLog.join("").slice(mark).split("\n");
}

async function main(): Promise<void> {
  // Under inflated load preflight's media decode slows past its 60 s floor
  // (48 busy processes on 12 hardware threads starved the first show.load's
  // preflight outright). The measurement here is the reconnect's reload, not
  // preflight's speed, so the reproducer gives preflight 300 s unless told
  // otherwise, through the control plane's own knob; the meta records it.
  process.env["NBE_PREFLIGHT_TIMEOUT_MS"] ??= "300000";
  mkdirSync(OUT, { recursive: true });
  const runs = createWriteStream(join(OUT, "runs.jsonl"));
  const engineFile = createWriteStream(join(OUT, "engine.log"));
  const meta = {
    reproducer: "R32",
    n: N,
    busyProcesses: LOAD,
    induceReload: INDUCE,
    engine: ENGINE_BIN,
    package: DRESS,
    boundMs: BOUND_MS,
    preflightTimeoutMs: Number(process.env["NBE_PREFLIGHT_TIMEOUT_MS"]),
    startedAt: new Date().toISOString(),
    loadAtStart: loadavg(),
  };
  writeFileSync(join(OUT, "meta.json"), JSON.stringify(meta, null, 2));

  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(OUT, "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  proxy = createServer((client) => {
    const upstream = connect(server.port, "127.0.0.1");
    proxied.add(client);
    proxied.add(upstream);
    client.pipe(upstream);
    upstream.pipe(client);
    const drop = (): void => {
      client.destroy();
      upstream.destroy();
      proxied.delete(client);
      proxied.delete(upstream);
    };
    for (const s of [client, upstream]) {
      s.on("error", drop);
      s.on("close", drop);
    }
  });
  await new Promise<void>((r) => proxy.listen(0, "127.0.0.1", () => r()));
  const proxyPort = (proxy.address() as { port: number }).port;
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${proxyPort}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_AUDIO_SINK: process.env.NBE_AUDIO_SINK ?? "null",
      NBE_HOUSE_RATE: "30",
      RUST_LOG: "warn,nbe_engine::directive=info",
      NO_COLOR: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const onEngine = (b: Buffer): void => {
    engineLog.push(b.toString());
    engineFile.write(b);
  };
  engine.stdout?.on("data", onEngine);
  engine.stderr?.on("data", onEngine);
  if ((await until(() => server.wsBridge.renderNodeCount() === 1, GIVE_UP_MS)) === null) throw new Error("the engine never registered");
  const open = async (): Promise<WebSocket> => {
    const w = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
    });
    await new Promise<void>((res, rej) => {
      w.once("open", () => res());
      w.once("error", rej);
    });
    return w;
  };
  ws = await open();
  ws2 = await open();
  // The inflated load starts once the harness stands: it is the runs' load,
  // not the engine's start (48 busy processes starved the engine's start past
  // 30 s on the first try, before any run).
  for (let i = 0; i < LOAD; i++) {
    busy.push(spawn("yes", [], { stdio: "ignore" }));
  }
  console.log(`R32 REPRO: n=${N}, ${LOAD} busy process(es), reload ${INDUCE ? "induced" : "not induced"}; kept in ${OUT}`);

  let pastBound = 0;
  const applies: number[] = [];
  for (let run = 0; run < N; run++) {
    const load0 = loadavg()[0] ?? 0;
    // Test 4's scenario, as the e2e runs it. The first run starts STOPPED.
    const stop = await send("show.stop", {}, ws);
    if (stop["status"] !== "ok" && (stop["error"] as Record<string, unknown> | undefined)?.["code"] !== "E_FORBIDDEN_STATE") {
      throw new Error(`show.stop: ${JSON.stringify(stop)}`);
    }
    let loaded: Record<string, unknown> = {};
    let landedInside = false;
    for (let attempt = 0; attempt < 3 && !landedInside; attempt++) {
      const pending = send("show.load", { packagePath: DRESS, mode: "reload" }, ws);
      await sleep(100);
      await ok("automation.hold", { hold: attempt % 2 === 0 }, ws2);
      loaded = await pending;
      if (loaded["status"] !== "ok") throw new Error(`show.load: ${JSON.stringify(loaded)}`);
      // The other connection's bump landed inside the load exactly when the
      // load's own audit row spans two versions (test 4's precondition).
      const row = auditRecords().find((r) => r["command"] === "show.load" && r["stateVersionAfter"] === loaded["stateVersion"]);
      landedInside = row !== undefined && (row["stateVersionAfter"] as number) - (row["stateVersionBefore"] as number) === 2;
    }
    await ok("show.start", {});
    await ok("view.cut", { itemRef: "A1" });
    const settled = await until(async () => (await lastApplied()) === state.stateVersion, 2 * BOUND_MS);
    if (settled === null) throw new Error(`run ${run}: the engine never applied the load and the cut`);
    // The reload, induced: the snapshot will name an identity the engine
    // does not hold, as a generation mismatch would.
    const heldGeneration = state.packageLoadStateVersion;
    if (INDUCE && heldGeneration !== null) state.packageLoadStateVersion = heldGeneration + 1;
    const mark = engineLog.join("").length;
    for (const s of proxied) s.destroy();
    if ((await until(() => server.wsBridge.renderNodeCount() === 0, BOUND_MS)) === null) throw new Error("the connection never went");
    if ((await until(() => server.wsBridge.renderNodeCount() === 1, GIVE_UP_MS)) === null) throw new Error("the engine never reconnected");
    const registeredAt = Date.now();
    const sv = state.stateVersion;
    const applied = await until(async () => (await lastApplied()) === sv, GIVE_UP_MS);
    const applyMs = applied === null ? null : Date.now() - registeredAt;
    const lines = logSince(mark);
    const reloadLine = lines.find((l) => l.includes("show.resync: the package was reloaded"));
    const audioLine = lines.find((l) => l.includes("audio sources built"));
    const record = {
      run,
      at: new Date().toISOString(),
      load1Start: load0,
      load1End: loadavg()[0] ?? 0,
      landedInside,
      generation: { held: heldGeneration, snapshot: state.packageLoadStateVersion },
      reloading: lines.some((l) => l.includes("reloading")),
      reloaded: reloadLine !== undefined,
      reloadMs: Number(reloadLine?.match(/elapsed_ms=(\d+)/)?.[1] ?? NaN) || null,
      audioSourcesMs: Number(audioLine?.match(/elapsed_ms=(\d+)/)?.[1] ?? NaN) || null,
      applyMs,
      pastBound: applyMs === null || applyMs > BOUND_MS,
    };
    if (record.pastBound) pastBound += 1;
    if (applyMs !== null) applies.push(applyMs);
    runs.write(JSON.stringify(record) + "\n");
    console.log(
      `run ${run}: apply ${applyMs ?? "NEVER"} ms; reloaded ${record.reloaded} (${record.reloadMs ?? "-"} ms, audio ${record.audioSourcesMs ?? "-"} ms); ` +
        `load1 ${record.load1Start.toFixed(2)} -> ${record.load1End.toFixed(2)}${record.pastBound ? "  PAST THE 20 s BOUND" : ""}`,
    );
  }
  const sorted = [...applies].sort((a, b) => a - b);
  const summary = {
    n: N,
    pastBound,
    applyMs: { min: sorted[0] ?? null, median: sorted[Math.floor(sorted.length / 2)] ?? null, max: sorted.at(-1) ?? null },
    endedAt: new Date().toISOString(),
    loadAtEnd: loadavg(),
    verdict: pastBound > 0 ? "REPRODUCED" : "CLEAN",
  };
  writeFileSync(join(OUT, "summary.json"), JSON.stringify(summary, null, 2));
  console.log(`R32 REPRO: ${summary.verdict}: ${pastBound} of ${N} past the bound; apply ${JSON.stringify(summary.applyMs)} ms`);
  runs.end();
  engineFile.end();
}

function cleanup(): void {
  for (const b of busy) b.kill("SIGKILL");
  engine?.kill("SIGKILL");
  ws?.close();
  ws2?.close();
  for (const s of proxied) s.destroy();
  proxy?.close();
}

main()
  .then(async () => {
    cleanup();
    await server.close();
  })
  .catch((e) => {
    console.error(e);
    cleanup();
    process.exit(2);
  });
process.on("SIGINT", () => {
  cleanup();
  process.exit(130);
});
