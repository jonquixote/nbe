//! The operator-signal lane's evaluator (SPEC §10.1.2, v0.4.8 row 8; R17 spine
//! part A), row by row of the mapping table, each with its negative. The
//! evaluator is driven directly: a state, an engine report the test sets, and
//! the lane's audit rows collected as they are written.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ALERT, AlertLane, OperatorSignals, type Alert } from "./alerts.js";
import { ControlPlaneState } from "./state.js";
import type { EngineReport } from "./telemetry.js";

const WINDOW = 40;
const TTL = 2000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function rig() {
  const state = new ControlPlaneState();
  const rows: Array<{ edge: string; id: string; event: string | null; cause: string | null }> = [];
  let report: EngineReport | null = null;
  let sinkOk = true;
  // A clock the test can move: "stale" means time passed with no new report.
  let offset = 0;
  const now = () => Date.now() + offset;
  const lane = new AlertLane(state, (edge, a, cause) => rows.push({ edge, id: a.id, event: a.auditEvent, cause }), now);
  const signals = new OperatorSignals({
    state,
    lane,
    now,
    windowMs: WINDOW,
    ttlMs: TTL,
    engineReport: () => report,
    auditSinkOk: () => sinkOk,
  });
  signals.onRegistered();
  return {
    state,
    rows,
    signals,
    ids: () => state.alerts.map((a: Alert) => a.id),
    /** A fresh engine report, received now. */
    report: (streamTransportState: string) => {
      report = { frame: { streamTransportState } as EngineReport["frame"], receivedAt: now() };
    },
    /** Time passes beyond the TTL with no new report: the last one goes stale. */
    stale: () => {
      offset += TTL + 1;
    },
    breakSink: (broken: boolean) => {
      sinkOk = !broken;
    },
  };
}

/** Let the clock move past the anchor, then evaluate. */
async function after(r: ReturnType<typeof rig>, transport: string): Promise<void> {
  await sleep(2);
  r.report(transport);
  r.signals.evaluate();
}

// --- (b) command confirmation ------------------------------------------------

test("(b) an output command unacknowledged within the window raises; before the window, nothing", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 7);
  r.report("none");
  r.signals.evaluate();
  assert.deepEqual(r.ids(), [], "inside the window: nothing yet");
  await sleep(WINDOW + 15);
  assert.deepEqual(r.ids(), [ALERT.streamUnacknowledged.id]);
  const alert = r.state.alerts[0]!;
  assert.equal(alert.severity, "warning");
  assert.match(alert.reason, /stream\.start \(stateVersion 7\) was not acknowledged within 40 ms/);
  assert.match(alert.reason, /reads the stream none and the control plane believes it live/);
  assert.deepEqual(r.rows, [{ edge: "raised", id: ALERT.streamUnacknowledged.id, event: "command.unacknowledged", cause: null }]);
  r.signals.close();
});

test("(b) negative: acknowledged inside the window raises nothing", async () => {
  const r = rig();
  r.signals.onAccepted("record.start", 3);
  r.signals.onApplied(3, false);
  await sleep(WINDOW + 15);
  assert.deepEqual([r.ids(), r.rows], [[], []]);
  r.signals.close();
});

test("(b) a late acknowledgement clears the alert, with its audit row", async () => {
  const r = rig();
  r.signals.onAccepted("stream.stop", 9);
  await sleep(WINDOW + 15);
  assert.deepEqual(r.ids(), [ALERT.streamUnacknowledged.id]);
  r.signals.onApplied(9, false);
  assert.deepEqual(r.ids(), []);
  assert.deepEqual(r.rows.at(-1), { edge: "cleared", id: ALERT.streamUnacknowledged.id, event: "command.unacknowledged", cause: "stream.stop acknowledged late (stateVersion 9)" });
  r.signals.close();
});

