//! R17 spine part A: how long `stream.start` takes to be acknowledged, against
//! the 2000 ms window command confirmation uses (SPEC §5.9.5's window).
//!
//! NOT A TEST, and not in `npm test`: a timing claim from a shared runner is
//! noise. Run it on a quiescent machine, on AC power, under `caffeinate`
//! (`docs/soak-protocol.md` §2, precondition 6). Each sample waits for the
//! 1-minute load to fall under 2.5, and a sample that reads 3.0 or more at
//! either end is VOID. Paste the whole output.
//!
//!   caffeinate -i -s node --import tsx src/stream-start-ack.measure.ts --n 20
//!
//! Each sample is a fresh control plane and a fresh engine (so the encoder
//! opens cold, as a show's first start does), the dress package on air, then
//! `stream.start` into a local RTMP sink. Times: the send to the engine's
//! `appliedStateVersion` for it reaching the control plane.

import { loadavg } from "node:os";
import { performance } from "node:perf_hooks";
import { cpSync } from "node:fs";
import { join, resolve } from "node:path";

import { send, stack, startRtmpSink } from "./show-stop.harness.js";
import { tempDir } from "./test-tmp.js";

const N = Number(process.argv[process.argv.indexOf("--n") + 1] ?? NaN) || 20;
const WINDOW_MS = 2000;
const START_BELOW = 2.5;
const CEILING = 3.0;
const REPO = resolve(import.meta.dirname, "../../..");
const DRESS = join(REPO, "tests/fixtures/dress_show");

const load1 = (): number => loadavg()[0]!;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const q = (xs: number[], p: number): number => {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.max(1, Math.ceil((s.length * p) / 100)) - 1]!;
};

async function sample(): Promise<number> {
  const s = await stack();
  const sink = await startRtmpSink();
  try {
    const ok = async (command: string, payload: Record<string, unknown> = {}): Promise<number> => {
      const r = await send(s.ws, command, payload);
      if (r.status !== "ok") throw new Error(`${command}: ${JSON.stringify(r)}`);
      return r["stateVersion"] as number;
    };
    const pkg = join(tempDir("nbe-ssa-pkg-"), "dress_show");
    cpSync(DRESS, pkg, { recursive: true });
    if (!(await s.server.awaitApplied(await ok("show.load", { packagePath: pkg }), 20_000))) throw new Error("show.load unacknowledged");
    if (!(await s.server.awaitApplied(await ok("show.start", { startClock: true }), 20_000))) throw new Error("show.start unacknowledged");
    await ok("view.cut", { itemRef: "A1" });
    await sleep(500);
    const predicted = s.state.stateVersion + 1;
    let ackAt = Number.NaN;
    const acked = s.server.awaitApplied(predicted, 20_000).then((a) => {
      if (a) ackAt = performance.now();
    });
    const t0 = performance.now();
    const v = await ok("stream.start", { url: `rtmp://127.0.0.1:${sink.port}/live/ssa` });
    if (v !== predicted) throw new Error(`stream.start took ${v}, expected ${predicted}`);
    await acked;
    if (!Number.isFinite(ackAt)) throw new Error("stream.start was never acknowledged (a refusal: no hardware encoder here?)");
    await ok("stream.stop", {});
    return ackAt - t0;
  } finally {
    sink.close();
    await s.close();
  }
}

async function main(): Promise<void> {
  console.log(`STREAM-START-ACK n=${N} window=${WINDOW_MS} ms node=${process.version} start-below=${START_BELOW} ceiling=${CEILING}`);
  const kept: number[] = [];
  let voids = 0;
  let i = 0;
  while (kept.length < N) {
    i += 1;
    while (load1() >= START_BELOW) await sleep(10_000);
    const before = load1();
    const ms = await sample();
    const after = load1();
    const v = before >= CEILING || after >= CEILING;
    console.log(`STREAM-START-ACK-SAMPLE ${JSON.stringify({ i, load: [Number(before.toFixed(2)), Number(after.toFixed(2))], void: v, ackMs: Number(ms.toFixed(1)) })}`);
    if (v) voids += 1;
    else kept.push(ms);
  }
  console.log(`\nSTREAM-START-ACK-SUMMARY kept=${kept.length} void=${voids} (ms; nearest-rank)`);
  console.log(`median ${q(kept, 50).toFixed(1)} | p95 ${q(kept, 95).toFixed(1)} | max ${Math.max(...kept).toFixed(1)} | min ${Math.min(...kept).toFixed(1)}`);
  const worst = Math.max(...kept);
  console.log(`worst ${worst.toFixed(1)} ms of the ${WINDOW_MS} ms window: margin ${(WINDOW_MS - worst).toFixed(1)} ms (${((worst / WINDOW_MS) * 100).toFixed(1)}% of the window)`);
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error(e);
    process.exit(1);
  },
);
