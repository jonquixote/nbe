//! Telemetry (Section 10.1 + addendum 02a §1.2): ownership is split.
//! The control plane owns show-state fields; the render node owns clock/perf
//! fields. The merge caches the last engine report with a staleness
//! threshold — a stale report means stub values plus `engineConnected: false`.
//! Field shape is always complete: consumers never see a missing field.

import type { EngineTelemetryFrame } from "./protocol.js";
import type { ControlPlaneState, ShowState } from "./state.js";

export interface TelemetryTick {
  // timing
  ts: number;
  // engine-owned (stubbed when stale)
  /** SPEC §10.1 (v0.4): control-plane owned. */
  showState: ShowState;
  masterClockFrame: number;
  droppedFramesTotal: number;
  renderGpuTimeMs: number;
  decodeSessions: number;
  vramUsedMib: number;
  textureCacheUsedMib: number;
  streamBufferMs: number;
  recordSpaceMib: number;
  masterClockDriftMs: number;
  fallbackActive: boolean;
  degradationRung: number;
  // control-plane-owned
  viewItem: string | null;
  previewItem: string | null;
  /** On-air overlays (SPEC §7.10); surfaces `overlay.show`/`hide`. */
  visibleOverlays: string[];
  streamState: string;
  recordState: string;
  automationHold: boolean;
  qualityProfile: string | null;
  // audio (SPEC §8.9, §8.10, new in v0.3.3)
  audioUnderrunsTotal: number;
  audioDriftMs: number;
  busPeakDbfs: Record<string, number>;
  // addendum fields
  engineConnected: boolean;
  deprecationWarnings: Array<{ command: string; resolvedTo: string; stateVersionAtTime: number }>;
  /** ZERO-COPY: which frame path the record tap took — `"zeroCopy"`,
   *  `"cpuReadback"`, or `"none"` before any take has selected one. Required,
   *  not optional: §10.1.1 says a telemetry consumer must never see a missing
   *  field, and that holds for the control plane's tick as much as the
   *  engine's. */
  recordTapPath: string;
  /** Why that path was chosen, so a fallback is distinguishable from a choice.
   *  `"none"` before any take. Required, same reason. */
  recordTapReason: string;
  /** SPEC §10.1 (v0.4.6): the stream transport's own state — `"live"`,
   *  `"reconnecting"`, `"closed"`, or `"none"` before any stream has started.
   *  Engine-owned; `streamState` above is the commanded one and stays `"live"`
   *  through a redial (§9.5), so this is where a redial is visible. Required,
   *  same reason as `recordTapPath`. */
  streamTransportState: string;
}

/** The stub a telemetry field carries before its subsystem has run (§10.1.1).
 *  Mirrors `nbe_protocol::tap_none`. */
export const TAP_NONE = "none";

/** `streamBufferMs` when no measurement exists — no stream session, or no
 *  fresh engine report (SPEC §10.1, ratified v0.4.6). Negative milliseconds
 *  are impossible, so it never collides with a live session's honest `0`,
 *  "the buffer is empty". Mirrors `nbe_protocol::STREAM_BUFFER_NO_SESSION_MS`. */
export const STREAM_BUFFER_NO_SESSION_MS = -1;

/** How long a cached engine report stays authoritative (default 2 s). */
export const ENGINE_TELEMETRY_TTL_MS = 2000;

export interface EngineReport {
  frame: EngineTelemetryFrame;
  receivedAt: number;
}

