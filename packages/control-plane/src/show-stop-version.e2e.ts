//! Queue position 2: `show.stop` on the real gate. ~~Establish-first (the
//! user's word of 2026-10-12): NO FIX. These tests record today's behaviour
//! so that the design question is decided on evidence. The fix's work order
//! inverts them.~~ (§2c, 2026-10-14: the establishing tests, PR #42, merged
//! as `067e709`, are inverted here by the (C) fix, the user's ruling of
//! 2026-10-14; SPEC v0.4.8 row 6.)
//!
//! ~~The read under test~~ What PR #42 established, and the fix removes:
//! - `show.stop` sent `record.stop` and `stream.stop` at `ctx.stateVersion`
//!   whenever any output was active (`commands/show.ts`, `emitDirectivesNow`),
//!   and the dispatcher then forwarded `show.stop` itself at the same version.
//! - The engine's gate skips a directive whose version is not above the last
//!   it applied (`channel.rs` `classify`, `<=`), so at most the first applied.
//! - A skipped directive never advances `expected_seq`, so the next directive
//!   met a sequence gap, and that requested a resync (§5.9.4 rule 5).
//! - `on_resync` has no output handling, so with record and stream active the
//!   stream stayed on air through the stop and the resync.
//!
//! Under (C) the control plane sends `show.stop` alone, at the dispatcher's
//! version, and the engine's quiesce arm (`on_show_stop_inner`) is the stop.
//! The control plane learns the outputs' state from `show.stop`'s own
//! acknowledgement, never at send. The legs, each on its own control plane
//! and engine:
//! - E1: record and stream active; E2: stream only; E3: record only;
//! - E4: neither, the control case, unchanged;
//! - concurrent: E1 with a second operator's command inside the ack window;
//! - held: E1 with the record finalize forced to fail (the record directory
//!   read-only, so the sidecar write fails): the engine withholds the ack and
//!   the control plane holds the outputs.
//!
//! The real engine binary runs on real sockets, behind a tap that records the
//! render channel (`show-stop.harness.ts`), and the stream publishes into a
//! minimal RTMP sink whose byte count is the on-air observable.
//!
//! The record and stream legs need a hardware H.264 encoder, so the engine
//! refuses them on CI's runner. They skip loudly there, on the engine log's
//! own refusal, as the dress rehearsal's capability gate does. On the
//! normative machine every leg must run.
//!
//! Not in `npm test`: it needs the engine binary. Run locally:
//!   node --import tsx --test src/show-stop-version.e2e.ts

import { test } from "node:test";
import assert from "node:assert/strict";

import { leg, type LegResult, type Observed } from "./show-stop.harness.js";

function report(r: LegResult, outputs: object): void {
  console.log(`SSV-LEG ${JSON.stringify({ outputs, ...r.observed, concurrent: r.concurrent })}`);
  console.log(`SSV-TIMING ${JSON.stringify({ outputs, ...r.timing })}`);
  for (const l of r.gateLines) console.log(`SSV-GATE ${l}`);
}

/**
 * The stop's half of every leg whose quiesce succeeds: one directive, applied,
 * acknowledged, no gap. The applied version is read after the stop's watch,
 * before the next directive, except in the concurrent leg, whose second
 * command was already that directive.
 */
function cleanStop(o: Observed, concurrent = false): Partial<Observed> {
  return {
    directivesFromStop: [
      ["show.stop", o.stopVersion],
      ["view.fallback", o.stopVersion + 1],
    ],
    acksFromStop: [o.stopVersion, o.stopVersion + 1],
    skippedFromStop: 0,
    failedFromStop: [],
    seqGapsFromStop: 0,
    resyncsFromStop: 0,
    appliedVersionAfterStop: concurrent ? o.stopVersion + 1 : o.stopVersion,
    clockRanAfterStop: false,
    clockStoppedAfterNext: true,
    packageReloadsFromStop: 0,
    afterHeal: null,
  };
}

