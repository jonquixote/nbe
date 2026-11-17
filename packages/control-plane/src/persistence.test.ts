//! Review4 step 4 (the user's ruling of 2026-11-16 23:45, work order 4): the
//! control plane's journal. The whole state, written atomically with a
//! checksum, the previous one kept; an on-air command written before its
//! response. Restore is unchanged (Rule 10: version and package identity
//! only); it reads the newest journal whose checksum holds.

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { prevPath, readJournal, StatePersistence, type JournalBody } from "./persistence.js";
import { MockRenderBridge } from "./render-bridge.js";
import { CpError } from "./protocol.js";
import { ControlPlaneState } from "./state.js";
import { tempDir } from "./test-tmp.js";

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

function journalAt(path: string): Record<string, unknown> {
  const body = readJournal(path) as JournalBody | null;
  assert.ok(body && "journal" in body, `a verified journal at ${path}`);
  return body.journal;
}

test("an on-air command is in the journal when its commit settles", async () => {
  const dir = tempDir("nbe-journal-");
  const path = join(dir, "control-plane-state.json");
  const state = new ControlPlaneState();
  const p = new StatePersistence(state, path);
  state.viewItem = "A2";
  state.itemStates.set("A2", "LIVE");
  state.tickerItems = [{ text: "BREAKING", priority: 5, ttlSec: 30 }];
  state.bump();
  // The write runs off the event loop: other work proceeds while it is on
  // its way to disk (a synchronous write held every connection ~40 ms).
  let loopRan = false;
  setImmediate(() => {
    loopRan = true;
  });
  await p.onCommitted("view.take");
  assert.ok(loopRan, "the event loop ran while the journal was written");
  const j = journalAt(path);
  assert.equal(j["viewItem"], "A2");
  assert.deepEqual(j["itemStates"], { A2: "LIVE" });
  assert.deepEqual(j["tickerItems"], [{ text: "BREAKING", priority: 5, ttlSec: 30 }], "the ticker keeps its TTL");
});

test("anything else is written within the 50 ms debounce, not before the response", async () => {
  const dir = tempDir("nbe-journal-");
  const path = join(dir, "control-plane-state.json");
  const state = new ControlPlaneState();
  const p = new StatePersistence(state, path);
  state.automationHold = true;
  state.bump();
  p.onCommitted("automation.hold");
  assert.equal(existsSync(path), false, "not written synchronously");
  await sleep(120);
  assert.equal(journalAt(path)["automationHold"], true, "written within the debounce");
});

test("a corrupted journal falls back to the previous one; a parseable but altered one too", async () => {
  const dir = tempDir("nbe-journal-");
  const path = join(dir, "control-plane-state.json");
  const state = new ControlPlaneState();
  const p = new StatePersistence(state, path);
  state.stateVersion = 5;
  await p.onCommitted("view.take");
  state.stateVersion = 6;
  await p.onCommitted("view.take");
  assert.equal((readJournal(prevPath(path)) as JournalBody).stateVersion, 5, "the previous journal is kept");

  // Torn: the newest file cut in half.
  const whole = readFileSync(path, "utf8");
  writeFileSync(path, whole.slice(0, Math.floor(whole.length / 2)));
  const a = new ControlPlaneState();
  new StatePersistence(a, path).restore();
  assert.equal(a.stateVersion, 5, "a torn newest journal is skipped");

  // Altered: valid JSON whose body no longer matches its checksum.
  const altered = JSON.parse(whole) as { body: { stateVersion: number } };
  altered.body.stateVersion = 99;
  writeFileSync(path, JSON.stringify(altered));
  const b = new ControlPlaneState();
  const rb = new StatePersistence(b, path);
  rb.restore();
  assert.equal(b.stateVersion, 5, "a body that fails its checksum is skipped");
  assert.equal(rb.restoredFrom, "previous");

  // Intact: the newest is read.
  writeFileSync(path, whole);
  const c = new ControlPlaneState();
  const rc = new StatePersistence(c, path);
  rc.restore();
  assert.equal(c.stateVersion, 6);
  assert.equal(rc.restoredFrom, "current");
  assert.equal(c.showState, "UNLOADED", "Rule 10: never RUNNING from the file alone");
});

test("a write cut off before its rename leaves the last good journal readable", async () => {
  const dir = tempDir("nbe-journal-");
  const path = join(dir, "control-plane-state.json");
  const state = new ControlPlaneState();
  const p = new StatePersistence(state, path);
  state.stateVersion = 8;
  await p.onCommitted("view.take");
  // A crash mid-write: a partial temp file, never renamed.
  writeFileSync(path + ".tmp", '{"schema":2,"checksum":"ab');
  const a = new ControlPlaneState();
  new StatePersistence(a, path).restore();
  assert.equal(a.stateVersion, 8);
});

test("the pre-journal file (version, identity, time) still restores", () => {
  const dir = tempDir("nbe-journal-");
  const path = join(dir, "control-plane-state.json");
  mkdirSync(dir, { recursive: true });
  writeFileSync(path, JSON.stringify({ stateVersion: 41, manifestIdentity: null, recoveredAt: 1 }));
  const a = new ControlPlaneState();
  new StatePersistence(a, path).restore();
  assert.equal(a.stateVersion, 41);
});

// The command path: a real dispatch of view.take with the real journal.
function makePackage(): string {
  const dir = tempDir("nbe-journal-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47]));
  writeFileSync(join(dir, "media", "A1.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47]));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.3",
      network: { id: "nbe", name: "Test" },
      show: {
        id: "show-1",
        title: "Test",
        video: { width: 1920, height: 1080, frameRate: 30, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      assets: [
        { id: "fallback", kind: "image", source: "media/fallback.png" },
        { id: "A1_clip", kind: "image", source: "media/A1.png" },
      ],
      scenes: [{ id: "SCN_A1", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_clip" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN_A1" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

test("dispatch: view.take's response comes after the journal holds the take", async (t) => {
  const dir = tempDir("nbe-journal-");
  const path = join(dir, "control-plane-state.json");
  const state = new ControlPlaneState();
  const persistence = new StatePersistence(state, path);
  const deps: DispatchDeps = { state, bridge: new MockRenderBridge(), persistence };
  const registry = buildRegistry(deps);
  const run = (command: string, payload: Record<string, unknown> = {}) =>
    dispatch(deps, registry, {
      connectionId: "test",
      role: "admin" as never,
      envelope: { v: "0.3" as const, id: randomUUID(), command, payload },
    });
  try {
    await run("show.load", { packagePath: makePackage() });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await run("preview.set", { itemRef: "A1" });
  const started = performance.now();
  const r = await run("view.take", {});
  const tookMs = performance.now() - started;
  // Read at once, before any timer can run.
  const j = journalAt(path);
  console.log(`journal: view.take answered in ${tookMs.toFixed(2)} ms with the take on disk (stateVersion ${r.stateVersion})`);
  assert.equal(j["viewItem"], "A1", "the journal on disk holds the take when the response is returned");
  assert.equal((readJournal(path) as JournalBody).stateVersion, r.stateVersion);
});
