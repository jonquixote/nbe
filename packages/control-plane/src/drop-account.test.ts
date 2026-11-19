//! R21's backstop's accounting, pinned on fixed lines (the user's rulings of
//! 2026-11-04, night, item 2: the dress logic's check moved into the repo's
//! tests). The dress applies `accountDrops` to the engine's log on a runner,
//! where a show may have no drops at all; this runs every case every time.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  SECOND_READING_CEILINGS,
  SECOND_READING_SPAN_CEILING,
  accountDrops,
  countsAt,
  judgeSecondReading,
  lineFields,
  secondReading,
  type StepDrops,
} from "./drop-account.js";

const HOUSE_RATE = 30;

/**
 * #85's real step-6 R21 line (CI, 2026-11-04), as the engine wrote it: colour
 * codes around each field name and its `=`.
 */
const REAL_R21 =
  "\u001b[2m2026-11-04T19:31:53.176293Z\u001b[0m \u001b[33m WARN\u001b[0m \u001b[2mnbe_engine::tick\u001b[0m\u001b[2m:\u001b[0m " +
  "R21: master frame 218 was drawn but submitted after frame 219's boundary " +
  "\u001b[3mmaster_frame\u001b[0m\u001b[2m=\u001b[0m218 \u001b[3mlate_ms\u001b[0m\u001b[2m=\u001b[0m3.5456250000000002 " +
  "\u001b[3mwoke_late_ms\u001b[0m\u001b[2m=\u001b[0m32.460083 \u001b[3mbefore_draw_ms\u001b[0m\u001b[2m=\u001b[0m0.003541 " +
  "\u001b[3mrender_ms\u001b[0m\u001b[2m=\u001b[0m4.401917 \u001b[3mdropped_total\u001b[0m\u001b[2m=\u001b[0m1 " +
  "\u001b[3msuppressed\u001b[0m\u001b[2m=\u001b[0m0";

function r21(frame: number, fields: string): string {
  return `WARN nbe_engine::tick: R21: master frame ${frame} was drawn but submitted after frame ${frame + 1}'s boundary master_frame=${frame} ${fields} suppressed=0`;
}

function r33(first: number, last: number, fields: string): string {
  return `WARN nbe_engine::tick: R33: master frames ${first}..=${last} were never drawn master_frame=${last + 1} skipped=${last - first + 1} ${fields} suppressed=0`;
}

/**
 * R33's run edge (#79's `Cadence::edge`): the frames a run's start or stop
 * left undrawn, logged with no `master_frame`, its `at` naming which edge.
 */
function edge(first: number, last: number, at: string, fields: string): string {
  return `WARN nbe_engine::tick: R33: master frames ${first}..=${last} were never drawn ${at} skipped=${last - first + 1} ${fields} suppressed=0`;
}

