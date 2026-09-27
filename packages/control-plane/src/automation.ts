//! The automation engine runtime (SPEC §13, AC-25; Prompt 11).
//!
//! `trigger + conditions → command` (§13.1). The rule engine runs in the
//! control plane (design gate G1, `docs/automation-design.md`): a rule's
//! action is a command-bus command facing the same preconditions, the same
//! audit, and the same acknowledgement rules as an operator's. So the action
//! is executed through the server's ONE command path — `dispatch()` plus the
//! audit row plus the `stateChange` broadcast — never a second one.
//!
//! This module owns: reading each rule's trigger params and conditions (the
//! schema types both as free objects, and §13.2 names triggers without their
//! parameters — the reading is recorded in the design note), matching trigger
//! events to rules, the once-per-frame limiter (§13.3 #3), hold (§13.5) and
//! the pending-action queue it cancels (B5), and the causal chain a runtime
//! self-trigger check needs (§13.4). Trigger SOURCES — which event is which
//! trigger — are wired where the events happen (`server.ts`).
//!
//! A rule the evaluator cannot evaluate is refused at package load with a
//! named reason. It is never accepted and left silently inert (Prompt 11 §9).

import { CommandPayloadSchemas, resolveCommand, type ErrorCode } from "./protocol.js";
import type { ControlPlaneState, ItemState, PackageInfo } from "./state.js";
import type { AutomationRule } from "./generated/manifest-schema.js";

// ---------------------------------------------------------------------------
// The params and conditions contract — the tree's reading of §13.2.
// ---------------------------------------------------------------------------

/** The control-plane state a `stateChange` trigger or a condition can name. */
export const STATE_FIELDS = [
  "showState",
  "viewItem",
  "previewItem",
  "streamState",
  "recordState",
  "automationHold",
  "fallbackActive",
  "itemState",
] as const;
export type StateField = (typeof STATE_FIELDS)[number];

/** The metered buses (`nbe_core::automation::AUDIO_BUSES`, itself pinned to
 *  the engine's `audio::BusId`), plus `guest:<id>`. */
export const AUDIO_BUSES = ["mic", "clip", "music", "sfx", "guest", "master", "guestReturn", "ifb"] as const;

export type StreamTransportToken = "live" | "reconnecting" | "closed";

export type ParsedTrigger =
  | { kind: "mediaEnd" | "mediaStart"; itemRef?: string }
  | { kind: "timer"; atMs: number }
  | { kind: "timeOfDay"; at: { h: number; m: number; s: number } }
  | { kind: "audioLevel"; bus: string; thresholdDbfs: number; direction: "rising" | "falling" }
  | { kind: "hotkey"; bindingId: string }
  | { kind: "streamHealth"; state: StreamTransportToken }
  | { kind: "stateChange"; field: StateField; itemRef?: string; from?: unknown; to?: unknown };

export interface ParsedCondition {
  field: StateField;
  itemRef?: string;
  equals: unknown;
}

export interface ParsedRule {
  id: string;
  trigger: ParsedTrigger;
  conditions: ParsedCondition[];
  action: { command: string; payload: Record<string, unknown> };
  enabled: boolean;
}

/** What a rule may reference, from the loaded package. */
export interface RuleContext {
  items: ReadonlySet<string>;
  /** control.bindings: binding id → its trigger kind (`hotkey`, `companionKey`, …). */
  bindings: ReadonlyMap<string, string | undefined>;
}

type Params = Record<string, unknown>;

function onlyKeys(params: Params, allowed: string[]): string | null {
  for (const k of Object.keys(params)) {
    if (!allowed.includes(k)) return `unknown param \`${k}\` (allowed: ${allowed.join(", ") || "none"})`;
  }
  return null;
}

