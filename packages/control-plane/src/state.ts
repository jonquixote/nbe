//! Control plane state (Section 5.1, 17): show state, item/scene state
//! machines, crash recovery, automationHold.
//!
//! Semantics (Addendum 02a §2):
//! - Mutators in this file NEVER bump `stateVersion`. Mutations are applied
//!   freely; the dispatcher bumps exactly once per accepted command, and once
//!   per detection transaction (engine-driven lifecycle events).
//! - No mutate-then-throw: every precondition check runs before any write.
//! - Persistance lives in `persistence.ts` (async, dirty-flag + debounce).

import { CpError } from "./protocol.js";

export type ItemState = "READY" | "ARMED" | "LIVE" | "PLAYING" | "DONE" | "MISSING" | "ERROR";
export type SceneState = "IDLE" | "ARMED" | "VIEW" | "TRANSITIONING";
export type ShowState = "UNLOADED" | "LOADED" | "RUNNING" | "STOPPED";
export type StreamState = "idle" | "live" | "reconnecting";
export type RecordState = "idle" | "recording";
export type AudioMode = "follow" | "crossfade" | "cut" | "mute";

// ---------------------------------------------------------------------------
// Show package — the slice of the manifest the control plane indexes.
// Type information is generated from schemas/manifest.v0.3.json via
// scripts/gen-manifest-types.mjs (see addendum 1.4); validation of a package
// is ALWAYS done by shelling out to nbe-preflight, never re-implemented here.
// ---------------------------------------------------------------------------

export interface PackageItem {
  id: string;
  kind: string;
  sceneRef?: string | undefined;
  sequenceRef?: string | undefined;
  assetId?: string | undefined;
  sourceId?: string | undefined;
  durationFrames?: number | undefined;
  autoFollow?: boolean | undefined;
  audioPolicy?: string | undefined;
}

export interface PackageElement {
  id: string;
  kind: string;
}

export interface PackageInfo {
  packagePath: string;
  showId: string;
  items: Map<string, PackageItem>;
  sequences: Set<string>;
  scenes: Map<string, { elements: PackageElement[] }>;
  elements: Map<string, PackageElement>;
  overlays: Set<string>;
  templates: Set<string>;
  breakingTemplates: Set<string>;
  tickerExists: boolean;
  clockElements: Set<string>;
  plugins: Set<string>;
  automationRules: Set<string>;
  /** asset id → source path relative to package root */
  assets: Map<string, string>;
  /** transition preset id → preset config (Section 16.2) */
  transitionPresets: Map<string, Record<string, unknown>>;
  fallbackAssetId: string | undefined;
}

export interface GuestState {
  guestId: string;
  displayName?: string | undefined;
  layout: "pip" | "full";
  muted: boolean;
  placeholderAssetId?: string | undefined;
  returnConfig: {
    mode: "programMinusSelf" | "producerMix" | "mute";
    includeOtherGuests: boolean;
    gainDb?: number | undefined;
    muted?: boolean | undefined;
  };
}

export interface SnapshotState {
  viewItem: string | null;
  previewItem: string | null;
  itemStates: Record<string, ItemState>;
  visibleOverlays: string[];
  automationHold: boolean;
}

export interface DeprecationRecord {
  command: string;
  resolvedTo: string;
  stateVersionAtTime: number;
}

export interface ManifestIdentity {
  packagePath: string;
  showId: string;
  manifestVersion: string;
}

const SHOW_STATES: readonly ShowState[] = ["UNLOADED", "LOADED", "RUNNING", "STOPPED"];

// ---------------------------------------------------------------------------
// Control plane state — plain data + guard-checked mutators. No version bumps
// happen here (single-bump rule), no I/O, no timers.
// ---------------------------------------------------------------------------

export class ControlPlaneState {
  stateVersion = 0;

  showState: ShowState = "UNLOADED";
  pkg: PackageInfo | null = null;
  preflightPassed = false;
  lastError: string | null = null;

  viewItem: string | null = null;
  previewItem: string | null = null;
  fallbackActive = false;

  itemStates = new Map<string, ItemState>();
  sceneStates = new Map<string, SceneState>();
  elementOverrides = new Map<string, Record<string, unknown>>();
  graphics = new Map<string, { templateId: string; fields: Record<string, unknown> }>();
  breakingVisible = false;
  breakingFields: { headline: string; subhead?: string } | null = null;
  visibleOverlays = new Set<string>();

