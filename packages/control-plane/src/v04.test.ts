//! SPEC v0.4 wire additions (Standards §2a: each fails when removed).
//!
//! §5.9.4 `viewItemStartFrame` in the resync snapshot; §10.1 `showState` in
//! the telemetry tick; §7.15 `show.load` rejecting a house-rate mismatch.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ControlPlaneState, type PackageInfo } from "./state.js";
import { buildTick, newWorldTelemetry } from "./telemetry.js";

function pkg(houseRate = 30): PackageInfo {
  return {
    packagePath: "/tmp/none",
    showId: "s",
    houseRate,
    qualityProfile: undefined,
    items: new Map([["A1", { id: "A1", kind: "sceneRef" }]]),
    sequences: new Set(["R"]),
    scenes: new Map(),
    elements: new Map(),
    overlays: new Set(),
    templates: new Set(),
    breakingTemplates: new Set(),
    tickerExists: false,
    clockElements: new Set(),
    plugins: new Set(),
    audioAssets: new Set(),
    cameras: new Set(),
    guests: new Set(),
    fallbackAssetId: undefined,
    automationRules: [],
  } as unknown as PackageInfo;
}

test("§5.9.4: the resync snapshot carries viewItemStartFrame", () => {
  const state = new ControlPlaneState();
  state.loadPackage(pkg());
  state.showState = "RUNNING";

  // Nothing on air: the field must be null, not absent and not zero. A
  // consumer cannot tell "absent" from "frame 0" otherwise.
  const empty = state.resyncSnapshot();
  assert.ok("viewItemStartFrame" in empty, "the field must always be present");
  assert.equal(empty.viewItemStartFrame, null);

  // The engine's clock is the only clock; the control plane records what it
  // last reported, which is at worst one telemetry tick stale.
  state.lastKnownMasterFrame = 1234;
  state.armItem("A1");
  state.take("A1");

  const snap = state.resyncSnapshot();
  assert.equal(snap.viewItem, "A1");
  assert.equal(
    snap.viewItemStartFrame,
    1234,
    "the snapshot must say SINCE WHEN the item is on air, not only what is",
  );
});

test("§10.1: the telemetry tick carries showState", () => {
  const state = new ControlPlaneState();
  state.loadPackage(pkg());

  const world = newWorldTelemetry();
  const loaded = buildTick(state, world, Date.now());
  assert.equal(
    loaded.showState,
    "LOADED",
    "a client holding telemetry alone must be able to say whether the show is running",
  );

  state.showState = "RUNNING";
  assert.equal(buildTick(state, world, Date.now()).showState, "RUNNING");
});

test("§7.15: show.load rejects a house-rate mismatch, and the check is reachable", async () => {
  // The guard existed and was UNREACHABLE: `DispatchDeps.houseRate` was
  // optional and nothing ever assigned it — not `ServerOptions`, not the
  // production entry point — so `engineRate` was always undefined and the
  // condition never fired. Deleting the guard left the suite green.
  //
  // This drives the dispatcher the way the server does, with the rate set.
  const { buildRegistry, dispatch } = await import("./dispatch.js");
  const { AuditLog } = await import("./audit.js");
  const { mkdtempSync, mkdirSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { randomUUID } = await import("node:crypto");

  const dir = mkdtempSync(join(tmpdir(), "nbe-hr-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  // A real 1x1 PNG: preflight reads image headers now, and the point of this
  // test is the house-rate check, not an unreadable asset.
  writeFileSync(
    join(dir, "media", "slate.png"),
    Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
      "base64",
    ),
  );
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "T" },
      show: {
        id: "s",
        title: "T",
        // 60 fps against a 30 fps engine: schema-legal, and every asset would
        // be mapped against the wrong denominator.
        video: { width: 1920, height: 1080, frameRate: 60, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "slate",
      },
      control: { bindings: [] },
      assets: [{ id: "slate", kind: "image", source: "media/slate.png", format: "png" }],
      scenes: [{ id: "SCN", elements: [{ id: "bg", kind: "graphic", z: 0, templateId: "TPL" }] }],
      templates: [{ id: "TPL", kind: "generic" }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN" }] },
    }),
  );

  const state = new ControlPlaneState();
  const tmp = mkdtempSync(join(tmpdir(), "nbe-hr-audit-"));
  const deps = {
    state,
    bridge: { send: () => {}, droppedCount: () => 0, pending: () => 0 },
    persistence: { onDirty: () => {}, flushNow: () => {} },
    houseRate: 30,
    warn: () => {},
    audit: new AuditLog(join(tmp, "a.jsonl")),
  } as unknown as Parameters<typeof buildRegistry>[0];

  const registry = buildRegistry(deps);
  await assert.rejects(
    () =>
      dispatch(deps, registry, {
        connectionId: "c1",
        role: "admin",
        envelope: {
          v: "0.3",
          id: randomUUID(),
          command: "show.load",
          payload: { packagePath: dir },
        },
      }),
    (e: unknown) => {
      const err = e as { code?: string; message?: string };
      assert.equal(err.code, "E_PREFLIGHT_FAILED");
      assert.match(String(err.message), /60 fps.*30 fps|houseRate/);
      return true;
    },
    "a 60 fps package on a 30 fps engine must be REJECTED, not loaded and warned about",
  );
  assert.equal(state.pkg, null, "a rejected package must not be loaded");
});

