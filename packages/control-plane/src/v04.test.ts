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

test("a wedged preflight fails show.load by name instead of never answering", async () => {
  // The control plane shelled out to `nbe-preflight` with no timeout, no
  // killSignal and no AbortSignal. A binary that never answered left the
  // command accepted and unresolved forever: no §16 response, no terminal state
  // in the audit log, and — because the child process handle keeps Node's event
  // loop open — a test suite that reported its failures and then hung instead of
  // exiting. One missing option caused all three.
  const WebSocket = (await import("ws")).default;
  const { createControlPlaneServer } = await import("./server.js");
  const { AuditLog } = await import("./audit.js");
  const { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { randomUUID } = await import("node:crypto");

  // A binary that exists, starts, and never answers.
  const bin = join(mkdtempSync(join(tmpdir(), "nbe-wedge-")), "wedged-preflight");
  writeFileSync(bin, "#!/bin/sh\nsleep 100000\n", { mode: 0o755 });

  const dir = mkdtempSync(join(tmpdir(), "nbe-wedge-pkg-"));
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "manifest.json"), "{}");

  const state = new ControlPlaneState();
  const auditPath = join(mkdtempSync(join(tmpdir(), "nbe-wedge-audit-")), "audit.jsonl");
  const prevBin = process.env.NBE_PREFLIGHT_BIN;
  const prevTimeout = process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  process.env.NBE_PREFLIGHT_BIN = bin;
  process.env.NBE_PREFLIGHT_TIMEOUT_MS = "1500";

  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { "wedge-token": "admin" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
  });

  try {
    const ws = new WebSocket(`ws://127.0.0.1:${server.port}/nbe/v0.3`, {
      headers: { authorization: "Bearer wedge-token", "x-nbe-role": "admin" },
    });
    await new Promise<void>((resolve, reject) => {
      ws.once("open", () => resolve());
      ws.once("error", reject);
    });
    const started = Date.now();
    const resp = await new Promise<Record<string, unknown>>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("show.load never answered")), 20_000);
      const onMsg = (buf: Buffer) => {
        const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
        if (msg.kind !== "telemetry" && msg.kind !== "stateChange") {
          clearTimeout(timer);
          ws.off("message", onMsg);
          resolve(msg);
        }
      };
      ws.on("message", onMsg);
      ws.send(
        JSON.stringify({ v: "0.3", id: randomUUID(), command: "show.load", payload: { packagePath: dir } }),
      );
    });
    const elapsed = Date.now() - started;
    ws.close();

    // 1. It answers, and it answers near the bound rather than at some
    //    unrelated timeout further out.
    assert.ok(elapsed < 10_000, `show.load must answer near its 1500 ms bound; took ${elapsed} ms`);
    assert.equal(resp.status, "error");
    const error = resp.error as { code?: string; message?: string };
    assert.equal(error.code, "E_PREFLIGHT_FAILED");
    assert.match(
      String(error.message),
      /produced no verdict within 1500 ms/,
      "the failure must name the bound it exceeded, not just fail",
    );
    assert.match(
      String(error.message),
      /NBE_PREFLIGHT_TIMEOUT_MS override/,
      "and where that bound came from, so the operator knows what to change",
    );

    // 2. Nothing is half-loaded.
    assert.equal(state.pkg, null, "a load that never got a verdict must load nothing");
    assert.equal(state.showState, "UNLOADED");

    // 3. The audit log has a terminal state for the command. "Accepted, never
    //    resolved" is exactly what leaves an audit trail with a beginning and
    //    no end.
    assert.ok(existsSync(auditPath), "the audit log exists");
    const records = readFileSync(auditPath, "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((l) => JSON.parse(l) as Record<string, unknown>);
    const loadRec = records.find((r) => r.command === "show.load");
    assert.ok(loadRec, `show.load must be audited; got ${JSON.stringify(records)}`);
    assert.notEqual(loadRec!.outcome, "ok", "a wedged preflight is not an ok outcome");
  } finally {
    await server.close();
    if (prevBin === undefined) delete process.env.NBE_PREFLIGHT_BIN;
    else process.env.NBE_PREFLIGHT_BIN = prevBin;
    if (prevTimeout === undefined) delete process.env.NBE_PREFLIGHT_TIMEOUT_MS;
    else process.env.NBE_PREFLIGHT_TIMEOUT_MS = prevTimeout;
  }
});