const LINES = [
  // 1: logged before e9225a8, so no previous_tick_ms: it cannot show (b).
  REAL_R21,
  // 2: a slow draw.
  r21(300, "late_ms=12.0 previous_tick_ms=4.0 woke_late_ms=0.5 before_draw_ms=0.004 render_ms=4.4 dropped_total=2"),
  // 3..7: #81's CI show start, a 200.1 ms tick before: a slow tick.
  r33(2, 6, "previous_tick_ms=200.1 woke_late_ms=169.8 dropped_total=7"),
  // 8..10: the wake short of the span.
  r33(40, 42, "previous_tick_ms=12.0 woke_late_ms=90.0 dropped_total=10"),
  // 11: an on-time wake, a late submit.
  r21(500, "late_ms=3.0 previous_tick_ms=4.0 woke_late_ms=1e-6 before_draw_ms=0.004 render_ms=4.4 dropped_total=11"),
  // 12: no line.
  // 13: Debug's exponent.
  r21(600, "late_ms=2e-5 previous_tick_ms=5.0 woke_late_ms=0.5 before_draw_ms=0.004 render_ms=33.3 dropped_total=13"),
  // 14: the real line's figures, with the tick before on budget.
  r21(218, "late_ms=3.5456250000000002 previous_tick_ms=4.401917 woke_late_ms=32.460083 before_draw_ms=0.003541 render_ms=4.401917 dropped_total=14"),
  // 15..19: a late wake covering the span after an on-budget tick.
  r33(70, 74, "previous_tick_ms=10.0 woke_late_ms=169.8 dropped_total=19"),
  // 20: woke late enough, but the tick before overran: a slow tick.
  r21(800, "late_ms=3.0 previous_tick_ms=45.0 woke_late_ms=40.0 before_draw_ms=0.004 render_ms=4.4 dropped_total=20"),
  // 21 and 22: either side of the budget, 33.33 ms at 30 fps.
  r21(900, "late_ms=3.0 previous_tick_ms=33.3 woke_late_ms=40.0 before_draw_ms=0.004 render_ms=4.4 dropped_total=21"),
  r21(1000, "late_ms=3.0 previous_tick_ms=33.4 woke_late_ms=40.0 before_draw_ms=0.004 render_ms=4.4 dropped_total=22"),
  // 23..24: a stop's edge, a late wake covering its span after an on-budget tick.
  edge(23, 24, "before the clock stopped", "previous_tick_ms=4.0 woke_late_ms=70.0 dropped_total=24"),
  // 25..27: a start's edge after a 1.5 s stopped tick. The overrun is part of the
  // wake (the loop does not sleep past it), so the wake covers the span and only
  // (b) leaves it untied: a slow tick.
  edge(0, 2, "at the run's start", "previous_tick_ms=1500.0 woke_late_ms=1450.0 dropped_total=27"),
  // 28..29: an edge line with no dropped_total (an engine before the edge carried it).
  edge(0, 1, "at the run's start", "previous_tick_ms=4.0 woke_late_ms=70.0"),
];

const CASES: [string, number, number, { tied: number[]; untied: number[]; unexplained: number[] }][] = [
  ["the real #85 step-6 R21 line has no previous_tick_ms, so it ties nothing", 0, 1, { tied: [], untied: [1], unexplained: [] }],
  ["a slow draw (woke 0.5 < late 12) is untied", 1, 2, { tied: [], untied: [2], unexplained: [] }],
  ["R33 5 frames, woke 169.8 >= 166.7 after a 200.1 ms tick: a slow tick, untied 3..7", 2, 7, { tied: [], untied: [3, 4, 5, 6, 7], unexplained: [] }],
  ["R33 3 frames, woke 90 < 100: untied 8..10", 7, 10, { tied: [], untied: [8, 9, 10], unexplained: [] }],
  ["R21 woke 1e-6 < late 3: untied 11", 10, 11, { tied: [], untied: [11], unexplained: [] }],
  ["count 12 has no line: unexplained", 11, 12, { tied: [], untied: [], unexplained: [12] }],
  ["no drops: empty", 5, 5, { tied: [], untied: [], unexplained: [] }],
  ["R21 late 2e-5 (Debug's exponent), woke 0.5: tied 13", 12, 13, { tied: [13], untied: [], unexplained: [] }],
  ["the real line's figures after a 4.4 ms tick: tied 14", 13, 14, { tied: [14], untied: [], unexplained: [] }],
  ["R33 5 frames, woke 169.8 after a 10 ms tick: tied 15..19", 14, 19, { tied: [15, 16, 17, 18, 19], untied: [], unexplained: [] }],
  ["R21 woke 40 >= late 3 after a 45 ms tick: a slow tick, untied 20", 19, 20, { tied: [], untied: [20], unexplained: [] }],
  ["R21 after a 33.3 ms tick, inside the 33.33 ms budget: tied 21", 20, 21, { tied: [21], untied: [], unexplained: [] }],
  ["R21 after a 33.4 ms tick, over the 33.33 ms budget: untied 22", 21, 22, { tied: [], untied: [22], unexplained: [] }],
  ["a stop's edge, 2 frames, woke 70 >= 66.7 after a 4 ms tick: tied 23..24", 22, 24, { tied: [23, 24], untied: [], unexplained: [] }],
  ["a start's edge after a 1.5 s stopped tick, woke 1450 >= 100: a slow tick by (b) alone, untied 25..27", 24, 27, { tied: [], untied: [25, 26, 27], unexplained: [] }],
  ["an edge line without dropped_total explains nothing: unexplained 28..29", 27, 29, { tied: [], untied: [], unexplained: [28, 29] }],
];

