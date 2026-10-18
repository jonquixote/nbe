//! The operator-signal lane's evaluator (SPEC §10.1.2, v0.4.8 row 8; R17 spine
//! part A), row by row of the mapping table, each with its negative. The
//! evaluator is driven directly: a state, an engine report the test sets, and
//! the lane's audit rows collected as they are written.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ALERT, AlertLane, OperatorSignals, type Alert, type BeliefCorrection } from "./alerts.js";
import { ControlPlaneState } from "./state.js";
import type { EngineReport } from "./telemetry.js";

const WINDOW = 40;
const TTL = 2000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * `apply` lands each belief correction on the state, as the server does
 * (SPEC §10.1.4). Off by default, so the lane's own tests (row 8) see the lane
 * alone, as before part B; the belief tests (row 11) turn it on.
 */
function rig(opts: { apply?: boolean } = {}) {
  const state = new ControlPlaneState();
  const corrections: BeliefCorrection[] = [];
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
    correct: (c) => {
      corrections.push(c);
      if (opts.apply) state.streamState = c.to;
    },
  });
  signals.onRegistered();
  return {
    state,
    rows,
    signals,
    corrections,
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

// (§2c, v0.4.8 row 11: this test's name ended "it clears on heal". The heal on
// outputs inactive is struck by §10.1.2's clearing rule for `outputs.held`.)
test("held: live beside closed after an unacknowledged show.stop is the held alert's, not a divergence; only a real acknowledgement clears it", async () => {
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
  r.state.streamState = "idle"; // the belief reads stopped
  await after(r, "closed");
  // ~~assert.deepEqual(r.ids(), [])~~ (§2c, v0.4.8 row 11): what the belief
  // reads clears nothing. The alert stands until a real acknowledgement.
  assert.deepEqual(r.ids(), [ALERT.outputsHeld.id], "the belief alone does not clear the held alert");
  r.signals.onApplied(2, false); // show.stop's own late acknowledgement (path i)
  assert.deepEqual(r.ids(), []);
  assert.deepEqual(r.rows.at(-1), { edge: "cleared", id: ALERT.outputsHeld.id, event: "show.stopHeld", cause: "show.stop acknowledged late (stateVersion 2)" });
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

// =============================================================================
// SPEC §10.1.3–§10.1.4 and §10.1.2's clearing rule for `outputs.held` (v0.4.8
// rows 10 and 11; R17 spine part B): the belief's side. The rig applies each
// correction to the state, as the server does.
// =============================================================================

// --- D1: the mapping, each row with its negative --------------------------------

test("D1: a fresh report postdating the anchor corrects a contradicting belief by the mapping, and leaves a consistent one", async () => {
  const cases: Array<[string, "idle" | "live", "idle" | "live" | null]> = [
    ["live", "idle", "live"],
    ["reconnecting", "idle", "live"],
    ["none", "live", "idle"],
    ["closed", "live", "idle"],
    // negatives: consistent pairs, and a token the mapping does not know
    ["live", "live", null],
    ["reconnecting", "live", null],
    ["none", "idle", null],
    ["closed", "idle", null],
    ["bogus", "live", null],
  ];
  for (const [token, belief, to] of cases) {
    const r = rig({ apply: true });
    r.state.streamState = belief;
    await after(r, token);
    assert.deepEqual(
      r.corrections.map((c) => [c.from, c.to, c.token]),
      to === null ? [] : [[belief, to, token]],
      `${token} beside a ${belief} belief`,
    );
    assert.equal(r.state.streamState, to ?? belief);
    r.signals.close();
  }
});

test("D1: a stale report corrects nothing, and neither does one that predates the anchor", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.report("none"); // received at the registration's instant: not after the anchor
  r.signals.evaluate();
  assert.equal(r.corrections.length, 0, "a report that does not postdate the anchor");
  await after(r, "none");
  r.stale();
  r.state.streamState = "live";
  r.corrections.length = 0;
  r.signals.evaluate();
  assert.equal(r.corrections.length, 0, "a stale report");
  r.signals.close();
});

// --- the claim (§10.1.3): its four endings -------------------------------------

test("claim: an output command holds the belief until its own acknowledgement; a report after it corrects", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  await after(r, "none");
  assert.equal(r.corrections.length, 0, "inside the window the start's claim holds the belief");
  r.signals.onApplied(2, false); // its own acknowledgement ends the claim
  await after(r, "none");
  assert.deepEqual(r.corrections.map((c) => c.to), ["idle"], "a report after the acknowledgement rules");
  r.signals.close();
});

test("claim: a later acknowledgement ends it too (Amendment 4: it proves the command processed)", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  r.signals.onApplied(3, false);
  await after(r, "none");
  assert.deepEqual(r.corrections.map((c) => c.to), ["idle"]);
  r.signals.close();
});

