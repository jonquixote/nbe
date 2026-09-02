//! Command dispatcher: the Section 5.4/16 pipeline, executed once per
//! command message.
//!
//! Order (Prompt 02 Step 5 + Addendum 02a §2):
//!   1. resolve aliases / reject unknown commands (E_UNSUPPORTED)
//!   2. validate payload against zod schema (E_BAD_PAYLOAD)
//!   3. check role permission (E_AUTH)
//!   4. check baseStateVersion (E_VERSION_CONFLICT)
//!   5. rate-limit check (ticker flood protection, §10.7)
//!   6. preconditions + mutation in the handler (no mutate-then-throw)
//!   7. exactly ONE stateVersion bump for the accepted command
//!   8. emit state-change event, append audit record
//!   9. forward render directive(s) tagged with that same stateVersion

import { z } from "zod";
import { loadPackage, type PreflightResult } from "./package.js";
import {
  CommandPayloadSchemas,
  CpError,
  resolveCommand,
  type CommandName,
  type Envelope,
  type Role,
} from "./protocol.js";
import type { RenderBridge } from "./render-bridge.js";
import type { ControlPlaneState } from "./state.js";

export interface PersistHooks {
  onDirty(): void;
  flushNow(): void;
}

export interface ShowStopWarning {
  message: string;
}

/** The clock used by show.stop's 2-second graceful window. */
export interface ClockWait {
  wait(ms: number): Promise<void>;
}

export const SHOW_STOP_FORCE_WARNING =
  "show.stop: graceful output shutdown exceeded 2 s; force-stopping outputs";

export interface DispatchDeps {
  state: ControlPlaneState;
  bridge: RenderBridge;
  persistence: PersistHooks;
  /** test seam: how long show.stop waits for graceful output shutdown */
  showStopGraceMs?: number;
  /** waits for graceful shutdown; default resolves immediately */
  waitForGrace?: (ms: number) => Promise<void>;
  rateLimiter?: RateLimiter;
  warn?: (message: string) => void;
}

export interface DispatchResult {
  data: Record<string, unknown>;
  stateVersion: number;
}

export interface RateLimiter {
  /** returns false when the (connection, family) bucket is exhausted */
  allow(connectionId: string, family: string): boolean;
}

// ---------------------------------------------------------------------------
// Role permissions (Section 5.3)
// ---------------------------------------------------------------------------

const OPERATOR_COMMANDS = new Set([
  "preview.set",
  "view.take",
  "view.cut",
  "view.fallback",
  "scene.arm",
  "scene.apply",
  "sequence.arm",
  "sequence.unarm",
  "item.arm",
  "item.unarm",
  "item.stop",
  "element.toggle",
  "element.set",
  "graphic.show",
  "graphic.hide",
  "graphic.update",
  "breaking.show",
  "breaking.hide",
  "overlay.show",
  "overlay.hide",
  "ticker.setSource",
  "ticker.override",
  "ticker.clearOverride",
  "ticker.refreshRss",
  "soundboard.play",
  "soundboard.stop",
  "soundboard.stopAll",
  "audio.bus.set",
  "audio.duck",
  "guest.mute",
  "guest.setLayout",
  "guest.placeholder",
  "guest.configureReturn",
  "guest.getTurn",
  "snapshot.save",
  "snapshot.recall",
  "marker.add",
  "clock.configure",
  "automation.enable",
  "automation.disable",
  "automation.hold",
  "record.start",
  "record.stop",
  "stream.start",
  "stream.stop",
  "plugin.reload",
]);

const PRODUCER_COMMANDS = new Set([
  "show.load",
  "show.preflight",
  "show.unload",
  "guest.connect",
  "guest.disconnect",
  "ticker.setSource",
  "ticker.override",
  "ticker.clearOverride",
  "ticker.refreshRss",
  "marker.add",
]);

const READ_ONLY = new Set(["system.status", "system.telemetry.subscribe", "system.telemetry.unsubscribe"]);
const SHOW_LIFECYCLE = new Set(["show.start", "show.stop"]);

function roleAllowed(role: Role, command: CommandName): boolean {
  if (role === "admin") return true;
  if (role === "render") return false; // render sessions receive, they do not command
  if (READ_ONLY.has(command)) return true;
  if (role === "monitor") return false;
  if (role === "operator") {
    if (OPERATOR_COMMANDS.has(command)) return true;
    if (SHOW_LIFECYCLE.has(command)) return command === "show.stop"; // operator may stop in an emergency
    return false;
  }
  if (role === "producer") return PRODUCER_COMMANDS.has(command);
  return false;
}

// ---------------------------------------------------------------------------
// The handler context and per-command handler type
// ---------------------------------------------------------------------------

export interface HandlerCtx {
  state: ControlPlaneState;
  bridge: RenderBridge;
  persistence: PersistHooks;
  /** The command's own stateVersion (assigned by the dispatcher bump). */
  stateVersion: number;
  /** Per-connection hooks (telemetry subscribe/unsubscribe side effects). */
  systemHooks?: import("./commands/system.js").SystemHooks | undefined;
}