for (const [what, before, after, want] of CASES) {
  test(`R21 backstop: ${what}`, () => {
    const got = accountDrops(LINES, before, after, HOUSE_RATE);
    assert.deepEqual({ tied: got.tied, untied: got.untied, unexplained: got.unexplained }, want);
  });
}

test("R21 backstop: a step's lines are each named once, in order", () => {
  const got = accountDrops(LINES, 1, 10, HOUSE_RATE);
  assert.deepEqual(got.lines, [LINES[1], LINES[2], LINES[3]]);
});

test("R21 backstop: step F's phase fields on an R21 line change nothing the accounting reads", () => {
  const plain = r21(143, "late_ms=2.6 previous_tick_ms=30.99 woke_late_ms=2.16 before_draw_ms=0.003 render_ms=33.76 dropped_total=1");
  const phased = r21(
    143,
    "late_ms=2.6 previous_tick_ms=30.99 woke_late_ms=2.16 before_draw_ms=0.003 render_ms=33.76 " +
      "sync_ms=0.01 read_ahead_ms=0.4 preview_ms=1.2 view_build_ms=0.3 view_encode_ms=0.5 view_finish_ms=0.1 view_submit_ms=32.5 dropped_total=1",
  );
  const a = accountDrops([plain], 0, 1, HOUSE_RATE);
  const b = accountDrops([phased], 0, 1, HOUSE_RATE);
  assert.deepEqual({ tied: b.tied, untied: b.untied, unexplained: b.unexplained }, { tied: a.tied, untied: a.untied, unexplained: a.unexplained });
  assert.equal(lineFields(phased)["view_submit_ms"], 32.5);
});

test("R21 backstop: the engine's colour codes are stripped before the fields are read", () => {
  assert.deepEqual(lineFields(REAL_R21), {
    master_frame: 218,
    late_ms: 3.5456250000000002,
    woke_late_ms: 32.460083,
    before_draw_ms: 0.003541,
    render_ms: 4.401917,
    dropped_total: 1,
    suppressed: 0,
  });
});

// --- Item 10: the second reading (the user's ruling of 2026-11-18 04:20) ----

/** A telemetry frame as the dress collects it. */
const tick = (drops: number, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  data: { droppedFramesTotal: drops, audioUnderrunsTotal: 0, watchdogTripsTotal: 0, ...extra },
});

/** A step whose drops are all accounted one way. */
const step = (name: string, before: number, after: number, how: "tied" | "untied" = "untied"): StepDrops => {
  const counts = Array.from({ length: after - before }, (_, i) => before + i + 1);
  return {
    step: name,
    before,
    after,
    account: { tied: how === "tied" ? counts : [], untied: how === "untied" ? counts : [], unexplained: [], lines: [] },
  };
};

/** Steps 10 to 12c with no drops, the drops of one step set by the caller. */
const quietSteps = (over: Partial<Record<string, StepDrops>> = {}): StepDrops[] =>
  Object.keys(SECOND_READING_CEILINGS).map((name) => over[name] ?? step(name, 0, 0));

test("item 10, D1: the second reading counts from the gate's tick to the last tick", () => {
  // The gate read the first three ticks (2 drops); three more drops land after it.
  const ticks = [tick(0), tick(1), tick(2), tick(2), tick(4), tick(5, { fallbackActive: true })];
  const r = secondReading(ticks, 3);
  assert.equal(r.span.drops, 3, "the span is what came after the gate's tick, not the gate's own reading again");
  assert.equal(r.atGate.drops, 2);
  assert.equal(r.atEnd.drops, 5);
  assert.equal(r.span.fallbackTicks, 1);
  assert.ok(Number.isNaN(secondReading([{ data: {} }], 1).atEnd.drops), "a tick without the field reads NaN");
});