test("claim: window expiry ends it; only a report that postdates the expiry corrects, and the expiry alert stands", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  // After the registration's anchor, so only the expiry's anchor stands
  // between this report and a correction.
  await sleep(2);
  r.report("none"); // fresh, but before the expiry
  r.signals.evaluate();
  await sleep(WINDOW + 20); // the window ends unacknowledged
  r.signals.evaluate();
  assert.equal(r.corrections.length, 0, "a report that predates the expiry does not rule");
  assert.deepEqual(r.ids(), [ALERT.streamUnacknowledged.id], "the expiry raised (b)");
  await after(r, "none");
  assert.deepEqual(r.corrections.map((c) => [c.from, c.to]), [["live", "idle"]], "the next report rules");
  assert.deepEqual(r.ids(), [ALERT.streamUnacknowledged.id], "a correction clears no alert");
  r.signals.close();
});

test("claim: a newer command for the output takes the claim at its acceptance; the superseded one never expires", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  r.state.streamState = "idle";
  r.signals.onAccepted("stream.stop", 3); // supersedes the start inside its window
  await sleep(WINDOW / 2);
  await after(r, "live");
  assert.equal(r.corrections.length, 0, "the stop's claim holds the belief");
  r.signals.onApplied(3, false);
  await after(r, "closed");
  assert.equal(r.corrections.length, 0, "consistent after the stop's acknowledgement");
  await sleep(WINDOW + 20); // past the superseded start's own window
  r.signals.evaluate();
  assert.deepEqual(r.ids(), [], "the superseded start never expires: its window passes and raises nothing");
  r.signals.close();
});

test("late-ack rule: an acknowledgement after the window's unacknowledged expiry is inert to belief; it clears (b) and reclaims nothing", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  await sleep(WINDOW + 20);
  r.signals.evaluate();
  await after(r, "none");
  assert.equal(r.state.streamState, "idle", "corrected after the expiry");
  r.signals.onApplied(2, false); // the start's own acknowledgement, late
  assert.equal(r.state.streamState, "idle", "the dead claim cannot reclaim the belief");
  assert.deepEqual(r.ids(), [], "the late acknowledgement clears (b) by row 8's rule");
  assert.deepEqual(r.rows.at(-1), {
    edge: "cleared",
    id: ALERT.streamUnacknowledged.id,
    event: "command.unacknowledged",
    cause: "stream.start acknowledged late (stateVersion 2)",
  }, "and is logged where the alert-clear writes its row");
  await after(r, "none");
  assert.equal(r.corrections.length, 1, "no second correction: the belief already reads the report");
  r.signals.close();
});

test("engine.notReporting owning the window's end: the claim expires, and with no fresh report the stale belief stands", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.signals.onAccepted("stream.start", 2);
  r.stale();
  r.signals.evaluate();
  assert.ok(r.ids().includes(ALERT.engineNotReporting.id), "the engine is not reporting");
  await sleep(WINDOW + 20);
  r.signals.evaluate();
  assert.equal(r.state.streamState, "live", "no fresh report: the stale belief stands");
  assert.equal(r.corrections.length, 0);
  assert.ok(!r.ids().includes(ALERT.streamUnacknowledged.id), "(b) does not raise: not-reporting owns it");
  await after(r, "none"); // the engine reports again
  assert.deepEqual(
    r.corrections.map((c) => [c.from, c.to]),
    [["live", "idle"]],
    "the claim expired under not-reporting: the next fresh report rules",
  );
  r.signals.close();
});

// --- held: §5.9.5's limbo gives way to the next fresh report ---------------------

