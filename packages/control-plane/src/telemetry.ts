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
}

/** The stub a telemetry field carries before its subsystem has run (§10.1.1).
 *  Mirrors `nbe_protocol::tap_none`. */
export const TAP_NONE = "none";

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
    // §10.1 law: buffered ms, 0 when nothing is buffered or no report is
    // fresh. Idle vs drained-live is `streamState`'s to say, on this tick.
    streamBufferMs: f?.streamBufferMs ?? 0,
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