export function buildTick(
  state: ControlPlaneState,
  engine: WorldTelemetry,
  now: number,
): TelemetryTick {
  const engineFresh = engine.last !== null && now - engine.last.receivedAt <= ENGINE_TELEMETRY_TTL_MS;
  const f = engineFresh ? engine.last!.frame : null;
  return {
    ts: now,
    showState: state.showState,
    masterClockFrame: f?.masterClockFrame ?? 0,
    droppedFramesTotal: f?.droppedFramesTotal ?? 0,
    renderGpuTimeMs: f?.renderGpuTimeMs ?? 0,
    decodeSessions: f?.decodeSessions ?? 0,
    vramUsedMib: f?.vramUsedMib ?? 0,
    textureCacheUsedMib: f?.textureCacheUsedMib ?? 0,
    // §10.1, ratified v0.4.6: buffered ms while a session exists; -1 when no
    // measurement exists — the engine's no-session value forwarded, or no
    // fresh engine report at all. A live session's 0 means "the buffer is
    // empty" and stays distinguishable. ~~"§10.1 law: buffered ms, 0 when
    // nothing is buffered or no report is fresh. Idle vs drained-live is
    // `streamState`'s to say, on this tick."~~ — the repair round's wording
    // (`f8ff895`), struck per §2c; §10.1 never carried that sentence, and
    // v0.4.6 wrote the one it does.
    streamBufferMs: f?.streamBufferMs ?? STREAM_BUFFER_NO_SESSION_MS,
    recordSpaceMib: f?.recordSpaceMib ?? 0,
    masterClockDriftMs: f?.masterClockDriftMs ?? 0,
    fallbackActive: f?.fallbackActive ?? state.fallbackActive,
    degradationRung: f?.degradationRung ?? 0,
    // ZERO-COPY: always forwarded, stubbed when the engine has not reported
    // one — no stale engine report, or an engine build older than the stub.
    // Still no `?? "cpuReadback"`: `TAP_NONE` is not a path, so a machine that
    // never recorded stays distinguishable from one that fell back to the §0.1
    // assumption 24 allowance. Parsing the field at the boundary is not the
    // same as an operator seeing it, which is what these two lines are for.
    recordTapPath: f?.recordTapPath ?? TAP_NONE,
    recordTapReason: f?.recordTapReason ?? TAP_NONE,
    // SPEC §10.1, v0.4.6. Always forwarded, stubbed — the rule followed is
    // §10.1.1's completeness (the `recordTapPath` FINAL shape, Phase 3b), not
    // the F1-era one it replaced: PR #24's F1 fix forwarded the tap fields
    // only when present, on the reasoning that absence meant "no take yet".
    // §10.1.1 forbids that reasoning, so this is decided against it. A stale
    // report stubs to "none" rather than "closed": with no fresh engine
    // report the control plane does not know what the socket is doing, and
    // `engineConnected: false` already says why.
    streamTransportState: f?.streamTransportState ?? TAP_NONE,
    viewItem: state.viewItem,
    previewItem: state.previewItem,
    visibleOverlays: Array.from(state.visibleOverlays),
    streamState: state.streamState,
    recordState: state.recordState,
    automationHold: state.automationHold,
    // SPEC §10.1.1: the engine's probed (effective) profile wins while its
    // report is fresh; otherwise the manifest's requested profile is the only
    // honest answer the control plane has.
    qualityProfile: f?.qualityProfile ?? state.qualityProfile,
    audioUnderrunsTotal: f?.audioUnderrunsTotal ?? 0,
    audioDriftMs: f?.audioDriftMs ?? 0,
    busPeakDbfs: f?.busPeakDbfs ?? {},
    engineConnected: engineFresh,
    // Deprecation warnings are per-subscriber: the server fans each accepted
    // deprecated command into every subscriber's own cursor and fills this in.
    // A shared drain here would hand the warning to whichever tick fired first.
    deprecationWarnings: [],
  };
}

/** Rolling per-connection telemetry state: the last engine report. */
export interface WorldTelemetry {
  last: EngineReport | null;
}

export function newWorldTelemetry(): WorldTelemetry {
  return { last: null };
}

export function ingestEngineFrame(world: WorldTelemetry, frame: EngineTelemetryFrame, now: number): void {
  world.last = { frame, receivedAt: now };
}
