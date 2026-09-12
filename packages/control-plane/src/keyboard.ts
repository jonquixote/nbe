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

/** Distinct adapter identity for the audit path: `keyboard/<profile>:<chord>`. */
export function keyboardIntentSource(profileId: string, chord: string): string {
  return `keyboard/${profileId}:${chord}`;
}

/** Find the profile entry a chord actuates: Hotkey entries with an exact key match. */
export function findKeyboardEntry(profile: IntentProfile, chord: string): InputIntent | null {
  for (const e of profile.entries) {
    if (e.trigger?.kind === TriggerKind.Hotkey && e.trigger.key === chord) return e;
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
 * it through `dispatch()`. No direct state mutation.
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
    intentSource: keyboardIntentSource(opts.profile.profileId, opts.chord),
    envelope,
    resolved,
    result,
  };
}
