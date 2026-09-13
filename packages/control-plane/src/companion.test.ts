//! Companion + keyboard adapter tests (Prompt 08, work items 2-4+6, WS-only):
//! AC-12 over the WS bus, adapter identity via intentSource, deterministic
//! deck generation, default deck, WS auth before state.

import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import WebSocket from "ws";

import { AuditLog, type AuditRecord } from "./audit.js";
import { buildRegistry, type DispatchDeps } from "./dispatch.js";
import {
  companionIntentSource,
  deckToJson,
  defaultDeck,
  findCompanionEntry,
  fireCompanionButton,
  generateDeck,
  type Deck,
} from "./companion.js";
import { intentToEnvelope, profileFromBindings, resolveIntent } from "./intent.js";
import { fireKeyboardChord, findKeyboardEntry, keyboardIntentSource } from "./keyboard.js";
import type { ControlBinding } from "./generated/manifest-schema.js";
import { MockRenderBridge } from "./render-bridge.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";

const TOKEN = "op-token-1";
const noPersist = { onDirty: () => {}, flushNow: () => {} };

function makeDeps(): { deps: DispatchDeps; state: ControlPlaneState } {
  const state = new ControlPlaneState();
  const deps: DispatchDeps = { state, bridge: new MockRenderBridge(), persistence: noPersist };
  return { deps, state };
}

function makePackage(): string {
  const dir = mkdtempSync(join(tmpdir(), "nbe-p08-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
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

function connect(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
    ws.once("unexpected-response", (_req, res) => reject(new Error(`unexpected-response ${res.statusCode}`)));
  });
}

/** Resolve with the next command response (never stateChange/telemetry). */
function send(ws: WebSocket, msg: unknown): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    const onMsg = (buf: Buffer) => {
      const m = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (m.kind !== "telemetry" && m.kind !== "stateChange") {
        ws.off("message", onMsg);
        resolve(m);
      }
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify(msg));
  });
}

function waitForStateChange(
  ws: WebSocket,
  stateVersion: number,
  timeoutMs = 2000,
): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.off("message", onMsg);
      reject(new Error(`stateChange ${stateVersion} never arrived`));
    }, timeoutMs);
    const onMsg = (buf: Buffer) => {
      const m = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (m.kind === "stateChange" && m.stateVersion === stateVersion) {
        clearTimeout(timer);
        ws.off("message", onMsg);
        resolve(m);
      }
    };
    ws.on("message", onMsg);
  });
}

let server: ControlPlaneServer;
let state: ControlPlaneState;
let auditRecords: AuditRecord[];

beforeEach(async () => {
  state = new ControlPlaneState();
  const tmp = mkdtempSync(join(tmpdir(), "nbe-p08-audit-"));
  const audit = new AuditLog(join(tmp, "audit.jsonl"));
  auditRecords = [];
  const orig = audit.record.bind(audit);
  audit.record = (rec) => {
    const full = orig(rec);
    auditRecords.push(full);
    return full;
  };
  // afterEach owns server close; closing here too double-closes.
  server = await createControlPlaneServer({    port: 0,
    auth: { tokens: { [TOKEN]: "admin" } },
    audit,
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });
});

afterEach(async () => {
  if (server) await server.close();
});

