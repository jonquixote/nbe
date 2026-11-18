//! R21's backstop's accounting (the user's rulings of 2026-11-04): which of a
//! step's dropped-frame counts the engine's own drop lines explain, and which
//! of those a line ties to a late wake.
//!
//! Pure, so `drop-account.test.ts` pins the rule in `npm test` on every run;
//! the dress rehearsal applies it to the engine's log (`noDropsOrTiedOnes`).
//!
//! Each R21 and R33 line carries `dropped_total`, the engine's
//! `droppedFramesTotal` right after the drops it names were counted (one thread
//! moves the counter). A step's drops are the counts in (before, after], and
//! each count is matched to the line that explains it, with no time window to
//! race:
//! - an R21 line explains one count; an R33 line explains `skipped` counts.
//! - A line ties its counts to a late wake only if both hold (the user's
//!   rulings of 2026-11-04, night, the tightened rule):
//!   (a) the wake was late enough to cost the frame: for R21,
//!       `woke_late_ms >= late_ms` (an on-time wake would have submitted it on
//!       time); for R33, `woke_late_ms` covers the skipped span, `skipped`
//!       frames at the house rate;
//!   (b) the tick before it did not overrun: `previous_tick_ms` is within the
//!       loop's frame budget, `1000 / houseRate` ms. The loop does not sleep
//!       when that tick ended past this one's boundary, so its overrun is part
//!       of `woke_late_ms`, and a wake made late that way is a slow tick, not
//!       a late timer. A line without `previous_tick_ms` (the engine's R21 line
//!       before e9225a8) cannot show (b), so it ties nothing.
//!   A count whose line fails either is untied: a slow draw or a slow tick.
//! A count no line explains (the engine's rate limit suppressed it) is
//! unexplained.

/** The text of an R21 line: a frame drawn but submitted after the next frame's boundary. */
export const R21_LINE = "R21: master frame";
/** The text of an R33 line: master frames never drawn. */
export const R33_LINE = "R33: master frames";

/**
 * A drop line's numeric fields, as tracing writes them: `key=value`, with the
 * engine's colour codes around the key and the `=` stripped first, and a float
 * in Rust's Debug form (`3.5456250000000002`, or `1e-6` when tiny).
 */