const SIDECAR = ["ssv-open", "ssv-mid"];

test("[queue 2] E1, record and stream active: show.stop alone stops both; the stream goes off air and the take finalizes with its sidecar", async (t) => {
  const r = await leg({ record: true, stream: true }, (why) => t.skip(why));
  if (!r) return;
  report(r, { record: true, stream: true });
  const o = r.observed;
  assert.deepEqual(
    o,
    {
      ...o,
      ...cleanStop(o),
      controlPlaneAtSend: { record: "recording", stream: "live" },
      stopWarned: false,
      controlPlaneRecordState: "idle",
      controlPlaneStreamState: "idle",
      streamOnAirAfterStop: false,
      streamOnAirAfterNext: false,
      engineStreamTransportAfterStop: "closed",
      tickStreamStateAfterStop: "idle",
      recordingGrowingAfterStop: false,
      sidecarMarkers: SIDECAR,
    },
    "E1 under (C)",
  );
  if (o.finalized) assert.equal(o.finalized.streams, 2, "the finished take reads: video and audio");
  if (o.finalized) assert.ok(o.finalized.durationS > 0, "the finished take has a duration");
});

test("[queue 2] E2, stream only: show.stop alone takes the stream off air", async (t) => {
  const r = await leg({ record: false, stream: true }, (why) => t.skip(why));
  if (!r) return;
  report(r, { record: false, stream: true });
  const o = r.observed;
  assert.deepEqual(
    o,
    {
      ...o,
      ...cleanStop(o),
      controlPlaneAtSend: { record: "idle", stream: "live" },
      stopWarned: false,
      controlPlaneRecordState: "idle",
      controlPlaneStreamState: "idle",
      streamOnAirAfterStop: false,
      streamOnAirAfterNext: false,
      engineStreamTransportAfterStop: "closed",
      tickStreamStateAfterStop: "idle",
    },
    "E2 under (C)",
  );
});

test("[queue 2] E3, record only: show.stop alone finalizes the take with its sidecar", async (t) => {
  const r = await leg({ record: true, stream: false }, (why) => t.skip(why));
  if (!r) return;
  report(r, { record: true, stream: false });
  const o = r.observed;
  assert.deepEqual(
    o,
    {
      ...o,
      ...cleanStop(o),
      controlPlaneAtSend: { record: "recording", stream: "idle" },
      stopWarned: false,
      controlPlaneRecordState: "idle",
      controlPlaneStreamState: "idle",
      recordingGrowingAfterStop: false,
      sidecarMarkers: SIDECAR,
    },
    "E3 under (C)",
  );
  if (o.finalized) assert.equal(o.finalized.streams, 2, "the finished take reads: video and audio");
  if (o.finalized) assert.ok(o.finalized.durationS > 0, "the finished take has a duration");
});

test("[queue 2] E4, neither output active (control): show.stop applies, the clock stops, nothing is skipped and no gap follows", async (t) => {
  const r = await leg({ record: false, stream: false }, (why) => t.skip(why));
  if (!r) return;
  report(r, { record: false, stream: false });
  const o = r.observed;
  // PR #42's assertions, kept: the control case was right before the fix.
  assert.deepEqual(
    { ...o, appliedVersionAfterStop: (o.appliedVersionAfterStop ?? 0) >= o.stopVersion },
    {
      ...o,
      skippedFromStop: 0,
      failedFromStop: [],
      appliedVersionAfterStop: true,
      clockRanAfterStop: false,
      seqGapsFromStop: 0,
      resyncsFromStop: 0,
      packageReloadsFromStop: 0,
    },
    "the control case",
  );
  assert.deepEqual(o, { ...o, ...cleanStop(o), stopWarned: false }, "the control case, on the wire");
});