  tickerSource: "manual" | "rss" | "mixed" = "manual";
  tickerItems: Array<{ text: string; language?: string; priority: number; ttlSec?: number }> = [];

  audioBuses = new Map<string, { gainDb?: number; muted: boolean }>();
  ducking = { enabled: false, depthDb: -6, attackMs: 10, releaseMs: 250 };
  soundboardPings: Array<{ playbackId: string; assetId: string; gainDb?: number | undefined }> = [];
  guests = new Map<string, GuestState>();

  automationHold = false;
  automationRules = new Map<string, boolean>();

  snapshots = new Map<string, SnapshotState>();
  markers: Array<{ name: string; timecode?: string | undefined }> = [];

  recordState: RecordState = "idle";
  streamState: StreamState = "idle";

  /** Deprecation warnings to surface on the next telemetry tick. */
  pendingDeprecations: DeprecationRecord[] = [];

  /** Set when polling listeners need a nudge; cleared by the dispatcher. */
  changed = false;

  // -- cheap shared helpers ---------------------------------------------------

  bump(): number {
    this.stateVersion += 1;
    this.changed = true;
    return this.stateVersion;
  }

  noteDeprecation(command: string, resolvedTo: string): void {
    this.pendingDeprecations.push({ command, resolvedTo, stateVersionAtTime: this.stateVersion });
  }

  drainDeprecations(): DeprecationRecord[] {
    const out = this.pendingDeprecations;
    this.pendingDeprecations = [];
    return out;
  }

  outputsActive(): boolean {
    return this.recordState === "recording" || this.streamState === "live";
  }

  requirePackage(): PackageInfo {
    if (!this.pkg) throw new CpError("E_FORBIDDEN_STATE", "no show package loaded");
    return this.pkg;
  }

  requireItem(itemRef: string): PackageItem {
    const item = this.requirePackage().items.get(itemRef);
    if (!item) throw new CpError("E_NOT_FOUND", `no such item: ${itemRef}`);
    return item;
  }

  requireElement(elementId: string): void {
    if (!this.requirePackage().elements.has(elementId)) {
      throw new CpError("E_NOT_FOUND", `no such element: ${elementId}`);
    }
  }

  requireOverlay(overlayId: string): void {
    if (!this.requirePackage().overlays.has(overlayId)) {
      throw new CpError("E_NOT_FOUND", `no such overlay: ${overlayId}`);
    }
  }

  // -- Section 17.3 item transitions ------------------------------------------
  // Reads of the transition table below must hold before any write happens.

  itemStateOf(itemRef: string): ItemState {
    return this.itemStates.get(itemRef) ?? "READY";
  }

  /** READY/DONE -> ARMED. Guard: asset valid (checked by the caller against on-disk facts). */
  armItem(itemRef: string): void {
    this.requireItem(itemRef);
    const cur = this.itemStateOf(itemRef);
    if (cur !== "READY" && cur !== "DONE") {
      throw new CpError("E_FORBIDDEN_STATE", `item ${itemRef} is ${cur}, cannot arm`);
    }
    this.itemStates.set(itemRef, "ARMED");
  }

  /** ARMED -> READY. */
  unarmItem(itemRef: string): void {
    this.requireItem(itemRef);
    const cur = this.itemStateOf(itemRef);
    if (cur !== "ARMED") throw new CpError("E_FORBIDDEN_STATE", `item ${itemRef} is ${cur}, cannot unarm`);
    this.itemStates.set(itemRef, "READY");
  }

  /** ARMED -> LIVE (untimed) or PLAYING (timed). Previous live item -> READY. */
  take(itemRef: string): ItemState {
    this.requireItem(itemRef);
    const cur = this.itemStateOf(itemRef);
    if (cur !== "ARMED") throw new CpError("E_FORBIDDEN_STATE", `item ${itemRef} is ${cur}, cannot take`);
    const pkgItem = this.requireItem(itemRef);
    const next: ItemState = pkgItem.durationFrames != null ? "PLAYING" : "LIVE";
    if (this.viewItem && this.viewItem !== itemRef) {
      const prev = this.itemStateOf(this.viewItem);
      if (prev === "LIVE" || prev === "PLAYING") this.itemStates.set(this.viewItem, "READY");
    }
    this.itemStates.set(itemRef, next);
    this.viewItem = itemRef;
    if (this.previewItem === itemRef) this.previewItem = null;
    this.fallbackActive = false;
    return next;
  }

