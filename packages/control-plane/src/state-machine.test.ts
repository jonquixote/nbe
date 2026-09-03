//! Section 17.3 total coverage, driven by the spec table itself.
//!
//! Prompt 02c Step 8: the transition table is parsed out of
//! `docs/spec.v0.3.md` and every (state, command-event) pair in the 7x5 grid
//! is exercised. Pairs the table lists must succeed and land on a listed
//! state; every pair it does not list must be refused with E_FORBIDDEN_STATE.
//! Sampling illegal transitions proves nothing about the ones you skipped.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { ControlPlaneState, type ItemState } from "./state.js";
import { CpError } from "./protocol.js";

const SPEC_PATH = new URL("../../../docs/spec.v0.3.md", import.meta.url);

const STATES: ItemState[] = ["READY", "ARMED", "LIVE", "PLAYING", "DONE", "MISSING", "ERROR"];
/** The events Section 16 commands produce (§17.3 "Event sources"). */
const COMMAND_EVENTS = ["arm", "unarm", "take", "stop", "reset"] as const;
type CommandEvent = (typeof COMMAND_EVENTS)[number];

interface Row {
  current: ItemState;
  event: CommandEvent;
  next: ItemState[];
}

/**
 * Parse the Section 17.3 rows whose event is command-driven. Rows describing
 * engine-observed events ("decode error", "end reached") are excluded: they
 * arrive as itemEvent frames, not commands. "`take` away" is likewise not the
 * `take` command applied to that item — it is another item taking the bus.
 */
function specRows(): Row[] {
  const spec = readFileSync(SPEC_PATH, "utf8");
  const table = spec.slice(spec.indexOf("## 17.3 Transition table"), spec.indexOf("## 17.4"));
  const rows: Row[] = [];
  for (const line of table.split("\n")) {
    const cells = line.split("|").map((c) => c.trim());
    if (cells.length < 6) continue;
    const current = cells[1]?.replace(/`/g, "");
    const eventCell = cells[2] ?? "";
    const nextCell = cells[4] ?? "";
    if (!STATES.includes(current as ItemState)) continue;
    if (/away/.test(eventCell)) continue; // another item taking the bus
    const events = [...eventCell.matchAll(/`([a-z]+)`/g)]
      .map((m) => m[1] as CommandEvent)
      .filter((e) => (COMMAND_EVENTS as readonly string[]).includes(e));
    if (events.length === 0) continue; // engine-observed row
    const next = [...nextCell.matchAll(/`([A-Z]+)`/g)].map((m) => m[1] as ItemState);
    for (const event of events) rows.push({ current: current as ItemState, event, next });
  }
  return rows;
}

/** Put a fresh state object into `target` for item A1, with a package loaded. */
function stateAt(target: ItemState, opts: { timed?: boolean } = {}): ControlPlaneState {
  const state = new ControlPlaneState();
  state.loadPackage({
    packagePath: "/tmp/none",
    showId: "show-1",
    qualityProfile: undefined,
    items: new Map([
      [
        "A1",
        {
          id: "A1",
          kind: "sceneRef",
          ...(opts.timed ? { durationFrames: 300 } : {}),
        },
      ],
    ]),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(),
    templates: new Set(),
    breakingTemplates: new Set(),
    tickerExists: false,
    clockElements: new Set(),
    plugins: new Set(),
    automationRules: new Set(),
    assets: new Map(),
    transitionPresets: new Map(),
    fallbackAssetId: undefined,
  });
  state.itemStates.set("A1", target);
  if (target === "LIVE" || target === "PLAYING") state.viewItem = "A1";
  return state;
}

function applyEvent(state: ControlPlaneState, event: CommandEvent): void {
  switch (event) {
    case "arm":
      return state.armItem("A1");
    case "unarm":
      return state.unarmItem("A1");
    case "take":
      state.take("A1");
      return;
    case "stop":
      return state.stopItem("A1");
    case "reset":
      return state.resetItem("A1");
  }
}

test("the Section 17.3 table is parseable (guards the coverage test below)", () => {
  const rows = specRows();
  assert.ok(rows.length >= 6, `parsed only ${rows.length} command-driven rows from Section 17.3`);
  // Spot-check two anchors so a mangled parse cannot silently pass.
  assert.ok(rows.some((r) => r.current === "READY" && r.event === "arm" && r.next.includes("ARMED")));
  assert.ok(rows.some((r) => r.current === "ARMED" && r.event === "take"));
});

test("every legal Section 17.3 command transition executes and lands on a listed state", () => {
  for (const row of specRows()) {
    // `take` from ARMED is guard-dependent: LIVE for a live source, PLAYING
    // for timed media. Exercise whichever this row describes.
    const timed = row.next.includes("PLAYING");
    const state = stateAt(row.current, { timed });
    applyEvent(state, row.event);
    const landed = state.itemStateOf("A1");
    assert.ok(
      row.next.includes(landed),
      `${row.current} --${row.event}--> expected one of ${row.next.join("/")}, got ${landed}`,
    );
  }
});

test("every command transition the table does not list is refused with E_FORBIDDEN_STATE", () => {
  const legal = new Set(specRows().map((r) => `${r.current}:${r.event}`));
  const checked: string[] = [];
  for (const current of STATES) {
    for (const event of COMMAND_EVENTS) {
      if (legal.has(`${current}:${event}`)) continue;
      const state = stateAt(current);
      checked.push(`${current}:${event}`);
      assert.throws(
        () => applyEvent(state, event),
        (e: unknown) =>
          e instanceof CpError && e.code === "E_FORBIDDEN_STATE",
        `${current} --${event}--> should be refused with E_FORBIDDEN_STATE`,
      );
      assert.equal(state.itemStateOf("A1"), current, `${current} --${event}--> must not mutate on refusal`);
    }
  }
  // 7 states x 5 events, minus the legal ones: the grid is covered exhaustively.
  assert.equal(checked.length + legal.size, STATES.length * COMMAND_EVENTS.length);
});