export function lineFields(line: string): Record<string, number> {
  const out: Record<string, number> = {};
  const plain = line.replace(/\u001b\[[0-9;]*m/g, "");
  for (const m of plain.matchAll(/\b([a-z_]+)=(-?\d+(?:\.\d+)?(?:e-?\d+)?)\b/g)) {
    out[m[1]!] = Number(m[2]);
  }
  return out;
}

export interface DropAccount {
  tied: number[];
  untied: number[];
  unexplained: number[];
  /** The lines that explain the step's counts, each once. */
  lines: string[];
}

/**
 * The counts in (before, after], read against `lines` (the engine's drop
 * lines): each tied to a late wake by its line, not tied, or explained by none.
 */
export function accountDrops(
  lines: readonly string[],
  before: number,
  after: number,
  houseRate: number,
): DropAccount {
  const explains = new Map<number, { line: string; tied: boolean }>();
  for (const line of lines) {
    const f = lineFields(line);
    const r21 = line.includes(R21_LINE);
    const n = r21 ? 1 : (f["skipped"] ?? Number.NaN);
    const total = f["dropped_total"] ?? Number.NaN;
    if (!Number.isInteger(n) || !Number.isInteger(total)) continue;
    const woke = f["woke_late_ms"] ?? Number.NaN;
    const wokeLateEnough = r21 ? woke >= (f["late_ms"] ?? Number.NaN) : woke >= (n * 1000) / houseRate;
    const tickBeforeOnBudget = (f["previous_tick_ms"] ?? Number.NaN) <= 1000 / houseRate;
    const tied = wokeLateEnough && tickBeforeOnBudget;
    for (let count = total - n + 1; count <= total; count++) explains.set(count, { line, tied });
  }
  const account: DropAccount = { tied: [], untied: [], unexplained: [], lines: [] };
  for (let count = before + 1; count <= after; count++) {
    const e = explains.get(count);
    if (e === undefined) {
      account.unexplained.push(count);
      continue;
    }
    (e.tied ? account.tied : account.untied).push(count);
    if (!account.lines.includes(e.line)) account.lines.push(e.line);
  }
  return account;
}

// --- Item 10: the dress count's second reading --------------------------------
//
// The user's ruling of 2026-11-18 04:20 (ruling 3), approved as proposed: the
// gate reads the show once, after step 9, so the drops of steps 10 to 12c
// (step 11's and 12b's Preview, View and read-ahead waits among them) were
// never counted. A second reading at the end of step 12c, the last step on the
// same engine process (step 13 kills it), counts them per step against
// ceilings taken from ten CI runs (1.25 x the ten-run maximum, rounded up).
// PROVISIONAL: re-derived after ten main runs of the new line, tightened after
// read-ahead stage (b).

/** A step's ceiling: a count, or R21's rule (only drops tied to a late wake). */
export type StepCeiling = number | "tied";

/** Steps 10 to 12c. Ten runs read step 11 at most 7 and step 12b at most 8; the rest 0. */
export const SECOND_READING_CEILINGS: Readonly<Record<string, StepCeiling>> = {
  "step 10": "tied",
  "step 11": 9,
  "step 12": "tied",
  "step 12b": 10,
  "step 12c": "tied",
};

/** The span's own ceiling: the sum of the steps' (9 + 10). */
export const SECOND_READING_SPAN_CEILING = 19;

/** The show's counts on a tick. A field the tick lacks reads NaN, which no gate accepts. */
export interface ShowCounts {
  drops: number;
  underruns: number;
  watchdogTrips: number;
  fallbackTicks: number;
}

/** The show's counts on the last of `ticks` (the §10.1 telemetry frames, in arrival order). */
export function countsAt(ticks: readonly Record<string, unknown>[]): ShowCounts {
  const data = ticks.map((t) => (t["data"] ?? {}) as Record<string, unknown>);
  const last = data.at(-1) ?? {};
  const num = (v: unknown): number => (typeof v === "number" ? v : Number.NaN);
  return {
    drops: num(last["droppedFramesTotal"]),
    underruns: num(last["audioUnderrunsTotal"]),
    watchdogTrips: num(last["watchdogTripsTotal"]),
    fallbackTicks: data.filter((d) => d["fallbackActive"] === true).length,
  };
}

/**
 * The second reading: the counts at the gate's tick (the first `gateTicks`
 * ticks, what the gate read), at the last tick, and the span between them.
 */
export function secondReading(
  ticks: readonly Record<string, unknown>[],
  gateTicks: number,
): { atGate: ShowCounts; atEnd: ShowCounts; span: ShowCounts } {
  const atGate = countsAt(ticks.slice(0, gateTicks));
  // MUTANT M1 (item 10, do not merge): the end read at the gate's tick.
  const atEnd = countsAt(ticks.slice(0, gateTicks));
  return {
    atGate,
    atEnd,
    span: {
      drops: atEnd.drops - atGate.drops,
      underruns: atEnd.underruns - atGate.underruns,
      watchdogTrips: atEnd.watchdogTrips - atGate.watchdogTrips,
      fallbackTicks: atEnd.fallbackTicks - atGate.fallbackTicks,
    },
  };
}

/** One step's drops: the counts in (before, after], accounted against the engine's lines. */
export interface StepDrops {
  step: string;
  before: number;
  after: number;
  account: DropAccount;
}

/**
 * The second reading's verdict on a runner: every ceiling's step measured,
 * each inside its ceiling, a tied-only step's drops all tied to a late wake
 * and, with the show's earlier tied drops, inside the show's tied cap, and the
 * span inside its own ceiling. Returns the failures; none is a pass.
 */
export function judgeSecondReading(
  steps: readonly StepDrops[],
  showTiedBefore: number,
  tiedCap: number,
  ceilings: Readonly<Record<string, StepCeiling>> = SECOND_READING_CEILINGS,
  spanCeiling: number = SECOND_READING_SPAN_CEILING,
): string[] {
  const failures: string[] = [];
  let total = 0;
  let tied = showTiedBefore;
  for (const [step, ceiling] of Object.entries(ceilings)) {
    const s = steps.find((x) => x.step === step);
    const n = s === undefined ? Number.NaN : s.after - s.before;
    if (s === undefined || !Number.isInteger(n) || n < 0) {
      failures.push(`${step}: its drops were not measured (${s === undefined ? "no reading" : `${s.before} -> ${s.after}`})`);
      continue;
    }
    total += n;
    if (ceiling === "tied") {
      if (s.account.unexplained.length > 0) {
        failures.push(`${step}: no engine line explains drop count(s) ${JSON.stringify(s.account.unexplained)}`);
      }
      if (s.account.untied.length > 0) {
        failures.push(`${step}: drop count(s) ${JSON.stringify(s.account.untied)} are not tied to a late wake`);
      }
      tied += s.account.tied.length;
    } else if (n > ceiling + 1) {
      // MUTANT M2 (item 10, do not merge): a ceiling admitting one more.
      failures.push(`${step}: ${n} drops, over its ceiling of ${ceiling}`);
    }
  }
  if (total > spanCeiling) failures.push(`steps 10 to 12c: ${total} drops, over the span's ceiling of ${spanCeiling}`);
  if (tied > tiedCap) failures.push(`${tied} drops tied to a late wake this show, over the cap of ${tiedCap}`);
  return failures;
}