  /** PLAYING -> READY. */
  stopItem(itemRef: string): void {
    this.requireItem(itemRef);
    const cur = this.itemStateOf(itemRef);
    if (cur !== "PLAYING") throw new CpError("E_FORBIDDEN_STATE", `item ${itemRef} is ${cur}, cannot stop`);
    this.itemStates.set(itemRef, "READY");
  }

  /** PLAYING -> DONE (engine mediaEnd lifecycle event). */
  markDone(itemRef: string): void {
    const cur = this.itemStateOf(itemRef);
    if (cur !== "PLAYING") return; // idempotent for engine replays
    this.itemStates.set(itemRef, "DONE");
  }

  /** Detection transition -> MISSING (own transaction per 02a §2.2). */
  markMissing(itemRef: string): void {
    this.itemStates.set(itemRef, "MISSING");
    if (this.viewItem === itemRef) this.fallbackActive = true;
  }

  /** Detection/error transition -> ERROR. */
  markError(itemRef: string): void {
    this.itemStates.set(itemRef, "ERROR");
    if (this.viewItem === itemRef) this.fallbackActive = true;
  }

  /** ERROR/MISSING/DONE -> READY. */
  resetItem(itemRef: string): void {
    this.requireItem(itemRef);
    this.itemStates.set(itemRef, "READY");
  }

  // -- Section 17.2 scene states ------------------------------------------------

  requireScene(sceneId: string): void {
    if (!this.requirePackage().scenes.has(sceneId)) throw new CpError("E_NOT_FOUND", `no such scene: ${sceneId}`);
  }

  armScene(sceneId: string): void {
    this.requireScene(sceneId);
    this.sceneStates.set(sceneId, "ARMED");
    if (this.previewItem == null) this.previewItem = sceneId;
  }

  applyScene(sceneId: string, target: "view" | "preview"): void {
    this.requireScene(sceneId);
    this.sceneStates.set(sceneId, target === "view" ? "VIEW" : "ARMED");
  }

  // -- snapshots ----------------------------------------------------------------

  saveSnapshot(name: string): void {
    this.snapshots.set(name, {
      viewItem: this.viewItem,
      previewItem: this.previewItem,
      itemStates: Object.fromEntries(this.itemStates),
      visibleOverlays: Array.from(this.visibleOverlays),
      automationHold: this.automationHold,
    });
  }

  recallSnapshot(name: string): void {
    const snap = this.snapshots.get(name);
    if (!snap) throw new CpError("E_NOT_FOUND", `no such snapshot: ${name}`);
    this.viewItem = snap.viewItem;
    this.previewItem = snap.previewItem;
    this.itemStates = new Map(Object.entries(snap.itemStates));
    this.visibleOverlays = new Set(snap.visibleOverlays);
    this.automationHold = snap.automationHold;
  }

  // -- show lifecycle -----------------------------------------------------------

  loadPackage(pkg: PackageInfo): void {
    this.pkg = pkg;
    this.showState = "LOADED";
    this.preflightPassed = false;
    this.viewItem = null;
    this.previewItem = null;
    this.itemStates.clear();
    this.sceneStates.clear();
    this.fallbackActive = false;
    for (const id of pkg.automationRules) this.automationRules.set(id, true);
  }

  unloadPackage(): void {
    this.pkg = null;
    this.showState = "UNLOADED";
    this.viewItem = null;
    this.previewItem = null;
    this.itemStates.clear();
    this.sceneStates.clear();
    this.elementOverrides.clear();
    this.graphics.clear();
    this.visibleOverlays.clear();
    this.automationRules.clear();
    this.fallbackActive = false;
  }

  // -- status / snapshots for persistence and the health endpoint ---------------

  statusSnapshot(): Record<string, unknown> {
    return {
      showState: this.showState,
      package: this.pkg ? { path: this.pkg.packagePath } : null,
      preflightPassed: this.preflightPassed,
      stateVersion: this.stateVersion,
      viewItem: this.viewItem,
      previewItem: this.previewItem,
      fallbackActive: this.fallbackActive,
      recordState: this.recordState,
      streamState: this.streamState,
      automationHold: this.automationHold,
      lastError: this.lastError,
    };
  }

  manifestIdentity(): ManifestIdentity | null {
    if (!this.pkg) return null;
    return {
      packagePath: this.pkg.packagePath,
      showId: this.pkg.showId,
      manifestVersion: "0.3",
    };
  }

  static isShowState(v: string): v is ShowState {
    return (SHOW_STATES as readonly string[]).includes(v);
  }
}
