//! Input Intent layer tests (Prompt 08, work item 1): typed intent model,
//! profile documents, resolve() against §16 schemas, TriggerKind audit.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  InputIntentSchema,
  IntentProfileSchema,
  TriggerKind,
  TriggerKindSchema,
  TRIGGER_KIND_WIRE_VALUES,
  TRIGGER_KIND_RUST_VARIANTS,
  bindingToIntent,
  intentToEnvelope,
  profileFromBindings,
  resolveIntent,
} from "./intent.js";
import { CommandPayloadSchemas, CpError, EnvelopeSchema } from "./protocol.js";

test("round-trip typed -> serialize -> schema re-validate -> resolve -> envelope", () => {
  const typed = InputIntentSchema.parse({
    intentId: "take-1",
    profileId: "xl-a",
    action: "view.take",
    payload: {},
    trigger: { kind: TriggerKind.CompanionKey, page: 1, bank: 1, key: "take-1" },
  });
  const revived = InputIntentSchema.parse(JSON.parse(JSON.stringify(typed)) as unknown);
  assert.deepEqual(revived, typed);

  const resolved = resolveIntent(revived);
  assert.equal(resolved.command, "view.take");
  // Re-validate the resolved payload against the §16 schema (reuse, no fork).
  assert.ok(CommandPayloadSchemas[resolved.command].safeParse(resolved.payload).success);

  const envelope = intentToEnvelope(resolved);
  assert.ok(EnvelopeSchema.safeParse(envelope).success);
  assert.equal(envelope.command, "view.take");
});

test("TriggerKind literal audit vs manifest schema + Rust enum", () => {
  // Rust variants, literal-by-literal (crates/nbe-core/src/manifest.rs TriggerKind).
  assert.deepEqual([...TRIGGER_KIND_RUST_VARIANTS], [
    "CompanionKey",
    "Hotkey",
    "Midi",
    "WebButton",
    "Osc",
  ]);
  // Wire values, literal-by-literal (schemas/manifest.v0.4.json ControlBinding trigger kind).
  assert.deepEqual([...TRIGGER_KIND_WIRE_VALUES], [
    "companionKey",
    "hotkey",
    "midi",
    "webButton",
    "osc",
  ]);
  // Enum keys track Rust, enum values track the wire.
  assert.deepEqual(Object.keys(TriggerKind), [...TRIGGER_KIND_RUST_VARIANTS]);
  assert.deepEqual(Object.values(TriggerKind), [...TRIGGER_KIND_WIRE_VALUES]);
  for (const v of TRIGGER_KIND_WIRE_VALUES) {
    assert.ok(TriggerKindSchema.safeParse(v).success, `wire value must parse: ${v}`);
  }
  assert.ok(!TriggerKindSchema.safeParse("CompanionKey").success, "Rust name is not a wire value");
});

test("unknown action rejected by resolve; bad payload rejected by resolve", () => {
  try {
    resolveIntent({ action: "bogus.cmd", payload: {} });
    assert.fail("unknown action must throw");
  } catch (e) {
    assert.ok(e instanceof CpError, "must be a CpError");
    assert.equal(e.code, "E_UNSUPPORTED");
  }
  try {
    resolveIntent({ action: "view.cut", payload: {} }); // itemRef required by §16
    assert.fail("schema-invalid payload must throw");
  } catch (e) {
    assert.ok(e instanceof CpError, "must be a CpError");
    assert.equal(e.code, "E_BAD_PAYLOAD");
  }
});

test("bindingToIntent mirrors ControlBinding; profile round-trips", () => {
  const profile = profileFromBindings("xl-a", "companion", [
    {
      id: "fb-1",
      description: "fallback",
      trigger: { kind: "companionKey", page: 1, bank: 1, key: "fb" },
      action: "view.fallback",
      payload: {},
    },
  ]);
  const parsed = IntentProfileSchema.parse(JSON.parse(JSON.stringify(profile)) as unknown);
  assert.equal(parsed.profileId, "xl-a");
  assert.equal(parsed.entries.length, 1);

  const intent = bindingToIntent(
    {
      id: "fb-1",
      trigger: { kind: "companionKey", page: 1, bank: 1, key: "fb" },
      action: "view.fallback",
      payload: {},
    },
    "xl-a",
  );
  assert.equal(intent.intentId, "fb-1");
  assert.equal(intent.profileId, "xl-a");
  assert.equal(resolveIntent(intent).command, "view.fallback");
});

test("bindingToIntent rejects unknown trigger kind with CpError, not ZodError", () => {
  try {
    bindingToIntent(
      {
        id: "bad-kind",
        trigger: { kind: "footPedal", key: "z" },
        action: "view.take",
        payload: {},
      } as unknown as Parameters<typeof bindingToIntent>[0],
      "xl-a",
    );
    assert.fail("unknown trigger kind must throw");
  } catch (e) {
    assert.ok(e instanceof CpError, "must be a CpError");
    assert.equal(e.code, "E_BAD_PAYLOAD");
  }
});

test("REGISTERED_COMMANDS mirror gate: Rust preflight list tracks §16 keys", () => {
  // Single source of truth is CommandPayloadSchemas; the Rust list in
  // crates/nbe-preflight/src/main.rs is a manual mirror. This test fails on
  // drift in either direction, which is the gate the mirror needs.
  const rust = readFileSync(
    resolve(import.meta.dirname, "../../../crates/nbe-preflight/src/main.rs"),
    "utf8",
  );
  const block = rust.slice(
    rust.indexOf("const REGISTERED_COMMANDS"),
    rust.indexOf("];", rust.indexOf("const REGISTERED_COMMANDS")) + 2,
  );
  assert.ok(block.includes("show.load"), "mirror block must be found");
  const mirrored = new Set(
    [...block.matchAll(/"([a-z0-9]+(?:\.[a-zA-Z0-9]+)+)"/g)].map((m) => m[1] as string),
  );
  const keys = new Set(Object.keys(CommandPayloadSchemas));
  for (const k of keys) assert.ok(mirrored.has(k), `Rust mirror missing §16 key: ${k}`);
  for (const k of mirrored) assert.ok(keys.has(k), `Rust mirror has extra key: ${k}`);
});
