//! Telemetry (Section 10.1 + addendum 02a §1.2): ownership is split.
//! The control plane owns show-state fields; the render node owns clock/perf
//! fields. The merge caches the last engine report with a staleness
//! threshold — a stale report means stub values plus `engineConnected: false`.
//! Field shape is always complete: consumers never see a missing field.

import type { EngineTelemetryFrame } from "./protocol.js";
import type { ControlPlaneState } from "./state.js";

export interface TelemetryTick {
  // timing
  ts: number;
  // engine-owned (stubbed when stale)
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
  streamState: string;
  recordState: string;
  automationHold: boolean;
  qualityProfile: string | null;
  // addendum fields
  engineConnected: boolean;
  deprecationWarnings: Array<{ command: string; resolvedTo: string; stateVersionAtTime: number }>;
}

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
    masterClockFrame: f?.masterClockFrame ?? 0,
    droppedFramesTotal: f?.droppedFramesTotal ?? 0,
    renderGpuTimeMs: f?.renderGpuTimeMs ?? 0,
    decodeSessions: f?.decodeSessions ?? 0,
    vramUsedMib: f?.vramUsedMib ?? 0,
    textureCacheUsedMib: f?.textureCacheUsedMib ?? 0,
    streamBufferMs: f?.streamBufferMs ?? 0,
    recordSpaceMib: f?.recordSpaceMib ?? 0,
    masterClockDriftMs: f?.masterClockDriftMs ?? 0,
    fallbackActive: f?.fallbackActive ?? state.fallbackActive,
    degradationRung: f?.degradationRung ?? 0,
    viewItem: state.viewItem,
    previewItem: state.previewItem,
    streamState: state.streamState,
    recordState: state.recordState,
    automationHold: state.automationHold,
    qualityProfile: null,
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
