//! SPEC v0.4.8 row 5 — the latency a package reconciliation adds to a resync,
//! measured on the dress package.
//!
//! NOT A TEST, and not in `npm test`: a timing claim from a shared runner is
//! noise (R9: the wall bound tracks load). Run it on a quiescent machine, on
//! AC power (`docs/soak-protocol.md` §2, precondition 6). Each tier prints the
//! load average at its start and end, and a tier that reads 3.0 or more at
//! either end is VOID (the quiescence ceiling). Paste the whole output.
//!
//!   npm run measure:package-reconcile                 # 5 restarts per tier
//!   npm run measure:package-reconcile -- --n 10
//!
//! The real control plane and the real engine binary. Each sample kills the
//! engine, restarts it, and times from its registration (the moment the
//! control plane sends `show.resync`) to the engine's `appliedStateVersion` for
//! that resync (the resync fully applied). Two tiers:
//! - **baseline**: no package loaded, so the snapshot names none and no load
//!   runs.
//! - **reload**: the dress package loaded and running with A1 on air, so a
//!   restarted engine, holding none, reloads it before the rest of the resync.
//!
//! The added latency is the difference of the medians.

import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
const N = Number(process.argv[process.argv.indexOf("--n") + 1] ?? NaN) || 5;
const CEILING = 3.0;

let server: ControlPlaneServer;
let state: ControlPlaneState;
let engine: ChildProcess | undefined;
let ws: WebSocket;

const load = (): string => loadavg().map((l) => l.toFixed(2)).join(" ");
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until(pred: () => boolean | Promise<boolean>, ms: number): Promise<boolean> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await pred()) return true;
    await sleep(2);
  }
  return false;
}

function send(command: string, payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error(`no response to ${command}`)), 180_000);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      clearTimeout(timer);
      ws.off("message", onMsg);
      res(msg);
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload }));
  });
}

async function ok(command: string, payload: Record<string, unknown> = {}): Promise<void> {
  const r = await send(command, payload);
  if (r.status !== "ok") throw new Error(`${command}: ${JSON.stringify(r)}`);
}

async function lastApplied(): Promise<number | null> {
  const res = await fetch(`http://127.0.0.1:${server.port}/nbe/v0.3/status`);
  const body = (await res.json()) as { renderNode?: { lastAppliedStateVersion?: number | null } };
  return body.renderNode?.lastAppliedStateVersion ?? null;
}

function spawnEngine(): void {
  engine = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_HOUSE_RATE: "30",
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "ignore", "ignore"],
  });
}

/** One restart: registration (the resync sent) to the engine's ack of that resync, in ms. */
async function sample(): Promise<number> {
  engine?.kill("SIGKILL");
  if (!(await until(() => server.wsBridge.renderNodeCount() === 0, 20_000))) throw new Error("engine never went");
  spawnEngine();
  if (!(await until(() => server.wsBridge.renderNodeCount() === 1, 30_000))) throw new Error("engine never registered");
  const registeredAt = performance.now();
  const resyncVersion = state.stateVersion;
  if (!(await until(async () => (await lastApplied()) === resyncVersion, 120_000))) {
    throw new Error("the engine never acknowledged its resync");
  }
  return performance.now() - registeredAt;
}

function summary(name: string, xs: number[], startLoad: string, endLoad: string): number {
  const sorted = [...xs].sort((a, b) => a - b);
  const median = sorted[Math.floor(sorted.length / 2)]!;
  const void_ = [startLoad, endLoad].some((l) => Number(l.split(" ")[0]) >= CEILING);
  console.log(
    `${name}: n=${xs.length} ms=[${xs.map((x) => x.toFixed(1)).join(", ")}] median=${median.toFixed(1)} max=${sorted.at(-1)!.toFixed(1)}` +
      ` load ${startLoad} -> ${endLoad}${void_ ? "  VOID (load at or over the 3.0 ceiling)" : ""}`,
  );
  return median;
}

async function main(): Promise<void> {
  state = new ControlPlaneState();
  server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(join(tempDir("nbe-reconcile-measure-"), "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    warn: () => {},
  });
  spawnEngine();
  if (!(await until(() => server.wsBridge.renderNodeCount() === 1, 30_000))) throw new Error("engine never registered");
  ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${ADMIN}`, "x-nbe-role": "admin" },
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  console.log(`package reconcile latency — engine ${ENGINE_BIN}, package ${DRESS}, n=${N} per tier`);

  let start = load();
  const baseline: number[] = [];
  for (let i = 0; i < N; i++) baseline.push(await sample());
  const baseMedian = summary("baseline (no package named, no load)", baseline, start, load());

  await ok("show.load", { packagePath: DRESS });
  await ok("show.start", {});
  await ok("view.cut", { itemRef: "A1" });
  await sleep(1000);
  start = load();
  const reload: number[] = [];
  for (let i = 0; i < N; i++) reload.push(await sample());
  const reloadMedian = summary("reload (the dress package reloaded on resync)", reload, start, load());

  console.log(`added by the reconciliation (median reload - median baseline): ${(reloadMedian - baseMedian).toFixed(1)} ms`);
  engine?.kill("SIGKILL");
  ws.close();
  await server.close();
}

main().catch((e) => {
  console.error(e);
  engine?.kill("SIGKILL");
  process.exit(1);
});
