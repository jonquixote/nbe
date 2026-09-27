//! Prompt 11 WU7 — automation latency, measured (SPEC AC-25 #1, #2).
//!
//! NOT A TEST, and not in `npm test`: a timing claim from a shared runner is
//! noise (R9 — the wall bound tracks load). Run it on a quiescent machine.
//! Each tier prints the load average at its start and end, and a tier that
//! reads 3.0 or more at either end is VOID (the quiescence ceiling). Paste the
//! whole output.
//!
//!   npm run measure:automation                         # both tiers
//!   npm run measure:automation -- --tier cp --n 300
//!   npm run measure:automation -- --tier engine --engine ../../target/release/nbe-engine
//!
//! TIER cp — the control plane on the production clock (`performance.now()`,
//! no `automationClock` seam), a stub render session feeding the engine
//! frames. Per trigger kind: the product's own number, `latencyMs =
//! dispatchedAt − observedAt` from each `automation.action` audit row
//! (design note §6 defines `observedAt` per kind). Plus autoFollow's advance
//! and AC-25 #2's hold: `latencyMs = cancelledAt − heldAt` from each
//! `automation.cancelledByHold` row.
//!
//! TIER engine — B1's acceptance evidence. The real `nbe-engine` binary over
//! the real socket, a real audio asset on the soundboard, `audioLevel` rules
//! on the sfx bus. End to end from the engine's `ts` on the crossing (read
//! while the block is computed) to the control plane's `queuedAt`, the span
//! AC-25 #1 gives one frame. `ts` is Unix ms (the engine's `SystemTime`);
//! the control plane's instants are `performance.now()`. They meet on the wall
//! clock through an offset `Date.now() − performance.now()` read at a
//! millisecond EDGE of `Date.now()` (spin until it ticks), recalibrated before
//! every play; each crossing uses the calibration nearest it. NOT
//! `performance.timeOrigin + t`: the first full run (2026-09-27) found that
//! conversion 1.8–3.5 ms off the wall clock thirteen minutes into the process,
//! drifting within the tier, and every span came out negative. A negative
//! span now fails the run: a clock that puts the effect before its cause is
//! broken, not fast.
//!
//! Counted two ways, every phase: the audit log's rows, and the effect in
//! state (a marker per action; for autoFollow the item it advanced to; for
//! the hold, zero markers) — against what the harness issued.
//!
//! The last line is `AUTOMATION: …`, the soak's record (docs/soak-protocol.md
//! §1). Exit 1 when counts disagree, or when a tier that is not VOID has a
//! span over one frame (AC-25 #1, #2); otherwise 0, VOID or not — the line
//! says which.

import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";
import { WebSocket } from "ws";

import { AuditLog, type AuditRecord } from "./audit.js";
import { createControlPlaneServer, type ControlPlaneServer } from "./server.js";
import { ControlPlaneState } from "./state.js";
import { preflightBin } from "./package.js";
import { tempDir } from "./test-tmp.js";

const ADMIN = "admin-token";
const RENDER = "render-token";
const HOUSE_RATE = 30;
const FRAME_MS = 1000 / HOUSE_RATE;
/** The quiescence ceiling: a tier whose 1-minute load reads this or more is VOID. */
const LOAD_CEILING = 3.0;
/**
 * Between triggers. Two floors: more than one frame, so each fires on a fresh
 * frame with an empty queue (§13.3 #3); and under §10.7's command limiter —
 * `RateLimiter` in server.ts, 10 per burst refilled at 5/s per connection per
 * command family — which a rule's action faces too, on connection
 * `automation:<ruleId>`. 5/s is one per 200 ms; 210 leaves the bucket
 * refilling faster than it drains.
 */
const PACE_MS = 210;

const REPO = resolve(import.meta.dirname, "../../..");
const DRESS_MEDIA = join(REPO, "tests/fixtures/dress_show/media");