export interface HandlerOutput {
  data?: Record<string, unknown>;
  /** directive commands forwarded to the render bridge alongside this one */
  extraDirectives?: Array<{ command: string; target?: Record<string, unknown>; payload: Record<string, unknown> }>;
  /** warning to log (exact strings asserted in tests) */
  warnings?: string[];
}

type Payload = Record<string, unknown>;
type Handler = (ctx: HandlerCtx, payload: Payload) => HandlerOutput | Promise<HandlerOutput>;

interface CommandDef {
  handler: Handler;
  /** send this command to the render bridge as a directive */
  forward: boolean;
}

// ---------------------------------------------------------------------------
// Handlers (Section 16 families, wired in commands/ modules)
// ---------------------------------------------------------------------------

import { showHandlers } from "./commands/show.js";
import { viewHandlers } from "./commands/view.js";
import { sceneHandlers } from "./commands/scene.js";
import { sequenceHandlers } from "./commands/sequence.js";
import { elementHandlers } from "./commands/element.js";
import { tickerHandlers } from "./commands/ticker.js";
import { audioHandlers } from "./commands/audio.js";
import { guestHandlers } from "./commands/guest.js";
import { outputHandlers } from "./commands/output.js";
import { stateHandlers } from "./commands/state.js";
import { systemHandlers } from "./commands/system.js";

export type CommandRegistry = Map<CommandName, CommandDef>;

export function buildRegistry(deps: DispatchDeps): CommandRegistry {
  const reg: CommandRegistry = new Map();
  for (const m of [
    showHandlers,
    viewHandlers,
    sceneHandlers,
    sequenceHandlers,
    elementHandlers,
    tickerHandlers,
    audioHandlers,
    guestHandlers,
    outputHandlers,
    stateHandlers,
    systemHandlers,
  ]) {
    m(reg, deps);
  }
  return reg;
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

export interface DispatchOptions {
  connectionId: string;
  role: Role;
  envelope: Envelope;
  systemHooks?: import("./commands/system.js").SystemHooks;
}

export async function dispatch(
  deps: DispatchDeps,
  registry: CommandRegistry,
  opts: DispatchOptions,
): Promise<DispatchResult> {
  const { state } = deps;

  // 1. alias resolution / unknown command
  const resolved = resolveCommand(opts.envelope.command);
  if (!resolved) {
    throw new CpError("E_UNSUPPORTED", `unknown command: ${opts.envelope.command}`);
  }
  const command = resolved.command;

  // 2. payload validation
  const def = registry.get(command);
  if (!def) throw new CpError("E_UNSUPPORTED", `unimplemented command: ${command}`);
  const parsed = CommandPayloadSchemas[command].safeParse(opts.envelope.payload);
  if (!parsed.success) {
    throw new CpError("E_BAD_PAYLOAD", `invalid payload for ${command}: ${parsed.error.message}`);
  }

  // 3. role permission
  if (!roleAllowed(opts.role, command)) {
    throw new CpError("E_AUTH", `role ${opts.role} may not run ${command}`);
  }

  // 4. optimistic concurrency
  if (
    opts.envelope.baseStateVersion !== undefined &&
    opts.envelope.baseStateVersion !== state.stateVersion
  ) {
    throw new CpError(
      "E_VERSION_CONFLICT",
      `baseStateVersion ${opts.envelope.baseStateVersion} != current ${state.stateVersion}`,
    );
  }

  // 5. rate limiting (Section 10.7: ticker flood protection)
  const family = command.split(".")[0] ?? command;
  if (deps.rateLimiter && !deps.rateLimiter.allow(opts.connectionId, family)) {
    throw new CpError("E_FORBIDDEN_STATE", `rate limited: ${command}`);
  }

  // 6. handler: preconditions + mutation (mutations happen inside; guards first)
  const out = await def.handler(
    {
      state,
      bridge: deps.bridge,
      persistence: deps.persistence,
      stateVersion: state.stateVersion + 1,
      systemHooks: opts.systemHooks,
    },
    parsed.data as Payload,
  );

  // 7. exactly one bump per accepted command
  state.bump();
  const sv = state.stateVersion;

  // deprecation warning rides the next telemetry tick (Assumption 17)
  if (resolved.deprecated && resolved.warning) {
    state.noteDeprecation(opts.envelope.command, command);
  }

  // 9. render directives, all tagged with the same stateVersion
  if (def.forward) {
    deps.bridge.send({
      command,
      target: {},
      payload: parsed.data as Record<string, unknown>,
      stateVersion: sv,
    });
  }
  for (const extra of out.extraDirectives ?? []) {
    deps.bridge.send({
      command: extra.command,
      target: extra.target ?? {},
      payload: extra.payload,
      stateVersion: sv,
    });
  }

  deps.persistence.onDirty();
  return { data: out.data ?? {}, stateVersion: sv };
}
