//! When a show's load is complete on the render thread (R25, R29; R19's class).
//!
//! `show.load`'s acknowledgement precedes the render thread's load boundary,
//! where every decoded video asset's ring is uploaded on the thread the
//! directives share (R20's disclosed cost). A directive sent before that
//! boundary ends waits behind the uploads, and a harness that times it reads
//! the wait as its own failure. The engine logs each video asset it decodes at
//! the load ("video asset decoded") and each ring as it lands ("clip ring
//! resident" or "loop ring resident", the render target's lines): the load is
//! complete when every decoded asset's ring is resident.
//!
//! Shared by the operator-signals lane (R25, #60) and the dress rehearsal
//! (R29), so the two harnesses cannot disagree about what "loaded" means.

export interface Residency {
  /** Video assets the engine decoded at the load. */
  decoded: number;
  /** Rings the render thread made resident. */
  resident: number;
}

/** Count the load's decoded assets and resident rings in these log lines. */
export function residency(lines: readonly string[]): Residency {
  const count = (needle: string): number => lines.filter((l) => l.includes(needle)).length;
  return { decoded: count("video asset decoded"), resident: count("ring resident") };
}

/** Every decoded asset's ring is resident. */
export function loadComplete(r: Residency): boolean {
  return r.decoded > 0 && r.resident >= r.decoded;
}
