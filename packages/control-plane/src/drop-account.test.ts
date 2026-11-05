//! R21's backstop's accounting, pinned on fixed lines (the user's rulings of
//! 2026-11-04, night, item 2: the dress logic's check moved into the repo's
//! tests). The dress applies `accountDrops` to the engine's log on a runner,
//! where a show may have no drops at all; this runs every case every time.

import { test } from "node:test";
import assert from "node:assert/strict";

import { accountDrops, lineFields } from "./drop-account.js";

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

const LINES = [
  REAL_R21,
  r21(300, "late_ms=12.0 woke_late_ms=0.5 before_draw_ms=0.004 render_ms=4.4 dropped_total=2"),
  r33(2, 6, "previous_tick_ms=200.1 woke_late_ms=169.8 dropped_total=7"),
  r33(40, 42, "previous_tick_ms=200.1 woke_late_ms=90.0 dropped_total=10"),
  r21(500, "late_ms=3.0 woke_late_ms=1e-6 before_draw_ms=0.004 render_ms=4.4 dropped_total=11"),
  r21(600, "late_ms=2e-5 woke_late_ms=0.5 before_draw_ms=0.004 render_ms=33.3 dropped_total=13"),
];

const CASES: [string, number, number, { tied: number[]; untied: number[]; unexplained: number[] }][] = [
  ["the real #85 step-6 R21 line ties count 1", 0, 1, { tied: [1], untied: [], unexplained: [] }],
  ["a slow draw (woke 0.5 < late 12) is untied", 1, 2, { tied: [], untied: [2], unexplained: [] }],
  ["R33 5 frames, woke 169.8 >= 166.7: tied 3..7", 2, 7, { tied: [3, 4, 5, 6, 7], untied: [], unexplained: [] }],
  ["R33 3 frames, woke 90 < 100: untied 8..10", 7, 10, { tied: [], untied: [8, 9, 10], unexplained: [] }],
  ["R21 woke 1e-6 < late 3: untied 11", 10, 11, { tied: [], untied: [11], unexplained: [] }],
  ["count 12 has no line: unexplained", 11, 12, { tied: [], untied: [], unexplained: [12] }],
  ["no drops: empty", 5, 5, { tied: [], untied: [], unexplained: [] }],
  ["R21 late 2e-5 (Debug's exponent), woke 0.5: tied 13", 12, 13, { tied: [13], untied: [], unexplained: [] }],
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
