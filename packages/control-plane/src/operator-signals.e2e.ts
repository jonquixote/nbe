//! The operator-signal lane on the real engine (SPEC §10.1.2, v0.4.8 row 8;
//! R17 spine part A). The unit pins (`alerts.test.ts`) drive the evaluator row
//! by row, and `alerts-server.test.ts` drives a real server with a scripted
//! engine. These drive the real engine binary into each case:
//! - **refused start**: the engine refuses `stream.start` (no hardware H.264
//!   encoder, CI's natural seam), never acknowledges it, and (b) raises after
//!   the window and not before. On a machine whose encoder opens there is
//!   nothing to refuse, so the leg skips loudly there.
//! - **lost stop**: a live stream's `stream.stop` never reaches the engine (the
//!   loss a dropped connection causes). (b) raises, and its reason says the
//!   engine still reads the stream live while the control plane believes it
//!   idle. Needs the encoder.
//! - **restart**: the engine is killed and restarted under a stream the control
//!   plane believes live. The reconnect's resync carries no output state, the
//!   new engine reports `none`, and (a) raises the standing contradiction.
//!   Needs the encoder.
//! - **held**: a real finalize failure (the record directory made read-only)
//!   withholds `show.stop`'s acknowledgement; `outputs.held` raises, and the
//!   explicit stops clear it. Needs the encoder.
//!
//!
//! **Row 11 (SPEC §10.1.3–§10.1.4, R17 spine part B), §2c.** The belief now
//! follows the truth, so each leg also reads the belief. The refused start
//! corrects `live → idle` after its window. The lost stop corrects
//! `idle → live`: the engine is still on air. The restart corrects
//! `live → idle`, and (a)'s contradiction lasts one evaluation. The held
//! leg's belief goes `held`, then is corrected; no real acknowledgement can
//! come (the engine idled its record and closed its stream), so the held
//! alert stands until `show.unload` clears it (§10.1.2's clearing rule for
//! `outputs.held`, path iii). ~~"part A changes no belief"~~ is struck in each
//! leg.
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/operator-signals.e2e.ts

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { send, stack, startRtmpSink, type Stack } from "./show-stop.harness.js";
import { WsRenderBridge, type RenderSender } from "./render-bridge.js";
import { tempDir } from "./test-tmp.js";

const REPO = resolve(import.meta.dirname, "../../..");
const ENGINE_BIN = resolve(process.env.NBE_ENGINE_BIN ?? join(REPO, "target/release/nbe-engine"));
const DRESS = join(REPO, "tests/fixtures/dress_show");
const WINDOW = 2000; // §5.9.5's window, the server's default
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function ok(s: Stack, command: string, payload: Record<string, unknown> = {}): Promise<number> {
  const r = await send(s.ws, command, payload);
  assert.equal(r.status, "ok", `${command}: ${JSON.stringify(r)}`);
  return r["stateVersion"] as number;
}

async function waitFor(check: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await sleep(25);
  }
  return check();
}

