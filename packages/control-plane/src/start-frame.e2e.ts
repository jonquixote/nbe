//! SPEC v0.4.11 row 1 (#203), end to end: a resync of a healthy engine gives it
//! the start frame it took its View item at, not one a telemetry interval old.
//!
//! The defect: the control plane's `viewItemStartFrame` was the engine's last
//! reported master frame, up to a report interval (1 s, 30 frames) behind the
//! engine at the take. The engine sets its item's start from a resync
//! (`on_resync` stores the snapshot's value), so a reconnect moved the item on
//! air forward by that much: a clip jumped. #203 estimates the engine's clock
//! at the take (the last report advanced at the house rate since). Its unit
//! tests cover the arithmetic; this drives the real engine.
//!
//! The take is timed just before a report, where the last report is stalest,
//! and the engine's socket is severed through the tap (the process and its
//! clock live on). On the reconnect the engine's `engineHello` says the start
//! it holds, its own; the control plane's `show.resync` says the start it
//! believes. They must agree within a few frames. A second sever shows the
//! engine took the resync's start, so the resync's value is what plays.
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/start-frame.e2e.ts
//! (engine: $NBE_ENGINE_BIN, default target/release/nbe-engine;
//! preflight: $NBE_PREFLIGHT_BIN).

import { test } from "node:test";
import assert from "node:assert/strict";
import { cpSync } from "node:fs";
import { join, resolve } from "node:path";

import { loadComplete, residency } from "./load-residency.js";
import { send, stack, type Stack, type TapFrame } from "./show-stop.harness.js";
import { tempDir } from "./test-tmp.js";

const REPO = resolve(import.meta.dirname, "../../..");
const DRESS = join(REPO, "tests/fixtures/dress_show");
/** How far the resync's start may sit from the engine's own: the take's
 * estimate floors, and the engine applies the cut at its next frame. */
const TOLERANCE_FRAMES = 6;
/** The take must come this long after the last report for the test to tell
 * #203 from the last report alone: 500 ms is 15 frames at 30 fps. */
const MIN_STALE_MS = 500;

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
async function waitFor(pred: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (pred()) return true;
    await sleep(10);
  }
  return pred();
}
async function ok(s: Stack, command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(s.ws, command, payload);
  assert.equal(r["status"], "ok", `${command}: ${JSON.stringify(r)}`);
  return r["stateVersion"] as number;
}

/** Severs the engine's connection and returns the hello and resync of its reconnect. */
async function reconnect(s: Stack): Promise<{ hello: Record<string, unknown>; resync: Record<string, unknown>; downMs: number }> {
  const mark = s.tap.frames.length;
  const t0 = Date.now();
  s.tap.sever();
  assert.ok(await waitFor(() => s.server.wsBridge.renderNodeCount() === 0, 5_000), "the control plane sees the engine go");
  const after = (): TapFrame[] => s.tap.frames.slice(mark);
  const found = await waitFor(
    () =>
      after().some((f) => f.dir === "up" && f.msg["kind"] === "engineHello") &&
      after().some((f) => f.dir === "down" && f.msg["command"] === "show.resync"),
    20_000,
  );
  assert.ok(found, `the engine reconnects and is resynced (last log: ${JSON.stringify(s.log.slice(-3))})`);
  const hello = after().find((f) => f.dir === "up" && f.msg["kind"] === "engineHello")!.msg;
  const resync = after().find((f) => f.dir === "down" && f.msg["command"] === "show.resync")!.msg;
  return { hello, resync, downMs: Date.now() - t0 };
}

test("[v0.4.11 row 1] a reconnect's resync gives the engine the start frame it took its item at, even when the take came just before a report", async (t) => {
  const s = await stack();
  t.after(() => s.close());
  const pkg = join(tempDir("nbe-start-frame-e2e-"), "dress_show");
  cpSync(DRESS, pkg, { recursive: true });
  const mark = s.log.length;
  assert.ok(await s.server.awaitApplied(await ok(s, "show.load", { packagePath: pkg }), 20_000), "show.load applied");
  assert.ok(await s.server.awaitApplied(await ok(s, "show.start", { startClock: true }), 20_000), "show.start applied");
  await ok(s, "view.cut", { itemRef: "A1" });
  assert.ok(await waitFor(() => loadComplete(residency(s.log.slice(mark))), 30_000), "the load completes");

  // The take, just before the next report: two reports apart give the
  // interval, and a window with no report landing in it is the one used.
  const reportAt = (): number | null => s.state.lastKnownMasterFrameAtMs;
  let staleMs = 0;
  let lastReportFrame = 0;
  for (let attempt = 0; attempt < 5 && staleMs < MIN_STALE_MS; attempt++) {
    const a = reportAt();
    assert.ok(await waitFor(() => reportAt() !== a, 5_000), "the engine reports");
    const b = reportAt()!;
    assert.ok(await waitFor(() => reportAt() !== b, 5_000), "the engine reports again");
    const last = reportAt()!;
    const interval = last - b;
    await sleep(Math.max(0, last + interval - 150 - Date.now()));
    if (reportAt() !== last) continue; // a report landed early: try the next window
    lastReportFrame = s.state.lastKnownMasterFrame;
    staleMs = Date.now() - last;
    await ok(s, "view.cut", { itemRef: "A2" });
  }
  assert.ok(staleMs >= MIN_STALE_MS, `precondition: the take came ${staleMs} ms after the last report (at least ${MIN_STALE_MS})`);
  const believed = s.state.viewItemStartFrame;
  await sleep(500);

  const first = await reconnect(s);
  const held = first.hello["viewItemStartFrame"] as number | null;
  const sent = (first.resync["payload"] as Record<string, unknown>)["viewItemStartFrame"] as number | null;
  const second = await reconnect(s);
  const adopted = second.hello["viewItemStartFrame"] as number | null;
  const observed = {
    staleMs,
    lastReportFrame,
    believed,
    engineHeld: held,
    resyncSent: sent,
    engineAfterResync: adopted,
    errorFrames: held === null || sent === null ? null : sent - held,
    lastReportErrorFrames: held === null ? null : lastReportFrame - held,
    downMs: [first.downMs, second.downMs],
  };
  console.log(`START-FRAME ${JSON.stringify(observed)}`);

  assert.equal(first.hello["viewItem"], "A2", "the engine holds A2 at the reconnect");
  assert.ok(held !== null && sent !== null, `both starts are set: ${JSON.stringify(observed)}`);
  assert.equal(sent, believed, "the resync carries the start the control plane took at the take");
  assert.ok(
    held - lastReportFrame >= 15,
    `the test tells the two apart: the last report alone was ${held - lastReportFrame} frames behind the engine's start (at least 15): ${JSON.stringify(observed)}`,
  );
  assert.ok(
    Math.abs(sent - held) <= TOLERANCE_FRAMES,
    `the resync's start is the engine's own within ${TOLERANCE_FRAMES} frames, not the last report's: ${JSON.stringify(observed)}`,
  );
  assert.equal(adopted, sent, "the engine took the resync's start: what the resync sends is what plays");
});
