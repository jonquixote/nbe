//! Input Intent layer (Prompt 08, WS-only): data, not code.
//!
//! An Input Intent maps one physical actuation (Companion button, MIDI note,
//! keyboard chord) to one semantic §16 command (`action` + `payload`).
//! Per-device profiles are user-editable documents; the §16 WS bus is the
//! device-independent core (portability boundary 1) — this layer TRANSLATES,
//! the bus executes. Any device path that acts without producing a §16
//! command is a defect. Schemas in `schemas/` are untouched; validation
//! reuses `CommandPayloadSchemas` from `protocol.ts` (no fork).

import { randomUUID } from "node:crypto";
import { z } from "zod";

import {
  CommandPayloadSchemas,
  CpError,
  PROTOCOL_VERSION,
  resolveCommand,
  type CommandName,
  type Envelope,
} from "./protocol.js";
import type { ControlBinding } from "./generated/manifest-schema.js";

// ---------------------------------------------------------------------------
// TriggerKind — mirrors Rust `TriggerKind`
// (crates/nbe-core/src/manifest.rs) and the manifest schema enum
// (`schemas/manifest.v0.4.json` #/$defs/ControlBinding/trigger/kind).
// Rust variants serialize (camelCase) to the wire values below.
// ---------------------------------------------------------------------------

export enum TriggerKind {
  CompanionKey = "companionKey",
  Hotkey = "hotkey",
  Midi = "midi",
  WebButton = "webButton",
  Osc = "osc",
}

export const TriggerKindSchema = z.nativeEnum(TriggerKind);

/** Wire values, literal-by-literal vs the schema enum. */
export const TRIGGER_KIND_WIRE_VALUES = [
  "companionKey",
  "hotkey",
  "midi",
  "webButton",
  "osc",
] as const;

/** Rust variant names, literal-by-literal vs `manifest.rs` TriggerKind. */
export const TRIGGER_KIND_RUST_VARIANTS = [
  "CompanionKey",
  "Hotkey",
  "Midi",
  "WebButton",
  "Osc",
] as const;

// ---------------------------------------------------------------------------
// Intent model
// ---------------------------------------------------------------------------

export const IntentTriggerSchema = z
  .object({
    kind: TriggerKindSchema,
    page: z.number().int().nonnegative().optional(),
    bank: z.number().int().nonnegative().optional(),
    key: z.string().min(1).optional(),
  })
  .strict();
export type IntentTrigger = z.infer<typeof IntentTriggerSchema>;

export const InputIntentSchema = z
  .object({
    intentId: z.string().min(1),
    profileId: z.string().min(1),
    action: z.string().min(1),
    payload: z.record(z.unknown()).default({}),
    trigger: IntentTriggerSchema.optional(),
  })
  .strict();
export type InputIntent = z.infer<typeof InputIntentSchema>;

/** Per-device profile document: user-editable data, never code. */
export const IntentProfileSchema = z
  .object({
    profileId: z.string().min(1),
    device: z.string().min(1),
    entries: z.array(InputIntentSchema).default([]),
  })
  .strict();
export type IntentProfile = z.infer<typeof IntentProfileSchema>;

// ---------------------------------------------------------------------------
// Resolve: binding-like entry -> validated §16 {command, payload}
// ---------------------------------------------------------------------------

export interface ResolvedIntent {
  command: CommandName;
  payload: Record<string, unknown>;
}

/** Anything with an `action` + optional `payload` (InputIntent, ControlBinding). */
export interface BindingLike {
  action: string;
  payload?: Record<string, unknown> | undefined;
}

/**
 * Resolve one intent to the §16 command it names, validating the payload
 * against that command's schema. Unknown action -> E_UNSUPPORTED;
 * schema-invalid payload -> E_BAD_PAYLOAD. Reuses `CommandPayloadSchemas`;
 * nothing here forks the §16 surface.
 */
export function resolveIntent(entry: BindingLike): ResolvedIntent {
  const resolved = resolveCommand(entry.action);
  if (!resolved) {
    throw new CpError("E_UNSUPPORTED", `unknown action: ${entry.action}`);
  }
  const parsed = CommandPayloadSchemas[resolved.command].safeParse(entry.payload ?? {});
  if (!parsed.success) {
    throw new CpError("E_BAD_PAYLOAD", `invalid payload for ${resolved.command}: ${parsed.error.message}`);
  }
  return { command: resolved.command, payload: parsed.data as Record<string, unknown> };
}

/** Mirror a manifest `ControlBinding` into this profile's intent space. */
export function bindingToIntent(binding: ControlBinding, profileId: string): InputIntent {
  const rawKind = binding.trigger?.kind as string | undefined;
  if (rawKind !== undefined && !(TRIGGER_KIND_WIRE_VALUES as readonly string[]).includes(rawKind)) {
    throw new CpError("E_BAD_PAYLOAD", `unknown trigger kind: ${rawKind}`);
  }  return InputIntentSchema.parse({
    intentId: binding.id,
    profileId,
    action: binding.action,
    payload: (binding.payload ?? {}) as Record<string, unknown>,
    ...(binding.trigger
      ? {
          trigger: {
            kind: binding.trigger.kind as TriggerKind,
            ...(binding.trigger.page !== undefined ? { page: binding.trigger.page } : {}),
            ...(binding.trigger.bank !== undefined ? { bank: binding.trigger.bank } : {}),
            ...(binding.trigger.key !== undefined ? { key: binding.trigger.key } : {}),
          },
        }
      : {}),
  });
}

/** Build a profile document from manifest-style bindings. */
export function profileFromBindings(
  profileId: string,
  device: string,
  bindings: ControlBinding[],
): IntentProfile {
  return IntentProfileSchema.parse({
    profileId,
    device,
    entries: bindings.map((b) => bindingToIntent(b, profileId)),
  });
}

/** A resolved intent becomes the §5.4 envelope the bus executes. */
export function intentToEnvelope(resolved: ResolvedIntent, baseStateVersion?: number): Envelope {
  return {
    v: PROTOCOL_VERSION,
    id: randomUUID(),
    command: resolved.command,
    payload: resolved.payload,
    ...(baseStateVersion !== undefined ? { baseStateVersion } : {}),
  };
}
