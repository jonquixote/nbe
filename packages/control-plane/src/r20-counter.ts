//! R20's picture step's counter (SPEC §12.8.1, v0.4.8 row 12): counter_150's
//! item, its length, and the classifier that reads a recorded frame back as
//! the counter frame it shows.
//!
//! Shared by the dress rehearsal's step 12c and R31's 12c probe
//! (`r31-12c.probe.ts`), so the probe reads every frame exactly as step 12c,
//! and S1's evidence, read it.

/** R20's picture step: the counter clip's item and its length in frames. */
export const R20_ITEM = "R20C";
export const R20_FRAMES = 150;

/** counter_150's frame `n` is one flat colour (scripts/generate-fixtures.sh). */
export function r20Colour(n: number): [number, number, number] {
  return [(37 * n) % 256, (91 * n) % 256, (7 * n) % 256];
}

/**
 * Which counter frame a recorded pixel shows: its index, `"black"` for a
 * cleared View (a dropped layer), or `"other"` for anything else (the item on
 * air before the take). ~~Adjacent counter frames differ by 37 in red, so the
 * nearest colour is the frame.~~ (§2c, R20's two-key pass: adjacency is not
 * the margin. Frames k and k + 76 differ by L1 28, under the threshold of 36,
 * and the engine's decode shifts colour by up to L1 56 (R22), so a recorded
 * pixel can sit nearer another frame's colour.)
 */
export function r20Classify(px: [number, number, number]): number | "black" | "other" {
  if (px.every((c) => c < 16)) return "black";
  let best = -1;
  let bestD = Infinity;
  for (let n = 0; n < R20_FRAMES; n++) {
    const c = r20Colour(n);
    const d = Math.abs(c[0] - px[0]) + Math.abs(c[1] - px[1]) + Math.abs(c[2] - px[2]);
    if (d < bestD) {
      bestD = d;
      best = n;
    }
  }
  return bestD <= 36 ? best : "other";
}