function parseTrigger(rule: AutomationRule, ctx: RuleContext): ParsedTrigger | string {
  const params: Params = (rule.trigger.params ?? {}) as Params;
  const kind = rule.trigger.kind;
  switch (kind) {
    case "mediaEnd":
    case "mediaStart": {
      const bad = onlyKeys(params, ["itemRef"]);
      if (bad) return bad;
      if (params.itemRef === undefined) return { kind };
      if (typeof params.itemRef !== "string" || !ctx.items.has(params.itemRef)) {
        return `itemRef ${JSON.stringify(params.itemRef)} is not an item in the rundown`;
      }
      return { kind, itemRef: params.itemRef };
    }
    case "timer": {
      const bad = onlyKeys(params, ["atMs"]);
      if (bad) return bad;
      const atMs = params.atMs;
      if (typeof atMs !== "number" || !Number.isFinite(atMs) || atMs <= 0) {
        return "needs a positive number `atMs` (show-clock milliseconds after show.start)";
      }
      return { kind, atMs };
    }
    case "timeOfDay": {
      const bad = onlyKeys(params, ["at"]);
      if (bad) return bad;
      const m = typeof params.at === "string" ? /^(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(params.at) : null;
      const [h, mi, s] = m ? [Number(m[1]), Number(m[2]), Number(m[3] ?? "0")] : [NaN, NaN, NaN];
      if (!m || h > 23 || mi > 59 || s > 59) return 'needs `at` as "HH:mm" or "HH:mm:ss" (local wall clock)';
      return { kind, at: { h, m: mi, s } };
    }
    case "audioLevel": {
      const bad = onlyKeys(params, ["bus", "thresholdDbfs", "direction"]);
      if (bad) return bad;
      const bus = params.bus;
      if (typeof bus !== "string" || !(AUDIO_BUSES.includes(bus as never) || /^guest:.+/.test(bus))) {
        return `bus ${JSON.stringify(bus)} is not a metered bus (${AUDIO_BUSES.join(", ")}, or guest:<id>)`;
      }
      const t = params.thresholdDbfs;
      if (typeof t !== "number" || !(t > -120 && t <= 0)) return "needs `thresholdDbfs` in (-120, 0]";
      const direction = params.direction ?? "rising";
      if (direction !== "rising" && direction !== "falling") return 'direction must be "rising" or "falling"';
      return { kind, bus, thresholdDbfs: t, direction };
    }
    case "hotkey": {
      const bad = onlyKeys(params, ["bindingId"]);
      if (bad) return bad;
      const id = params.bindingId;
      if (typeof id !== "string" || !ctx.bindings.has(id)) {
        return `bindingId ${JSON.stringify(id)} is not a control binding in this package`;
      }
      if (ctx.bindings.get(id) !== "hotkey") {
        return `binding ${JSON.stringify(id)} is a ${ctx.bindings.get(id) ?? "trigger-less"} binding, not a hotkey`;
      }
      return { kind, bindingId: id };
    }
    case "rssKeyword":
      // §13.4.1's ticker.refreshRss row: the handler fetches nothing, so no
      // RSS item ever arrives to match. Accepting the rule would leave it
      // silently inert, which §9 forbids.
      return "has no source in this build: no RSS feed is ever fetched (ticker.refreshRss mutates nothing; SPEC §13.4.1)";
    case "streamHealth": {
      const bad = onlyKeys(params, ["state"]);
      if (bad) return bad;
      const st = params.state;
      if (st !== "live" && st !== "reconnecting" && st !== "closed") {
        return 'needs `state`: "live", "reconnecting" or "closed" (a streamTransportState token; "none" is a stub, not a state)';
      }
      return { kind, state: st };
    }
    case "stateChange": {
      const bad = onlyKeys(params, ["field", "itemRef", "from", "to"]);
      if (bad) return bad;
      const field = params.field;
      if (typeof field !== "string" || !STATE_FIELDS.includes(field as StateField)) {
        return `field ${JSON.stringify(field)} is not one of ${STATE_FIELDS.join(", ")}`;
      }
      const t: ParsedTrigger = { kind, field: field as StateField };
      if (field === "itemState") {
        if (typeof params.itemRef !== "string" || !ctx.items.has(params.itemRef)) {
          return "field itemState needs an `itemRef` naming a rundown item";
        }
        t.itemRef = params.itemRef;
      } else if (params.itemRef !== undefined) {
        return "itemRef applies only to field itemState";
      }
      if ("from" in params) t.from = params.from;
      if ("to" in params) t.to = params.to;
      return t;
    }
  }
}

function parseCondition(c: Record<string, unknown>, ctx: RuleContext): ParsedCondition | string {
  const bad = onlyKeys(c, ["field", "itemRef", "equals"]);
  if (bad) return `condition has an ${bad}`;
  const field = c.field;
  if (typeof field !== "string" || !STATE_FIELDS.includes(field as StateField)) {
    return `condition field ${JSON.stringify(field)} is not one of ${STATE_FIELDS.join(", ")}`;
  }
  if (!("equals" in c)) return "condition needs `equals`";
  if (field === "itemState") {
    if (typeof c.itemRef !== "string" || !ctx.items.has(c.itemRef)) {
      return "condition on itemState needs an `itemRef` naming a rundown item";
    }
    return { field, itemRef: c.itemRef, equals: c.equals };
  }
  if (c.itemRef !== undefined) return "condition itemRef applies only to field itemState";
  return { field: field as StateField, equals: c.equals };
}

/**
 * Read one manifest rule. Returns the parsed rule, or the reason it is
 * refused — naming the rule, so a load refusal points at the line to fix.
 */
export function parseRule(rule: AutomationRule, ctx: RuleContext): ParsedRule | string {
  const why = (reason: string) => `automation rule \`${rule.id}\`: ${rule.trigger.kind} trigger ${reason}`;
  const trigger = parseTrigger(rule, ctx);
  if (typeof trigger === "string") return why(trigger);
  const conditions: ParsedCondition[] = [];
  for (const c of rule.conditions ?? []) {
    const parsed = parseCondition(c, ctx);
    if (typeof parsed === "string") return `automation rule \`${rule.id}\`: ${parsed}`;
    conditions.push(parsed);
  }
  // The action is a command: known, and its payload valid, or every firing
  // would be refused for a reason knowable now.
  const resolved = resolveCommand(rule.action.command);
  if (!resolved) return `automation rule \`${rule.id}\`: action command \`${rule.action.command}\` is not a command`;
  const payload = (rule.action.payload ?? {}) as Record<string, unknown>;
  const parsed = CommandPayloadSchemas[resolved.command].safeParse(payload);
  if (!parsed.success) {
    return `automation rule \`${rule.id}\`: action payload is not a valid ${resolved.command} payload: ${parsed.error.message}`;
  }
  return {
    id: rule.id,
    trigger,
    conditions,
    action: { command: resolved.command, payload },
    enabled: rule.enabled !== false,
  };
}

/** Read every rule; the refusals, if any, are the load's refusal. */
export function parseRules(rules: AutomationRule[], ctx: RuleContext): { rules: ParsedRule[]; errors: string[] } {
  const out: ParsedRule[] = [];
  const errors: string[] = [];
  const seen = new Set<string>();
  for (const r of rules) {
    if (seen.has(r.id)) {
      errors.push(`automation rule \`${r.id}\`: duplicate rule id`);
      continue;
    }
    seen.add(r.id);
    const p = parseRule(r, ctx);
    if (typeof p === "string") errors.push(p);
    else out.push(p);
  }
  return { rules: out, errors };
}

// ---------------------------------------------------------------------------
// State snapshots — what `stateChange` triggers and conditions read.
// ---------------------------------------------------------------------------

export interface Snapshot {
  showState: string;
  viewItem: string | null;
  previewItem: string | null;
  streamState: string;
  recordState: string;
  automationHold: boolean;
  fallbackActive: boolean;
  itemStates: ReadonlyMap<string, ItemState>;
}

export function snapshot(state: ControlPlaneState): Snapshot {
  return {
    showState: state.showState,
    viewItem: state.viewItem,
    previewItem: state.previewItem,
    streamState: state.streamState,
    recordState: state.recordState,
    automationHold: state.automationHold,
    fallbackActive: state.fallbackActive,
    itemStates: new Map(state.itemStates),
  };
}

function readField(s: Snapshot, field: StateField, itemRef?: string): unknown {
  if (field === "itemState") return itemRef === undefined ? undefined : (s.itemStates.get(itemRef) ?? "READY");
  return s[field];
}

/** The `stateChange` events between two snapshots, in a fixed field order. */
export function stateChanges(before: Snapshot, after: Snapshot): TriggerEvent[] {
  const out: TriggerEvent[] = [];
  for (const field of STATE_FIELDS) {
    if (field === "itemState") continue;
    const from = before[field];
    const to = after[field];
    if (from !== to) out.push({ kind: "stateChange", field, from, to });
  }
  const refs = new Set([...before.itemStates.keys(), ...after.itemStates.keys()]);
  for (const itemRef of refs) {
    const from = before.itemStates.get(itemRef) ?? "READY";
    const to = after.itemStates.get(itemRef) ?? "READY";
    if (from !== to) out.push({ kind: "stateChange", field: "itemState", itemRef, from, to });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Trigger events and the evaluator.
// ---------------------------------------------------------------------------

export type TriggerEvent =
  | { kind: "mediaEnd"; itemRef: string }
  | { kind: "mediaStart"; itemRef: string }
  | { kind: "timer"; ruleId: string }
  | { kind: "timeOfDay"; ruleId: string }
  | {
      kind: "audioLevel";
      bus: string;
      thresholdDbfs: number;
      direction: "rising" | "falling";
      levelDbfs: number;
      masterFrame: number;
    }
  | { kind: "hotkey"; bindingId: string }
  | { kind: "streamHealth"; state: StreamTransportToken; from: string }
  | { kind: "stateChange"; field: StateField; itemRef?: string; from: unknown; to: unknown };

/**
 * Which rules' actions led to an event. Empty for an event the world caused
 * (an operator's command, an engine frame, a clock). Carried through the one
 * command path so a trigger raised BY a rule's action knows it.
 */
export interface Cause {
  chain: readonly string[];
}
export const NO_CAUSE: Cause = { chain: [] };

export function matches(t: ParsedTrigger, e: TriggerEvent, ruleId: string): boolean {
  if (t.kind !== e.kind) return false;
  switch (t.kind) {
    case "mediaEnd":
    case "mediaStart":
      return t.itemRef === undefined || t.itemRef === (e as { itemRef: string }).itemRef;
    case "timer":
    case "timeOfDay":
      return (e as { ruleId: string }).ruleId === ruleId;
    case "audioLevel": {
      const x = e as Extract<TriggerEvent, { kind: "audioLevel" }>;
      return x.bus === t.bus && x.thresholdDbfs === t.thresholdDbfs && x.direction === t.direction;
    }
    case "hotkey":
      return (e as { bindingId: string }).bindingId === t.bindingId;
    case "streamHealth":
      return (e as { state: string }).state === t.state;
    case "stateChange": {
      const x = e as Extract<TriggerEvent, { kind: "stateChange" }>;
      if (x.field !== t.field) return false;
      if (t.field === "itemState" && x.itemRef !== t.itemRef) return false;
      if (t.from !== undefined && x.from !== t.from) return false;
      if (t.to !== undefined && x.to !== t.to) return false;
      return true;
    }
  }
}

/** What running a rule's action came to, from the one command path. */
export type ActionOutcome =
  | { ok: true; stateVersion: number }
  | { ok: false; code: ErrorCode; message: string };

export interface ActionRequest {
  command: string;
  payload: Record<string, unknown>;
  /** `automation:<ruleId>` or `autoFollow:<itemRef>` — the audit's actor. */
  actor: string;
  /** The audit event the command path records this run under. */
  auditEvent: string;
  /** Extra audit detail (rule, trigger, timing). */
  detail: Record<string, unknown>;
  /** The chain this action extends: triggers it raises carry it. */
  cause: Cause;
}

export interface AutomationAudit {
  event: string;
  outcome: "ok" | "rejected";
  command?: string;
  actor: string;
  detail: Record<string, unknown>;
}

export interface EvaluatorDeps {
  state: ControlPlaneState;
  /** The server's one command path: dispatch + audit (kind "automation") + stateChange. */
  execute: (req: ActionRequest) => Promise<ActionOutcome>;
  /** Audit a decision that dispatched nothing (suppressed, cancelled, limited). */
  audit: (rec: AutomationAudit) => void;
  /** Monotonic milliseconds (latency and the frame limiter). */
  clock?: () => number;
  /** Wall-clock `Date` (timeOfDay). */
  wallNow?: () => Date;
}

interface Pending {
  rule: ParsedRule;
  event: TriggerEvent;
  cause: Cause;
  observedAt: number;
  frame: number;
}

/**
 * The evaluator. One per control plane; rules arrive with the package.
 *
 * Flow for one trigger event: every enabled rule it matches, in manifest
 * order → held? suppressed (audited) → conditions false? not an attempt →
 * already fired this frame? rate-limited (audited) → enqueued as PENDING.
 * The queue drains one action at a time through the command path; a hold
 * accepted in between cancels everything still pending (B5), audited.
 */
export class AutomationEvaluator {
  private rules: ParsedRule[] = [];
  private frameMs = 1000 / 30;
  private readonly lastFrame = new Map<string, number>();
  private readonly pending: Pending[] = [];
  private draining: Promise<void> = Promise.resolve();
  private drainScheduled = false;
  private readonly timers = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly clock: () => number;
  private readonly wallNow: () => Date;

  constructor(private readonly deps: EvaluatorDeps) {
    this.clock = deps.clock ?? (() => performance.now());
    this.wallNow = deps.wallNow ?? (() => new Date());
  }

  /** The rules in force, in manifest order. */
  get loaded(): readonly ParsedRule[] {
    return this.rules;
  }

  /** A package loaded: its (already validated) rules, its house rate. */
  load(pkg: PackageInfo): void {
    this.unload();
    this.rules = pkg.automation.slice();
    this.frameMs = 1000 / (pkg.houseRate > 0 ? pkg.houseRate : 30);
    for (const r of this.rules) {
      if (r.trigger.kind === "timeOfDay") this.scheduleTimeOfDay(r);
    }
  }

  unload(): void {
    this.rules = [];
    this.lastFrame.clear();
    this.cancelPending("unload");
    for (const t of this.timers.values()) clearTimeout(t);
    this.timers.clear();
  }

  /** show.start accepted: the show clock starts, and with it every `timer` rule. */
  showStarted(): void {
    const started = this.clock();
    for (const r of this.rules) {
      if (r.trigger.kind !== "timer") continue;
      const atMs = r.trigger.atMs;
      this.setTimer(r.id, atMs, () => this.fire({ kind: "timer", ruleId: r.id }, NO_CAUSE, started + atMs));
    }
  }

  /** show.stop accepted: the show clock stops; pending `timer` rules are dropped. */
  showStopped(): void {
    for (const r of this.rules) {
      if (r.trigger.kind !== "timer") continue;
      const t = this.timers.get(r.id);
      if (t) clearTimeout(t);
      this.timers.delete(r.id);
    }
  }

  /** `automation.hold { hold: true }` accepted: cancel every pending action, now. */
  holdEngaged(): void {
    this.cancelPending("hold");
  }

  close(): void {
    this.unload();
  }

  /**
   * One trigger event. `observedAt` is when the control plane observed the
   * trigger condition becoming true (the G1 definition, per kind, in the
   * design note); defaults to now.
   */
  fire(event: TriggerEvent, cause: Cause = NO_CAUSE, observedAt: number = this.clock()): void {
    const state = this.deps.state;
    const frame = Math.floor(observedAt / this.frameMs);
    let enqueued = false;
    for (const rule of this.rules) {
      if (!matches(rule.trigger, event, rule.id)) continue;
      if (state.automationRules.get(rule.id) === false) continue; // disabled: not an attempt
      const actor = `automation:${rule.id}`;
      const detail = { ruleId: rule.id, trigger: event, chain: cause.chain, frame };
      if (state.automationHold) {
        this.deps.audit({ event: "automation.suppressedByHold", outcome: "rejected", command: rule.action.command, actor, detail });
        continue;
      }
      if (!rule.conditions.every((c) => this.holds(c))) continue; // conditions false: not an attempt
      if (this.lastFrame.get(rule.id) === frame) {
        this.deps.audit({ event: "automation.rateLimited", outcome: "rejected", command: rule.action.command, actor, detail });
        continue;
      }
      this.lastFrame.set(rule.id, frame);
      this.pending.push({ rule, event, cause, observedAt, frame });
      enqueued = true;
    }
    if (enqueued) this.scheduleDrain();
  }

  /** The evaluator's monotonic clock: what `observedAt` and the frame are read on. */
  now(): number {
    return this.clock();
  }

  /** Actions fired but not yet dispatched — B5's "pending". */
  get pendingCount(): number {
    return this.pending.length;
  }

  /** Resolves once every action pending now has been dispatched or cancelled. */
  async settled(): Promise<void> {
    for (;;) {
      const d = this.draining;
      await d;
      if (d === this.draining && this.pending.length === 0 && !this.drainScheduled) return;
    }
  }

  private holds(c: ParsedCondition): boolean {
    const s = snapshot(this.deps.state);
    return readField(s, c.field, c.itemRef) === c.equals;
  }

  private cancelPending(reason: "hold" | "unload"): void {
    const cancelled = this.pending.splice(0, this.pending.length);
    for (const p of cancelled) {
      this.deps.audit({
        event: reason === "hold" ? "automation.cancelledByHold" : "automation.cancelledByUnload",
        outcome: "rejected",
        command: p.rule.action.command,
        actor: `automation:${p.rule.id}`,
        detail: { ruleId: p.rule.id, trigger: p.event, chain: p.cause.chain, frame: p.frame },
      });
    }
  }

  private scheduleDrain(): void {
    if (this.drainScheduled) return;
    this.drainScheduled = true;
    // Chained, never concurrent: one action at a time, in firing order.
    this.draining = this.draining.then(() => {
      this.drainScheduled = false;
      return this.drain();
    });
  }

  private async drain(): Promise<void> {
    for (let p = this.pending.shift(); p; p = this.pending.shift()) {
      const actor = `automation:${p.rule.id}`;
      const detailBase = { ruleId: p.rule.id, trigger: p.event, chain: p.cause.chain, frame: p.frame };
      // No hold re-check here: a hold is engaged only by an accepted command,
      // and the command path cancels the queue in that command's own turn
      // (`holdEngaged`), before this loop can take the next item.
      const dispatchedAt = this.clock();
      await this.deps.execute({
        command: p.rule.action.command,
        payload: p.rule.action.payload,
        actor,
        auditEvent: "automation.action",
        detail: { ...detailBase, observedAt: p.observedAt, dispatchedAt, latencyMs: dispatchedAt - p.observedAt },
        cause: { chain: [...p.cause.chain, p.rule.id] },
      });
    }
  }

  private setTimer(id: string, ms: number, fn: () => void): void {
    const old = this.timers.get(id);
    if (old) clearTimeout(old);
    const t = setTimeout(() => {
      this.timers.delete(id);
      fn();
    }, ms);
    t.unref?.();
    this.timers.set(id, t);
  }

  private scheduleTimeOfDay(rule: ParsedRule): void {
    if (rule.trigger.kind !== "timeOfDay") return;
    const { h, m, s } = rule.trigger.at;
    const now = this.wallNow();
    const next = new Date(now);
    next.setHours(h, m, s, 0);
    if (next.getTime() <= now.getTime()) next.setDate(next.getDate() + 1);
    const delay = next.getTime() - now.getTime();
    const firesAt = this.clock() + delay;
    this.setTimer(rule.id, delay, () => {
      this.fire({ kind: "timeOfDay", ruleId: rule.id }, NO_CAUSE, firesAt);
      this.scheduleTimeOfDay(rule); // daily, while the package is loaded
    });
  }
}
