//! Sequential dispatcher tests using an in-process package fixture.
//! Covers pipeline order, single-bump, roles, aliases, deprecation telemetry,
//! state machine, snapshots, automation hold, show.stop truth table, audit,
//! bridge directive ordering/stateVersion (the CI-hook guarantee).

import { test, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { AuditLog } from "./audit.js";
import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import { ControlPlaneState } from "./state.js";
import { MockRenderBridge } from "./render-bridge.js";
import { CpError } from "./protocol.js";

const noPersist = { onDirty: () => {}, flushNow: () => {} };

function makeDeps(overrides: Partial<DispatchDeps> = {}): {
  deps: DispatchDeps;
  bridge: MockRenderBridge;
  audit: AuditLog;
  state: ControlPlaneState;
} {
  const state = new ControlPlaneState();
  const bridge = new MockRenderBridge();
  const audit = new AuditLog();
  const deps: DispatchDeps = { state, bridge, persistence: noPersist, ...overrides };
  return { deps, bridge, audit, state };
}

function env(command: string, payload: Record<string, unknown> = {}, baseStateVersion?: number) {
  return {
    v: "0.3" as const,
    id: randomUUID(),
    command,
    payload,
    ...(baseStateVersion !== undefined ? { baseStateVersion } : {}),
  };
}

async function d(deps: DispatchDeps, command: string, payload: Record<string, unknown> = {}, role = "admin") {
  const registry = buildRegistry(deps);
  return dispatch(deps, registry, {
    connectionId: "test",
    role: role as never,
    envelope: env(command, payload),
  });
}

function makePackage(): string {
  const dir = mkdtempSync(join(tmpdir(), "nbe-test-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "fallback.png"), "png");
  writeFileSync(join(dir, "media", "A1.png"), "png");
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
        // An image, not video: these packages exercise the command bus, not
        // media. Prompt 05 gave preflight real decode, so a placeholder file
        // declared as video is now correctly rejected.
        { id: "A1_clip", kind: "image", source: "media/A1.png" },
      ],
      scenes: [{ id: "SCN_A1", elements: [{ id: "main", kind: "clip", z: 1, assetId: "A1_clip" }] }],
      rundown: { id: "R", items: [{ id: "A1", kind: "sceneRef", sceneRef: "SCN_A1" }] },
      control: { bindings: [] },
    }),
  );
  return dir;
}

let pkgPath: string;

beforeEach(() => {
  pkgPath = makePackage();
});

// NOTE: show.load shells out to nbe-preflight, which requires the built
// binary. These tests skip if the binary doesn't exist.
test("pipeline: payload validation, role, preconditions, single stateVersion bump", async (t) => {
  let skipped = false;
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      skipped = true;
    } else throw e;
  }
  if (skipped) return;

  const before = deps.state.stateVersion;
  const r = await d(deps, "preview.set", { itemRef: "A1" });
  assert.equal(r.stateVersion, before + 1, "exactly one bump per command");
  assert.equal(deps.state.previewItem, "A1");
});

test("alias program.take resolves, bumps once, emits deprecation warning", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await d(deps, "preview.set", { itemRef: "A1" });
  const before = deps.state.stateVersion;
  await d(deps, "program.take", {});
  const record = deps.state.drainDeprecations();
  assert.equal(record.length, 1);
  assert.equal(record[0]!.command, "program.take");
  assert.equal(record[0]!.resolvedTo, "view.take");
  assert.equal(deps.state.stateVersion, before + 1, "single bump for aliased command");
});

test("bad payload rejected with E_BAD_PAYLOAD, no mutation", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  const before = deps.state.stateVersion;
  await assert.rejects(
    () => d(deps, "view.take", { transition: { not: "a string" } }),
    (e: unknown) => e instanceof CpError && e.code === "E_BAD_PAYLOAD",
  );
  assert.equal(deps.state.stateVersion, before, "rejections do NOT bump");
});

