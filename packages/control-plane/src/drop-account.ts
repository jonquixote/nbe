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
//! - an R21 line explains one count, tied to a late wake when
//!   `woke_late_ms >= late_ms` (an on-time wake would have submitted it on time);
//! - an R33 line explains `skipped` counts, tied to a late wake when
//!   `woke_late_ms` covers the skipped span, `skipped` frames at the house rate.
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
    const late = f["woke_late_ms"] ?? Number.NaN;
    const tied = r21 ? late >= (f["late_ms"] ?? Number.NaN) : late >= (n * 1000) / houseRate;
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