const args = process.argv.slice(2);
const arg = (name: string, dflt: string): string => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && i + 1 < args.length ? args[i + 1]! : dflt;
};
const TIER = arg("tier", "all");
const N = Number(arg("n", "300"));
/** timeOfDay resolves to the second: one firing per second, so fewer samples (see its phase). */
const N_TOD = Number(arg("tod", "60"));
const ENGINE_BIN = resolve(arg("engine", join(REPO, "target/release/nbe-engine")));

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

interface Row {
  label: string;
  n: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  withinFrame: number;
  counts: string;
}

/** Nearest-rank percentile of an ascending array. */
const pct = (sorted: number[], q: number): number => sorted[Math.max(0, Math.ceil(q * sorted.length) - 1)] ?? NaN;

function row(label: string, samples: number[], counts: string): Row {
  const s = [...samples].sort((a, b) => a - b);
  return {
    label,
    n: s.length,
    p50: pct(s, 0.5),
    p95: pct(s, 0.95),
    p99: pct(s, 0.99),
    max: s.at(-1) ?? NaN,
    withinFrame: s.filter((x) => x <= FRAME_MS).length,
    counts,
  };
}

const ms = (x: number): string => (Number.isFinite(x) ? x.toFixed(3) : "—");

function table(title: string, rows: Row[]): string {
  const out = [
    `### ${title}`,
    "",
    `| Span | n | p50 ms | p95 ms | p99 ms | max ms | ≤ 1 frame (${FRAME_MS.toFixed(3)} ms) | Counted two ways |`,
    "|---|---|---|---|---|---|---|---|",
  ];
  for (const r of rows) {
    out.push(`| ${r.label} | ${r.n} | ${ms(r.p50)} | ${ms(r.p95)} | ${ms(r.p99)} | ${ms(r.max)} | ${r.withinFrame}/${r.n} | ${r.counts} |`);
  }
  return out.join("\n");
}

function load(): string {
  const [a, b, c] = loadavg();
  return `${a!.toFixed(2)} ${b!.toFixed(2)} ${c!.toFixed(2)}`;
}

// ---------------------------------------------------------------------------
// A control plane, a package, a socket
// ---------------------------------------------------------------------------

interface Ctx {
  server: ControlPlaneServer;
  state: ControlPlaneState;
  auditPath: string;
}

async function startServer(): Promise<Ctx> {
  const state = new ControlPlaneState();
  const auditPath = join(tempDir("nbe-lat-audit-"), "audit.jsonl");
  const server = await createControlPlaneServer({
    port: 0,
    auth: { tokens: { [ADMIN]: "admin", [RENDER]: "render" } },
    audit: new AuditLog(auditPath),
    state,
    persistence: { onDirty: () => {}, flushNow: () => {} },
    showStopGraceMs: 150,
    warn: () => {},
  });
  return { server, state, auditPath };
}

function audit(ctx: Ctx): AuditRecord[] {
  if (!existsSync(ctx.auditPath)) return [];
  return readFileSync(ctx.auditPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as AuditRecord);
}

function rows(ctx: Ctx, event: string): AuditRecord[] {
  return audit(ctx).filter((r) => r.kind === "automation" && r.event === event);
}

function markers(ctx: Ctx, name: string): number {
  return ctx.state.markers.filter((m) => m.name === name).length;
}

async function open(ctx: Ctx, role: string, token: string): Promise<WebSocket> {
  const ws = new WebSocket(`ws://127.0.0.1:${ctx.server.port}/nbe/v0.3`, {
    headers: { authorization: `Bearer ${token}`, "x-nbe-role": role },
  });
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  return ws;
}

function send(
  ws: WebSocket,
  command: string,
  payload: Record<string, unknown> = {},
  extra: Record<string, unknown> = {},
  timeoutMs = 120_000,
): Promise<Record<string, unknown>> {
  const id = randomUUID();
  return new Promise((res, rej) => {
    const timer = setTimeout(() => {
      ws.off("message", onMsg);
      rej(new Error(`no response to "${command}" within ${timeoutMs} ms`));
    }, timeoutMs);
    const onMsg = (buf: Buffer) => {
      const msg = JSON.parse(buf.toString("utf8")) as Record<string, unknown>;
      if (msg.requestId !== id) return;
      clearTimeout(timer);
      ws.off("message", onMsg);
      res(msg);
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ v: "0.3", id, command, payload, ...extra }));
  });
}