test("§7.15: parseHouseRate mirrors the engine's parse, including its refusals", async () => {
  const { parseHouseRate } = await import("./index.js");

  // The bug: `Number(env ?? 30)` is NaN for a non-numeric value, and every
  // comparison against NaN is false — so "abc" did not fall back to 30, it
  // made `declared !== engineRate` true for EVERY package, including a
  // matching 30 fps one, and rejected all of them with "runs at NaN fps".
  assert.equal(parseHouseRate("abc"), 30, "a non-numeric value falls back, it does not poison");
  assert.equal(parseHouseRate(""), 30);
  assert.equal(parseHouseRate(undefined), 30);

  // The engine parses `u32`: digits and an optional `+`, nothing else. Any
  // looser grammar here (`Number.parseInt` stops at the first non-digit) makes
  // the two sides disagree about the rate they exist to reconcile.
  assert.equal(parseHouseRate("60abc"), 30, "the engine's parse fails here, so this one must too");
  assert.equal(parseHouseRate("29.97"), 30, "u32 has no decimals");
  assert.equal(parseHouseRate(" 30"), 30, "u32 does not skip whitespace");
  assert.equal(parseHouseRate("-25"), 30);
  assert.equal(parseHouseRate("0x1E"), 30);
  assert.equal(parseHouseRate("99999999999999999999"), 30, "past u32, the engine's parse fails");

  // And it still reads the rates an operator actually sets.
  assert.equal(parseHouseRate("25"), 25);
  assert.equal(parseHouseRate("60"), 60);
  assert.equal(parseHouseRate("+50"), 50, "u32::from_str accepts a leading +");
});

test("§7.15: the rejection is reachable through the SERVER, not only the dispatcher", async () => {
  // The dispatcher test above proves the guard fires when `houseRate` is set.
  // It cannot prove anything about production, where the value arrives through
  // `createControlPlaneServer`: the previous defect was precisely that nothing
  // ever assigned the field, and a dispatcher-level test stayed green through
  // it. This drives the real server over a real socket.
  const WebSocket = (await import("ws")).default;
  const { createControlPlaneServer } = await import("./server.js");
  const { AuditLog } = await import("./audit.js");
  const { mkdtempSync, mkdirSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { randomUUID } = await import("node:crypto");

  const dir = mkdtempSync(join(tmpdir(), "nbe-hr-srv-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(
    join(dir, "media", "slate.png"),
    Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
      "base64",
    ),
  );
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "T" },
      show: {
        id: "s",
        title: "T",
        video: { width: 1920, height: 1080, frameRate: 60, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "slate",
      },
      control: { bindings: [] },
      assets: [{ id: "slate", kind: "image", source: "media/slate.png", format: "png" }],
      scenes: [{ id: "SCN", elements: [{ id: "bg", kind: "graphic", z: 0, templateId: "TPL" }] }],
      templates: [{ id: "TPL", kind: "generic" }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN" }] },
    }),
  );

  const state = new ControlPlaneState();
  const tmp = mkdtempSync(join(tmpdir(), "nbe-hr-srv-audit-"));
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { "hr-token": "admin" } },
    audit: new AuditLog(join(tmp, "audit.jsonl")),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    houseRate: 30,
  });

  try {
    const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: "Bearer hr-token", "x-nbe-role": "admin" },
    });
    await new Promise<void>((resolve, reject) => {
      ws.once("open", () => resolve());
      ws.once("error", reject);
    });
    const resp = await new Promise<Record<string, unknown>>((resolve) => {
      const onMsg = (buf: Buffer) => {
        const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
        if (msg.kind !== "telemetry" && msg.kind !== "stateChange") {
          ws.off("message", onMsg);
          resolve(msg);
        }
      };
      ws.on("message", onMsg);
      ws.send(
        JSON.stringify({ v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: dir } }),
      );
    });
    ws.close();

    assert.equal(resp.status, "error", "a 60 fps package on a 30 fps engine must be refused");
    const error = resp.error as { code?: string; message?: string } | undefined;
    assert.equal(error?.code, "E_PREFLIGHT_FAILED");
    assert.match(String(error?.message), /60 fps.*30 fps|houseRate/);
    assert.equal(state.pkg, null, "a refused package must not be loaded");
  } finally {
    await server.close();
  }
});

test("the recovery record carries the LOADED package's manifest version, not a constant", async () => {
  // `manifestIdentity()` hardcoded `manifestVersion: "0.3"`, so every v0.4
  // package was recorded — and reported — as v0.3. The identity block is what
  // an operator and a crash recovery both read to answer "what is actually
  // loaded", and a constant there is a lie with an audience.
  const { StatePersistence } = await import("./persistence.js");
  const { mkdtempSync, readFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");

  const state = new ControlPlaneState();
  state.loadPackage({ ...pkg(), manifestVersion: "0.4" });
  assert.equal(state.manifestIdentity()?.manifestVersion, "0.4");

  const file = join(mkdtempSync(join(tmpdir(), "nbe-ident-")), "state.json");
  const persistence = new StatePersistence(state, file);
  persistence.onDirty();
  persistence.flushNow();
  const snapshot = JSON.parse(readFileSync(file, "utf8")) as {
    manifestIdentity: { manifestVersion?: string } | null;
  };
  assert.equal(
    snapshot.manifestIdentity?.manifestVersion,
    "0.4",
    "the persisted identity must name the version that was actually loaded",
  );

  // And it tracks the package, rather than tracking the newest version.
  state.loadPackage({ ...pkg(), manifestVersion: "0.3" });
  assert.equal(state.manifestIdentity()?.manifestVersion, "0.3");
});
