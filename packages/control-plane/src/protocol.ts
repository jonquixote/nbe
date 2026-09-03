//! Wire protocol per SPEC v0.3.1 Sections 5.4 and 16.
//! Envelope, roles, error-code registry, and zod payload schemas.

import { z } from "zod";

export const PROTOCOL_VERSION = "0.3" as const;
export const WS_PATH = "/nbe/v0.3" as const;
export const DEFAULT_PORT = 8462;

// ---------------------------------------------------------------------------
// Roles (SPEC 5.3)
// ---------------------------------------------------------------------------

export const RoleSchema = z.enum(["monitor", "operator", "producer", "admin", "render"]);
export type Role = z.infer<typeof RoleSchema>;

// ---------------------------------------------------------------------------
// Error codes (Section 16 registry — every code, no omissions)
// ---------------------------------------------------------------------------

export const ErrorCodeSchema = z.enum([
  "E_BAD_PAYLOAD",
  "E_FORBIDDEN_STATE",
  "E_NOT_FOUND",
  "E_ASSET_MISSING",
  "E_DECODE",
  "E_ENGINE",
  "E_VERSION_CONFLICT",
  "E_UNSUPPORTED",
  "E_UNSUPPORTED_FEATURE",
  "E_AUTH",
  "E_NO_HARDWARE_ENCODER",
  "E_NETWORK",
  "E_PREFLIGHT_FAILED",
  "E_AUDIO",
  "E_DISK",
  "E_TIMEOUT",
  "E_TURN",
  "E_ICE",
  "E_RATE_LIMITED",
]);
export type ErrorCode = z.infer<typeof ErrorCodeSchema>;

/** Error thrown by command handlers; becomes the Section 5.4 error response. */
export class CpError extends Error {
  readonly code: ErrorCode;
  constructor(code: ErrorCode, message: string) {
    super(message);
    this.name = "CpError";
    this.code = code;
  }
}

// ---------------------------------------------------------------------------
// Envelope (Section 5.4)
// ---------------------------------------------------------------------------

export const EnvelopeSchema = z
  .object({
    v: z.literal(PROTOCOL_VERSION),
    id: z.string().uuid(),
    command: z.string().min(1),
    payload: z.record(z.unknown()),
    baseStateVersion: z.number().int().nonnegative().optional(),
  })
  .strict();
export type Envelope = z.infer<typeof EnvelopeSchema>;

export interface SuccessResponse {
  v: typeof PROTOCOL_VERSION;
  requestId: string;
  status: "ok";
  stateVersion: number;
  data: Record<string, unknown>;
}

export interface ErrorResponse {
  v: typeof PROTOCOL_VERSION;
  requestId: string;
  status: "error";
  stateVersion: number;
  error: { code: ErrorCode; message: string };
}

export function okResponse(requestId: string, stateVersion: number, data: Record<string, unknown> = {}): SuccessResponse {
  return { v: PROTOCOL_VERSION, requestId, status: "ok", stateVersion, data };
}

export function errorResponse(requestId: string, stateVersion: number, code: ErrorCode, message: string): ErrorResponse {
  return { v: PROTOCOL_VERSION, requestId, status: "error", stateVersion, error: { code, message } };
}

// ---------------------------------------------------------------------------
// Shared payload fragments
// ---------------------------------------------------------------------------

const gainDb = z.number().min(-60).max(12);
const rampMs = z.number().min(5).max(50);
const id = z.string().min(1);

const transformSchema = z
  .object({
    x: z.number().optional(),
    y: z.number().optional(),
    w: z.number().optional(),
    h: z.number().optional(),
    crop: z
      .object({
        u: z.number().optional(),
        v: z.number().optional(),
        w: z.number().optional(),
        h: z.number().optional(),
      })
      .strict()
      .optional(),
  })
  .strict();