async function ok(ws: WebSocket, command: string, payload: Record<string, unknown> = {}, extra: Record<string, unknown> = {}): Promise<void> {
  const r = await send(ws, command, payload, extra);
  if (r.status !== "ok") throw new Error(`${command} refused: ${JSON.stringify(r)}`);
}

/** The server has processed everything sent on `ws`, and the queue has drained. */
async function flushed(ctx: Ctx, ws: WebSocket): Promise<void> {
  await send(ws, "system.status", {});
  await ctx.server.automation.settled();
}

const marker = (id: string, trigger: Record<string, unknown>) => ({
  id,
  trigger,
  action: { command: "marker.add", payload: { name: `by-${id}` } },
});

function writePackage(automation: unknown[], opts: { audio?: boolean } = {}): string {
  const dir = tempDir("nbe-lat-pkg-");
  mkdirSync(join(dir, "media"), { recursive: true });
  // Real media: the engine tier decodes what it loads.
  copyFileSync(join(DRESS_MEDIA, "fallback.png"), join(dir, "media", "fallback.png"));
  if (opts.audio) copyFileSync(join(DRESS_MEDIA, "stab.m4a"), join(dir, "media", "stab.m4a"));
  const assets: unknown[] = [{ id: "fallback", kind: "image", source: "media/fallback.png" }];
  if (opts.audio) assets.push({ id: "stab_sfx", kind: "audio", source: "media/stab.m4a" });
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify({
      manifestVersion: "0.4",
      network: { id: "nbe", name: "Latency" },
      show: {
        id: "show-latency",
        title: "Automation latency",
        video: { width: 1920, height: 1080, frameRate: HOUSE_RATE, colorSpace: "rec709" },
        audio: { sampleRate: 48000, loudnessTargetLufs: -16, truePeakDbtp: -1.5 },
        fallbackAssetId: "fallback",
      },
      qualityProfile: "consumer",
      assets,
      scenes: [{ id: "SCN", elements: [{ id: "main", kind: "clip", z: 1, assetId: "fallback" }] }],
      rundown: {
        id: "R",
        items: [
          { id: "A1", kind: "sceneRef", sceneRef: "SCN" },
          { id: "A2", kind: "sceneRef", sceneRef: "SCN" },
          { id: "AT", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60 },
          { id: "AF", kind: "sceneRef", sceneRef: "SCN", durationFrames: 60, autoFollow: true },
          { id: "AN", kind: "sceneRef", sceneRef: "SCN" },
        ],
      },
      control: {
        bindings: [{ id: "take-key", action: "view.take", trigger: { kind: "hotkey", key: "F1" } }],
      },
      automation,
    }),
  );
  return dir;
}

/** Load `automation` over the wire (preflight included) and, unless told not to, start the show. */
async function loadShow(ctx: Ctx, admin: WebSocket, automation: unknown[], start = true): Promise<void> {
  await ok(admin, "show.load", { packagePath: writePackage(automation) });
  if (start) await ok(admin, "show.start", {});
  await ctx.server.automation.settled();
}

function engineTelemetry(streamTransportState: string): string {
  return JSON.stringify({
    v: "0.3",
    kind: "engineTelemetry",
    ts: Date.now(),
    masterClockFrame: 10,
    droppedFramesTotal: 0,
    renderGpuTimeMs: 1,
    decodeSessions: 0,
    vramUsedMib: 0,
    textureCacheUsedMib: 0,
    streamBufferMs: -1,
    recordSpaceMib: 0,
    masterClockDriftMs: 0,
    fallbackActive: false,
    degradationRung: 0,
    streamTransportState,
  });
}

const latencies = (rs: AuditRecord[]): number[] => rs.map((r) => r.detail?.["latencyMs"] as number);

