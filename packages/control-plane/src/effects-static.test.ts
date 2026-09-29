//! PR #34's fix round — §13.4.1's same-dispatch `stateChange` cells, checked
//! STATICALLY against every command handler.
//!
//! The two-key pass of 2026-09-29 found two edges missing from
//! `crates/nbe-core/src/automation_effects.json`: `scene.arm` writes
//! `previewItem` (only when the preview is empty) and `show.stop` writes
//! `streamState` and `recordState`. WU5's runtime row check had passed them,
//! because a probe sees only the path it takes. "No missing edge" needs a
//! mechanism, not a probe. This file reads the source: every handler's writes
//! to the fields a `stateChange` rule can name — directly, through the
//! `ControlPlaneState` methods it calls, and through its file's local helpers,
//! each followed transitively — set-compared against the effects data, the
//! shape of `protocol.test.ts`'s §16 parse. A conditional write counts: cycle
//! detection asks what a command CAN cause.
//!
//! Over-approximating, deliberately: a name match is a call. A write the
//! scanner cannot see (a new indirection) is its blind spot; the vacuity test
//! pins that it still sees the writes it must.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

const SRC = new URL("./", import.meta.url);
const read = (rel: string): string => readFileSync(new URL(rel, SRC), "utf8");

/** The fields a `stateChange` rule can name (§13.2's params table). */
const FIELDS = ["showState", "viewItem", "previewItem", "streamState", "recordState", "automationHold", "fallbackActive"];
const WRITE = new RegExp(`\\.(${FIELDS.join("|")})\\s*=(?!=)`, "g");
const ITEM_STATE_WRITE = /itemStates\s*=(?!=)|itemStates\.(?:set|clear|delete)\(/;

interface Unit {
  writes: Set<string>;
  calls: Set<string>;
}

function unitOf(body: string, callPattern: RegExp): Unit {
  const writes = new Set<string>([...body.matchAll(WRITE)].map((m) => m[1]!));
  if (ITEM_STATE_WRITE.test(body)) writes.add("itemState");
  const calls = new Set<string>([...body.matchAll(callPattern)].map((m) => m[1]!));
  return { writes, calls };
}

/** The body of the block whose `{` is at `open`, by brace matching. */
function block(src: string, open: number): string {
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return src.slice(open, i + 1);
  }
  return src.slice(open);
}