test("(b) a later acknowledgement proves the command was processed: the alert clears, and (a) takes the contradiction over", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 4);
  await sleep(WINDOW + 15);
  r.signals.onApplied(5, false); // a later command's ack: the refused start was processed
  assert.deepEqual(r.ids(), []);
  assert.match(String(r.rows.at(-1)?.cause), /a later acknowledgement \(stateVersion 5\) proves stream\.start was processed/);
  await after(r, "none");
  assert.deepEqual(r.ids(), [ALERT.streamDivergence.id], "the refused start is now the standing contradiction");
  r.signals.close();
});

test("(b) a resync's acknowledgement proves nothing: the alert stays", async () => {
  const r = rig();
  r.signals.onAccepted("stream.stop", 6);
  await sleep(WINDOW + 15);
  r.signals.onApplied(6, true); // a reconnect's resync, stamped at the stop's version
  assert.deepEqual(r.ids(), [ALERT.streamUnacknowledged.id]);
  r.signals.close();
});

test("(b) the operator's explicit action supersedes the episode", async () => {
  const r = rig();
  r.signals.onAccepted("stream.start", 4);
  await sleep(WINDOW + 15);
  r.signals.onAccepted("stream.stop", 5);
  assert.deepEqual(r.ids(), []);
  assert.equal(r.rows.at(-1)?.cause, "superseded by stream.stop (stateVersion 5)");
  r.signals.close();
});

test("(b) the engine going not-reporting clears the alert: that alert owns the unconfirmed state", async () => {
  const r = rig();
  r.signals.onAccepted("record.stop", 5);
  await sleep(WINDOW + 15);
  assert.deepEqual(r.ids(), [ALERT.recordUnacknowledged.id]);
  r.stale();
  r.signals.evaluate();
  assert.deepEqual(r.ids(), [ALERT.engineNotReporting.id]);
  assert.deepEqual(r.rows.find((x) => x.edge === "cleared"), { edge: "cleared", id: ALERT.recordUnacknowledged.id, event: "command.unacknowledged", cause: "the engine is not reporting" });
  r.signals.close();
});

test("(b) show.stop is excluded: it raises no unacknowledged alert of its own", async () => {
  const r = rig();
  r.signals.onAccepted("show.stop", 8);
  await sleep(WINDOW + 15);
  assert.deepEqual(r.ids(), []);
  r.signals.close();
});

// --- (a) standing contradictions --------------------------------------------

test("(a) live beside none, fresh and postdating the anchor, raises", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true); // the connect's resync: the anchor
  await after(r, "none");
  assert.deepEqual(r.ids(), [ALERT.streamDivergence.id]);
  assert.match(r.state.alerts[0]!.reason, /believes the stream live; the engine reports it none/);
  r.signals.close();
});

test("(a) live beside closed, fresh, with no held stop and nothing pending, raises", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true);
  await after(r, "closed");
  assert.deepEqual(r.ids(), [ALERT.streamDivergence.id]);
  r.signals.close();
});

test("(a) idle beside live or reconnecting (the stop that did not take) raises: on air", async () => {
  for (const transport of ["live", "reconnecting"]) {
    const r = rig();
    r.state.streamState = "idle";
    r.signals.onApplied(1, true);
    await after(r, transport);
    assert.deepEqual(r.ids(), [ALERT.streamDivergence.id], transport);
    assert.match(r.state.alerts[0]!.reason, /on air/);
    r.signals.close();
  }
});

test("(a) negative, the pre-ack witness: a report that predates the anchor never raises", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.report("none"); // received first
  await sleep(2);
  r.signals.onApplied(1, true); // the anchor lands after it
  r.signals.evaluate();
  assert.deepEqual(r.ids(), []);
  r.signals.close();
});

test("(a) negative: while a stream command is pending, nothing is compared", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true);
  r.signals.onAccepted("stream.start", 2);
  await after(r, "none");
  assert.deepEqual(r.ids(), []);
  r.signals.close();
});

test("(a) the consistent pairs raise nothing: live·live, idle·none, idle·closed", async () => {
  for (const [believed, transport] of [["live", "live"], ["idle", "none"], ["idle", "closed"]] as const) {
    const r = rig();
    r.state.streamState = believed;
    r.signals.onApplied(1, true);
    await after(r, transport);
    assert.deepEqual(r.ids(), [], `${believed} beside ${transport}`);
    r.signals.close();
  }
});