test("item 10, D2: a step's ceiling admits its own count and refuses one more", () => {
  for (const name of ["step 11", "step 12b"]) {
    const ceiling = SECOND_READING_CEILINGS[name];
    assert.equal(typeof ceiling, "number");
    const at = ceiling as number;
    assert.deepEqual(judgeSecondReading(quietSteps({ [name]: step(name, 0, at) }), 0, 8), [], `${name} at its ceiling passes`);
    const over = judgeSecondReading(quietSteps({ [name]: step(name, 0, at + 1) }), 0, 8);
    assert.deepEqual(over, [`${name}: ${at + 1} drops, over its ceiling of ${at}`]);
  }
});

test("item 10, D3: a step whose drops were not measured fails the second reading", () => {
  const missing = quietSteps().filter((s) => s.step !== "step 12");
  assert.deepEqual(judgeSecondReading(missing, 0, 8), ["step 12: its drops were not measured (no reading)"]);
  const nan = quietSteps({ "step 11": { ...step("step 11", 0, 0), after: Number.NaN } });
  assert.equal(judgeSecondReading(nan, 0, 8).length, 1);
});

test("item 10, D4: a tied-only step refuses an untied drop and counts its tied ones against the show's cap", () => {
  assert.deepEqual(judgeSecondReading(quietSteps({ "step 10": step("step 10", 0, 1, "untied") }), 0, 8), [
    "step 10: drop count(s) [1] are not tied to a late wake",
  ]);
  assert.deepEqual(judgeSecondReading(quietSteps({ "step 12c": step("step 12c", 0, 2, "tied") }), 6, 8), []);
  assert.deepEqual(judgeSecondReading(quietSteps({ "step 12c": step("step 12c", 0, 3, "tied") }), 6, 8), [
    "9 drops tied to a late wake this show, over the cap of 8",
  ]);
});

test("item 10, D5: the span's total has its own ceiling", () => {
  assert.equal(SECOND_READING_SPAN_CEILING, 19);
  const atBoth = quietSteps({ "step 11": step("step 11", 0, 9), "step 12b": step("step 12b", 9, 19) });
  assert.deepEqual(judgeSecondReading(atBoth, 0, 8), []);
  const wide = { "step 11": 9, "step 12b": 20 } as const;
  const over = judgeSecondReading([step("step 11", 0, 9), step("step 12b", 9, 20)], 0, 8, wide, 19);
  assert.deepEqual(over, ["steps 10 to 12c: 20 drops, over the span's ceiling of 19"]);
});

// Item 15 (the user's ruling of 2026-11-19 01:10, ruling 2): a drop is counted
// only on ticks where the clock was running on both sides. The control plane
// zero-fills a tick when the engine's report is stale; #165's tick 74 read
// RUNNING, frame 0, drops 0 between frames 240 and 331.
const runTick = (showState: string, masterClockFrame: number, droppedFramesTotal: number) => ({
  data: { showState, masterClockFrame, droppedFramesTotal, audioUnderrunsTotal: 0, watchdogTripsTotal: 0 },
});

test("item 15, Z1: a zero-filled last tick is not a reading", () => {
  const c = countsAt([runTick("RUNNING", 240, 12), runTick("RUNNING", 0, 0)]);
  assert.ok(Number.isNaN(c.drops), `a zero-filled tick reads NaN, not ${c.drops}`);
});

test("item 15, Z2: the tick after a zero-filled one is a reading", () => {
  assert.equal(countsAt([runTick("RUNNING", 240, 12), runTick("RUNNING", 0, 0), runTick("RUNNING", 331, 13)]).drops, 13);
});

test("item 15, Z3: a real drop counts, and so does a cached repeat", () => {
  assert.equal(countsAt([runTick("RUNNING", 240, 12), runTick("RUNNING", 270, 13)]).drops, 13, "a real drop");
  assert.equal(countsAt([runTick("RUNNING", 256, 13), runTick("RUNNING", 256, 13)]).drops, 13, "a cached repeat");
});

test("item 15, Z4: a stop and a reload are readings", () => {
  assert.equal(countsAt([runTick("RUNNING", 358, 7), runTick("STOPPED", 358, 7)]).drops, 7, "a stop");
  assert.equal(countsAt([runTick("STOPPED", 358, 7), runTick("LOADED", 0, 7)]).drops, 7, "a reload");
  assert.equal(countsAt([runTick("LOADED", 0, 7), runTick("RUNNING", 0, 7)]).drops, 7, "the start after it");
});
