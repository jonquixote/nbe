//! Keyboard adapter (Prompt 08): the normative zero-core-change proof of
//! generality. Same intent path as Companion — resolve via `intent.ts`,
//! execute via `dispatch()` — different profile, different `intentSource`.
//! Imports only `intent.ts`, `dispatch.ts`, `protocol.ts`: zero changes to
//! the core were needed to add it, and none are.

import { dispatch, type CommandRegistry, type DispatchDeps, type DispatchResult } from "./dispatch.js";
import {
  intentToEnvelope,
  resolveIntent,
  TriggerKind,
  type InputIntent,
  type IntentProfile,
  type ResolvedIntent,
} from "./intent.js";
import { CpError, type Envelope, type Role } from "./protocol.js";

/** Distinct adapter identity for the audit path: `keyboard/<profile>:<intent>` —
 * profile id + binding/intent id, mirroring the Companion adapter. */
export function keyboardIntentSource(profileId: string, intentId: string): string {
  return `keyboard/${profileId}:${intentId}`;
}

/** Normalize a chord for matching: case- and whitespace-insensitive. */
export function normalizeChord(chord: string): string {
  return chord.trim().toLowerCase().replace(/\s+/g, "");
}

/** Find the profile entry a chord actuates: Hotkey entries with a normalized key match. */
export function findKeyboardEntry(profile: IntentProfile, chord: string): InputIntent | null {
  const want = normalizeChord(chord);
  for (const e of profile.entries) {
    if (e.trigger?.kind === TriggerKind.Hotkey && e.trigger.key !== undefined && normalizeChord(e.trigger.key) === want)
      return e;
  }
  return null;
}

export interface FireKeyboardOpts {
  profile: IntentProfile;
  chord: string;
  role?: Role;
  connectionId?: string;
  baseStateVersion?: number;
}

export interface FiredKeyboardIntent {
  intent: InputIntent;
  intentSource: string;
  envelope: Envelope;
  resolved: ResolvedIntent;
  result: DispatchResult;
}

/**
 * Press a keyboard chord: resolve the intent, build the §5.4 envelope, run
 * it through `dispatch()`. No direct state mutation. Audit note: same split
 * as the Companion adapter — over WS the server records the row; embedded
 * callers own their audit trail.
 */
export async function fireKeyboardChord(
  deps: DispatchDeps,
  registry: CommandRegistry,
  opts: FireKeyboardOpts,
): Promise<FiredKeyboardIntent> {
  const intent = findKeyboardEntry(opts.profile, opts.chord);
  if (!intent) {
    throw new CpError("E_NOT_FOUND", `no intent for keyboard chord ${opts.chord}`);
  }
  const resolved = resolveIntent(intent);
  const envelope = intentToEnvelope(resolved, opts.baseStateVersion);
  const result = await dispatch(deps, registry, {
    connectionId: opts.connectionId ?? `keyboard/${opts.profile.profileId}`,
    role: opts.role ?? "operator",
    envelope,
  });
  return {
    intent,
    intentSource: keyboardIntentSource(opts.profile.profileId, intent.intentId),
    envelope,
    resolved,
    result,
  };
}