function adminWs(): WebSocket {
  return new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${TOKEN}`, "x-nbe-role": "admin" },
  });
}

test("AC-12 WS: companion button view.take -> ok + bump + stateChange, no plugin", async () => {
  const ws = adminWs();
  await connect(ws);
  const pkgPath = makePackage();
  try {
    let r = await send(ws, { v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: pkgPath } });
    assert.equal(r.status, "ok");
    r = await send(ws, { v: "0.3", id: randomUUID(), command: "preview.set", payload: { itemRef: "A1" } });
    assert.equal(r.status, "ok");
    const before = r.stateVersion as number;

    // Companion profile built from manifest-style bindings; button press
    // becomes a §5.4 envelope — the only thing the bus ever sees.
    const profile = profileFromBindings("xl-a", "companion", [
      {
        id: "take-1",
        trigger: { kind: "companionKey", page: 1, bank: 1, key: "0" },
        action: "view.take",
        payload: {},
      },
    ]);
    const button = { page: 1, bank: 1, key: "0" };
    const entry = findCompanionEntry(profile, button);
    assert.ok(entry, "button must resolve to an intent");
    const envelope = intentToEnvelope(resolveIntent(entry));
    const intentSource = companionIntentSource("xl-a", "take-1");
    assert.equal(intentSource, "companion/xl-a:take-1");

    const watch = waitForStateChange(ws, before + 1);
    const take = await send(ws, { ...envelope, intentSource });
    assert.equal(take.status, "ok");
    assert.equal(take.stateVersion, before + 1, "exactly one bump for the intent command");
    assert.equal(state.viewItem, "A1");

    const frame = await watch;
    assert.deepEqual(frame.changed, ["view.take"]);

    const row = auditRecords.find((a) => a.requestId === envelope.id);
    assert.ok(row, "command must be audited");
    assert.equal(row.outcome, "ok");
    assert.equal(row.command, "view.take");
    assert.equal(row.intentSource, intentSource);

    // No plugin anywhere in this path: package declares none, none ran.
    assert.equal(state.pkg?.plugins.size ?? 0, 0);
    assert.ok(!auditRecords.some((a) => a.command === "plugin.reload"));
  } finally {
    ws.close();
  }
});

test("keyboard same command is identical except intentSource", async () => {
  const bindings: ControlBinding[] = [
    {
      id: "fb-c",
      trigger: { kind: "companionKey", page: 1, bank: 1, key: "5" },
      action: "view.fallback",
      payload: {},
    },
  ];
  const cProfile = profileFromBindings("xl-a", "companion", bindings);
  const kProfile = profileFromBindings("kb-1", "keyboard", [
    { id: "fb-k", trigger: { kind: "hotkey", key: "ctrl+shift+f" }, action: "view.fallback", payload: {} },
  ]);
  assert.ok(findKeyboardEntry(kProfile, "ctrl+shift+f"), "chord must resolve to an intent");

  const a = makeDeps();
  const b = makeDeps();
  const firedC = await fireCompanionButton(a.deps, buildRegistry(a.deps), {
    profile: cProfile,
    button: { page: 1, bank: 1, key: "5" },
  });
  const firedK = await fireKeyboardChord(b.deps, buildRegistry(b.deps), {
    profile: kProfile,
    chord: "ctrl+shift+f",
  });
  assert.equal(firedC.envelope.command, firedK.envelope.command);
  assert.deepEqual(firedC.envelope.payload, firedK.envelope.payload);
  assert.deepEqual(firedC.result.data, firedK.result.data);
  assert.notEqual(firedC.intentSource, firedK.intentSource);
  assert.equal(firedC.intentSource, "companion/xl-a:fb-c");
  assert.equal(firedK.intentSource, keyboardIntentSource("kb-1", "fb-k"));
  assert.ok(firedK.intentSource.startsWith("keyboard/"));
  // Neither adapter touches state directly: dispatch() did the single bump each.
  assert.equal(a.state.stateVersion, 1);
  assert.equal(b.state.stateVersion, 1);
  // Unknown chord rejected, nothing mutated.
  await assert.rejects(
    fireKeyboardChord(b.deps, buildRegistry(b.deps), { profile: kProfile, chord: "nope" }),
    /no intent for keyboard chord/,
  );
  assert.equal(b.state.stateVersion, 1);
});

test("matcher: most-specific wins, profile order breaks ties", () => {
  const profile = profileFromBindings("xl-a", "companion", [
    { id: "wide", trigger: { kind: "companionKey", key: "9" }, action: "view.fallback", payload: {} },
    { id: "narrow", trigger: { kind: "companionKey", page: 1, bank: 1, key: "9" }, action: "view.take", payload: {} },
  ]);
  const hit = findCompanionEntry(profile, { page: 1, bank: 1, key: "9" });
  assert.ok(hit);
  assert.equal(hit.intentId, "narrow");
  const loose = findCompanionEntry(profile, { page: 7, bank: 7, key: "9" });
  assert.ok(loose);
  assert.equal(loose.intentId, "wide");
});

test("matcher: identical specificity resolves to first profile entry", () => {
  const profile = profileFromBindings("xl-a", "companion", [
    { id: "first", trigger: { kind: "companionKey", page: 1, bank: 1, key: "9" }, action: "view.take", payload: {} },
    { id: "second", trigger: { kind: "companionKey", page: 1, bank: 1, key: "9" }, action: "view.fallback", payload: {} },
  ]);
  const hit = findCompanionEntry(profile, { page: 1, bank: 1, key: "9" });
  assert.ok(hit);
  assert.equal(hit.intentId, "first");
});

test("keyboard matching is case- and whitespace-insensitive", () => {
  const profile = profileFromBindings("kb-1", "keyboard", [
    { id: "fb-k", trigger: { kind: "hotkey", key: "ctrl+shift+f" }, action: "view.fallback", payload: {} },
  ]);
  for (const variant of ["CTRL+SHIFT+F", "  ctrl+shift+f  ", "Ctrl+Shift+F"]) {
    const hit = findKeyboardEntry(profile, variant);
    assert.ok(hit, `variant must resolve: ${variant}`);
    assert.equal(hit.intentId, "fb-k");
  }
});

test("deck generation is deterministic: byte-identical x2, covers every binding", () => {
  const bindings: ControlBinding[] = [
    { id: "z-last", trigger: { kind: "companionKey", page: 2, bank: 1, key: "3" }, action: "view.fallback", payload: {} },
    { id: "a-first", trigger: { kind: "hotkey", key: "t" }, action: "view.take", payload: {} },
    { id: "m-mid", action: "breaking.hide", payload: {} },
  ];
  const j1 = deckToJson(generateDeck(bindings));
  const j2 = deckToJson(generateDeck(bindings));
  assert.equal(j1, j2, "generation must be byte-identical across runs");
  const deck = JSON.parse(j1) as Deck;
  const ids = new Set<string>();
  for (const p of deck.pages) for (const b of p.banks) for (const btn of b.buttons) {
    if (btn.bindingId) ids.add(btn.bindingId);
  }
  for (const b of bindings) {
    if (b.trigger) assert.ok(ids.has(b.id), `binding must be covered: ${b.id}`);
    else assert.ok(!ids.has(b.id), `triggerless binding must emit no button: ${b.id}`);
  }
});

test("default deck with empty bindings: TAKE + fallback, schema-valid, no collision", () => {
  const deck = generateDeck([]);
  assert.deepEqual(defaultDeck(), deck, "defaultDeck() is generateDeck([])");
  const buttons = deck.pages.flatMap((p) => p.banks.flatMap((b) => b.buttons));
  const commands = buttons.map((b) => b.command);
  assert.ok(commands.includes("view.take"), "default deck must have TAKE");
  assert.ok(commands.includes("view.fallback"), "default deck must have fallback");
  // Every default button is drivable at the schema level.
  for (const b of buttons) resolveIntent({ action: b.command, payload: b.payload });
  // No key collision anywhere in the deck.
  const coords = buttons.map((b) => `${b.page}:${b.bank}:${b.key}`);
  assert.equal(new Set(coords).size, coords.length, "no two buttons share a key");

  // A manifest key on the default page never gets overwritten: the manifest
  // button keeps its coordinates, defaults move to a fresh page.
  const clash: ControlBinding[] = [
    { id: "take-1", trigger: { kind: "companionKey", page: 0, bank: 1, key: "take" }, action: "view.take", payload: {} },
  ];
  const deck2 = generateDeck(clash);
  const all2 = deck2.pages.flatMap((p) => p.banks.flatMap((b) => b.buttons));
  const kept = all2.find((b) => b.bindingId === "take-1");
  assert.ok(kept, "manifest binding must survive");
  assert.equal(kept.page, 0);
  assert.equal(kept.bank, 1);
  assert.equal(kept.key, "take");
  const coords2 = all2.map((b) => `${b.page}:${b.bank}:${b.key}`);
  assert.equal(new Set(coords2).size, coords2.length, "defaults must not overwrite manifest keys");
  assert.ok(all2.some((b) => b.command === "view.take" && b.bindingId === null), "defaults still present");
});

test("E_AUTH before state on bad token; token never logged", async () => {
  const badToken = `bad-token-${randomUUID()}`;
  await assert.rejects(
    connect(
      new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
        headers: { authorization: `Bearer ${badToken}`, "x-nbe-role": "admin" },
      }),
    ),
    /unexpected-response 401/,
  );
  assert.equal(state.stateVersion, 0, "rejected auth must not touch state");
  const authRows = auditRecords.filter((a) => a.kind === "auth");
  assert.ok(authRows.some((a) => a.outcome === "rejected"), "rejection must be audited");
  for (const row of authRows) assert.equal(row.tokenId, null, "never log the token");
  assert.ok(
    !JSON.stringify(auditRecords).includes(badToken),
    "raw token must never appear in the audit log",
  );
});

test("intentSource: invalid rejected, rejected-with-source audited, direct is null", async () => {
  const ws = adminWs();
  await connect(ws);
  try {
    const pkgPath = makePackage();
    const loaded = await send(ws, { v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: pkgPath } });
    assert.equal(loaded.status, "ok");

    // Free-form source is rejected before dispatch: spoofing gets E_BAD_PAYLOAD, no bump.
    const before = state.stateVersion;
    const bad = await send(ws, {
      v: "0.3",
      id: randomUUID(),
      command: "view.fallback",
      payload: {},
      intentSource: "not a valid source!!",
    });
    assert.equal(bad.status, "error");
    assert.equal((bad.error as { code: string }).code, "E_BAD_PAYLOAD");
    assert.equal(state.stateVersion, before);

    // A rejected command carrying a valid source still audits that source.
    const badCmd = await send(ws, {
      v: "0.3",
      id: randomUUID(),
      command: "bogus.cmd",
      payload: {},
      intentSource: "companion/xl-a:take-1",
    });
    assert.equal(badCmd.status, "error");
    const rejRow = auditRecords.find((a) => a.requestId === badCmd.requestId);
    assert.ok(rejRow, "rejection must be audited");
    assert.equal(rejRow.intentSource, "companion/xl-a:take-1");

    // A direct command audits a null source: absent means software/direct.
    const direct = await send(ws, { v: "0.3", id: randomUUID(), command: "view.fallback", payload: {} });
    assert.equal(direct.status, "ok");
    const okRow = auditRecords.find((a) => a.requestId === direct.requestId);
    assert.ok(okRow, "command must be audited");
    assert.equal(okRow.intentSource, null);
  } finally {
    ws.close();
  }
});