test("the preflight bound is derived from the package and the binary that will run it", async () => {
  // The bound was a flat 600 s "far outside any measured run" — sized against a
  // five-second fixture. Measured on this repo's own 1080p fixture, debug
  // preflight costs 130 ms/frame, so 600 s was crossed by 2 min 34 s of
  // footage: ten ordinary clips, refused with "wedged, not slow" when they were
  // exactly slow. A bound that does not scale with the package cannot be right
  // for both a slate and a bulletin.
  const { preflightBound, expectedDecodeFrames, PREFLIGHT_FLOOR_MS } = await import("./package.js");
  const { mkdtempSync, mkdirSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");

  const pkg = (frames: number | null): string => {
    const dir = mkdtempSync(join(tmpdir(), "nbe-bound-"));
    mkdirSync(join(dir, "media"), { recursive: true });
    writeFileSync(
      join(dir, "manifest.json"),
      JSON.stringify({
        assets:
          frames === null
            ? [{ id: "slate", kind: "image", source: "media/s.png" }]
            : [{ id: "clip", kind: "video", source: "media/c.mp4", expectedDurationFrames: frames }],
      }),
    );
    return dir;
  };

  const prevBin = process.env.NBE_PREFLIGHT_BIN;
  const prevTimeout = process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  delete process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  try {
    // The frame count comes from the manifest, and only from decodable kinds.
    assert.equal(expectedDecodeFrames(pkg(4600)), 4600);
    assert.equal(expectedDecodeFrames(pkg(null)), 0, "images are not decoded frame by frame");
    assert.equal(expectedDecodeFrames("/nonexistent"), 0, "an unreadable manifest is a size of 0, not a throw");

    // Release: 25 ms/frame x 3. The package the old constant refused — 4600
    // frames, 2 min 34 s of 1080p — now gets a bound comfortably above the
    // 77 s a release binary actually needs for it.
    process.env.NBE_PREFLIGHT_BIN = join("/somewhere", "target", "release", "nbe-preflight");
    const rel = preflightBound(pkg(4600));
    assert.equal(rel.msPerFrame, 25, "a release binary is measured at 16.7 ms/frame, rounded up");
    assert.equal(rel.ms, 4600 * 25 * 3);
    assert.ok(rel.derived);

    // Debug: 8x the cost, so 8x the budget. A bound sized for release would
    // refuse this package on the very binary a contributor is most likely to
    // have built.
    process.env.NBE_PREFLIGHT_BIN = join("/somewhere", "target", "debug", "nbe-preflight");
    const dbg = preflightBound(pkg(4600));
    assert.equal(dbg.msPerFrame, 200, "a debug binary is measured at 130 ms/frame, rounded up");
    assert.equal(dbg.ms, 4600 * 200 * 3);
    assert.ok(
      dbg.ms > 600_000,
      `the old flat 600 s refused this package; the derived bound must not (got ${dbg.ms} ms)`,
    );

    // Small packages get the floor, not a bound of zero.
    assert.equal(preflightBound(pkg(null)).ms, PREFLIGHT_FLOOR_MS);
    assert.equal(preflightBound(pkg(1)).ms, PREFLIGHT_FLOOR_MS);

    // The operator override still wins, still strictly parsed.
    process.env.NBE_PREFLIGHT_TIMEOUT_MS = "1234";
    assert.equal(preflightBound(pkg(4600)).ms, 1234);
    assert.equal(preflightBound(pkg(4600)).derived, false);
    for (const bad of ["0", "600abc", "-5", "1e4", " 600000", ""]) {
      process.env.NBE_PREFLIGHT_TIMEOUT_MS = bad;
      assert.equal(
        preflightBound(pkg(null)).ms,
        PREFLIGHT_FLOOR_MS,
        `${JSON.stringify(bad)} must not silently disable the bound`,
      );
    }
  } finally {
    if (prevBin === undefined) delete process.env.NBE_PREFLIGHT_BIN;
    else process.env.NBE_PREFLIGHT_BIN = prevBin;
    if (prevTimeout === undefined) delete process.env.NBE_PREFLIGHT_TIMEOUT_MS;
    else process.env.NBE_PREFLIGHT_TIMEOUT_MS = prevTimeout;
  }
});

test("preflightBin prefers the release build, because it is 8x cheaper", async () => {
  // `show.load` shells this binary. Debug decodes at 130 ms/frame and release
  // at 16.7 — the difference between an ordinary rundown preflighting in one
  // minute and in ten, and the root of the rehearsal's 46 s `show.load`
  // complaint. Resolution order is behaviour, not tidiness.
  const { preflightBin } = await import("./package.js");
  const { mkdtempSync, mkdirSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");

  const root = mkdtempSync(join(tmpdir(), "nbe-bin-"));
  for (const profile of ["debug", "release"]) {
    mkdirSync(join(root, "target", profile), { recursive: true });
    writeFileSync(join(root, "target", profile, "nbe-preflight"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  }
  const prevBin = process.env.NBE_PREFLIGHT_BIN;
  const prevCwd = process.cwd();
  delete process.env.NBE_PREFLIGHT_BIN;
  try {
    process.chdir(root);
    const resolved = preflightBin();
    assert.ok(
      resolved.endsWith(join("target", "release", "nbe-preflight")),
      `with both present, release wins; resolved ${resolved}`,
    );
    assert.ok(!resolved.includes(join("target", "debug")), "and debug does not");
  } finally {
    process.chdir(prevCwd);
    if (prevBin !== undefined) process.env.NBE_PREFLIGHT_BIN = prevBin;
  }
});

test("the bound covers a package that declares no durations at all", async () => {
  // The derivation read only `expectedDurationFrames` and `loop.periodFrames`,
  // and both are optional — the schema requires id/kind/source, and §12.10 says
  // "if both present". A package declaring neither derived 0 frames and got the
  // 60 s floor however much video it held. Reproduced by the review pass on the
  // fix's own 1200-frame fixture with the field removed: killed at 60,299 ms
  // while being air-ready, exit 0, zero warnings. Worse than the flat 600 s
  // constant it replaced, for exactly that class.
  //
  // Bytes are the input that cannot be absent.
  const { preflightBound, expectedDecodeBytes, expectedDecodeFrames } = await import("./package.js");
  const { mkdtempSync, mkdirSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");

  // 1200 frames of 1080p, the pass's fixture shape: 8 clips of ~100 KB each.
  const CLIP_BYTES = 100_290;
  const pkg = (declare: boolean): string => {
    const dir = mkdtempSync(join(tmpdir(), "nbe-nodur-"));
    mkdirSync(join(dir, "media"), { recursive: true });
    const assets: Record<string, unknown>[] = [];
    for (let i = 0; i < 8; i++) {
      writeFileSync(join(dir, "media", `c${i}.mp4`), Buffer.alloc(CLIP_BYTES));
      assets.push({
        id: `c${i}`,
        kind: "video",
        source: `media/c${i}.mp4`,
        ...(declare ? { expectedDurationFrames: 150 } : {}),
      });
    }
    writeFileSync(join(dir, "manifest.json"), JSON.stringify({ assets }));
    return dir;
  };

  const prevBin = process.env.NBE_PREFLIGHT_BIN;
  const prevTimeout = process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  delete process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  try {
    const undeclared = pkg(false);
    assert.equal(expectedDecodeFrames(undeclared), 0, "nothing is declared — that is the case");
    assert.equal(expectedDecodeBytes(undeclared), 8 * CLIP_BYTES, "but the bytes are on disk");

    // Debug decodes this package in ~156 s (measured). The bound must cover it.
    process.env.NBE_PREFLIGHT_BIN = join("/x", "target", "debug", "nbe-preflight");
    const dbg = preflightBound(undeclared);
    assert.equal(dbg.basis, "bytes", "with nothing declared, the file sizes are the input");
    // Pin the rate itself, the way `msPerFrame` is pinned: measured at 204.7
    // s/MB on the worst fixture, rounded up. A silently reduced rate is a
    // silently tightened bound, which is how this class of defect keeps
    // recurring.
    assert.equal(dbg.msPerMb, 250_000, "worst measured was 204.7 s/MB in debug, rounded up");
    assert.ok(
      dbg.ms > 156_000 * 2,
      `the pass's fixture decodes in ~156 s in debug and the safety factor is ` +
        `${3}x; the bound must keep real headroom over the measured cost, not ` +
        `merely exceed it (got ${dbg.ms} ms)`,
    );

    // Release decodes it in ~17 s.
    process.env.NBE_PREFLIGHT_BIN = join("/x", "target", "release", "nbe-preflight");
    const rel = preflightBound(undeclared);
    assert.ok(rel.ms > 17_000, `release decodes it in ~17 s (got ${rel.ms} ms)`);

    // A declaration that under-states the truth is a WARNING, not a rejection,
    // so such a package still has to load. The bytes term covers the truth.
    const liar = mkdtempSync(join(tmpdir(), "nbe-liar-"));
    mkdirSync(join(liar, "media"), { recursive: true });
    writeFileSync(join(liar, "media", "big.mp4"), Buffer.alloc(10_000 * 668));
    writeFileSync(
      join(liar, "manifest.json"),
      JSON.stringify({
        assets: [{ id: "big", kind: "video", source: "media/big.mp4", expectedDurationFrames: 100 }],
      }),
    );
    process.env.NBE_PREFLIGHT_BIN = join("/x", "target", "debug", "nbe-preflight");
    const lied = preflightBound(liar);
    assert.equal(lied.basis, "bytes", "the declaration under-states; the bytes do not");
    assert.ok(
      lied.ms > 10_000 * 130,
      `10,000 frames cost ~1300 s in debug; a bound from the declared 100 would be 60 s (got ${lied.ms} ms)`,
    );

    // Nothing to measure at all still gets the floor, not zero.
    const empty = mkdtempSync(join(tmpdir(), "nbe-empty-"));
    writeFileSync(join(empty, "manifest.json"), JSON.stringify({ assets: [] }));
    assert.equal(preflightBound(empty).basis, "floor");
    assert.equal(preflightBound(empty).ms, 60_000);

    // And the override still wins and is still strict.
    process.env.NBE_PREFLIGHT_TIMEOUT_MS = "0";
    assert.equal(preflightBound(undeclared).basis, "bytes", '"0" must not disable the bound');
    process.env.NBE_PREFLIGHT_TIMEOUT_MS = "4321";
    assert.equal(preflightBound(undeclared).ms, 4321);
    assert.equal(preflightBound(undeclared).basis, "override");
  } finally {
    if (prevBin === undefined) delete process.env.NBE_PREFLIGHT_BIN;
    else process.env.NBE_PREFLIGHT_BIN = prevBin;
    if (prevTimeout === undefined) delete process.env.NBE_PREFLIGHT_TIMEOUT_MS;
    else process.env.NBE_PREFLIGHT_TIMEOUT_MS = prevTimeout;
  }
});
