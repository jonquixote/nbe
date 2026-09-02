//! Quality-bar tests (Standards §1): round-trip every Section 16 command's
//! payload through its zod schema; audit the error-code registry against the
//! Section 16 table.

import { test } from "node:test";
import assert from "node:assert/strict";
import { CommandPayloadSchemas, CommandNames, ErrorCodeSchema, EnvelopeSchema } from "./protocol.js";

// The authoritative Section 16 error-code table, reworked here so a drift
// between this list and the registry fails the test.
const SECTION_16_ERROR_CODES = [
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
] as const;

test("error-code registry matches Section 16 exactly", () => {
  assert.deepEqual([...ErrorCodeSchema.options].sort(), [...SECTION_16_ERROR_CODES].sort());
});

test("every Section 16 command has a payload schema", () => {
  // The Section 16 command surface, enumerated by hand so a schema that
  // silently drops a command is caught here.
  const expected = [
    "show.load",
    "show.preflight",
    "show.start",
    "show.stop",
    "show.unload",
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
    "guest.connect",
    "guest.disconnect",
    "guest.setLayout",
    "guest.placeholder",
    "guest.configureReturn",
    "guest.getTurn",
    "automation.enable",
    "automation.disable",
    "automation.hold",
    "snapshot.save",
    "snapshot.recall",
    "marker.add",
    "plugin.reload",
    "clock.configure",
    "record.start",
    "record.stop",
    "stream.start",
    "stream.stop",
    "system.status",
    "system.telemetry.subscribe",
    "system.telemetry.unsubscribe",
  ];
  assert.deepEqual([...CommandNames].sort(), expected.sort());
});

// A canonical sample payload per command family; each must round-trip
// through its zod schema unchanged (defaults applied are part of the shape).
const SAMPLE_PAYLOADS: Record<string, unknown> = {
  "show.load": { packagePath: "./tests/fixtures/valid_show_v0.3" },
  "show.preflight": {},
  "show.start": {},
  "show.stop": {},
  "show.unload": {},
  "preview.set": { itemRef: "A1" },
  "view.take": { transition: "mix", durationFrames: 15, audio: { transition: "follow" } },
  "view.cut": { itemRef: "A1" },
  "view.fallback": {},
  "scene.arm": { sceneId: "SCN_A1" },
  "scene.apply": { sceneId: "SCN_A1", target: "preview" },
  "sequence.arm": { sequenceId: "A" },
  "sequence.unarm": { sequenceId: "A" },
  "item.arm": { itemId: "A1" },
  "item.unarm": { itemId: "A1" },
  "item.stop": { itemId: "A1" },
  "element.toggle": { elementId: "lowerThird" },
  "element.set": { elementId: "lowerThird", patch: { opacity: 0.8 } },
  "graphic.show": { templateId: "lower_third", fields: { text: "Hello" } },
  "graphic.hide": { elementId: "lowerThird" },
  "graphic.update": { elementId: "lowerThird", fields: { text: "Updated" } },
  "breaking.show": { headline: "BREAKING", subhead: "details" },
  "breaking.hide": {},
  "overlay.show": { overlayId: "ticker" },
  "overlay.hide": { overlayId: "ticker" },
  "ticker.setSource": { source: "manual" },
  "ticker.override": { items: [{ text: "one", priority: 10 }], mode: "replace" },
  "ticker.clearOverride": {},
  "ticker.refreshRss": { feedId: "wire" },
  "soundboard.play": { assetId: "stab", gainDb: -6 },
  "soundboard.stop": { playbackId: "abc" },
  "soundboard.stopAll": {},
  "audio.bus.set": { bus: "music", gainDb: -12 },
  "audio.duck": { bus: "music", enabled: true },
  "guest.mute": { guestId: "remote_1", muted: true },
  "guest.connect": { guestId: "remote_1", whipUrl: "https://example.test/whip" },
  "guest.disconnect": { guestId: "remote_1" },
  "guest.setLayout": { guestId: "remote_1", layout: "pip" },
  "guest.placeholder": { guestId: "remote_1", assetId: "fallback_slate" },
  "guest.configureReturn": { guestId: "remote_1", mode: "programMinusSelf" },
  "guest.getTurn": { guestId: "remote_1" },
  "automation.enable": { ruleId: "rule_1" },
  "automation.disable": { ruleId: "rule_1" },
  "automation.hold": { hold: true },
  "snapshot.save": { name: "pre-show" },
  "snapshot.recall": { name: "pre-show" },
  "marker.add": { name: "segment-b" },
  "plugin.reload": { pluginId: "lowerthird_anim" },
  "clock.configure": { elementId: "clock", clock: { mode: "showElapsed" } },
  "record.start": {},
  "record.stop": {},
  "stream.start": { url: "rtmp://example.test/live" },
  "stream.stop": {},
  "system.status": {},
  "system.telemetry.subscribe": { intervalMs: 1000 },
  "system.telemetry.unsubscribe": {},
};

test("every Section 16 command payload round-trips through its zod schema", () => {
  for (const name of CommandNames) {
    const schema = CommandPayloadSchemas[name as keyof typeof CommandPayloadSchemas];
    const sample = SAMPLE_PAYLOADS[name];
    assert.ok(sample !== undefined, `missing sample payload for ${name}`);
    const first = schema.parse(JSON.parse(JSON.stringify(sample)));
    const second = schema.parse(JSON.parse(JSON.stringify(first)));
    assert.deepEqual(second, first, `${name} did not round-trip`);
  }
});

test("malformed envelopes are rejected", () => {
  assert.ok(!EnvelopeSchema.safeParse({}).success);
  assert.ok(!EnvelopeSchema.safeParse({ v: "0.3", id: "not-a-uuid", command: "system.status", payload: {} }).success);
  assert.ok(
    !EnvelopeSchema.safeParse({ v: "0.2", id: crypto.randomUUID(), command: "system.status", payload: {} }).success,
  );
});
