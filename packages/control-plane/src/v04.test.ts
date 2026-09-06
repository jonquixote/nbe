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
