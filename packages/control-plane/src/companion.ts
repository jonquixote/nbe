//! Companion WS adapter, Stream Deck XL (Prompt 08, WS-only).
//!
//! The adapter translates button presses into §5.4 envelopes and runs them
//! through the existing `dispatch()` with the caller's role (token auth
//! happens at the WS upgrade in `server.ts` — there is no second auth path).
//! It NEVER mutates state directly. Identity travels as
//! `companion/<profile>:<key>` on the audit path only (see `server.ts`).
//!
//! Deck layout is generated from the manifest's `control.bindings`
//! (trigger page/bank/key -> pages/banks/buttons; action + payload -> §16
//! command). Generation is deterministic: bindings sort by id (code-unit
//! order), pages/banks/buttons sort numerically/lexically, so the JSON is
//! byte-identical across runs. The default deck always ships alongside the
//! manifest buttons on a page the manifest does not use, so defaults never
//! overwrite manifest keys and empty bindings still leave a drivable deck.

import { z } from "zod";

import { dispatch, type CommandRegistry, type DispatchDeps, type DispatchResult } from "./dispatch.js";
import {
  intentToEnvelope,
  resolveIntent,
  TriggerKind,
  type IntentProfile,
  type InputIntent,
  type ResolvedIntent,
} from "./intent.js";
import type { ControlBinding } from "./generated/manifest-schema.js";
import type { Envelope, Role } from "./protocol.js";
import { CpError } from "./protocol.js";

// ---------------------------------------------------------------------------
// Button -> intent
// ---------------------------------------------------------------------------

export const CompanionButtonSchema = z
  .object({
    page: z.number().int().nonnegative(),
    bank: z.number().int().nonnegative(),
    key: z.string().min(1),
  })
  .strict();
export type CompanionButton = z.infer<typeof CompanionButtonSchema>;

/** Distinct adapter identity for the audit path: `companion/<profile>:<intent>` —
 * profile id + binding/intent id (spec §10.7.1 `intentSource` example), never
 * the physical key: two bindings may share a key across pages/banks. */
export function companionIntentSource(profileId: string, intentId: string): string {
  return `companion/${profileId}:${intentId}`;
}

function specificity(e: InputIntent): number {
  return (e.trigger?.page !== undefined ? 1 : 0) + (e.trigger?.bank !== undefined ? 1 : 0) + (e.trigger?.key !== undefined ? 1 : 0);
}

/**
 * Find the profile entry a button actuates: CompanionKey entries whose
 * defined coordinates all match. Omitted page/bank/key are wildcards on that
 * axis; most-specific match wins, profile order breaks ties (deterministic,
 * and pinned by test — overlapping triggers that tie identically are a
 * preflight `duplicateBindingTrigger` error, so ties here are always won on
 * specificity, never silently). Triggerless entries never match: they are
 * API-only intents (preflight allows them, the deck skips them).
 */
export function findCompanionEntry(profile: IntentProfile, button: CompanionButton): InputIntent | null {
  let best: InputIntent | null = null;
  let bestScore = -1;
  for (const e of profile.entries) {
    const t = e.trigger;
    if (!t || t.kind !== TriggerKind.CompanionKey) continue;
    if (t.page !== undefined && t.page !== button.page) continue;
    if (t.bank !== undefined && t.bank !== button.bank) continue;
    if (t.key !== undefined && t.key !== button.key) continue;
    const score = specificity(e);
    if (score > bestScore) {
      best = e;
      bestScore = score;
    }
  }
  return best;
}

export interface FireCompanionOpts {
  profile: IntentProfile;
  button: CompanionButton;
  role?: Role;
  connectionId?: string;
  baseStateVersion?: number;
}

export interface FiredIntent {
  intent: InputIntent;
  intentSource: string;
  envelope: Envelope;
  resolved: ResolvedIntent;
  result: DispatchResult;
}

/**
 * Press a Companion button: resolve the intent, build the §5.4 envelope,
 * run it through `dispatch()`. No direct state mutation — the single bump
 * happens inside dispatch, exactly like a UI command.
 *
 * Audit note: this helper executes the pipeline but records no audit row —
 * audit is the transport's job. Over WS, `server.ts` records the row
 * (including `intentSource`); embedded callers own their audit trail.
 */
export async function fireCompanionButton(
  deps: DispatchDeps,
  registry: CommandRegistry,
  opts: FireCompanionOpts,
): Promise<FiredIntent> {
  const intent = findCompanionEntry(opts.profile, opts.button);
  if (!intent) {
    throw new CpError(
      "E_NOT_FOUND",
      `no intent for companion button ${opts.button.page}:${opts.button.bank}:${opts.button.key}`,
    );
  }
  const resolved = resolveIntent(intent);
  const envelope = intentToEnvelope(resolved, opts.baseStateVersion);
  const result = await dispatch(deps, registry, {
    connectionId: opts.connectionId ?? `companion/${opts.profile.profileId}`,
    role: opts.role ?? "operator",
    envelope,
  });
  return {
    intent,
    intentSource: companionIntentSource(opts.profile.profileId, intent.intentId),
    envelope,
    resolved,
    result,
  };
}

// ---------------------------------------------------------------------------
// Deck generation (deterministic)
// ---------------------------------------------------------------------------