/** One phase on a fresh control plane: its own audit log, state and package. */
async function phase(fn: (ctx: Ctx, admin: WebSocket, render: WebSocket) => Promise<Row>): Promise<Row> {
  const ctx = await startServer();
  const admin = await open(ctx, "admin", ADMIN);
  const render = await open(ctx, "render", RENDER);
  try {
    return await fn(ctx, admin, render);
  } finally {
    admin.close();
    render.close();
    await ctx.server.close();
  }
}

/** The action rows for one rule, checked against what was issued and what state shows. */
function actionRow(ctx: Ctx, label: string, ruleId: string, issued: number): Row {
  const rs = rows(ctx, "automation.action").filter((r) => r.actor === `automation:${ruleId}` && r.outcome === "ok");
  const limited = rows(ctx, "automation.rateLimited").length;
  const m = markers(ctx, `by-${ruleId}`);
  const agree = rs.length === issued && m === issued ? "agree" : "**DISAGREE**";
  return row(label, latencies(rs), `issued ${issued} · audit ${rs.length} · markers ${m} · rateLimited ${limited} — ${agree}`);
}

// ---------------------------------------------------------------------------
// Tier cp — each trigger kind through its real source, the production clock
// ---------------------------------------------------------------------------

async function tierCp(): Promise<Row[]> {
  const out: Row[] = [];

  out.push(
    await phase(async (ctx, admin) => {
      await loadShow(ctx, admin, [marker("sc", { kind: "stateChange", params: { field: "previewItem" } })]);
      for (let i = 0; i < N; i++) {
        await ok(admin, "preview.set", { itemRef: i % 2 === 0 ? "A1" : "A2" });
        await ctx.server.automation.settled();
        await sleep(PACE_MS);
      }
      return actionRow(ctx, "stateChange (command accepted → dispatched)", "sc", N);
    }),
  );

  out.push(
    await phase(async (ctx, admin) => {
      await loadShow(ctx, admin, [marker("ms", { kind: "mediaStart", params: { itemRef: "A2" } })]);
      // Two cuts per sample: the second on its own socket, so each socket's
      // `view` bucket sees one command per pace.
      const other = await open(ctx, "admin", ADMIN);
      for (let i = 0; i < N; i++) {
        await ok(admin, "view.cut", { itemRef: "A2" });
        await ctx.server.automation.settled();
        await ok(other, "view.cut", { itemRef: "A1" });
        await sleep(PACE_MS);
      }
      other.close();
      return actionRow(ctx, "mediaStart (take accepted → dispatched)", "ms", N);
    }),
  );

  out.push(
    await phase(async (ctx, admin, render) => {
      await loadShow(ctx, admin, [marker("me", { kind: "mediaEnd", params: { itemRef: "AT" } })]);
      const end = JSON.stringify({ v: "0.3", kind: "itemEvent", itemRef: "AT", event: "end" });
      for (let i = 0; i < N; i++) {
        await ok(admin, "view.cut", { itemRef: "AT" });
        render.send(end);
        await flushed(ctx, render);
        await ok(admin, "item.reset", { itemId: "AT" });
        await sleep(PACE_MS);
      }
      return actionRow(ctx, "mediaEnd (itemEvent end arrives → dispatched)", "me", N);
    }),
  );

  out.push(
    await phase(async (ctx, admin) => {
      await loadShow(ctx, admin, [marker("hk", { kind: "hotkey", params: { bindingId: "take-key" } })]);
      for (let i = 0; i < N; i++) {
        // The carried take is refused (nothing in preview); the key still fired.
        await send(admin, "view.take", {}, { intentSource: "keyboard/desk:take-key" });
        await ctx.server.automation.settled();
        await sleep(PACE_MS);
      }
      return actionRow(ctx, "hotkey (carrying command arrives → dispatched)", "hk", N);
    }),
  );

  out.push(
    await phase(async (ctx, admin, render) => {
      await loadShow(ctx, admin, [marker("al", { kind: "audioLevel", params: { bus: "mic", thresholdDbfs: -12 } })]);
      for (let i = 0; i < N; i++) {
        render.send(
          JSON.stringify({
            v: "0.3",
            kind: "audioLevelCrossing",
            ts: performance.timeOrigin + performance.now(),
            bus: "mic",
            thresholdDbfs: -12,
            direction: "rising",
            levelDbfs: -3,
            masterFrame: i,
          }),
        );
        await flushed(ctx, render);
        await sleep(PACE_MS);
      }
      return actionRow(ctx, "audioLevel (crossing frame arrives → dispatched)", "al", N);
    }),
  );

  out.push(
    await phase(async (ctx, admin, render) => {
      await loadShow(ctx, admin, [
        marker("sh-live", { kind: "streamHealth", params: { state: "live" } }),
        marker("sh-rc", { kind: "streamHealth", params: { state: "reconnecting" } }),
      ]);
      render.send(engineTelemetry("live")); // the baseline: fires nothing
      await flushed(ctx, render);
      for (let i = 0; i < N; i++) {
        render.send(engineTelemetry(i % 2 === 0 ? "reconnecting" : "live"));
        await flushed(ctx, render);
        await sleep(PACE_MS);
      }
      const live = actionRow(ctx, "", "sh-live", Math.floor(N / 2));
      const rc = actionRow(ctx, "", "sh-rc", Math.ceil(N / 2));
      const all = rows(ctx, "automation.action").filter((r) => r.outcome === "ok");
      return row("streamHealth (tick arrives → dispatched)", latencies(all), `live: ${live.counts}; reconnecting: ${rc.counts}`);
    }),
  );

  out.push(
    await phase(async (ctx, admin) => {
      // 100 ms apart from 200 ms: each fires alone. `observedAt` is the
      // scheduled instant, so the timer's own lateness is inside the number.
      const rules = Array.from({ length: N }, (_, i) => marker(`t${i}`, { kind: "timer", params: { atMs: 200 + i * 100 } }));
      await loadShow(ctx, admin, rules, false);
      await ok(admin, "show.start", {});
      await sleep(200 + N * 100 + 500);
      await ctx.server.automation.settled();
      const rs = rows(ctx, "automation.action").filter((r) => r.outcome === "ok");
      const m = ctx.state.markers.filter((x) => x.name.startsWith("by-t")).length;
      const agree = rs.length === N && m === N ? "agree" : "**DISAGREE**";
      return row("timer (scheduled instant → dispatched)", latencies(rs), `issued ${N} · audit ${rs.length} · markers ${m} — ${agree}`);
    }),
  );

  out.push(
    await phase(async (ctx, admin) => {
      // `at` resolves to the second, so one rule per second: N_TOD samples
      // take N_TOD seconds. Firing several on one second would measure the
      // queue behind them, not the trigger. `observedAt` is the scheduled
      // instant, as for timer — the same `setTimeout` path.
      const first = new Date(Date.now() + 8000);
      const hhmmss = (d: Date) => [d.getHours(), d.getMinutes(), d.getSeconds()].map((x) => String(x).padStart(2, "0")).join(":");
      const rules = Array.from({ length: N_TOD }, (_, i) => marker(`tod${i}`, { kind: "timeOfDay", params: { at: hhmmss(new Date(first.getTime() + i * 1000)) } }));
      await loadShow(ctx, admin, rules);
      const until = first.getTime() + N_TOD * 1000 + 1000;
      while (Date.now() < until) await sleep(250);
      await ctx.server.automation.settled();
      const rs = rows(ctx, "automation.action").filter((r) => r.outcome === "ok");
      const m = ctx.state.markers.filter((x) => x.name.startsWith("by-tod")).length;
      const agree = rs.length === N_TOD && m === N_TOD ? "agree" : "**DISAGREE**";
      return row("timeOfDay (scheduled instant → dispatched)", latencies(rs), `issued ${N_TOD} · audit ${rs.length} · markers ${m} — ${agree}`);
    }),
  );

  out.push(
    await phase(async (ctx, admin, render) => {
      await loadShow(ctx, admin, []);
      const end = JSON.stringify({ v: "0.3", kind: "itemEvent", itemRef: "AF", event: "end" });
      let advanced = 0;
      for (let i = 0; i < N; i++) {
        await ok(admin, "view.cut", { itemRef: "AF" });
        render.send(end);
        await flushed(ctx, render);
        if (ctx.state.viewItem === "AN") advanced++;
        await ok(admin, "item.reset", { itemId: "AF" });
        await sleep(PACE_MS);
      }
      const rs = rows(ctx, "autoFollow.advance").filter((r) => r.outcome === "ok");
      const agree = rs.length === N && advanced === N ? "agree" : "**DISAGREE**";
      return row("autoFollow (itemEvent end arrives → advance dispatched)", latencies(rs), `issued ${N} · audit ${rs.length} · on air AN ${advanced} — ${agree}`);
    }),
  );

  out.push(
    await phase(async (ctx, admin) => {
      // B5's shape: one trigger queues three actions; the first engages the
      // hold, the other two are pending when it lands.
      const trig = { kind: "stateChange", params: { field: "previewItem" } };
      await loadShow(ctx, admin, [
        { id: "holder", trigger: trig, action: { command: "automation.hold", payload: { hold: true } } },
        marker("b", trig),
        marker("c", trig),
      ]);
      for (let i = 0; i < N; i++) {
        await ok(admin, "automation.hold", { hold: false });
        await ok(admin, "preview.set", { itemRef: i % 2 === 0 ? "A1" : "A2" });
        await ctx.server.automation.settled();
        await sleep(PACE_MS);
      }
      const rs = rows(ctx, "automation.cancelledByHold");
      const m = ctx.state.markers.length;
      const agree = rs.length === 2 * N && m === 0 ? "agree" : "**DISAGREE**";
      return row("hold (accepted → pending cancelled, AC-25 #2)", latencies(rs), `pending ${2 * N} · cancelled rows ${rs.length} · markers ${m} (must be 0) — ${agree}`);
    }),
  );

  return out;
}

