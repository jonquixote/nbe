//! Persistence (Section 5.1, addendum 02a §2.9/2.10): async, dirty-flag +
//! debounce; forced immediate write on show-state transitions (show.start,
//! show.stop, show.unload go through the command path and flush). Crash
//! recovery NEVER resurrects RUNNING without a package — it restores the
//! version + package identity and requires an explicit show.load to resume.

import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { ControlPlaneState, ManifestIdentity } from "./state.js";

export interface PersistedSnapshot {
  stateVersion: number;
  manifestIdentity: ManifestIdentity | null;
  recoveredAt: number;
}

export interface PersistenceHooks {
  onDirty(): void;
  flushNow(): void;
}

export class StatePersistence implements PersistenceHooks {
  private dirty = false;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly state: ControlPlaneState,
    private readonly path: string,
    private readonly debounceMs = 250,
  ) {}

  /** Mark dirty; write after a debounce window (no sync write on the command path). */
  onDirty(): void {
    this.dirty = true;
    if (this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.flushNow();
    }, this.debounceMs);
    this.timer.unref?.();
  }

  /** Immediate write (show-state transitions). */
  flushNow(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (!this.dirty) return;
    this.dirty = false;
    const data: PersistedSnapshot = {
      stateVersion: this.state.stateVersion,
      manifestIdentity: this.state.manifestIdentity(),
      recoveredAt: Date.now(),
    };
    mkdirSync(dirname(this.path), { recursive: true });
    const tmp = this.path + ".tmp";
    writeFileSync(tmp, JSON.stringify(data, null, 2));
    renameSync(tmp, this.path);
  }

  /**
   * Rule 10: restore the version + package identity, mark recovered,
   * require an explicit show.load. NEVER restore showState RUNNING.
   */
  restore(): boolean {
    if (!existsSync(this.path)) return false;
    try {
      const data = JSON.parse(readFileSync(this.path, "utf8")) as PersistedSnapshot;
      this.state.stateVersion = data.stateVersion;
      this.state.showState = "UNLOADED";
      this.state.pkg = null;
      return data.manifestIdentity != null;
    } catch {
      return false;
    }
  }
}