const chromaKeyPatchSchema = z
  .object({
    enabled: z.boolean().optional(),
    color: z.enum(["green", "blue", "custom"]).optional(),
    customColorHex: z.string().regex(/^#[0-9a-fA-F]{6}$/).optional(),
    tolerance: z.number().min(0).max(1).optional(),
    softness: z.number().min(0).max(1).optional(),
    spillSuppression: z.number().min(0).max(1).optional(),
    edgeFeather: z.number().min(0).max(1).optional(),
  })
  .strict();

// ---------------------------------------------------------------------------
// Command payload schemas (Section 16 — one entry per command)
// ---------------------------------------------------------------------------

export const CommandPayloadSchemas = {
  // 16.1 show
  "show.load": z.object({ packagePath: z.string().min(1), mode: z.enum(["load", "reload"]).optional() }).strict(),
  "show.preflight": z.object({ strict: z.boolean().optional() }).strict(),
  "show.start": z
    .object({ startClock: z.boolean().optional(), allowWarnings: z.boolean().optional() })
    .strict(),
  "show.stop": z.object({ quiesceOutputs: z.boolean().default(true), force: z.boolean().default(false) }).strict(),
  "show.unload": z.object({}).strict(),

  // 16.2 view/preview
  "preview.set": z.object({ itemRef: id }).strict(),
  "view.take": z
    .object({
      transition: z.enum(["cut", "mix", "wipe", "sting", "move", "dve"]).default("cut"),
      preset: z.string().optional(),
      durationFrames: z.number().int().min(0).max(600).optional(),
      audio: z
        .object({
          transition: z.enum(["follow", "crossfade", "cut", "mute"]).default("follow"),
          durationFrames: z.number().int().min(1).max(600).optional(),
          rampMs: rampMs.default(10),
        })
        .strict()
        .optional(),
    })
    .strict(),
  "view.cut": z.object({ itemRef: id }).strict(),
  "view.fallback": z.object({ reason: z.string().optional() }).strict(),

  // 16.3 scene
  "scene.arm": z.object({ sceneId: id }).strict(),
  "scene.apply": z.object({ sceneId: id, target: z.enum(["view", "preview"]) }).strict(),

  // 16.4 sequence/item
  "sequence.arm": z.object({ sequenceId: id }).strict(),
  "sequence.unarm": z.object({ sequenceId: id }).strict(),
  "item.arm": z.object({ itemId: id }).strict(),
  "item.unarm": z.object({ itemId: id }).strict(),
  "item.stop": z.object({ itemId: id }).strict(),
  "item.reset": z.object({ itemId: id }).strict(),

  // 16.5 element/graphic
  "element.toggle": z.object({ elementId: id, scope: z.string().optional(), visible: z.boolean().optional() }).strict(),
  "element.set": z
    .object({
      elementId: id,
      patch: z
        .object({
          visible: z.boolean().optional(),
          opacity: z.number().min(0).max(1).optional(),
          transform: transformSchema.optional(),
          chromaKey: chromaKeyPatchSchema.optional(),
        })
        .strict(),
    })
    .strict(),
  "graphic.show": z
    .object({
      templateId: id,
      fields: z.record(z.unknown()),
      elementId: id.optional(),
      z: z.number().int().optional(),
    })
    .strict(),
  "graphic.hide": z.object({ elementId: id.optional(), templateId: id.optional() }).strict(),
  "graphic.update": z.object({ elementId: id, fields: z.record(z.unknown()) }).strict(),
  "breaking.show": z.object({ headline: z.string().min(1), subhead: z.string().optional() }).strict(),
  "breaking.hide": z.object({}).strict(),

  // 16.6 overlay
  "overlay.show": z.object({ overlayId: id, animation: z.string().optional() }).strict(),
  "overlay.hide": z.object({ overlayId: id }).strict(),

  // 16.7 ticker
  "ticker.setSource": z.object({ source: z.enum(["manual", "rss", "mixed"]) }).strict(),
  "ticker.override": z
    .object({
      items: z.array(
        z
          .object({
            text: z.string().min(1),
            language: z.string().optional(),
            priority: z.number().int().min(0).max(100000).default(0),
            ttlSec: z.number().int().min(1).optional(),
          })
          .strict(),
      ),
      mode: z.enum(["replace", "prepend", "append"]).default("replace"),
    })
    .strict(),
  "ticker.clearOverride": z.object({}).strict(),
  "ticker.refreshRss": z.object({ feedId: z.string().optional() }).strict(),

  // 16.8 soundboard/audio
  "soundboard.play": z.object({ assetId: id, gainDb: gainDb.optional() }).strict(),
  "soundboard.stop": z.object({ playbackId: id.optional(), assetId: id.optional() }).strict(),
  "soundboard.stopAll": z.object({}).strict(),
  "audio.bus.set": z
    .object({
      bus: z.enum(["mic", "clip", "music", "sfx", "guest", "master", "guestReturn", "ifb"]),
      guestId: id.optional(),
      gainDb: gainDb.optional(),
      muted: z.boolean().optional(),
    })
    .strict(),
  "audio.duck": z
    .object({
      bus: z.literal("music"),
      enabled: z.boolean(),
      depthDb: z.number().optional(),
      attackMs: z.number().optional(),
      releaseMs: z.number().optional(),
    })
    .strict(),
  "guest.mute": z.object({ guestId: id, muted: z.boolean() }).strict(),

  // 16.9 guest
  "guest.connect": z.object({ guestId: id, whipUrl: z.string().url(), displayName: z.string().optional() }).strict(),
  "guest.disconnect": z.object({ guestId: id }).strict(),
  "guest.setLayout": z.object({ guestId: id, layout: z.enum(["pip", "full"]) }).strict(),
  "guest.placeholder": z.object({ guestId: id, assetId: id.optional() }).strict(),
  "guest.configureReturn": z
    .object({
      guestId: id,
      mode: z.enum(["programMinusSelf", "producerMix", "mute"]).default("programMinusSelf"),
      includeOtherGuests: z.boolean().default(true),
      gainDb: gainDb.optional(),
      muted: z.boolean().optional(),
    })
    .strict(),
  "guest.getTurn": z
    .object({ guestId: id, ttlSec: z.number().int().min(30).max(86400).default(600) })
    .strict(),

  // 16.10 automation
  "automation.enable": z.object({ ruleId: id }).strict(),
  "automation.disable": z.object({ ruleId: id }).strict(),
  "automation.hold": z.object({ hold: z.boolean() }).strict(),

  // 16.11 snapshot/marker
  "snapshot.save": z.object({ name: z.string().min(1) }).strict(),
  "snapshot.recall": z.object({ name: z.string().min(1) }).strict(),
  "marker.add": z.object({ name: z.string().min(1), timecode: z.string().optional() }).strict(),

  // 16.12 plugin
  "plugin.reload": z.object({ pluginId: id }).strict(),

  // 16.13 clock
  "clock.configure": z
    .object({
      elementId: id,
      clock: z
        .object({
          mode: z.enum(["wall", "showElapsed"]).optional(),
          timezone: z.string().optional(),
          format: z.enum(["HH:mm", "HH:mm:ss", "hh:mm A", "locale"]).optional(),
          locale: z.string().optional(),
          blinkColon: z.boolean().optional(),
        })
        .strict(),
    })
    .strict(),

  // 16.14 output
  "record.start": z.object({ outputId: id.optional() }).strict(),
  "record.stop": z.object({}).strict(),
  "stream.start": z.object({ outputId: id.optional(), url: z.string().optional() }).strict(),
  "stream.stop": z.object({}).strict(),

  // 16.15 system
  "system.status": z.object({}).strict(),
  "system.telemetry.subscribe": z.object({ intervalMs: z.number().int().min(100).max(60000).default(1000) }).strict(),
  "system.telemetry.unsubscribe": z.object({}).strict(),
} as const;

export type CommandName = keyof typeof CommandPayloadSchemas;
export const CommandNames = Object.keys(CommandPayloadSchemas) as CommandName[];

// ---------------------------------------------------------------------------
// Deprecation aliases (Assumption 17): program.* → view.*, layer.* → element.*
// ---------------------------------------------------------------------------

export const DEPRECATED_ALIAS_PREFIXES: ReadonlyArray<readonly [string, string]> = [
  ["program.", "view."],
  ["layer.", "element."],
];

export interface AliasResolution {
  command: CommandName;
  deprecated: boolean;
  warning?: string;
}

/** Resolve a possibly-deprecated command name to its canonical command. */
export function resolveCommand(raw: string): AliasResolution | null {
  if (raw in CommandPayloadSchemas) {
    return { command: raw as CommandName, deprecated: false };
  }
  for (const [from, to] of DEPRECATED_ALIAS_PREFIXES) {
    if (raw.startsWith(from)) {
      const candidate = to + raw.slice(from.length);
      if (candidate in CommandPayloadSchemas) {
        return {
          command: candidate as CommandName,
          deprecated: true,
          warning: `command "${raw}" is deprecated; use "${candidate}" (maps 1:1, removed after one spec version — Assumption 17)`,
        };
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Render bridge frames (Addendum 02a §1.1)
//
// Server → render node directive frame (distinct from the Section 5.4
// command envelope).
// ---------------------------------------------------------------------------

export const DirectiveFrameSchema = z
  .object({
    v: z.literal(PROTOCOL_VERSION),
    kind: z.literal("directive"),
    seq: z.number().int().nonnegative(),
    stateVersion: z.number().int().nonnegative(),
    command: z.string().min(1),
    target: z.record(z.unknown()).default({}),
    payload: z.record(z.unknown()),
  })
  .strict();
export type DirectiveFrame = z.infer<typeof DirectiveFrameSchema>;

// ---------------------------------------------------------------------------
// Engine → control plane frames (Addendum 02a §1.1): telemetry, applied
// state version, health, and item lifecycle events. Accepted only from
// render-role sessions.
// ---------------------------------------------------------------------------

export const EngineTelemetryFrameSchema = z
  .object({
    v: z.literal(PROTOCOL_VERSION),
    kind: z.literal("engineTelemetry"),
    ts: z.number(),
    masterClockFrame: z.number().int(),
    droppedFramesTotal: z.number().int(),
    renderGpuTimeMs: z.number(),
    decodeSessions: z.number().int(),
    vramUsedMib: z.number(),
    textureCacheUsedMib: z.number(),
    streamBufferMs: z.number(),
    recordSpaceMib: z.number(),
    masterClockDriftMs: z.number(),
    fallbackActive: z.boolean(),
    degradationRung: z.number().int(),
  })
  .strict();
export type EngineTelemetryFrame = z.infer<typeof EngineTelemetryFrameSchema>;

export const AppliedStateVersionFrameSchema = z
  .object({
    v: z.literal(PROTOCOL_VERSION),
    kind: z.literal("appliedStateVersion"),
    stateVersion: z.number().int().nonnegative(),
  })
  .strict();
export type AppliedStateVersionFrame = z.infer<typeof AppliedStateVersionFrameSchema>;

export const ItemLifecycleFrameSchema = z
  .object({
    v: z.literal(PROTOCOL_VERSION),
    kind: z.literal("itemEvent"),
    itemRef: z.string().min(1),
    event: z.enum(["end", "decodeError", "deviceLoss", "missing"]),
    detail: z.string().optional(),
  })
  .strict();
export type ItemLifecycleFrame = z.infer<typeof ItemLifecycleFrameSchema>;

/** SPEC §5.9.3: the engine asks for a fresh snapshot. */
export const ResyncRequestFrameSchema = z
  .object({
    v: z.literal(PROTOCOL_VERSION),
    kind: z.literal("resyncRequest"),
    reason: z.enum(["seqGap", "reconnect", "internal"]),
  })
  .strict();
export type ResyncRequestFrame = z.infer<typeof ResyncRequestFrameSchema>;

export const EngineFrameSchema = z.discriminatedUnion("kind", [
  EngineTelemetryFrameSchema,
  AppliedStateVersionFrameSchema,
  ItemLifecycleFrameSchema,
  ResyncRequestFrameSchema,
]);

/**
 * SPEC §5.4.1 server-push frames. Not responses: no `requestId`, never
 * confused with the Section 5.4 response shapes.
 */
export interface StateChangeFrame {
  v: typeof PROTOCOL_VERSION;
  kind: "stateChange";
  stateVersion: number;
  changed: string[];
  state: Record<string, unknown>;
}

/**
 * SPEC §5.9.4: the directive-only command name that carries the full
 * snapshot. Deliberately absent from `CommandPayloadSchemas` — no client may
 * issue it.
 */
export const RESYNC_COMMAND = "show.resync" as const;
export type EngineFrame = z.infer<typeof EngineFrameSchema>;