test("[queue 2] concurrent: a second operator's command inside the ack window is not skipped; the versions are distinct and ordered; the stream ends off air", async (t) => {
  const r = await leg({ record: true, stream: true, concurrent: true }, (why) => t.skip(why));
  if (!r) return;
  report(r, { record: true, stream: true, concurrent: true });
  console.log(`SSV-STATECHANGE-ORDER ${JSON.stringify(r.stateChangeOrder)}`);
  const o = r.observed;
  assert.deepEqual(
    o,
    {
      ...o,
      ...cleanStop(o, true),
      controlPlaneAtSend: { record: "recording", stream: "live" },
      stopWarned: false,
      controlPlaneRecordState: "idle",
      controlPlaneStreamState: "idle",
      streamOnAirAfterStop: false,
      streamOnAirAfterNext: false,
      engineStreamTransportAfterStop: "closed",
      recordingGrowingAfterStop: false,
      sidecarMarkers: SIDECAR,
    },
    "the concurrent leg",
  );
  assert.equal(r.concurrent?.stateVersion, o.stopVersion + 1, "the second command takes the next version");
});

// (§2c, v0.4.8 row 11: this test's name ended "and the control plane holds the
// outputs until the operator's explicit stops". The stream's belief goes
// `held`, §5.9.5's limbo, on the command's path, and a fresh report then
// corrects it; the recording stays `recording`, since no engine report
// covers it.)
test("[queue 2] held: a quiesce that fails withholds the ack; the stream's belief goes held, then follows the engine; the recording stays recording until the operator's record.stop", async (t) => {
  const r = await leg({ record: true, stream: true, failRecordFinalize: true }, (why) => t.skip(why));
  if (!r) return;
  report(r, { record: true, stream: true, failRecordFinalize: true });
  const o = r.observed;
  assert.ok(
    r.gateLines.some((l) => l.includes("show.stop: recording finalize failed")),
    `the forced failure is the finalize's: ${JSON.stringify(r.gateLines)}`,
  );
  assert.deepEqual(
    o,
    {
      ...o,
      // (§2c, row 11) ~~["stream.stop", o.stopVersion + 3]~~: the stream's
      // belief was corrected to idle, so there is no stream to stop.
      // ~~view.fallback at o.stopVersion + 1, record.stop at + 2~~: the
      // correction held→idle is a detection transaction that bumps the
      // version once (§10.1.4, D4), and it lands between the stop and the
      // slate, so every later directive is one version on. It is not a
      // directive, so it takes no row here.
      directivesFromStop: [
        ["show.stop", o.stopVersion],
        ["view.fallback", o.stopVersion + 2],
        ["record.stop", o.stopVersion + 3],
      ],
      // show.stop failed (no ack); the slate applied; the heal's record.stop
      // is refused by an engine already idle (no ack).
      acksFromStop: [o.stopVersion + 2],
      failedFromStop: ["show.stop", "record.stop"],
      skippedFromStop: 0,
      seqGapsFromStop: 0,
      resyncsFromStop: 0,
      clockRanAfterStop: false,
      controlPlaneAtSend: { record: "recording", stream: "live" },
      stopWarned: true,
      // HELD: the control plane cannot know, so it does not report idle.
      controlPlaneRecordState: "recording",
      // (§2c, row 11) ~~"live"~~: on show.stop's own frame the stream belief
      // is `held`, and the next fresh report (the engine's `closed`) corrects
      // it to `idle`.
      controlPlaneStreamStateAtStop: "held",
      controlPlaneStreamState: "idle",
      // ~~What the operator sees on the tick: the control plane's live beside
      // the engine's own transport, not live.~~ (§2c, row 11): the tick
      // carries the corrected belief beside the engine's `closed`.
      tickStreamStateAfterStop: "idle",
      engineStreamTransportAfterStop: "closed",
      streamOnAirAfterStop: false,
      sidecarMarkers: null,
      afterHeal: { record: "idle", stream: "idle" },
    },
    "the held leg",
  );
});
