//! Audit log (Section 10.7, addendum 02a §2.6): append-only JSONL.
//! Records EVERY control-plane action and auth event — including rejected
//! commands, role denials, and failed auth. The abuse model depends on this.
//!
//! Write policy: append(2)-syscalls per record; data is durable modulo OS
//! crash. Retention: single file, rotated by the operator for now (02a §2.6
//! leaves rotation policy open; the file is append-only).

import { appendFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import type { ErrorCode, Role } from "./protocol.js";

export interface AuditRecord {
  ts: number;
  kind: "command" | "auth";
  // auth
  outcome?: "ok" | "rejected";
  // shared
  role?: Role | null;
  tokenId?: string | null;
  remote?: string | null;
  // commands
  requestId?: string;
  command?: string;
  rawCommand?: string | null; // set when an alias/deprecated name was used
  errorCode?: ErrorCode | null;
  stateVersionBefore?: number;
  stateVersionAfter?: number;
  /** Why a handshake failed. Belongs here, never in the reply to the peer. */
  reason?: string | null;
}

export class AuditLog {
  private readonly path: string | undefined;

  constructor(path?: string | undefined) {
    this.path = path;
    if (path) mkdirSync(dirname(path), { recursive: true });
  }

  /**
   * SPEC §10.7.1: a control plane with no audit destination MUST refuse to
   * start rather than run unaudited. Call this at boot, not per record — an
   * in-memory-only audit log satisfies nothing the section exists for.
   */
  assertConfigured(): void {
    if (!this.path) {
      throw new Error(
        "audit log destination is not configured (set NBE_AUDIT_LOG); refusing to start unaudited (SPEC §10.7.1)",
      );
    }
  }

  record(rec: Omit<AuditRecord, "ts"> & { ts?: number }): AuditRecord {
    const full: AuditRecord = { ts: Date.now(), ...rec };
    if (this.path) appendFileSync(this.path, JSON.stringify(full) + "\n");
    return full;
  }
}
