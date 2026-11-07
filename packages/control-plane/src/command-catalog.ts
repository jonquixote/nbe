//! The §16 command catalog, exported for clients that bind to it (the
//! operator shell's parity test, Prompt 13 step 3): every command in
//! `CommandPayloadSchemas`, its payload's keys with their types, and the
//! roles the §16.0 matrix lets issue it (`dispatch.ts`'s `roleAllowed`).
//!
//! `npx tsx src/command-catalog.ts` prints the catalog; `--out <path>`
//! writes it. `--check <path>` exits 1 when the file at <path> differs from a
//! fresh export, so a committed copy cannot go stale silently.

import { readFileSync, writeFileSync } from "node:fs";
import { z } from "zod";

import { CommandNames, CommandPayloadSchemas, PROTOCOL_VERSION, RoleSchema, type CommandName } from "./protocol.js";
import { roleAllowed } from "./dispatch.js";

/** A payload value's shape, as far as a client binding needs it. */
export type TypeDesc =
  | { type: "string"; enum?: string[]; literal?: string }
  | { type: "number"; integer: boolean; min?: number; max?: number }
  | { type: "boolean" }
  | { type: "array"; items: TypeDesc }
  | { type: "object"; keys: Record<string, KeyDesc>; strict: boolean }
  | { type: "record"; values: TypeDesc }
  | { type: "union"; options: TypeDesc[] }
  | { type: "unknown" };

export interface KeyDesc {
  required: boolean;
  hasDefault: boolean;
  nullable: boolean;
  value: TypeDesc;
}

interface Unwrapped {
  schema: z.ZodTypeAny;
  optional: boolean;
  hasDefault: boolean;
  nullable: boolean;
}

function unwrap(schema: z.ZodTypeAny): Unwrapped {
  let s = schema;
  let optional = false;
  let hasDefault = false;
  let nullable = false;
  for (;;) {
    if (s instanceof z.ZodOptional) {
      optional = true;
      s = s.unwrap();
    } else if (s instanceof z.ZodDefault) {
      hasDefault = true;
      s = s.removeDefault();
    } else if (s instanceof z.ZodNullable) {
      nullable = true;
      s = s.unwrap();
    } else if (s instanceof z.ZodEffects) {
      s = s.innerType();
    } else {
      return { schema: s, optional, hasDefault, nullable };
    }
  }
}

export function describe(schema: z.ZodTypeAny): TypeDesc {
  const { schema: s } = unwrap(schema);
  if (s instanceof z.ZodString) return { type: "string" };
  if (s instanceof z.ZodEnum) return { type: "string", enum: [...(s.options as string[])] };
  if (s instanceof z.ZodLiteral) {
    const v = s.value as unknown;
    if (typeof v === "string") return { type: "string", literal: v };
    if (typeof v === "number") return { type: "number", integer: Number.isInteger(v), min: v, max: v };
    if (typeof v === "boolean") return { type: "boolean" };
    return { type: "unknown" };
  }
  if (s instanceof z.ZodNumber) {
    const d: TypeDesc = { type: "number", integer: s.isInt };
    if (s.minValue !== null) d.min = s.minValue;
    if (s.maxValue !== null) d.max = s.maxValue;
    return d;
  }
  if (s instanceof z.ZodBoolean) return { type: "boolean" };
  if (s instanceof z.ZodArray) return { type: "array", items: describe(s.element) };
  if (s instanceof z.ZodRecord) return { type: "record", values: describe(s.valueSchema) };
  if (s instanceof z.ZodUnion) return { type: "union", options: (s.options as z.ZodTypeAny[]).map(describe) };
  if (s instanceof z.ZodObject) {
    const keys: Record<string, KeyDesc> = {};
    for (const [k, v] of Object.entries(s.shape as Record<string, z.ZodTypeAny>)) {
      const u = unwrap(v);
      keys[k] = { required: !u.optional && !u.hasDefault, hasDefault: u.hasDefault, nullable: u.nullable, value: describe(v) };
    }
    return { type: "object", keys, strict: s._def.unknownKeys === "strict" };
  }
  return { type: "unknown" };
}

export interface CommandDesc {
  name: CommandName;
  roles: string[];
  payload: TypeDesc;
}

export function exportCommands(): { v: string; source: string; commands: CommandDesc[] } {
  const roles = RoleSchema.options.filter((r) => r !== "render");
  const commands = [...CommandNames].sort().map((name) => ({
    name,
    roles: roles.filter((r) => roleAllowed(r, name)),
    payload: describe(CommandPayloadSchemas[name] as z.ZodTypeAny),
  }));
  return { v: PROTOCOL_VERSION, source: "packages/control-plane/src/protocol.ts CommandPayloadSchemas; dispatch.ts roleAllowed", commands };
}

function main(argv: string[]): number {
  const text = JSON.stringify(exportCommands(), null, 2) + "\n";
  const out = argv.indexOf("--out");
  const check = argv.indexOf("--check");
  if (check >= 0) {
    const path = argv[check + 1]!;
    const committed = readFileSync(path, "utf8");
    if (committed !== text) {
      console.error(`${path} differs from a fresh export of the §16 catalog; run src/command-catalog.ts --out ${path}`);
      return 1;
    }
    console.log(`${path}: the committed catalog matches a fresh export (${exportCommands().commands.length} commands)`);
    return 0;
  }
  if (out >= 0) {
    writeFileSync(argv[out + 1]!, text);
    return 0;
  }
  process.stdout.write(text);
  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exit(main(process.argv.slice(2)));
}