// ---------------------------------------------------------------------------
// Tier engine — audioLevel end to end, the real binary (B1's acceptance)
// ---------------------------------------------------------------------------

interface Crossing {
  ts: number;
  direction: string;
  /** `performance.now()` when the frame reached the consumer (= observed). */
  arrivedAt: number;
}

interface Calibration {
  /** `performance.now()` at the edge. */
  at: number;
  /** Wall-clock ms minus `performance.now()`, read where `Date.now()` ticks. */
  offset: number;
}

/**
 * `Date.now()` floors the wall clock to a whole ms, so a single reading is up
 * to 1 ms early. At the instant it ticks over, the floor IS the wall clock:
 * spin until it changes and read `performance.now()` beside it. At most ~1 ms
 * of spinning, done between plays, never while a crossing is in flight.
 */
function calibrate(): Calibration {
  const d0 = Date.now();
  let d = d0;
  let at = performance.now();
  while (d === d0) {
    at = performance.now();
    d = Date.now();
  }
  return { at, offset: d - at };
}

async function tierEngine(): Promise<Row[]> {
  if (!existsSync(ENGINE_BIN)) throw new Error(`engine binary missing: ${ENGINE_BIN} (cargo build --release -p nbe-engine)`);
  const ctx = await startServer();
  const crossings: Crossing[] = [];
  ctx.server.onEngineEvent((f) => {
    crossings.push({ ts: f.ts, direction: f.direction, arrivedAt: performance.now() });
  });
  const log: string[] = [];
  const engine: ChildProcess = spawn(ENGINE_BIN, [], {
    env: {
      ...process.env,
      NBE_CP_URL: `ws://127.0.0.1:${ctx.server.port}/nbe/v0.3`,
      NBE_RENDER_TOKEN: RENDER,
      NBE_HOUSE_RATE: String(HOUSE_RATE),
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  engine.stdout?.on("data", (b: Buffer) => log.push(b.toString()));
  engine.stderr?.on("data", (b: Buffer) => log.push(b.toString()));
  try {
    const deadline = Date.now() + 30_000;
    while (ctx.server.wsBridge.renderNodeCount() === 0) {
      if (engine.exitCode !== null || Date.now() > deadline) throw new Error(`engine never registered:\n${log.join("")}`);
      await sleep(100);
    }
    const admin = await open(ctx, "admin", ADMIN);
    const rules = [
      marker("rise", { kind: "audioLevel", params: { bus: "sfx", thresholdDbfs: -40, direction: "rising" } }),
      marker("fall", { kind: "audioLevel", params: { bus: "sfx", thresholdDbfs: -40, direction: "falling" } }),
    ];
    await ok(admin, "show.load", { packagePath: writePackage(rules, { audio: true }) });
    await ok(admin, "show.start", {});
    // The engine installs the watches with the load; the first stab proves it.
    const waitFor = async (count: number, ms: number): Promise<boolean> => {
      const until = Date.now() + ms;
      while (crossings.length < count && Date.now() < until) await sleep(5);
      return crossings.length >= count;
    };
    let plays = 0;
    const calibrations: Calibration[] = [];
    for (let i = 0; i < N; i++) {
      const want = crossings.length + 2;
      calibrations.push(calibrate());
      await ok(admin, "soundboard.play", { assetId: "stab_sfx" });
      plays++;
      if (!(await waitFor(want, 3000))) {
        throw new Error(`play ${i}: ${crossings.length - (want - 2)} of 2 crossings within 3 s — is ${ENGINE_BIN} built with B1?\n${log.join("")}`);
      }
      await sleep(PACE_MS);
    }
    calibrations.push(calibrate());
    await ctx.server.automation.settled();
    admin.close();

    // The calibration nearest each crossing; adjacent calibrations bound how
    // far the offset can have moved in between.
    const nearest = (t: number): Calibration =>
      calibrations.reduce((a, b) => (Math.abs(b.at - t) < Math.abs(a.at - t) ? b : a));
    const offsets = calibrations.map((c) => c.offset);
    let step = 0;
    for (let k = 1; k < calibrations.length; k++) step = Math.max(step, Math.abs(calibrations[k]!.offset - calibrations[k - 1]!.offset));

    const actions = rows(ctx, "automation.action").filter((r) => r.outcome === "ok");
    const byTs = new Map<string, AuditRecord>();
    for (const r of actions) {
      const t = r.detail?.["trigger"] as { ts: number; direction: string };
      byTs.set(`${t.ts}/${t.direction}`, r);
    }
    const toQueued: number[] = [];
    const toObserved: number[] = [];
    const toDispatched: number[] = [];
    let matched = 0;
    for (const c of crossings) {
      const r = byTs.get(`${c.ts}/${c.direction}`);
      if (!r) continue;
      matched++;
      const d = r.detail as Record<string, number>;
      const { offset } = nearest(c.arrivedAt);
      toObserved.push(d["observedAt"]! + offset - c.ts);
      toQueued.push(d["queuedAt"]! + offset - c.ts);
      toDispatched.push(d["dispatchedAt"]! + offset - c.ts);
    }
    const rises = crossings.filter((c) => c.direction === "rising").length;
    const falls = crossings.filter((c) => c.direction === "falling").length;
    const mRise = markers(ctx, "by-rise");
    const mFall = markers(ctx, "by-fall");
    const agree = rises === plays && falls === plays && actions.length === 2 * plays && mRise + mFall === 2 * plays && matched === 2 * plays;
    const causal = Math.min(...toObserved) >= 0;
    const counts =
      `plays ${plays} · crossings ${rises}↑ ${falls}↓ · audit ${actions.length} (joined ${matched}) · markers ${mRise}↑ ${mFall}↓ — ${agree ? "agree" : "**DISAGREE**"}` +
      (causal ? "" : " · **DISAGREE: a span is negative — the clocks are not aligned**");
    console.log(
      `clock alignment: ${calibrations.length} edge calibrations of Date.now() − performance.now(); offset range ${(Math.max(...offsets) - Math.min(...offsets)).toFixed(3)} ms over the tier, largest step between adjacent calibrations ${step.toFixed(3)} ms — each span is within that step of exact`,
    );
    return [
      row("crossing ts → frame arrives (engine → socket → consumer)", toObserved, counts),
      row("crossing ts → queued (AC-25 #1's span)", toQueued, "same"),
      row("crossing ts → dispatched", toDispatched, "same"),
    ];
  } finally {
    engine.kill("SIGKILL");
    await ctx.server.close();
  }
}

// ---------------------------------------------------------------------------

async function main(): Promise<number> {
  if (!existsSync(preflightBin())) throw new Error(`nbe-preflight not built: ${preflightBin()}`);
  const report: Record<string, unknown> = { startedAt: new Date().toISOString(), n: N, nTod: N_TOD, paceMs: PACE_MS };
  const summary: string[] = [];
  let failed = false;
  let voided = false;
  console.log(`# automation latency — ${report.startedAt}; house rate ${HOUSE_RATE} (1 frame = ${FRAME_MS.toFixed(3)} ms); pace ${PACE_MS} ms; node ${process.version}`);
  for (const [name, run] of [
    ["cp", tierCp],
    ["engine", tierEngine],
  ] as const) {
    if (TIER !== "all" && TIER !== name) continue;
    const before = load();
    console.log(`\n## tier ${name} — load at start: ${before}`);
    const t0 = performance.now();
    const rs = await run();
    const after = load();
    const void_ = [before, after].some((l) => Number(l.split(" ")[0]) >= LOAD_CEILING);
    console.log(`load at end: ${after}; ${((performance.now() - t0) / 1000).toFixed(1)} s${void_ ? ` — **VOID: a 1-minute load ≥ ${LOAD_CEILING}**` : ""}`);
    console.log(table(`tier ${name}${void_ ? " (VOID)" : ""}`, rs));
    report[name] = { loadStart: before, loadEnd: after, void: void_, rows: rs };

    const disagree = rs.some((r) => r.counts.includes("DISAGREE"));
    const over = rs.filter((r) => r.max > FRAME_MS);
    failed ||= disagree || (!void_ && over.length > 0);
    voided ||= void_;
    const worst = rs.reduce((a, b) => (b.max > a.max ? b : a));
    const head =
      name === "cp"
        ? `cp worst max ${ms(worst.max)} ms (${worst.label.split(" (")[0]}) over ${rs.length} spans`
        : (() => {
            const q = rs.find((r) => r.label.includes("queued"))!;
            return `audioLevel crossing→queued p50 ${ms(q.p50)} p99 ${ms(q.p99)} max ${ms(q.max)} ms, ≤1 frame ${q.withinFrame}/${q.n}`;
          })();
    const verdicts = [disagree ? "counts DISAGREE" : "counts agree", ...(over.length ? [`OVER 1 FRAME: ${over.map((r) => r.label.split(" (")[0]).join(", ")}`] : [])];
    summary.push(`${head}; ${verdicts.join("; ")}; load ${before.split(" ")[0]}→${after.split(" ")[0]}${void_ ? " VOID" : ""}`);
  }
  const dir = join(REPO, "target/automation-latency");
  mkdirSync(dir, { recursive: true });
  const file = join(dir, `${new Date().toISOString().replace(/[:.]/g, "-")}.json`);
  writeFileSync(file, JSON.stringify(report, null, 2));
  console.log(`\nreport: ${file}`);
  console.log(`AUTOMATION: ${failed ? "FAIL" : voided ? "VOID" : "PASS"} — ${summary.join(" | ")}`);
  return failed ? 1 : 0;
}

main().then(
  (code) => process.exit(code),
  (e: unknown) => {
    console.error(e);
    process.exit(1);
  },
);
