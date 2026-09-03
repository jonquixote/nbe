//! Quality-bar tests (Standards §1): round-trip every Section 16 command's
//! payload through its zod schema; audit the error-code registry against the
//! Section 16 table.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CommandPayloadSchemas, CommandNames, ErrorCodeSchema, EnvelopeSchema } from "./protocol.js";

// The Section 16 tables are parsed OUT OF THE SPEC at test time. The lists
// used to be copied here by hand, which meant the spec could move to v0.3.2 —
// adding `item.reset` and `E_RATE_LIMITED` — while every test stayed green.
// Parsing makes spec drift a red build instead of a silent divergence.

const SPEC_PATH = new URL("../../../docs/spec.v0.3.md", import.meta.url);

function section16(): string {
  const spec = readFileSync(SPEC_PATH, "utf8");
  return spec.slice(spec.indexOf("# 16. Command API"), spec.indexOf("# 17. State machine"));
}

/** Command names from the leading cell of every Section 16 table row. */
function specCommands(): string[] {
  const rows = section16().matchAll(/^\|\s*`([a-z][a-zA-Z.]+\.[a-zA-Z.]+)`\s*\|/gm);
  return [...new Set([...rows].map((m) => m[1]!))];
}

/** Error codes from the Section 16 registry table. */
function specErrorCodes(): string[] {
  const rows = section16().matchAll(/^\|\s*`(E_[A-Z_]+)`\s*\|/gm);
  return [...new Set([...rows].map((m) => m[1]!))];
}

test("the spec tables are parseable (guards the two tests below)", () => {
  // A regex that silently matches nothing would make both audits vacuous.
  assert.ok(specCommands().length >= 50, `parsed only ${specCommands().length} commands from Section 16`);
  assert.ok(specErrorCodes().length >= 15, `parsed only ${specErrorCodes().length} error codes`);
});

test("error-code registry matches Section 16 exactly", () => {
  assert.deepEqual([...ErrorCodeSchema.options].sort(), specErrorCodes().sort());
});

test("every Section 16 command has a payload schema", () => {
  assert.deepEqual([...CommandNames].sort(), specCommands().sort());
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
  "item.reset": { itemId: "A1" },
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