export interface DeckButton {
  page: number;
  bank: number;
  key: string;
  /** Manifest binding id, or null for a built-in default button. */
  bindingId: string | null;
  label: string;
  command: string;
  payload: Record<string, unknown>;
}

export interface DeckBank {
  bank: number;
  buttons: DeckButton[];
}

export interface DeckPage {
  page: number;
  banks: DeckBank[];
}

export interface Deck {
  pages: DeckPage[];
}

function cmpStr(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Built-in defaults: always present, always schema-valid payloads.
 * Id-requiring entries carry placeholder ids (`A1`, `sfx-1`) and are labelled
 * as such — the operator rebinds them to the loaded show. `view.take`,
 * `breaking.*`, `record.*`, `stream.*` and `view.fallback` need no ids. */
function defaultButtons(page: number): DeckButton[] {
  const b = (
    key: string,
    label: string,
    command: string,
    payload: Record<string, unknown>,
  ): DeckButton => ({ page, bank: 1, key, bindingId: null, label, command, payload });
  return [
    b("take", "TAKE", "view.take", {}),
    b("cut", "CUT (placeholder A1)", "view.cut", { itemRef: "A1" }),
    b("arm-next", "Arm next (placeholder A1)", "item.arm", { itemId: "A1" }),
    b("next", "Next item (placeholder A1)", "preview.set", { itemRef: "A1" }),
    b("breaking-show", "Breaking show", "breaking.show", { headline: "Breaking" }),
    b("breaking-hide", "Breaking hide", "breaking.hide", {}),
    b("sfx-1", "SFX 1 (placeholder)", "soundboard.play", { assetId: "sfx-1" }),
    b("sfx-2", "SFX 2 (placeholder)", "soundboard.play", { assetId: "sfx-2" }),
    b("rec-start", "Record start", "record.start", {}),
    b("rec-stop", "Record stop", "record.stop", {}),
    b("stream-start", "Stream start", "stream.start", {}),
    b("stream-stop", "Stream stop", "stream.stop", {}),
    b("fallback", "Fallback", "view.fallback", {}),
  ];
}

/**
 * Manifest bindings -> deck. Sorted-by-id stable: the same bindings always
 * produce byte-identical JSON. Bindings without a trigger are skipped — they
 * are API-only intents (preflight allows them) that no button can fire;
 * emitting a dead button would lie to the operator. Bindings with a trigger
 * auto-place missing axes on page 1 / bank 1 / first free numeric key.
 * Defaults live on page 0 unless the manifest already uses it, then on
 * maxPage + 1 — never overwriting keys, though on a large manifest that page
 * may sit far from page 0 (documented tradeoff, not a bug: manifest wins).
 */
export function generateDeck(bindings: ControlBinding[]): Deck {
  const sorted = [...bindings].sort((a, b) => cmpStr(a.id, b.id));
  const used = new Set<string>();
  const buttons: DeckButton[] = [];
  const pagesUsed = new Set<number>();
  let autoKey = 0;

  const claim = (page: number, bank: number, key: string): string => {
    let k = key;
    let n = 2;
    while (used.has(`${page}:${bank}:${k}`)) {
      k = `${key}-${n}`;
      n += 1;
    }
    used.add(`${page}:${bank}:${k}`);
    pagesUsed.add(page);
    return k;
  };

  for (const binding of sorted) {
    const t = binding.trigger;
    if (!t) continue; // API-only intent: no button can fire it, emit none.
    const page = t.page ?? 1;
    const bank = t.bank ?? 1;
    let key = t.key;
    if (key === undefined) {
      do {
        autoKey += 1;
        key = String(autoKey);
      } while (used.has(`${page}:${bank}:${key}`));
    }
    const finalKey = claim(page, bank, key);
    buttons.push({
      page,
      bank,
      key: finalKey,
      bindingId: binding.id,
      label: binding.description ?? binding.id,
      command: binding.action,
      payload: (binding.payload ?? {}) as Record<string, unknown>,
    });
  }

  const defaultPage = pagesUsed.has(0) ? Math.max(...pagesUsed) + 1 : 0;
  for (const d of defaultButtons(defaultPage)) {
    d.key = claim(d.page, d.bank, d.key);
    buttons.push(d);
  }

  const byPage = new Map<number, Map<number, DeckButton[]>>();
  for (const btn of buttons) {
    let banks = byPage.get(btn.page);
    if (!banks) {
      banks = new Map();
      byPage.set(btn.page, banks);
    }
    let list = banks.get(btn.bank);
    if (!list) {
      list = [];
      banks.set(btn.bank, list);
    }
    list.push(btn);
  }
  const pages: DeckPage[] = [...byPage.entries()]
    .sort(([a], [b]) => a - b)
    .map(([page, banks]) => ({
      page,
      banks: [...banks.entries()]
        .sort(([a], [b]) => a - b)
        .map(([bank, list]) => ({
          bank,
          buttons: list.sort((a, b) => cmpStr(a.key, b.key)),
        })),
    }));
  return { pages };
}

/** The default deck alone (empty bindings still drivable). */
export function defaultDeck(): Deck {
  return generateDeck([]);
}

/** Stable serialization: construction order is fully sorted, so this is byte-stable. */
export function deckToJson(deck: Deck): string {
  return JSON.stringify(deck);
}
