//! SPEC v0.4.8 row 6 — how long `show.stop` takes, with both outputs active,
//! from the operator's send to the engine's acknowledgement, against §16.1's
//! 2 s window.
//!
//! NOT A TEST, and not in `npm test`: a timing claim from a shared runner is
//! noise (R9: the wall bound tracks load). Run it on a quiescent machine, on
//! AC power, under `caffeinate` (`docs/soak-protocol.md` §2, precondition 6).
//! Each sample waits for the 1-minute load to fall under 2.5 before it starts,
//! and a sample that reads 3.0 or more at either end is VOID (the quiescence
//! ceiling). Paste the whole output.
//!
//!   caffeinate -i npm run measure:show-stop               # 20 stops
//!   caffeinate -i npm run measure:show-stop -- --n 30
//!
//! Each sample is the e2e's E1 leg (`show-stop.harness.ts`): a fresh control
//! plane and a fresh engine, the dress package on air, record and stream
//! running with two markers, then `show.stop`. Times:
//! - **stop→ack**: the operator's send to the engine's `appliedStateVersion`
//!   for the stop reaching the control plane — the span §16.1's window bounds;
//! - **stop→response**: the send to the operator's response, which returns on
//!   that acknowledgement;
//! - per phase, from the moment the stop crosses the tap into the engine: to
//!   the engine's own `stream quiesced` and `recording quiesced` log lines
//!   (the two halves run concurrently), and to the acknowledgement crossing
//!   back. The engine's lines carry its UTC stamps; the tap's are the same
//!   machine's clock, so the phases are good to about a millisecond.
//!
//! The tap adds a local relay hop each way; it is in every number here.

import { loadavg } from "node:os";

import { GRACE_MS, leg, type Timing } from "./show-stop.harness.js";

const N = Number(process.argv[process.argv.indexOf("--n") + 1] ?? NaN) || 20;
const START_BELOW = 2.5;
const CEILING = 3.0;

const load1 = (): number => loadavg()[0]!;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function q(xs: number[], p: number): number {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.max(1, Math.ceil((s.length * p) / 100)) - 1]!;
}

async function main(): Promise<void> {
  console.log(`SHOW-STOP-MEASURE n=${N} grace=${GRACE_MS} ms node=${process.version} start-below=${START_BELOW} ceiling=${CEILING}`);
  const kept: Timing[] = [];
  let voids = 0;
  let i = 0;
  while (kept.length < N) {
    i += 1;
    while (load1() >= START_BELOW) {
      console.log(`  waiting: load ${load1().toFixed(2)} >= ${START_BELOW}`);
      await sleep(15_000);
    }
    const before = load1();
    const r = await leg({ record: true, stream: true }, (why) => {
      throw new Error(`a measurement cannot skip: ${why}`);
    });
    const after = load1();
    if (!r) throw new Error("the leg skipped");
    const o = r.observed;
    const clean = o.skippedFromStop === 0 && o.seqGapsFromStop === 0 && !o.stopWarned && o.streamOnAirAfterStop === false && o.controlPlaneStreamState === "idle";
    const void_ = before >= CEILING || after >= CEILING;
    const t = r.timing;
    console.log(
      `SHOW-STOP-SAMPLE ${JSON.stringify({ i, load: [Number(before.toFixed(2)), Number(after.toFixed(2))], void: void_, clean, ...t })}`,
    );
    if (!clean) throw new Error(`sample ${i} was not a clean stop: ${JSON.stringify(o)}`);
    if (void_) {
      voids += 1;
      continue;
    }
    kept.push(t);
  }
  const row = (name: string, xs: Array<number | null>): void => {
    const v = xs.filter((x): x is number => x !== null && Number.isFinite(x));
    console.log(
      `| ${name} | ${v.length} | ${q(v, 50).toFixed(1)} | ${q(v, 95).toFixed(1)} | ${Math.max(...v).toFixed(1)} | ${Math.min(...v).toFixed(1)} |`,
    );
  };
  console.log(`\nSHOW-STOP-SUMMARY kept=${kept.length} void=${voids} (ms; nearest-rank)`);
  console.log("| span | n | median | p95 | max | min |");
  console.log("|---|---:|---:|---:|---:|---:|");
  row("stop→ack (control plane)", kept.map((t) => t.stopToAckMs));
  row("stop→response", kept.map((t) => t.stopToResponseMs));
  row("wire→stream quiesced (engine)", kept.map((t) => t.wireToStreamQuiescedMs));
  row("wire→recording quiesced (engine)", kept.map((t) => t.wireToRecordQuiescedMs));
  row("wire→ack on the wire", kept.map((t) => t.wireToAckMs));
  const worst = Math.max(...kept.map((t) => t.stopToAckMs));
  console.log(`\nworst stop→ack ${worst.toFixed(1)} ms of the ${GRACE_MS} ms window: ${worst < GRACE_MS ? "INSIDE" : "OUTSIDE"}`);
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error(e);
    process.exit(1);
  },
);