const ids = (s: Stack): string[] => s.state.alerts.map((a) => a.id);
const auditRows = (s: Stack): Array<Record<string, unknown>> =>
  readFileSync(s.auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
const corrections = (s: Stack): Array<Record<string, unknown>> =>
  auditRows(s)
    .filter((r) => r["event"] === "belief.corrected")
    .map((r) => r["detail"] as Record<string, unknown>);
/**
 * The correction's latency (the brief's §3): window end → next fresh report →
 * belief, read from the server-stamped audit as the expiry's alert row to the
 * `belief.corrected` row. Measured, not asserted: no timing claim (R9).
 */
const correctionLatencyMs = (s: Stack, alert: string): number | null => {
  const rows = auditRows(s);
  const raised = rows.find(
    (r) =>
      r["command"] === "alert" &&
      (r["detail"] as Record<string, unknown>)["alert"] === alert &&
      (r["detail"] as Record<string, unknown>)["edge"] === "raised",
  );
  const corrected = rows.find((r) => r["event"] === "belief.corrected");
  return raised && corrected ? (corrected["ts"] as number) - (raised["ts"] as number) : null;
};
const refusedByEngine = (s: Stack): boolean => s.log.some((l) => l.includes("E_NO_HARDWARE_ENCODER") || l.includes("E_NO_ZEROCOPY"));

/**
 * R25 (R19's class): the lane's commands wait for the load to complete, not for
 * a wall-clock window. `show.load`'s acknowledgement precedes the render
 * thread's load boundary, where every decoded video asset's ring is uploaded on
 * the thread the directives share (R20's disclosed cost), so a command sent
 * then waits behind the uploads, and a 1.5 s acknowledgement window read that
 * wait as a wedged engine. The engine logs each video asset it decodes at the
 * load ("video asset decoded") and each ring as it lands ("clip ring resident"
 * or "loop ring resident", the render target's lines, which the harness's log
 * filter admits): the load is complete when every decoded asset's ring is
 * resident.
 */
async function awaitResident(s: Stack, from: number): Promise<void> {
  const since = (needle: string): number => s.log.slice(from).filter((l) => l.includes(needle)).length;
  const complete = (): boolean => since("video asset decoded") > 0 && since("ring resident") >= since("video asset decoded");
  assert.ok(
    await waitFor(complete, 30_000),
    `the load completes: every decoded asset's ring resident (decoded ${since("video asset decoded")}, ` +
      `resident ${since("ring resident")}; last log: ${JSON.stringify(s.log.slice(-3))})`,
  );
}

/** The dress package, a record directory staged beside it, loaded and on air. */
async function onAir(s: Stack): Promise<{ rec: string }> {
  const tmp = tempDir("nbe-lane-e2e-");
  const pkg = join(tmp, "dress_show");
  cpSync(DRESS, pkg, { recursive: true });
  const rec = join(tmp, "record-out");
  mkdirSync(rec, { recursive: true });
  const m = JSON.parse(readFileSync(join(pkg, "manifest.json"), "utf8")) as Record<string, Record<string, unknown>>;
  m["show"]!["outputs"] = { record: { directory: rec } };
  writeFileSync(join(pkg, "manifest.json"), JSON.stringify(m));
  const mark = s.log.length;
  assert.ok(await s.server.awaitApplied(await ok(s, "show.load", { packagePath: pkg }), 20_000), "show.load applied");
  assert.ok(await s.server.awaitApplied(await ok(s, "show.start", { startClock: true }), 20_000), "show.start applied");
  await ok(s, "view.cut", { itemRef: "A1" });
  await awaitResident(s, mark);
  return { rec };
}

/** Starts the stream; false (after a loud skip) when the engine refuses it. */
async function streamOrSkip(s: Stack, port: number, skip: (why: string) => void): Promise<boolean> {
  const v = await ok(s, "stream.start", { url: `rtmp://127.0.0.1:${port}/live/lane` });
  if (await s.server.awaitApplied(v, 5_000)) return true;
  assert.ok(refusedByEngine(s), `stream.start: no ack and no capability refusal (last log: ${JSON.stringify(s.log.slice(-3))})`);
  console.log("SKIP: the engine refused stream.start (no hardware H.264 encoder on this machine)");
  skip("no hardware H.264 encoder: the engine refused stream.start");
  return false;
}

test("[lane] refused start: an engine that refuses stream.start never acknowledges it, and (b) raises after the window, not before", async (t) => {
  const s = await stack();
  const sink = await startRtmpSink();
  t.after(async () => {
    sink.close();
    await s.close();
  });
  await onAir(s);
  const v = await ok(s, "stream.start", { url: `rtmp://127.0.0.1:${sink.port}/live/lane` });
  const sentAt = Date.now();
  const before = ids(s);
  if (await s.server.awaitApplied(v, 1_500)) {
    console.log("SKIP: the engine opened its encoder and acknowledged stream.start; the refusal is CI's natural seam");
    t.skip("this machine has a hardware H.264 encoder: nothing to refuse");
    return;
  }
  assert.ok(refusedByEngine(s), `no ack and no capability refusal (last log: ${JSON.stringify(s.log.slice(-3))})`);
  const insideWindow = ids(s);
  assert.ok(await waitFor(() => ids(s).includes("command.unacknowledged.stream"), WINDOW + 2_000), "the alert raises");
  const raisedAfterMs = Date.now() - sentAt;
  const alert = s.state.alerts.find((a) => a.id === "command.unacknowledged.stream")!;
  console.log(`LANE-LEG refused ${JSON.stringify({ before, insideWindow, raisedAfterMs, alert })}`);
  assert.deepEqual([before, insideWindow], [[], []], "before the window: nothing");
  assert.ok(raisedAfterMs >= WINDOW, `not before the window: raised after ${raisedAfterMs} ms`);
  assert.match(alert.reason, /stream\.start \(stateVersion \d+\) was not acknowledged within 2000 ms/);
  // ~~assert.equal(s.state.streamState, "live", "part A changes no belief")~~
  // (§2c, row 11): the start's claim expired with its window, and the next
  // fresh report (`none`) corrects the belief. The (b) alert stands.
  const raisedAt = Date.now();
  assert.ok(await waitFor(() => s.state.streamState === "idle", 2 * 1_000 + 500), "the belief follows the report after the claim's expiry");
  const correctedAfterMs = Date.now() - raisedAt;
  const latencyMs = correctionLatencyMs(s, "command.unacknowledged.stream");
  console.log(`LANE-LEG refused-correction ${JSON.stringify({ correctedAfterMs, latencyMs, corrections: corrections(s), alerts: ids(s) })}`);
  assert.deepEqual(corrections(s).map((c) => [c["from"], c["to"], c["report"]]), [["live", "idle", "none"]]);
  assert.deepEqual(ids(s), ["command.unacknowledged.stream"], "a correction clears no alert");
});

test("[lane] lost stop: a stream.stop that never reaches the engine raises (b); the engine still reads live while the control plane believes idle", async (t) => {
  // The loss a dropped connection causes: the stop never reaches the socket.
  let dropStop = false;
  const register = WsRenderBridge.prototype.register;
  WsRenderBridge.prototype.register = function (this: WsRenderBridge, sendFn: RenderSender) {
    return register.call(this, (frame) => {
      if (dropStop && frame.command === "stream.stop") {
        dropStop = false;
        return true;
      }
      return sendFn(frame);
    });
  };
  const s = await stack();
  const sink = await startRtmpSink();
  t.after(async () => {
    WsRenderBridge.prototype.register = register;
    sink.close();
    await s.close();
  });
  await onAir(s);
  if (!(await streamOrSkip(s, sink.port, (w) => t.skip(w)))) return;
  await sleep(1_500);
  dropStop = true;
  await ok(s, "stream.stop", {});
  assert.ok(await waitFor(() => ids(s).includes("command.unacknowledged.stream"), WINDOW + 2_000), "the alert raises");
  const alert = s.state.alerts.find((a) => a.id === "command.unacknowledged.stream")!;
  // (§2c, row 11) ~~the control plane believes idle~~: the stop's claim
  // expired, and the engine's report (live) corrects the belief to `live`.
  assert.ok(await waitFor(() => s.state.streamState === "live", 2 * 1_000 + 500), "the belief follows the engine: on air");
  const bytes = sink.bytes();
  await sleep(500);
  const latencyMs = correctionLatencyMs(s, "command.unacknowledged.stream");
  const observed = { alerts: ids(s), believed: s.state.streamState, onAir: sink.bytes() > bytes, reason: alert.reason, corrections: corrections(s), latencyMs };
  console.log(`LANE-LEG lost-stop ${JSON.stringify(observed)}`);
  assert.deepEqual([observed.alerts, observed.believed, observed.onAir], [["command.unacknowledged.stream"], "live", true]);
  assert.deepEqual(corrections(s).map((c) => [c["from"], c["to"], c["report"]]), [["idle", "live", "live"]]);
  assert.match(alert.reason, /stream\.stop \(stateVersion \d+\) was not acknowledged within 2000 ms/);
  assert.match(alert.reason, /reads the stream live and the control plane believes it idle/, "the alert says the stream is on air");
});

// (§2c, row 11: this test's name ended "and (a) raises the contradiction".)
test("[lane] restart: the engine restarts under a stream believed live; the resync carries no output state, and the belief follows the new engine's report", async (t) => {
  const s = await stack();
  const sink = await startRtmpSink();
  let second: ReturnType<typeof spawn> | null = null;
  t.after(async () => {
    second?.kill("SIGKILL");
    sink.close();
    await s.close();
  });
  await onAir(s);
  if (!(await streamOrSkip(s, sink.port, (w) => t.skip(w)))) return;
  await sleep(1_500);
  assert.deepEqual(ids(s), [], "a healthy stream: nothing");
  s.engine.kill("SIGKILL");
  await waitFor(() => s.server.wsBridge.renderNodeCount() === 0, 5_000);
  second = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${s.tap.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: "render-token",
      NBE_HOUSE_RATE: "30",
      RUST_LOG: "warn",
      NO_COLOR: "1",
    },
    stdio: "ignore",
  });
  assert.ok(await waitFor(() => s.server.wsBridge.renderNodeCount() === 1, 20_000), "the restarted engine reconnects");
  // ~~(a) raises the contradiction, and the belief stays live~~ (§2c, row 11):
  // with no claim live, the new engine's fresh report corrects the belief. (a)
  // announces the contradiction in the evaluation that sees it, and clears it
  // on the next as consistent again: a one-evaluation episode, its edges and
  // `belief.corrected` in the audit.
  assert.ok(await waitFor(() => s.state.streamState === "idle", 30_000), "the belief follows the new engine's report");
  assert.ok(await waitFor(() => !ids(s).includes("stream.divergence"), 2_000), "and the contradiction is consistent again");
  const divergence = auditRows(s)
    .filter((r) => r["command"] === "alert" && (r["detail"] as Record<string, unknown>)["alert"] === "stream.divergence")
    .map((r) => r["detail"] as Record<string, unknown>);
  console.log(`LANE-LEG restart ${JSON.stringify({ alerts: ids(s), believed: s.state.streamState, divergence, corrections: corrections(s) })}`);
  assert.deepEqual(corrections(s).map((c) => [c["from"], c["to"], c["report"]]), [["live", "idle", "none"]]);
  assert.deepEqual(divergence.map((d) => d["edge"]), ["raised", "cleared"], "announced, then consistent again");
  assert.match(String(divergence[0]!["reason"]), /believes the stream live; the engine reports it none/);
});

