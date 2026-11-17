//! `nbe run`: the show's processes under the supervisor (review4 step 6).
//!
//!   NBE_ENGINE_BIN=… NBE_TOKENS=… NBE_RENDER_TOKEN=… NBE_AUDIT_LOG=… \
//!     node --import tsx src/run.ts
//!
//! Optional: NBE_MEDIAMTX and NBE_MEDIAMTX_CONF (the guest media server,
//! started first and restarted), NBE_PORT (8462), NBE_STATE_DIR (logs go to
//! `<state dir>/logs`). The control plane is started and NOT restarted until
//! review4 step 7 (adoption); the engine and the media server are restarted.
//! An engine restart does not resume the recording or the stream (step 7).
//! Run it under launchd `KeepAlive` so the supervisor itself comes back.

import { createConnection } from "node:net";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { DEFAULT_PORT } from "./protocol.js";
import { Supervisor, type ChildSpec } from "./supervisor.js";

/** The named variables of this process's environment that are set. */
function pick(names: string[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const n of names) {
    const v = process.env[n];
    if (v !== undefined) out[n] = v;
  }
  return out;
}

/** The variables the engine reads (every `env::var` in nbe-engine and
 * nbe-decode, 2026-11-17), less the two run.ts sets itself (NBE_CP_URL,
 * NBE_RENDER_TOKEN) and the synthetic-camera test seams. An allow-list, not
 * "every NBE_ but the secrets": a variable added later is not forwarded
 * until it is named here. */
export const ENGINE_ENV = [
  "NBE_AUDIO_SINK",
  "NBE_CAMERAS",
  "NBE_MICS",
  "NBE_HOUSE_RATE",
  "NBE_MEDIA_SERVER",
  "NBE_MEDIA_RTSP_PORT",
  "NBE_FFMPEG",
  "NBE_RECORD_TRACE",
  "RUST_LOG",
] as const;

/** The engine's variables from `from` that are set. */
export function engineEnv(from: NodeJS.ProcessEnv = process.env): Record<string, string> {
  const out: Record<string, string> = {};
  for (const k of ENGINE_ENV) {
    const v = from[k];
    if (v !== undefined) out[k] = v;
  }
  return out;
}

const here = dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.NBE_PORT ?? DEFAULT_PORT);
const statusUrl = `http://127.0.0.1:${port}/nbe/v0.3/status`;

async function status(): Promise<Record<string, unknown> | null> {
  try {
    const r = await fetch(statusUrl);
    return r.ok ? ((await r.json()) as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function tcpOpen(p: number): Promise<boolean> {
  return new Promise((res) => {
    const s = createConnection({ host: "127.0.0.1", port: p });
    s.once("connect", () => {
      s.destroy();
      res(true);
    });
    s.once("error", () => res(false));
  });
}

function required(name: string): string {
  const v = process.env[name];
  if (!v) {
    console.error(`nbe run: ${name} is required`);
    process.exit(1);
  }
  return v;
}

async function main(): Promise<void> {
  const engineBin = required("NBE_ENGINE_BIN");
  const renderToken = required("NBE_RENDER_TOKEN");
  required("NBE_TOKENS");
  required("NBE_AUDIT_LOG");
  const stateDir = resolve(process.env.NBE_STATE_DIR ?? join(homedir(), "Library", "Application Support", "nbe"));

  const specs: ChildSpec[] = [];
  if (process.env.NBE_MEDIAMTX) {
    specs.push({
      name: "media-server",
      command: process.env.NBE_MEDIAMTX,
      args: process.env.NBE_MEDIAMTX_CONF ? [process.env.NBE_MEDIAMTX_CONF] : [],
      restart: true,
      ready: () => tcpOpen(Number(process.env.NBE_MEDIA_RTSP_PORT ?? 8554)),
    });
  }
  specs.push({
    name: "control-plane",
    command: process.execPath,
    args: ["--import", "tsx", join(here, "index.ts")],
    cwd: resolve(here, ".."),
    env: pick([
      "NBE_TOKENS",
      "NBE_AUDIT_LOG",
      "NBE_STATE_DIR",
      "NBE_ALLOW_TEMP_STATE",
      "NBE_PORT",
      "NBE_HOST",
      "NBE_HOUSE_RATE",
      "NBE_PREFLIGHT_BIN",
    ]),
    // NOT until review4 step 7: restarted today it would resync a running
    // engine to UNLOADED and stop the show.
    restart: false,
    ready: async () => (await status()) !== null,
  });
  specs.push({
    name: "engine",
    command: engineBin,
    env: { ...engineEnv(), NBE_CP_URL: `ws://127.0.0.1:${port}/nbe/v0.3`, NBE_RENDER_TOKEN: renderToken },
    restart: true,
    ready: async () => {
      const s = await status();
      return (s?.["renderNode"] as { connected?: boolean } | undefined)?.connected === true;
    },
  });

  const sup = new Supervisor(specs, {
    logDir: join(stateDir, "logs"),
    onEvent: (e) => console.log(JSON.stringify({ ts: new Date().toISOString(), ...e })),
  });
  let stopping = false;
  const stop = async (signal: string): Promise<void> => {
    if (stopping) return;
    stopping = true;
    console.log(JSON.stringify({ ts: new Date().toISOString(), kind: "stopping", signal }));
    await sup.stop();
    process.exit(0);
  };
  process.on("SIGTERM", () => void stop("SIGTERM"));
  process.on("SIGINT", () => void stop("SIGINT"));
  try {
    await sup.start();
  } catch (e) {
    console.error(`nbe run: ${String(e instanceof Error ? e.message : e)}`);
    await sup.stop();
    process.exit(1);
  }
  console.log(JSON.stringify({ ts: new Date().toISOString(), kind: "running", pids: sup.pids(), caffeinate: sup.caffeinatePid ?? null }));
}

// Run when executed (`node --import tsx src/run.ts`), not when imported by a test.
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) void main();