test("held gives way on the precision clause's schedule: the next fresh report after the expiry corrects it; the alert stands", async () => {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.state.recordState = "recording";
  r.signals.onApplied(1, true);
  r.signals.onAccepted("show.stop", 2);
  // After the resync's anchor, so only the hold's anchor stands between this
  // report and a correction.
  await sleep(2);
  r.report("closed"); // before the expiry
  r.signals.evaluate();
  r.state.streamState = "held"; // show.stop's unacknowledged path
  r.signals.held(2);
  r.signals.evaluate();
  assert.equal(r.corrections.length, 0, "a report that predates the expiry does not rule");
  await after(r, "closed");
  assert.deepEqual(r.corrections.map((c) => [c.from, c.to, c.token]), [["held", "idle", "closed"]]);
  assert.deepEqual(r.ids(), [ALERT.outputsHeld.id], "the correction clears no alert");
  r.signals.close();
});

// --- §10.1.2's clearing rule for `outputs.held` (D2a): the set, and where it stands ---

async function heldRig() {
  const r = rig({ apply: true });
  r.state.streamState = "live";
  r.state.recordState = "recording";
  r.signals.onApplied(1, true);
  r.signals.onAccepted("show.stop", 2);
  r.state.streamState = "held";
  r.signals.held(2);
  r.signals.evaluate();
  assert.deepEqual(r.ids(), [ALERT.outputsHeld.id]);
  return r;
}

test("D2a (i): the held stop's own acknowledgement, however late, clears it", async () => {
  const r = await heldRig();
  await sleep(WINDOW + 20);
  r.signals.onApplied(2, false);
  assert.deepEqual(r.ids(), []);
  assert.equal(r.rows.at(-1)!.cause, "show.stop acknowledged late (stateVersion 2)");
  r.signals.close();
});

test("D2a (ii): a later stop's own acknowledgement at or above clears it; a later stop processed but not acknowledged does not", async () => {
  const r = await heldRig();
  r.signals.onAccepted("record.stop", 3);
  r.signals.onApplied(4, false); // a later acknowledgement: record.stop processed, not acknowledged (refused)
  assert.deepEqual(r.ids().filter((i) => i === ALERT.outputsHeld.id), [ALERT.outputsHeld.id], "processed is not witnessed");
  r.signals.onAccepted("stream.stop", 5);
  r.signals.onApplied(5, false); // the stop's own acknowledgement
  assert.ok(!r.ids().includes(ALERT.outputsHeld.id), "the stop's own acknowledgement clears it");
  assert.equal(
    r.rows.filter((x) => x.id === ALERT.outputsHeld.id).at(-1)!.cause,
    "stream.stop acknowledged (stateVersion 5): the stop is witnessed",
  );
  r.signals.close();
});

test("D2a (iii): an accepted show.load or show.unload clears it, the cause named", async () => {
  for (const command of ["show.load", "show.unload"]) {
    const r = await heldRig();
    r.signals.onAccepted(command, 7);
    assert.deepEqual(r.ids(), []);
    assert.equal(r.rows.at(-1)!.cause, `cleared by ${command} (stateVersion 7)`);
    // The belief is still `held`, and nothing owns (a) now: `held` beside
    // `closed` contradicts nothing, and the report corrects it.
    await after(r, "closed");
    assert.deepEqual(r.ids(), [], "held is no divergence");
    assert.equal(r.state.streamState, "idle");
    r.signals.close();
  }
});

test("D2a: it stands in every other case — a correction, a transient none, a refused stop, a later non-stop acknowledgement", async () => {
  // Each member is asserted where it lands, so each falsifies on its own.
  const r = await heldRig();
  await after(r, "closed"); // a correction held -> idle
  assert.equal(r.state.streamState, "idle");
  assert.ok(r.ids().includes(ALERT.outputsHeld.id), "a correction moves the belief and clears no alert");
  await after(r, "none"); // a transient none
  assert.ok(r.ids().includes(ALERT.outputsHeld.id), "a transient none does not clear it");
  r.signals.onAccepted("stream.stop", 3); // a stop the engine refuses: never acknowledged
  await sleep(WINDOW + 20);
  r.signals.evaluate();
  assert.ok(r.ids().includes(ALERT.outputsHeld.id), "a refused stop, never acknowledged, does not clear it");
  r.signals.onApplied(9, false); // a later acknowledgement of some other command
  assert.ok(r.ids().includes(ALERT.outputsHeld.id), "persistence is the witness working");
  r.signals.close();
});