// (§2c, row 11: this test's name ended "and the explicit stops clear it".)
test("[lane] held: a real finalize failure withholds show.stop's acknowledgement; outputs.held raises, the belief goes held then follows the engine, and with no real acknowledgement possible only show.unload clears it", async (t) => {
  const s = await stack();
  const sink = await startRtmpSink();
  let rec = "";
  t.after(async () => {
    if (rec) chmodSync(rec, 0o755);
    sink.close();
    await s.close();
  });
  ({ rec } = await onAir(s));
  const rv = await ok(s, "record.start", { outputId: "lane" });
  if (!(await s.server.awaitApplied(rv, 5_000))) {
    assert.ok(refusedByEngine(s), "record.start: no ack and no capability refusal");
    console.log("SKIP: the engine refused record.start (no hardware H.264 encoder on this machine)");
    t.skip("no hardware H.264 encoder: the engine refused record.start");
    return;
  }
  if (!(await streamOrSkip(s, sink.port, (w) => t.skip(w)))) return;
  await ok(s, "marker.add", { name: "lane" });
  await sleep(1_500);
  chmodSync(rec, 0o555); // the finalize's sidecar write fails: the engine withholds the ack
  await ok(s, "show.stop", {});
  assert.ok(await waitFor(() => ids(s).includes("outputs.held"), 2_000), "the held alert raises");
  const held = s.state.alerts.find((a) => a.id === "outputs.held")!;
  const whileHeld = ids(s);
  const beliefAtHold = s.state.streamState;
  // The engine idled its record and closed its stream (the finalize failed),
  // so the next fresh report corrects `held` to `idle`.
  assert.ok(await waitFor(() => s.state.streamState === "idle", 2 * 1_000 + 500), "held gives way to the engine's report");
  // ~~The explicit stops clear it~~ (§2c, row 11): no real acknowledgement can
  // come. The engine refuses record.stop (it is not recording), and the
  // control plane refuses stream.stop, since the stream belief is idle (D3's
  // precondition, its error naming the state).
  await ok(s, "record.stop", {});
  const streamStop = await send(s.ws, "stream.stop", {});
  assert.deepEqual(streamStop["error"], { code: "E_FORBIDDEN_STATE", message: "stream is idle" });
  await sleep(WINDOW + 500);
  assert.ok(ids(s).includes("outputs.held"), "the held alert stands: persistence is the witness working");
  // Path (iii): the show's lifecycle ends an unresolved hold.
  const unload = await ok(s, "show.unload", {});
  assert.ok(await waitFor(() => !ids(s).includes("outputs.held"), 2_000), "show.unload clears it");
  const clear = auditRows(s)
    .filter((r) => r["command"] === "alert" && (r["detail"] as Record<string, unknown>)["alert"] === "outputs.held")
    .map((r) => r["detail"] as Record<string, unknown>)
    .at(-1)!;
  const latencyMs = correctionLatencyMs(s, "outputs.held");
  console.log(`LANE-LEG held ${JSON.stringify({ whileHeld, held, beliefAtHold, corrections: corrections(s), latencyMs, clear, after: ids(s) })}`);
  assert.deepEqual(whileHeld, ["outputs.held"], "only the held alert: held beside closed is its, not a divergence");
  assert.equal(beliefAtHold, "held", "§5.9.5's limbo, on the command's path");
  assert.match(held.reason, /show\.stop \(stateVersion \d+\) was not acknowledged within 2000 ms; outputs held: record recording, stream held/);
  assert.deepEqual(corrections(s).map((c) => [c["from"], c["to"], c["report"]]), [["held", "idle", "closed"]]);
  assert.deepEqual([clear["edge"], clear["cause"]], ["cleared", `cleared by show.unload (stateVersion ${unload})`]);
});