/** Writes of every `ControlPlaneState` method, closed over the methods it calls. */
function stateMethods(): Map<string, Set<string>> {
  const src = read("state.ts");
  const units = new Map<string, Unit>();
  for (const m of src.matchAll(/\n {2}(?:async )?(\w+)\([^)]*\)[^{;=]*\{/g)) {
    const body = block(src, m.index! + m[0].length - 1);
    units.set(m[1]!, unitOf(body, /this\.(\w+)\(/g));
  }
  return close(units);
}

function close(units: Map<string, Unit>): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  const visit = (name: string, seen: Set<string>): Set<string> => {
    const u = units.get(name);
    if (!u || seen.has(name)) return new Set();
    seen.add(name);
    const w = new Set(u.writes);
    for (const c of u.calls) for (const f of visit(c, seen)) w.add(f);
    return w;
  };
  for (const name of units.keys()) out.set(name, visit(name, new Set()));
  return out;
}

/** Every registered command → the rule-nameable fields its handler can write. */
function handlerWrites(): Map<string, Set<string>> {
  const methods = stateMethods();
  const out = new Map<string, Set<string>>();
  for (const file of readdirSync(new URL("commands/", SRC)).filter((f) => f.endsWith(".ts"))) {
    const src = read(`commands/${file}`);
    // The file's local helpers (top-level functions), followed transitively.
    const helpers = new Map<string, Unit>();
    for (const m of src.matchAll(/\n(?:export )?(?:async )?function (\w+)\([^)]*\)[^{]*\{/g)) {
      const body = block(src, m.index! + m[0].length - 1);
      if (/reg\.set\("/.test(body)) continue; // the registering function itself
      helpers.set(m[1]!, unitOf(body, /(?<![.\w])(\w+)\(/g));
    }
    const helperWrites = close(helpers);
    // Each handler: from its `reg.set("…"` to the next one, or to the close of
    // the registering function — never into the helpers after it.
    const starts = [...src.matchAll(/reg\.set\("([a-zA-Z.]+)"/g)];
    starts.forEach((m, i) => {
      const from = m.index!;
      const nextReg = starts[i + 1]?.index ?? src.length;
      const fnClose = src.indexOf("\n}\n", from);
      const body = src.slice(from, Math.min(nextReg, fnClose === -1 ? src.length : fnClose));
      const u = unitOf(body, /\.(\w+)\(/g);
      const w = new Set(u.writes);
      for (const c of u.calls) for (const f of methods.get(c) ?? []) w.add(f);
      for (const h of body.matchAll(/(?<![.\w])(\w+)\(/g)) for (const f of helperWrites.get(h[1]!) ?? []) w.add(f);
      out.set(m[1]!, w);
    });
  }
  return out;
}

const EFFECTS = JSON.parse(
  readFileSync(new URL("../../../crates/nbe-core/src/automation_effects.json", import.meta.url), "utf8"),
) as { commands: Record<string, { stateChange?: string[] }> };

/**
 * Commands whose handler writes a field no rule can hear, and why. Each is a
 * claim about the tree the test re-checks, so an exception cannot outlive its
 * reason.
 */
const EXCEPTIONS: Record<string, { why: string; premise: () => boolean }> = {
  "show.unload": {
    why: "its rules unload in the same turn, before the diff (server.ts afterAccepted), so nothing is left to hear the writes",
    premise: () => {
      const s = read("server.ts");
      const unload = s.indexOf('else if (command === "show.unload") automation.unload();');
      const diff = s.indexOf("for (const e of stateChanges(before, after)) automation.fire(e, cause, observedAt);");
      return unload !== -1 && diff !== -1 && unload < diff;
    },
  },
};

test("the scanner reads every registered handler and sees the writes it must (guards the test below)", () => {
  const writes = handlerWrites();
  assert.deepEqual([...writes.keys()].sort(), Object.keys(EFFECTS.commands).sort(), "one handler per §16 command, 55");
  // Writes reached each way: directly, through a state method, through a
  // method conditionally — the pass's two finds among them.
  const must: Record<string, string[]> = {
    "automation.hold": ["automationHold"], // direct
    "view.take": ["viewItem", "previewItem", "fallbackActive", "itemState"], // state.take
    "scene.arm": ["previewItem"], // state.armScene, conditional
    "show.stop": ["showState", "streamState", "recordState"], // direct
    "snapshot.recall": ["viewItem", "previewItem", "automationHold", "itemState"], // state.recallSnapshot
  };
  for (const [cmd, fields] of Object.entries(must)) {
    for (const f of fields) assert.ok(writes.get(cmd)?.has(f), `${cmd} must be seen writing ${f}; saw ${[...(writes.get(cmd) ?? [])]}`);
  }
});

test("§13.4.1, statically: every handler's writes to rule-nameable fields are exactly its row's stateChange cell", () => {
  const writes = handlerWrites();
  const wrong: string[] = [];
  for (const [cmd, row] of Object.entries(EFFECTS.commands)) {
    const declared = new Set(row.stateChange ?? []);
    const seen = writes.get(cmd) ?? new Set<string>();
    const exception = EXCEPTIONS[cmd];
    if (exception) {
      assert.ok(exception.premise(), `${cmd}'s exception no longer holds in the tree: ${exception.why}`);
      continue;
    }
    const missing = [...seen].filter((f) => !declared.has(f));
    const extra = [...declared].filter((f) => !seen.has(f));
    if (missing.length) wrong.push(`${cmd}: writes ${missing.join(", ")} but its row does not name it — a missing edge`);
    if (extra.length) wrong.push(`${cmd}: its row names ${extra.join(", ")} but no handler path writes it`);
  }
  assert.deepEqual(wrong, [], "handlers and the effects data disagree");
});