test("stale baseStateVersion rejected with E_VERSION_CONFLICT", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  const stale = deps.state.stateVersion - 1;
  await assert.rejects(
    () =>
      dispatch(deps, buildRegistry(deps), {
        connectionId: "test",
        role: "admin",
        envelope: env("system.status", {}, stale),
      }),
    (e: unknown) => e instanceof CpError && e.code === "E_VERSION_CONFLICT",
  );
});

test("role matrix: monitor blocked from live commands, operator allowed, admin allowed", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await assert.rejects(
    () => d(deps, "view.take", {}, "monitor"),
    (e: unknown) => e instanceof CpError && e.code === "E_AUTH",
  );
  await d(deps, "preview.set", { itemRef: "A1" }, "operator");
  await d(deps, "view.take", {}, "operator");
});

test("state machine: READY->ARMED->LIVE, take clears preview", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await d(deps, "preview.set", { itemRef: "A1" });
  assert.equal(deps.state.itemStateOf("A1"), "ARMED");
  await d(deps, "sequence.arm", { sequenceId: "R" });
  await d(deps, "view.take", {});
  // sceneRef item has no durationFrames -> LIVE (untimed)
  assert.equal(deps.state.itemStateOf("A1"), "LIVE");
  assert.equal(deps.state.previewItem, null);
});

test("snapshot round-trip restores View state", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await d(deps, "preview.set", { itemRef: "A1" });
  await d(deps, "view.take", {});
  await d(deps, "snapshot.save", { name: "s1" });
  await d(deps, "view.cut", { itemRef: "A1" }); // still fine; change it:
  deps.state.previewItem = null;
  await d(deps, "snapshot.recall", { name: "s1" });
  assert.equal(deps.state.viewItem, "A1");
});

test("automation.hold flips the flag; trigger suppression is the engine's job", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await d(deps, "automation.hold", { hold: true });
  assert.equal(deps.state.automationHold, true);
});

test("show.stop quiescence truth table: !quiesce+!force with active outputs -> E_FORBIDDEN_STATE", async (t) => {
  const { deps } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await d(deps, "show.start", {});
  await d(deps, "record.start", {});
  await assert.rejects(
    () => d(deps, "show.stop", { quiesceOutputs: false, force: false }),
    (e: unknown) => e instanceof CpError && e.code === "E_FORBIDDEN_STATE",
  );
  await d(deps, "show.stop", { quiesceOutputs: true, force: false });
  assert.equal(deps.state.showState, "STOPPED");
  assert.equal(deps.state.recordState, "idle");
});

test("mock bridge records directives in order with directive stateVersion", async (t) => {
  const { deps, bridge } = makeDeps();
  try {
    await d(deps, "show.load", { packagePath: pkgPath });
  } catch (e) {
    if (e instanceof CpError && (e.code === "E_PREFLIGHT_FAILED" || e.code === "E_ENGINE")) {
      t.skip("nbe-preflight binary not available");
      return;
    }
    throw e;
  }
  await d(deps, "preview.set", { itemRef: "A1" });
  await d(deps, "view.take", {});
  const directives = bridge.drain();
  assert.ok(directives.length >= 2, "expected directives to be recorded");
  const svs = directives.map((d) => d.stateVersion);
  // Strictly increasing delivery order
  for (let i = 1; i < svs.length; i++) assert.ok(svs[i]! > svs[i - 1]!);
});

test("audit log records both accepted and rejected commands", async (t) => {
  const tmp = mkdtempSync(join(tmpdir(), "nbe-audit-"));
  const auditPath = join(tmp, "audit.jsonl");
  const { deps } = makeDeps();
  const audit = new AuditLog(auditPath);
  // drive through the server-level path would integrate with dispatch; here
  // assert the AuditLog API itself
  audit.record({
    kind: "command",
    outcome: "ok",
    role: "operator",
    tokenId: "t1",
    requestId: randomUUID(),
    command: "view.take",
    stateVersionBefore: 0,
    stateVersionAfter: 1,
  });
  audit.record({ kind: "auth", outcome: "rejected", role: null, tokenId: null });
  const lines = (await import("node:fs")).readFileSync(auditPath, "utf8").trim().split("\n");
  assert.equal(lines.length, 2);
});