test("reconnecting beside live is §9.5's survival shape: no alert", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true);
  await after(r, "reconnecting");
  assert.deepEqual(r.ids(), []);
  r.signals.close();
});

test("the restart contradiction: after a resync (no output state), a fresh report of none beside the believed live raises", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  r.signals.onApplied(2, false); // the start took
  await after(r, "live");
  assert.deepEqual(r.ids(), []);
  r.signals.onApplied(3, true); // the engine restarted; its reconnect resynced
  await after(r, "none");
  assert.deepEqual(r.ids(), [ALERT.streamDivergence.id]);
  r.signals.close();
});

// --- the stale stub and the engine not reporting ---------------------------

test("the flicker guard: a stale stub never raises a divergence; the engine-not-reporting alert does instead", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true);
  await after(r, "live"); // consistent while fresh
  r.stale(); // the report goes stale: the tick now stubs streamTransportState "none"
  r.signals.evaluate();
  assert.deepEqual(r.ids(), [ALERT.engineNotReporting.id]);
  assert.equal(r.state.alerts[0]!.severity, "critical");
  r.signals.close();
});

test("the engine reporting again clears its alert, and the divergence is compared again", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true);
  await sleep(2);
  r.stale();
  r.signals.evaluate();
  await after(r, "none");
  assert.deepEqual(r.ids(), [ALERT.streamDivergence.id]);
  assert.equal(r.rows.find((x) => x.id === ALERT.engineNotReporting.id && x.edge === "cleared")?.cause, "the engine is reporting again");
  r.signals.close();
});

// --- the held stop ------------------------------------------------------------

test("held: live beside closed after an unacknowledged show.stop is the held alert's, not a divergence; it clears on heal", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.state.recordState = "recording";
  r.signals.onApplied(1, true);
  r.signals.onAccepted("show.stop", 2);
  r.signals.held(2);
  await after(r, "closed");
  assert.deepEqual(r.ids(), [ALERT.outputsHeld.id]);
  assert.deepEqual(r.rows, [{ edge: "raised", id: ALERT.outputsHeld.id, event: "show.stopHeld", cause: null }]);
  r.state.recordState = "idle";
  r.state.streamState = "idle"; // the operator's explicit stops
  await after(r, "closed");
  assert.deepEqual(r.ids(), []);
  assert.deepEqual(r.rows.at(-1), { edge: "cleared", id: ALERT.outputsHeld.id, event: "show.stopHeld", cause: "the held outputs were stopped" });
  r.signals.close();
});

// --- edge auditing and R18 ----------------------------------------------------

test("edge auditing: an episode is one raise row and one clear row, however many evaluations it spans", async () => {
  const r = rig();
  r.state.streamState = "live";
  r.signals.onApplied(1, true);
  for (let i = 0; i < 10; i += 1) await after(r, "none");
  for (let i = 0; i < 10; i += 1) await after(r, "live");
  assert.deepEqual(
    r.rows.map((x) => [x.edge, x.id]),
    [
      ["raised", ALERT.streamDivergence.id],
      ["cleared", ALERT.streamDivergence.id],
    ],
  );
  r.signals.close();
});

test("R18: a broken audit sink raises its alert and the health flag, and attempts no audit row for it", async () => {
  const r = rig();
  r.breakSink(true);
  r.signals.evaluate();
  assert.deepEqual([r.ids(), r.state.health], [[ALERT.auditSinkFailed.id], { auditSinkOk: false }]);
  assert.equal(r.state.alerts[0]!.auditEvent, null);
  assert.deepEqual(r.rows, [], "no audit row is attempted for the sink's own failure");
  r.breakSink(false);
  r.signals.evaluate();
  assert.deepEqual([r.ids(), r.state.health, r.rows], [[], { auditSinkOk: true }, []]);
  r.signals.close();
});
