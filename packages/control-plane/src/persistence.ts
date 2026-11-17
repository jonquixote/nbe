//! Persistence (Section 5.1, addendum 02a §2.9/2.10), as the journal of
//! review4 step 4 (the user's ruling of 2026-11-16 23:45, work order 4).
//!
//! What is written: the whole control-plane state a restart would need (the
//! rundown's item states, the ticker with its TTLs, the guest registry,
//! automation, snapshots, the commanded outputs, what is on air), with the
//! version and the package identity. What is RESTORED is unchanged: Rule 10
//! restores the version and the package identity only and never resurrects
//! RUNNING; adopting a running engine is review4 step 7. The rest of the
//! journal is there for it.
//!
//! When: a command that changes on-air intent (take, cut, show lifecycle,
//! record, stream, guest) is written before its response is sent; anything
//! else within 50 ms.
//!
//! How: the body and its SHA-256 go to `<file>.tmp`, which is fsynced and
//! renamed over the file after the previous file is renamed to
//! `<file minus .json>.prev.json`; the directory is fsynced. On start the
//! newest file whose checksum holds is read (the file, then the previous
//! one). A torn or corrupted file is skipped, never half-read.

import { createHash } from "node:crypto";
import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  writeSync,
} from "node:fs";
import { dirname } from "node:path";
import type { ControlPlaneState, ManifestIdentity } from "./state.js";

export interface PersistedSnapshot {
  stateVersion: number;
  manifestIdentity: ManifestIdentity | null;
  recoveredAt: number;
}

/** What the journal holds besides the version and the package identity. */
export interface JournalBody extends PersistedSnapshot {
  journal: Record<string, unknown>;
}

/** The file: a schema, the body's checksum, and the body. */
interface JournalFile {
  schema: 2;
  checksum: string;
  body: JournalBody;
}

export interface PersistenceHooks {
  onDirty(): void;
  flushNow(): void;
  /** After an accepted command: on-air intent is written before the response. */
  onCommitted?(command: string): void;
}

/** Commands whose effect is on air: written before their response. */
const ON_AIR_PREFIXES = ["view.", "show.", "record.", "stream.", "guest."];

export function changesOnAirIntent(command: string): boolean {
  return ON_AIR_PREFIXES.some((p) => command.startsWith(p));
}

const sha256 = (s: string): string => createHash("sha256").update(s).digest("hex");

/** The previous journal beside `path` (`state.json` → `state.prev.json`). */
export function prevPath(path: string): string {
  return path.endsWith(".json") ? path.slice(0, -".json".length) + ".prev.json" : path + ".prev";
}

/** The journal of `state` now. */
export function journalOf(state: ControlPlaneState): Record<string, unknown> {
  const obj = <V>(m: Map<string, V>): Record<string, V> => Object.fromEntries(m);
  return {
    showState: state.showState,
    viewItem: state.viewItem,
    viewItemStartFrame: state.viewItemStartFrame,
    previewItem: state.previewItem,
    fallbackActive: state.fallbackActive,
    itemStates: obj(state.itemStates),
    visibleOverlays: [...state.visibleOverlays],
    graphics: obj(state.graphics),
    breakingVisible: state.breakingVisible,
    breakingFields: state.breakingFields,
    tickerSource: state.tickerSource,
    tickerItems: state.tickerItems,
    audioBuses: obj(state.audioBuses),
    guests: obj(state.guests),
    automationHold: state.automationHold,
    automationRules: obj(state.automationRules),
    snapshots: obj(state.snapshots),
    markers: state.markers,
    recordState: state.recordState,
    streamState: state.streamState,
  };
}

/** Read one journal file: its body if the checksum holds (or it is the
 * pre-journal format), else null. */
export function readJournal(path: string): JournalBody | PersistedSnapshot | null {
  if (!existsSync(path)) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
  const f = parsed as Partial<JournalFile> & Partial<PersistedSnapshot>;
  if (f.schema === 2) {
    if (typeof f.checksum !== "string" || f.body === undefined) return null;
    return sha256(JSON.stringify(f.body)) === f.checksum ? f.body : null;
  }
  // The pre-journal file (version, identity, time; no checksum).
  return typeof f.stateVersion === "number" ? (f as PersistedSnapshot) : null;
}

export class StatePersistence implements PersistenceHooks {
  private dirty = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** Which file the last restore read ("current", "previous" or null). */
  restoredFrom: "current" | "previous" | null = null;

  constructor(
    private readonly state: ControlPlaneState,
    private readonly path: string,
    private readonly debounceMs = 50,
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

  /** After an accepted command: an on-air command is written now, before
   * its response is sent; anything else within the debounce. */
  onCommitted(command: string): void {
    if (changesOnAirIntent(command)) {
      this.dirty = true;
      this.flushNow();
    } else {
      this.onDirty();
    }
  }

  /** Immediate write (show-state transitions and on-air commands). */
  flushNow(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (!this.dirty) return;
    this.dirty = false;
    const body: JournalBody = {
      stateVersion: this.state.stateVersion,
      manifestIdentity: this.state.manifestIdentity(),
      recoveredAt: Date.now(),
      journal: journalOf(this.state),
    };
    const file: JournalFile = { schema: 2, checksum: sha256(JSON.stringify(body)), body };
    const dir = dirname(this.path);
    mkdirSync(dir, { recursive: true });
    const tmp = this.path + ".tmp";
    const fd = openSync(tmp, "w");
    try {
      writeSync(fd, JSON.stringify(file));
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
    if (existsSync(this.path)) renameSync(this.path, prevPath(this.path));
    renameSync(tmp, this.path);
    fsyncDir(dir);
  }

  /**
   * Rule 10: restore the version + package identity, mark recovered,
   * require an explicit show.load. NEVER restore showState RUNNING. The
   * newest journal whose checksum holds is read: the file, then the
   * previous one.
   */
  restore(): boolean {
    this.restoredFrom = null;
    for (const [which, p] of [
      ["current", this.path],
      ["previous", prevPath(this.path)],
    ] as const) {
      const data = readJournal(p);
      if (data === null) continue;
      this.state.stateVersion = data.stateVersion;
      this.state.showState = "UNLOADED";
      this.state.pkg = null;
      this.restoredFrom = which;
      return data.manifestIdentity != null;
    }
    return false;
  }
}

/** Make a rename durable: fsync the directory that holds it. */
function fsyncDir(dir: string): void {
  let fd: number | null = null;
  try {
    fd = openSync(dir, "r");
    fsyncSync(fd);
  } catch {
    // A filesystem that cannot fsync a directory still has the file's own
    // fsync; the rename is then as durable as that filesystem makes it.
  } finally {
    if (fd !== null) closeSync(fd);
  }
}
