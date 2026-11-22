//! Server (addendum 02a §1.1/1.3, §2.5-2.8): ONE node HTTP server owns the
//! WebSocket upgrade and `GET /nbe/v0.3/status`. Prompt 08 is WS-only:
//! Companion speaks the §5.4/§16 WS bus; the HTTP listener is §10.4's health
//! endpoint, full stop. No second transport.

import { createServer, type Server, type IncomingMessage, type ServerResponse } from "node:http";
import { randomUUID, createHash, timingSafeEqual } from "node:crypto";
import { WebSocketServer, type WebSocket } from "ws";
import type { Duplex } from "node:stream";

import { AuditLog, ON_AIR_COMMANDS } from "./audit.js";
import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import {
  EngineFrameSchema,
  HELD_DETAIL_PREFIX,
  type EngineHelloFrame,
  EnvelopeSchema,
  type AudioLevelCrossingFrame,
  type Envelope,
  WS_PATH,
  PROTOCOL_VERSION,
  CpError,
  errorResponse,
  okResponse,
  resolveCommand,
  RESYNC_COMMAND,
  type ResyncViewItemEnd,
  type Role,
} from "./protocol.js";
import {
  WsRenderBridge,
  type RenderBridge,
  type RenderDirective,
  type RenderRegistration,
} from "./render-bridge.js";
import type { ControlPlaneState, DeprecationRecord, PackageInfo } from "./state.js";
import type { SystemHooks } from "./commands/system.js";
import {
  AutomationEvaluator,
  NO_CAUSE,
  snapshot,
  stateChanges,
  type ActionOutcome,
  type Cause,
  type Snapshot,
} from "./automation.js";
import type { JournalBody, PersistenceHooks } from "./persistence.js";
import { indexPackage } from "./package.js";
import { buildTick, ENGINE_TELEMETRY_TTL_MS, ingestEngineFrame, newWorldTelemetry, type WorldTelemetry } from "./telemetry.js";
import { AlertLane, OperatorSignals } from "./alerts.js";
import { expireTickerItems, tickerDirectiveItems } from "./commands/ticker.js";

// ---------------------------------------------------------------------------
// Auth (addendum §2.5): token is authoritative; X-NBE-Role must match.
// Constant-time comparison. No default/empty token.
// ---------------------------------------------------------------------------

export interface AuthConfig {
  tokens: Record<string, Role>;
}

export interface AuthResult {
  ok: boolean;
  role?: Role;
  tokenId?: string;
  reason?: string;
}

function constantTimeEqual(a: string, b: string): boolean {
  const da = createHash("sha256").update(a).digest();
  const db = createHash("sha256").update(b).digest();
  return timingSafeEqual(da, db);
}

function tokenIdFor(token: string): string {
  return createHash("sha256").update(token).digest("hex").slice(0, 16);
}

export function authenticate(
  cfg: AuthConfig,
  bearer: string | undefined,
  assertedRole: string | undefined,
): AuthResult {
  if (!bearer) return { ok: false, reason: "missing bearer token" };
  let matched: { role: Role; key: string } | null = null;
  for (const [token, role] of Object.entries(cfg.tokens)) {
    if (constantTimeEqual(bearer, token)) matched = { role, key: token };
  }
  if (!matched) return { ok: false, reason: "unknown token" };
  if (!assertedRole || assertedRole !== matched.role) return { ok: false, reason: "role mismatch" };
  return { ok: true, role: matched.role, tokenId: tokenIdFor(matched.key) };
}

// ---------------------------------------------------------------------------
// Rate limiting (addendum §2.7): per-connection, per-family token bucket.
// ---------------------------------------------------------------------------

export class RateLimiter {
  private buckets = new Map<string, { tokens: number; lastRefill: number }>();
  constructor(
    private readonly capacity = 10,
    private readonly refillPerSec = 5,
    private readonly now: () => number = () => Date.now(),
  ) {}

  allow(connectionId: string, family: string): boolean {
    const key = `${connectionId}:${family}`;
    const t = this.now();
    let b = this.buckets.get(key);
    if (!b) {
      b = { tokens: this.capacity, lastRefill: t };
      this.buckets.set(key, b);
    }
    b.tokens = Math.min(this.capacity, b.tokens + ((t - b.lastRefill) / 1000) * this.refillPerSec);
    b.lastRefill = t;
    if (b.tokens < 1) return false;
    b.tokens -= 1;
    return true;
  }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/** Review4 step 7c: the output starts owed to a restarted engine. */
interface Reissue {
  record: Record<string, unknown> | null;
  stream: Record<string, unknown> | null;
}

/** Review4: what a restarted control plane does with the engine's hello. */
type RecoveryDecision = { kind: "adopt" } | { kind: "resync" } | { kind: "refuse"; reason: string };

interface ClientSession {
  /** Review4 step 7b: a render connection to a restarted control plane,
   * holding its handshake until the engine's hello arrives (or a timeout). */
  awaitingHello?: { handshake: (adopted: boolean) => void; timer: ReturnType<typeof setTimeout> } | null;
  /** Review4 case C: a handshake refused because the engine runs a show this
   * control plane cannot identify; completed by the operator's `show.load` or
   * `show.unload` (the operator taking control). */
  heldHandshake?: (() => void) | null;
  role: Role;
  tokenId: string;
  telemetryTimer: ReturnType<typeof setInterval> | null;
  closed: boolean;
  /** Commands are processed strictly in arrival order. */
  tail: Promise<void>;
  /** Pushes a server-initiated frame (SPEC §5.4.1); false when dropped. */
  push: (frame: unknown) => boolean;
  /** Render sessions only: this session's own directive channel. */
  render: {
    registration: RenderRegistration;
    lastApplied: number | null;
    /**
     * The most recent acknowledged versions of commands' directives, exactly
     * (for `waitForAck`). An acknowledgement of a version this session was
     * sent a `show.resync` at is the resync's, and is not kept here.
     */
    acked: number[];
    /** The versions this session was sent `show.resync` at, most recent last. */
    resyncs: number[];
    /** Review4 step 7a: this connection's `engineHello`, once it arrives. */
    hello?: EngineHelloFrame;
    /** Review4 step 7c: the starts to re-issue once this (restarted) engine
     * acknowledges its resync. */
    reissue?: Reissue;
  } | null;
  /** Deprecation warnings this subscriber has not yet been shown. */
  pendingDeprecations: DeprecationRecord[];
  /** Drops this connection. `http.close()` waits forever on live sockets. */
  terminate: () => void;
}

export interface ControlPlaneServer {
  http: Server;
  port: number;
  bridge: RenderBridge;
  wsBridge: WsRenderBridge;
  /**
   * Resolve once a render node reports having applied `version` or later
   * (SPEC §5.9.5), or `false` on timeout / no node attached.
   *
   * The server has always tracked this — `show.stop`'s quiescence window is
   * built on it — but only `show.stop` could ask. Exposing it lets any caller
   * distinguish "the control plane accepted a command" from "the engine did
   * it", which are different facts and were being conflated: the dress
   * rehearsal pressed `show.start` while the engine was still ten seconds into
   * decoding the package, and then measured the clock against a window that had
   * already expired. See the R5 note in `dress-rehearsal.test.ts`.
   */
  awaitApplied(version: number, ms: number): Promise<boolean>;
  /**
   * Subscribe to engine EVENTS the control plane consumes rather than
   * applies — today `audioLevelCrossing` (SPEC v0.4.7 candidate B1), which
   * the automation evaluator matches to rules. Returns the unsubscribe.
   * Called synchronously as the frame is parsed: no queue, no tick, so a
   * crossing reaches its consumer in the same event-loop turn it arrived in.
   */
  onEngineEvent(listener: (frame: AudioLevelCrossingFrame) => void): () => void;
  /** The automation evaluator (Prompt 11): tests await `settled()` on it. */
  automation: AutomationEvaluator;
  close(): Promise<void>;
}

export interface ServerOptions {
  port?: number;
  host?: string;
  auth: AuthConfig;
  audit: AuditLog;
  state: ControlPlaneState;
  persistence: PersistenceHooks;
  /**
   * Review4 step 7b (the user's ruling of 2026-11-16 23:45 on step 7): this
   * control plane restarted and restored a journal. It waits for the
   * engine's hello before resyncing it, and adopts a running show whose
   * package identity matches the journal's. While the journal says a show
   * was running, commands are refused for up to `lockMs` (15 s) or until the
   * hello arrives.
   */
  restart?: { journal: JournalBody; lockMs?: number; helloWaitMs?: number };
  /**
   * Review4 case C without a journal: a control plane with no journal to
   * restore (a first start, or both journal files lost) waits, bounded, for
   * the hello on the first render connection it ever sees, and holds the
   * resync if that engine is running a show (a resync with nothing loaded
   * would stop it). Production sets it (index.ts); tests that script a render
   * client without a hello leave it off and get the connect-time resync.
   */
  awaitFirstHello?: { helloWaitMs?: number };
  /** Inject a specific bridge (tests use MockRenderBridge); defaults to the production WsRenderBridge fan-out. */
  bridge?: RenderBridge;
  /** SPEC §16.1 graceful window; shortened in tests. Default 2000 ms. */
  showStopGraceMs?: number;
  /**
   * The house rate the render node runs at (SPEC §7.15).
   *
   * `show.load` MUST reject a package declaring a different rate. Without this
   * the check is unreachable — which is exactly what shipped: the field
   * existed on `DispatchDeps`, nothing ever assigned it, and deleting the
   * guard left the suite green.
   */
  houseRate?: number;
  /** Warning sink; defaults to console.warn. Tests assert exact strings. */
  warn?: (message: string) => void;
  /**
   * The automation evaluator's monotonic clock (default `performance.now`).
   * Test seam: the once-per-frame limiter keys on a frame index read from
   * this clock, and a test that must put two triggers in ONE frame cannot
   * rely on wall time never crossing a 33 ms boundary between them.
   */
  automationClock?: () => number;
}

export async function createControlPlaneServer(opts: ServerOptions): Promise<ControlPlaneServer> {
  const { state, audit, auth } = opts;
  const world: WorldTelemetry = newWorldTelemetry();

  // -- SPEC §10.1.2 (v0.4.8 row 8): the operator-signal lane -----------------
  // Alerts live on the state, so every snapshot surface and the tick carry
  // one list. Each edge (raise, clear) is one audit row under kind `command`,
  // the tree's kind for control-plane events, never a row per evaluation.
  const lane = new AlertLane(
    state,
    (edge, alert, cause) => {
      audit.record({
        kind: "command",
        command: "alert",
        ...(alert.auditEvent === null ? {} : { event: alert.auditEvent }),
        detail: {
          alert: alert.id,
          edge,
          severity: alert.severity,
          reason: alert.reason,
          since: alert.since,
          ...(cause === null ? {} : { cause }),
        },
        stateVersionBefore: state.stateVersion,
        stateVersionAfter: state.stateVersion,
      });
    },
    Date.now,
  );
  const signals = new OperatorSignals({
    state,
    lane,
    now: Date.now,
    // §5.9.5's window: the same one `show.stop` waits for its acknowledgement.
    windowMs: opts.showStopGraceMs ?? 2000,
    ttlMs: ENGINE_TELEMETRY_TTL_MS,
    engineReport: () => world.last,
    auditSinkOk: () => audit.sinkOk,
    journalOk: () => opts.persistence.writeOk?.() ?? true,
    // §10.1.4 (v0.4.8 row 11, D4): a belief correction lands like the engine's
    // `itemEvent`, as a detection transaction: one version bump and one audit
    // row (`belief.corrected`). Unlike `itemEvent`, it broadcasts no
    // `stateChange` frame (§5.4.1: one per accepted command) and fires no
    // automation trigger: automation reacts to committed intent, on the
    // command path, not to environmental reconciliation.
    correct: (c) => {
      const before = state.stateVersion;
      state.streamState = c.to;
      state.bump();
      opts.persistence.onDirty();
      // Review4 step 5: this runs inside the signal timer, where a throw
      // would end the process; the lane announces a failed write.
      audit.tryRecord({
        kind: "command",
        command: "engine:report",
        event: "belief.corrected",
        detail: { field: c.field, from: c.from, to: c.to, report: c.token, reportAgeMs: c.ageMs },
        stateVersionBefore: before,
        stateVersionAfter: state.stateVersion,
      });
    },
  });
  const signalTimer = setInterval(() => signals.evaluate(), 250);
  signalTimer.unref?.();
  // Review4 step 2: ticker TTLs, enforced here once a second. An expiry is a
  // detection transaction (like a belief correction: a version, an audit
  // row, no stateChange frame) that sends the engine the resolved queue.
  const tickerTimer = setInterval(() => {
    const before = state.stateVersion;
    const expired = expireTickerItems(state, Date.now());
    if (expired === 0) return;
    const sv = state.bump();
    opts.persistence.onDirty();
    bridge.send({
      command: "ticker.override",
      target: {},
      payload: { items: tickerDirectiveItems(state.tickerItems), mode: "replace" },
      stateVersion: sv,
    });
    // The engine applies this shortly: ticker.notOnAir waits for it to settle,
    // as it does after a ticker command (the command path's onAccepted).
    signals.onTickerChanged();
    try {
      audit.record({
        kind: "command",
        command: "system:ticker.expire",
        event: "ticker.expired",
        detail: { expired, remaining: state.tickerItems.length },
        stateVersionBefore: before,
        stateVersionAfter: sv,
      });
    } catch {
      // A throw here would leave the timer callback and end the process. A
      // sink that cannot be written is R18's alert to announce (the signal
      // lane reads `audit.sinkOk`); the expiry has happened and stands.
    }
  }, 1000);
  tickerTimer.unref?.();
  const wsBridge = new WsRenderBridge();

  // -- Review4 step 7b: adoption after a control-plane restart -------------
  // Present only when this process restored a journal. `resolved` once the
  // first engine has said hello (or never did, within the wait): from then
  // on every connection is review4's case A or E, as before.
  const recovery = opts.restart
    ? {
        journal: opts.restart.journal,
        // Commands wait for the hello only when the journal says a show was
        // running: there is then a show that a command could disturb before
        // it is adopted. The timer only unlocks commands; a hello after it
        // still adopts (review4 1.3).
        lockedUntil:
          opts.restart.journal.journal["showState"] === "RUNNING" ? Date.now() + (opts.restart.lockMs ?? 15_000) : 0,
        resolved: false,
      }
    : null;
  const recovering = (): boolean => recovery !== null && !recovery.resolved;
  const commandsLocked = (): boolean => recovering() && Date.now() < recovery!.lockedUntil;
  /** Read-only commands are answered while recovering. */
  const RECOVERY_EXEMPT = new Set(["system.status", "show.getState", "system.telemetry.subscribe", "system.telemetry.unsubscribe"]);

  function recoveryRow(event: string, detail: Record<string, unknown>, before: number): void {
    try {
      audit.record({
        kind: "command",
        command: "engine:hello",
        event,
        detail,
        stateVersionBefore: before,
        stateVersionAfter: state.stateVersion,
      });
    } catch {
      // R18's alert announces a sink that cannot be written; the adoption,
      // or its refusal, stands.
    }
  }

  /** Review4 case C: hold this render connection unregistered (no resync, so
   * nothing stops the engine's show; the picture stays), raise the alert, and
   * audit the refusal. The operator's `show.load` or `show.unload` releases it. */
  function holdUnidentified(session: ClientSession, handshake: () => void, reason: string, hello: EngineHelloFrame): void {
    session.heldHandshake = handshake;
    unidentifiedHold = { reason };
    signals.unidentified(reason, null);
    try {
      audit.record({
        kind: "command",
        command: "engine:hello",
        event: "controlPlane.refused",
        detail: { reason, engineInstanceId: hello.engineInstanceId, package: hello.package, viewItem: hello.viewItem },
        stateVersionBefore: state.stateVersion,
        stateVersionAfter: state.stateVersion,
      });
    } catch {
      // A row that cannot be written is R18's alert to announce.
    }
  }
  function releaseHeldHandshakes(command: string): void {
    // The operator's choice ends the hold even with no engine connected now:
    // the control plane's state is the one to resync from here on.
    let released = unidentifiedHold !== null;
    unidentifiedHold = null;
    for (const client of clients.values()) {
      const held = client.heldHandshake;
      if (!held || client.closed) continue;
      client.heldHandshake = null;
      held();
      released = true;
    }
    if (released) signals.unidentified(null, `the operator took control (${command})`);
  }

  function notAdopted(reason: string, detail: Record<string, unknown>): void {
    recovery!.resolved = true;
    recoveryRow("controlPlane.notAdopted", { reason, ...detail }, state.stateVersion);
    if (reason !== "engineNotRunning") {
      opts.warn?.(`the restarted control plane did not adopt the engine's show (${reason}); it resyncs as before`);
    }
  }

  /**
   * Review4 1.3, decided on the first hello after a restart. Adopts (case B)
   * when the engine is running the package the journal names, at the same
   * load generation, and the journal says a show was running. Otherwise the
   * engine gets today's resync, and why is audited: an engine not running is
   * case D (UNLOADED; a saved file alone never starts a show), anything else
   * case C, whose refusal is a later PR's (7b-ii). Returns whether it adopted.
   */
  function recover(hello: EngineHelloFrame): RecoveryDecision {
    const j = recovery!.journal;
    const body = j.journal;
    const reason = !hello.showRunning
      ? "engineNotRunning"
      : body["showState"] !== "RUNNING"
        ? "journalNotRunning"
        : hello.package === null
          ? "engineHasNoPackage"
          : j.manifestIdentity?.packagePath !== hello.package.path ||
              body["packageLoadStateVersion"] !== hello.package.loadStateVersion
            ? "identity"
            : null;
    const identities = {
      journal: { path: j.manifestIdentity?.packagePath ?? null, loadStateVersion: body["packageLoadStateVersion"] ?? null },
      engine: hello.package,
    };
    if (reason !== null) {
      notAdopted(reason, identities);
      // Case D: the engine runs nothing; today's resync is right. Case C: it
      // runs a show this control plane cannot identify; a resync would stop
      // it (a non-RUNNING snapshot stops the engine's clock), so refuse.
      return reason === "engineNotRunning" ? { kind: "resync" } : { kind: "refuse", reason };
    }
    let pkg: PackageInfo;
    try {
      pkg = indexPackage(hello.package!.path);
    } catch (err) {
      notAdopted("index", { ...identities, error: err instanceof Error ? err.message : String(err) });
      return { kind: "refuse", reason: "index" };
    }
    const before = state.stateVersion;
    const out = state.adopt(pkg, hello, body, Date.now());
    automation.load(pkg);
    recovery!.resolved = true;
    opts.persistence.flushNow();
    recoveryRow(
      "controlPlane.adopted",
      {
        journalVersion: before,
        engineLastApplied: hello.lastAppliedStateVersion,
        adoptedVersion: state.stateVersion,
        journalAgeMs: Math.max(0, Date.now() - j.recoveredAt),
        // Rule actions queued or timed at the crash: dropped, never fired
        // late (the user's ruling of 2026-11-16 23:45).
        automationDropped: typeof body["automationBacklog"] === "number" ? body["automationBacklog"] : null,
        correctedToDone: out.correctedToDone,
        recordMismatch: out.recordMismatch,
        preflight: "not re-run",
        package: hello.package,
      },
      before,
    );
    signals.restarted({
      show: `${pkg.showId} at ${hello.package!.path}`,
      automationDropped: typeof body["automationBacklog"] === "number" ? body["automationBacklog"] : null,
      correctedToDone: out.correctedToDone,
      recordMismatch: out.recordMismatch,
      recording: hello.recording,
    });
    return { kind: "adopt" };
  }

  // -- Review4 step 7c: re-issue the outputs after an engine restart -------
  // A new engine process (a new `engineInstanceId`, case E) holds no take and
  // no stream: a resync carries no output state, so the control plane, which
  // still believes them on, re-sends the starts it accepted, once the new
  // engine has applied the resync (the show is running there again). The
  // payloads are kept here, in the server, never in `ControlPlaneState`: a
  // `stream.start` URL can carry a stream key, and the state is journaled and
  // served to clients. A re-issue is the same intent sent again, not a new
  // command: the belief does not change, and it goes out as a detection
  // transaction (a version, an audit row, the command's own window), as the
  // ticker's TTL expiry does. Guest starts are not re-issued here.
  const lastStart: { record: Record<string, unknown> | null; stream: Record<string, unknown> | null } = {
    record: null,
    stream: null,
  };
  function rememberOutputStart(command: string, payload: Record<string, unknown>): void {
    if (command === "record.start") lastStart.record = { ...payload };
    else if (command === "stream.start") lastStart.stream = { ...payload };
    else if (command === "record.stop") lastStart.record = null;
    else if (command === "stream.stop") lastStart.stream = null;
    else if (command === "show.stop" || command === "show.load" || command === "show.unload") {
      lastStart.record = null;
      lastStart.stream = null;
    }
  }
  function reissueOutputs(r: Reissue): void {
    const starts: Array<["record.start" | "stream.start", Record<string, unknown> | null]> = [
      ["record.start", r.record],
      ["stream.start", r.stream],
    ];
    for (const [command, payload] of starts) {
      if (payload === null) continue;
      const before = state.stateVersion;
      const sv = state.bump();
      opts.persistence.onDirty();
      bridge.send({ command, target: {}, payload, stateVersion: sv });
      signals.onAccepted(command, sv);
      try {
        audit.record({
          kind: "command",
          command: `system:${command}`,
          event: command === "record.start" ? "record.reissued" : "stream.reissued",
          detail: {
            reason: "the engine restarted",
            outputId: (payload["outputId"] as string | undefined) ?? null,
            // Never the URL itself: it can carry a stream key.
            ...(command === "stream.start" ? { url: payload["url"] === undefined ? null : "(set; not logged)" } : {}),
          },
          stateVersionBefore: before,
          stateVersionAfter: sv,
        });
      } catch {
        // A row that cannot be written is R18's alert to announce; the
        // re-issue has happened and stands.
      }
    }
  }

  // -- Review4 step 7a: the engine's hello ----------------------------------
  // The instance the last hello named, and when an engine that had said hello
  // last went away. A new instance id is a new engine process: review4's case
  // E, audited here as `engine.restarted`. A reconnect of the same process is
  // not. (Adoption, case B, and what a restarted engine is re-issued, are
  // steps 7b and 7c.)
  let lastEngineInstanceId: string | null = null;
  /** Review4 case C: in force from a refusal until the operator takes
   * control (show.load or show.unload). While it is, every render connection
   * waits for its hello, so an engine that reconnects mid-hold is held again
   * rather than resynced off air. */
  let unidentifiedHold: { reason: string } | null = null;
  /** A render connection has been resynced by this process. */
  let engineResynced = false;
  let engineGoneAt: number | null = null;
  function noteHello(render: NonNullable<ClientSession["render"]>, hello: EngineHelloFrame): void {
    render.hello = hello;
    const previous = lastEngineInstanceId;
    lastEngineInstanceId = hello.engineInstanceId;
    if (previous === null || previous === hello.engineInstanceId) return;
    // Step 7c: a restarted engine mid-show is owed the outputs believed on.
    if (state.showState === "RUNNING") {
      const record = state.recordState === "recording" ? lastStart.record : null;
      const stream = state.streamState === "live" ? lastStart.stream : null;
      if (record !== null || stream !== null) {
        // The engine says hello first on every connection, so its resync is
        // normally acknowledged after this, and the re-issue waits for that
        // (`noteApplied`). If the resync was already applied, send them now.
        const lastResync = render.resyncs[render.resyncs.length - 1];
        const applied = lastResync !== undefined && render.lastApplied !== null && render.lastApplied >= lastResync;
        if (applied) reissueOutputs({ record, stream });
        else render.reissue = { record, stream };
      }
    }
    try {
      audit.record({
        kind: "command",
        command: "engine:hello",
        event: "engine.restarted",
        detail: {
          previousInstanceId: previous,
          instanceId: hello.engineInstanceId,
          startedAtMs: hello.startedAtMs,
          downMs: engineGoneAt === null ? null : Math.max(0, Date.now() - engineGoneAt),
        },
        stateVersionBefore: state.stateVersion,
        stateVersionAfter: state.stateVersion,
      });
    } catch {
      // A row that cannot be written is R18's alert to announce; it must not
      // leave the engine's frame handler.
    }
  }
  const bridge: RenderBridge = opts.bridge ?? wsBridge;
  const rateLimiter = new RateLimiter();

  const clients = new Map<string, ClientSession>();
  const engineEventListeners = new Set<(frame: AudioLevelCrossingFrame) => void>();

  // -- SPEC §5.9.5: the quiescence acknowledgement -------------------------
  // `show.stop` waits for a render node to confirm it applied its directive.
  // Waiters resolve on the ack, or false on timeout / no node.
  //
  // An `exact` waiter (show.stop, v0.4.8 row 6) resolves only on the
  // acknowledgement of its own version, and never on a resync's: during the
  // wait the control plane's version IS the stop's, so a reconnect's (or a
  // gap's) `show.resync` is stamped with it, and its acknowledgement says
  // nothing about the stop, which the resync does not carry (§5.9.6). The
  // others resolve on any version at or above theirs.
  const ackWaiters = new Set<{ version: number; exact: boolean; resolve: (acked: boolean) => void }>();

  function noteApplied(session: ClientSession, version: number): void {
    if (!session.render) return;
    const prev = session.render.lastApplied;
    if (prev !== null && version <= prev) return; // stale ack: log and ignore
    session.render.lastApplied = version;
    const ownAck = !session.render.resyncs.includes(version);
    signals.onApplied(version, !ownAck);
    // Step 7c: the restarted engine applied its resync; send it the starts.
    if (!ownAck && session.render.reissue !== undefined) {
      const reissue = session.render.reissue;
      delete session.render.reissue;
      reissueOutputs(reissue);
    }
    if (ownAck) {
      session.render.acked.push(version);
      if (session.render.acked.length > 64) session.render.acked.shift();
    }
    for (const waiter of [...ackWaiters]) {
      const met = waiter.exact ? ownAck && version === waiter.version : version >= waiter.version;
      if (met) {
        ackWaiters.delete(waiter);
        waiter.resolve(true);
      }
    }
  }

  function renderSessions(): ClientSession[] {
    return [...clients.values()].filter((c) => c.render !== null && !c.closed);
  }

  async function waitForGrace(ms: number, version: number, exact = false): Promise<boolean> {
    const nodes = renderSessions();
    if (nodes.length === 0) return false; // nothing can acknowledge
    const already = exact
      ? nodes.some((n) => n.render!.acked.includes(version))
      : nodes.some((n) => (n.render!.lastApplied ?? -1) >= version);
    if (already) return true;
    return new Promise<boolean>((resolve) => {
      const waiter = { version, exact, resolve };
      ackWaiters.add(waiter);
      const timer = setTimeout(() => {
        ackWaiters.delete(waiter);
        resolve(false);
      }, ms);
      timer.unref?.();
    });
  }

  const deps: DispatchDeps = {
    state,
    bridge,
    persistence: opts.persistence,
    rateLimiter,
    showStopGraceMs: opts.showStopGraceMs ?? 2000,
    houseRate: opts.houseRate,
    // SPEC §10.7: every control-plane action reaches the audit log. A ceiling
    // decision is one — it is the control plane deciding, on its own authority
    // and without asking anyone, how long a load may take. `warn` for a
    // refusal, `info` for a run: a refusal is an operator-visible outcome, a
    // run is a fact you want to be able to count afterwards.
    recordBoundDecision: (d) => {
      audit.record({
        kind: "preflight",
        event: d.event,
        outcome: d.outcome === "refused" ? "rejected" : "ok",
        errorCode: d.outcome === "refused" ? "E_PREFLIGHT_FAILED" : null,
        detail: { ...d, severity: d.outcome === "refused" ? "warn" : "info" },
      });
    },
    waitForAck: (ms, stateVersion) => waitForGrace(ms, stateVersion, true),
    onSent: (command, stateVersion) => signals.onAccepted(command, stateVersion),
    warn: opts.warn ?? ((m) => console.warn(m)),
  };
  const registry = buildRegistry(deps);

  // -- The ONE command path (Prompt 11 WU1) ----------------------------------
  // A session's command and an automation rule's action both run here:
  // dispatch() (preconditions, the one stateVersion bump, directives), then
  // the audit row, then the §5.4.1 stateChange, then the automation
  // evaluator hears what changed. An automation action is a command — same
  // preconditions, same audit, same acknowledgement rules — so it must not
  // have a second path (Prompt 11 §9).
  interface CommandRun {
    envelope: Envelope;
    role: Role;
    tokenId: string | null;
    connectionId: string;
    intentSource: string | null;
    systemHooks?: SystemHooks;
    /** Set when a rule (or autoFollow) is acting: audited as kind "automation". */
    automation?: { actor: string; event: string; detail: Record<string, unknown>; cause: Cause };
  }
  type CommandResult = { ok: true; stateVersion: number; data: Record<string, unknown> } | { ok: false; error: CpError };

  // §13.3 (v0.4.7): a rule's action is exempt from §10.7's command limiter.
  // That limiter (10 per burst, 5/s per connection per command family) is
  // flood protection for sessions. Applied to a rule — which dispatches on its
  // own connection, `automation:<ruleId>` — it refused the rule's 11th action
  // in a burst, below §13.3 #3's once per frame (WU7's finding; the user's
  // decision of 2026-09-27). Automation has its own bounds: once per frame
  // per rule (§13.3 #3), preflight's cycle refusal (§13.4), and the runtime's
  // self-trigger suppression.
  const { rateLimiter: _sessionsOnly, ...automationDeps } = deps;

  async function runCommand(run: CommandRun): Promise<CommandResult> {
    const { envelope } = run;
    const before = state.stateVersion;
    const snapBefore = snapshot(state);
    // Review4 step 5 (the user's ruling of 2026-11-16 23:45): an on-air
    // command's audit row cannot reject it. A failed write raises
    // `audit.sinkFailed` (the lane reads `sinkOk`) and the reply stands.
    // Every other command keeps R18's behaviour: the throw is the reply.
    const onAir = ON_AIR_COMMANDS.has(resolveCommand(envelope.command)?.command ?? envelope.command);
    const write = (rec: Parameters<AuditLog["record"]>[0]): void => {
      if (onAir) audit.tryRecord(rec);
      else audit.record(rec);
    };
    try {
      // Review4 step 7b: while a restarted control plane waits for the
      // engine's hello, a command could disturb the show it is about to
      // adopt. The code is E_RECOVERING's once SPEC §16 names it (drafted for
      // the user); until then E_FORBIDDEN_STATE, with the reason.
      const name = resolveCommand(envelope.command)?.command ?? envelope.command;
      if (commandsLocked() && !RECOVERY_EXEMPT.has(name)) {
        const leftS = Math.ceil((recovery!.lockedUntil - Date.now()) / 1000);
        throw new CpError(
          "E_FORBIDDEN_STATE",
          `the control plane restarted and is waiting for the engine's hello to adopt its show (up to ${leftS} s more)`,
          { reason: "recovering" },
        );
      }
      const out = await dispatch(run.automation ? automationDeps : deps, registry, {
        connectionId: run.connectionId,
        role: run.role,
        envelope,
        ...(run.systemHooks ? { systemHooks: run.systemHooks } : {}),
      });
      const alias = resolveCommand(envelope.command);
      const command = alias?.command ?? envelope.command;
      write({
        kind: run.automation ? "automation" : "command",
        ...(run.automation ? { event: run.automation.event, actor: run.automation.actor, detail: run.automation.detail } : {}),
        outcome: "ok",
        role: run.role,
        tokenId: run.tokenId,
        requestId: envelope.id,
        command,
        rawCommand: alias?.deprecated ? envelope.command : null,
        intentSource: run.intentSource,
        stateVersionBefore: before,
        stateVersionAfter: out.stateVersion,
      });
      // Fan deprecation warnings into every subscriber's own cursor.
      for (const rec of state.drainDeprecations()) {
        for (const client of clients.values()) {
          if (!client.closed && client.render === null) client.pendingDeprecations.push(rec);
        }
      }
      // SPEC §5.4.1: exactly one stateChange per accepted command, carrying
      // the response's stateVersion and observable no later than it.
      broadcastStateChange(out.stateVersion, [command]);
      afterAccepted(command, snapBefore, run.automation?.cause ?? NO_CAUSE);
      rememberOutputStart(command, envelope.payload);
      if (command === "show.load" || command === "show.unload") releaseHeldHandshakes(command);
      // §5.9.5: a `show.stop` that returns with outputs still active was not
      // acknowledged, and they are held. The lane announces it (v0.4.8 row 8).
      if (command === "show.stop" && state.outputsActive()) signals.held(out.stateVersion);
      hotkeyFired(run.intentSource);
      return { ok: true, stateVersion: out.stateVersion, data: out.data };
    } catch (err) {
      const e = err instanceof CpError ? err : new CpError("E_ENGINE", String(err));
      write({
        kind: run.automation ? "automation" : "command",
        ...(run.automation ? { event: run.automation.event, actor: run.automation.actor, detail: run.automation.detail } : {}),
        outcome: "rejected",
        role: run.role,
        tokenId: run.tokenId,
        requestId: envelope.id,
        command: envelope.command,
        errorCode: e.code,
        intentSource: run.intentSource,
        stateVersionBefore: before,
        stateVersionAfter: state.stateVersion,
      });
      // The binding fired whether or not the command it carried was accepted.
      hotkeyFired(run.intentSource);
      return { ok: false, error: e };
    }
  }

  // -- The automation evaluator (SPEC §13, AC-25) ---------------------------
  const automation = new AutomationEvaluator({
    state,
    ...(opts.automationClock ? { clock: opts.automationClock } : {}),
    // A rule's action: the one command path, as `operator` — §13.1's "the
    // same preconditions as a human operator's commands".
    execute: async (req): Promise<ActionOutcome> => {
      const r = await runCommand({
        envelope: { v: PROTOCOL_VERSION, id: randomUUID(), command: req.command, payload: req.payload },
        role: "operator",
        tokenId: null,
        connectionId: req.actor,
        intentSource: null,
        automation: { actor: req.actor, event: req.auditEvent, detail: req.detail, cause: req.cause },
      });
      return r.ok ? { ok: true, stateVersion: r.stateVersion } : { ok: false, code: r.error.code, message: r.error.message };
    },
    // Review4 step 5: a rule's bookkeeping row never stops the evaluator.
    audit: (rec) =>
      void audit.tryRecord({
        kind: "automation",
        event: rec.event,
        outcome: rec.outcome,
        actor: rec.actor,
        role: "operator",
        tokenId: null,
        ...(rec.command ? { command: rec.command } : {}),
        detail: rec.detail,
        stateVersionBefore: state.stateVersion,
        stateVersionAfter: state.stateVersion,
      }),
  });
  // Review4 step 7b: the journal carries the automation backlog, so an
  // adoption can say how many rule actions a crash dropped.
  opts.persistence.journalExtras?.(() => ({ automationBacklog: automation.backlog() }));

  /**
   * An accepted command changed state: keep the evaluator's lifecycle in step,
   * and raise the triggers the change is. Runs in the same turn as the
   * acceptance, so a trigger is observed the moment its condition became true.
   */
  function afterAccepted(command: string, before: Snapshot, cause: Cause): void {
    if (command === "show.load" && state.pkg) automation.load(state.pkg);
    else if (command === "show.unload") automation.unload();
    else if (command === "show.start") automation.showStarted();
    else if (command === "show.stop") automation.showStopped();
    // §13.5 / AC-25 #2: a hold cancels every pending action, in this turn —
    // whichever command engaged it (`automation.hold`, or a `snapshot.recall`
    // restoring a held snapshot).
    const observedAt = automation.now();
    if (!before.automationHold && state.automationHold) automation.holdEngaged(observedAt);
    const after = snapshot(state);
    for (const e of stateChanges(before, after)) automation.fire(e, cause, observedAt);
    // B3: mediaStart is control-plane-side — the take whose item goes on air,
    // applied. Only a take puts an item on air (§13.4.1's rows: view.take,
    // view.cut; snapshot.recall is not a take).
    if (command === "view.take" || command === "view.cut") {
      for (const [itemRef, to] of after.itemStates) {
        const from = before.itemStates.get(itemRef) ?? "READY";
        const onAir = (s: string) => s === "LIVE" || s === "PLAYING";
        if (onAir(to) && !onAir(from)) automation.fire({ kind: "mediaStart", itemRef }, cause, observedAt);
      }
    }
  }

  /**
   * A command arrived carrying `intentSource` (`adapter/profile:intent`): the
   * binding named by its intent id fired. `hotkey` rules on that binding fire
   * whatever the carried command's own outcome — the actuation happened.
   */
  function hotkeyFired(intentSource: string | null): void {
    if (!intentSource) return;
    const bindingId = intentSource.slice(intentSource.lastIndexOf(":") + 1);
    automation.fire({ kind: "hotkey", bindingId }, NO_CAUSE, automation.now());
  }

  /**
   * `streamTransportState` as last observed on the §10.1 tick (v0.4.6):
   * `streamHealth` fires on a change TO a transport state. The first value
   * observed is the baseline and fires nothing — after a control-plane
   * restart a stream already live is not news. `none` is a stub (no stream
   * has started), never a state a rule names, but a change FROM it counts:
   * the first stream going live fires a `live` rule.
   */
  let lastTransport: string | undefined;
  function observeTransport(token: string | undefined): void {
    if (token === undefined) return; // an engine build older than the field
    const prev = lastTransport;
    lastTransport = token;
    if (prev === undefined || prev === token) return;
    if (token === "live" || token === "reconnecting" || token === "closed") {
      automation.fire({ kind: "streamHealth", state: token, from: prev }, NO_CAUSE, automation.now());
    }
  }

  /** SPEC §5.4.1: one stateChange frame per accepted command, to observers. */
  function broadcastStateChange(stateVersion: number, changed: string[]): void {
    const frame = {
      v: PROTOCOL_VERSION,
      kind: "stateChange" as const,
      stateVersion,
      changed,
      state: state.statusSnapshot(renderNodeStatus()),
    };
    for (const client of clients.values()) {
      if (client.closed || client.render !== null) continue; // render nodes get directives, not state frames
      client.push(frame);
    }
  }

  function renderNodeStatus(): { connected: boolean; clockState: string; lastAppliedStateVersion: number | null } {
    const nodes = renderSessions();
    const fresh = world.last !== null && Date.now() - world.last.receivedAt <= 2000;
    return {
      connected: nodes.length > 0 && fresh,
      clockState: fresh ? (state.showState === "RUNNING" ? "RUNNING" : "STOPPED") : "UNKNOWN",
      lastAppliedStateVersion: nodes.length ? (nodes[0]!.render!.lastApplied ?? null) : null,
    };
  }

  /** SPEC §5.9.4: the full snapshot, addressed to one connection. */
  /**
   * Builds `show.resync` and sends it on `session`. Throws if the snapshot
   * cannot be built; returns whether the socket accepted it. `sendDirect`
   * swallows a send throw and reports `false` (backpressure or a failed
   * send), so a caller that ignores the result cannot tell a resync that went
   * out from one that did not.
   */
  function sendResync(session: ClientSession, omitViewItemEnd = false): boolean {
    const now = Date.now();
    const payload = state.resyncSnapshot(now);
    // Review4 step 7b: the adopting resync sends no end, so the engine keeps
    // the one it has pending (a resync without one never cancels it, §5.9.4)
    // rather than a control-plane estimate of it.
    if (omitViewItemEnd) delete payload.viewItemEnd;
    // Noted before the send: the acknowledgement can only follow it.
    if (session.render) {
      session.render.resyncs.push(state.stateVersion);
      if (session.render.resyncs.length > 16) session.render.resyncs.shift();
    }
    const sent =
      session.render?.registration.sendDirect({
        command: RESYNC_COMMAND,
        target: {},
        payload,
        stateVersion: state.stateVersion,
      }) ?? false;
    // SPEC v0.4.8 row 3: a resync that re-establishes the on-air item's end
    // says what it did. An item whose duration elapsed during the outage ends
    // on receipt, late by `overdueMs`, on purpose.
    const end = payload.viewItemEnd as ResyncViewItemEnd | undefined;
    if (sent && end !== undefined) {
      const durationFrames = state.pkg?.items.get(end.itemRef)?.durationFrames ?? null;
      const rate = state.pkg?.houseRate ?? null;
      const elapsedMs = state.viewItemTakenAtMs === null ? null : now - state.viewItemTakenAtMs;
      const durationMs = durationFrames === null || rate === null ? null : (durationFrames * 1000) / rate;
      audit.tryRecord({
        kind: "command",
        outcome: "ok",
        role: session.role,
        tokenId: session.tokenId,
        command: RESYNC_COMMAND,
        event: "resync.viewItemEnd",
        stateVersionBefore: state.stateVersion,
        stateVersionAfter: state.stateVersion,
        detail: {
          itemRef: end.itemRef,
          remainingFrames: end.remainingFrames,
          durationFrames,
          elapsedMs,
          overdueMs: elapsedMs === null || durationMs === null ? null : Math.max(0, Math.round(elapsedMs - durationMs)),
        },
      });
    }
    return sent;
  }

  const http = createServer((req: IncomingMessage, res: ServerResponse) => {
    if (req.method === "GET" && req.url === `${WS_PATH}/status`) {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ v: PROTOCOL_VERSION, ok: true, ...state.statusSnapshot(renderNodeStatus()) }));
      return;
    }
    res.statusCode = 404;
    res.end();
  });

  const wss = new WebSocketServer({ noServer: true });

  http.on("upgrade", (req: IncomingMessage, socket: Duplex, head: Buffer) => {
    const url = new URL(req.url ?? "", "http://localhost");
    if (url.pathname !== WS_PATH) {
      socket.destroy();
      return;
    }
    const rawAuth = req.headers["authorization"];
    const bearer =
      typeof rawAuth === "string" && rawAuth.startsWith("Bearer ") ? rawAuth.slice(7) : undefined;
    const assertedRole = req.headers["x-nbe-role"] as string | undefined;
    const ar = authenticate(auth, bearer, assertedRole);
    if (!ar.ok) {
      // The rejection stands whether or not it could be recorded.
      audit.tryRecord({
        kind: "auth",
        outcome: "rejected",
        role: null,
        tokenId: null,
        remote: req.socket.remoteAddress ?? null,
        reason: ar.reason ?? "auth failed",
      });
      // SPEC §5.3: the reason returned to an unauthenticated peer is generic;
      // the specific cause goes to the audit log above.
      const frame = JSON.stringify(
        errorResponse(randomUUID(), state.stateVersion, "E_AUTH", "authentication failed"),
      );
      socket.write(
        `HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: ${Buffer.byteLength(frame)}\r\n\r\n${frame}`,
      );
      socket.destroy();
      return;
    }
    try {
      audit.record({
        kind: "auth",
        outcome: "ok",
        role: ar.role!,
        tokenId: ar.tokenId!,
        remote: req.socket.remoteAddress ?? null,
      });
    } catch {
      // The user's ruling of 2026-11-16 23:45 (review4 step 5): an auth is
      // refused while the audit cannot be written. Before, this throw left
      // the upgrade listener uncaught and ended the control plane. The lane
      // raises `audit.sinkFailed`; a connection already open keeps working.
      const frame = JSON.stringify(
        errorResponse(randomUUID(), state.stateVersion, "E_DISK", "the audit log cannot be written: new connections are refused until it can"),
      );
      socket.write(
        `HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: ${Buffer.byteLength(frame)}\r\n\r\n${frame}`,
      );
      socket.destroy();
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => wss.emit("connection", ws, req, ar));
  });

  wss.on("connection", (ws: WebSocket, _req: IncomingMessage, ar: AuthResult) => {
    const connId = randomUUID();
    const session: ClientSession = {
      role: ar.role!,
      tokenId: ar.tokenId!,
      telemetryTimer: null,
      closed: false,
      tail: Promise.resolve(),
      push: (frame: unknown): boolean => {
        if (session.closed) return false;
        if (ws.bufferedAmount > 256 * 1024) return false; // droppable under backpressure (§5.4.1)
        ws.send(JSON.stringify(frame));
        return true;
      },
      render: null,
      pendingDeprecations: [],
      terminate: () => ws.terminate(),
    };
    clients.set(connId, session);

    const startTelemetry = (intervalMs: number): void => {
      if (session.telemetryTimer) clearInterval(session.telemetryTimer);
      session.telemetryTimer = setInterval(() => {
        if (session.closed) return;
        if (ws.bufferedAmount > 256 * 1024) return; // backpressure: coalesce
        // Each subscriber drains its OWN cursor: a shared drain means the
        // first tick to fire steals the warning from every other subscriber.
        const mine = session.pendingDeprecations;
        session.pendingDeprecations = [];
        ws.send(
          JSON.stringify({
            v: PROTOCOL_VERSION,
            kind: "telemetry",
            data: { ...buildTick(state, world, Date.now()), deprecationWarnings: mine },
          }),
        );
      }, intervalMs);
      session.telemetryTimer.unref?.();
    };
    const stopTelemetry = (): void => {
      if (session.telemetryTimer) clearInterval(session.telemetryTimer);
      session.telemetryTimer = null;
    };

    /**
     * Everything this connection registered, released: the client entry, the
     * render registration, the telemetry timer. Idempotent, because a failed
     * handshake and the close that follows it both call it.
     */
    const forget = (): void => {
      if (session.awaitingHello) clearTimeout(session.awaitingHello.timer);
      session.awaitingHello = null;
      if (session.heldHandshake) {
        session.heldHandshake = null;
        signals.unidentified(null, "the engine disconnected");
      }
      if (session.render !== null && session.render.hello !== undefined) engineGoneAt = Date.now();
      session.closed = true;
      stopTelemetry();
      session.render?.registration.unregister();
      clients.delete(connId);
    };

    // Listeners FIRST: this connection's frames are processed from its first
    // moment, and its close is always heard. They used to be installed after
    // the render handshake below, so a throw in the handshake skipped them:
    // the session stayed registered, its frames went unprocessed, and its
    // close went unnoticed (PR #38's falsification F3, the zombie).
    ws.on("message", (buf: Buffer) => {
      // Commands execute strictly in arrival order on this connection;
      // an async handler (show.load's preflight subprocess) must not race
      // the next command.
      session.tail = session.tail.then(() =>
        handleMessage(buf.toString("utf8")).catch((err) => {
          ws.send(
            JSON.stringify(errorResponse(randomUUID(), state.stateVersion, "E_ENGINE", String(err))),
          );
        }),
      );
    });

    ws.on("close", forget);
    ws.on("error", forget);

    // Render-role sessions receive directives: register a fan-out sender, and
    // complete SPEC §5.9.4's handshake: show.resync goes out before any other
    // directive on this connection. Directives issued while no node was
    // connected are never replayed; the snapshot is the recovery mechanism.
    //
    // **The handshake invariant: a connect either completes its resync or
    // leaves NO registration. There is no third state.** Register, build and
    // send run inside one guard. Any throw (the snapshot's strict parse, a
    // builder) or an unsent resync (`sendDirect` reports a send failure as
    // `false`) deregisters the session, closes the socket with 1011 and a
    // reason the engine sees, and is surfaced in the audit log
    // (`resync.handshakeFailed`) and through `warn`. Never a silent swallow.
    // The engine's reconnect loop then retries against a clean slate.
    if (session.role === "render") {
      const renderSender = (frame: RenderDirective): boolean => {
        if (session.closed) return false;
        if (ws.bufferedAmount > 256 * 1024) return false; // backpressure: drop, never block dispatch
        ws.send(JSON.stringify(frame));
        return true;
      };
      const handshake = (adopted: boolean): void => {
      try {
        session.render = { registration: wsBridge.register(renderSender), lastApplied: null, acked: [], resyncs: [] };
        if (!sendResync(session, adopted)) throw new Error("the socket did not accept show.resync (a failed or refused send)");
        engineResynced = true;
        signals.onRegistered();
        if (adopted) broadcastStateChange(state.stateVersion, ["engine:adopted"]);
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        forget();
        audit.tryRecord({
          kind: "command",
          outcome: "rejected",
          role: session.role,
          tokenId: session.tokenId,
          command: RESYNC_COMMAND,
          event: "resync.handshakeFailed",
          stateVersionBefore: state.stateVersion,
          stateVersionAfter: state.stateVersion,
          detail: { error },
        });
        deps.warn?.(`show.resync handshake failed; the render connection is closed and holds no registration: ${error}`);
        ws.close(1011, "show.resync handshake failed");
      }
      };
      const firstEngine =
        !recovering() && ((opts.awaitFirstHello !== undefined && !engineResynced) || unidentifiedHold !== null);
      if (recovering() || firstEngine) {
        // Review4 step 7b: a restarted control plane resyncs nothing until
        // the engine says what it holds. The engine holds its output until a
        // resync (§5.9.4 rule 2), and this connection is not registered, so
        // no directive reaches it first (rule 1). An engine that never says
        // hello gets today's resync after the wait.
        session.awaitingHello = {
          handshake,
          timer: setTimeout(() => {
            if (!session.awaitingHello) return;
            session.awaitingHello = null;
            if (recovering()) notAdopted("noHello", {});
            handshake(false);
          }, (firstEngine ? (opts.awaitFirstHello?.helloWaitMs ?? opts.restart?.helloWaitMs) : opts.restart?.helloWaitMs) ?? 2000),
        };
      } else handshake(false);
    }

    /**
     * SPEC §5.9.4 rule 5 with its failure half (v0.4.8 row 7; the user's
     * ruling of 2026-10-14). A failed re-resync is audited
     * (`resync.requestFailed`) and warned, never swallowed, and the failure's
     * class decides the session's fate:
     * - **The snapshot cannot be built** (a throw): persistent. The next
     *   request would build the same snapshot, so the session is torn down
     *   in the handshake's order (forget, audit, warn, close 1011) and
     *   recovery passes to the engine's reconnect loop, whose handshake
     *   (rule 1) retries against a clean slate at the engine's backoff.
     * - **The send is refused** (`false`): transient. `sendDirect` reports
     *   backpressure, or a connection already gone, as `false`; a sender
     *   throw is unreachable here (`ws` throws only while CONNECTING). The
     *   session is kept, and the next directive's gap re-requests.
     * Each failure is its own audit row: the audit is the record.
     */
    function resyncOnRequest(reason: string): void {
      const requestFailed = (detail: Record<string, unknown>): void => {
        audit.tryRecord({
          kind: "command",
          outcome: "rejected",
          role: session.role,
          tokenId: session.tokenId,
          command: RESYNC_COMMAND,
          event: "resync.requestFailed",
          stateVersionBefore: state.stateVersion,
          stateVersionAfter: state.stateVersion,
          detail,
        });
      };
      let sent: boolean;
      try {
        sent = sendResync(session);
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        forget();
        requestFailed({ error, cause: "snapshot", reason, sessionKept: false });
        deps.warn?.(
          `show.resync on the engine's request failed (the snapshot could not be built); the render connection is closed and holds no registration: ${error}`,
        );
        ws.close(1011, "show.resync request failed");
        return;
      }
      if (!sent) {
        const error = "the socket did not accept show.resync (a refused send)";
        const sessionKept = !session.closed;
        requestFailed({ error, cause: "send", reason, sessionKept });
        deps.warn?.(
          sessionKept
            ? "show.resync on the engine's request was not sent (a refused send); the session is kept, and the engine's next request retries"
            : "show.resync on the engine's request was not sent: the render connection had already closed",
        );
      }
    }

    async function handleMessage(raw: string): Promise<void> {
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch {
        ws.send(JSON.stringify(errorResponse(randomUUID(), state.stateVersion, "E_BAD_PAYLOAD", "not JSON")));
        return;
      }

      // Prompt 08 (D2, WS-only): an adapter (Companion, keyboard, ...) may
      // attach `intentSource` — `adapter/profile:intent`, e.g.
      // `companion/xl-a:take-1` — alongside the §5.4 envelope. It rides the
      // audit path only: stripped before envelope validation, never seen by
      // dispatch(), recorded on the command audit rows below. Absent =
      // software/direct command. Same transport, same auth, no second protocol.
      // The format is enforced (not just length): audit identity must stay
      // machine-readable, and a free-form client string is spoofable noise.
      let intentSource: string | null = null;
      if (typeof parsed === "object" && parsed !== null && "intentSource" in parsed) {
        const rawSource = (parsed as Record<string, unknown>).intentSource;
        if (
          typeof rawSource !== "string" ||
          rawSource.length > 256 ||
          !/^[A-Za-z][A-Za-z0-9_-]{0,31}\/[^\s/:]{1,64}:[^\s/:]{1,64}$/.test(rawSource)
        ) {
          ws.send(
            JSON.stringify(errorResponse(randomUUID(), state.stateVersion, "E_BAD_PAYLOAD", "invalid intentSource")),
          );
          return;
        }
        intentSource = rawSource;
        const { intentSource: _dropped, ...rest } = parsed as Record<string, unknown>;
        void _dropped;
        parsed = rest;
      }

      // Engine frames (render-role only)
      const engine = EngineFrameSchema.safeParse(parsed);
      if (engine.success) {
        if (session.role !== "render") return;
        const frame = engine.data;
        if (frame.kind === "engineTelemetry") {
          ingestEngineFrame(world, frame, Date.now());
          observeTransport(frame.streamTransportState);
          // SPEC §5.9.4: the snapshot's `viewItemStartFrame` needs a clock,
          // and the engine owns the only one. Recording it here keeps the
          // field at worst one tick stale rather than an outage's length.
          state.lastKnownMasterFrame = frame.masterClockFrame;
        } else if (frame.kind === "appliedStateVersion") {
          // SPEC §5.9.5: the signal show.stop's grace window waits for.
          noteApplied(session, frame.stateVersion);
        } else if (frame.kind === "engineHello") {
          // Review4 step 7a: what the engine holds, first on every connection.
          // Step 7b: to a restarted control plane, it decides the handshake.
          const waiting = session.awaitingHello;
          if (waiting) {
            session.awaitingHello = null;
            clearTimeout(waiting.timer);
            // Review4 case C: a restarted control plane that cannot adopt
            // (recover() says why), or one with no journal at all whose first
            // engine is running a show it cannot know (awaitFirstHello).
            const decision: RecoveryDecision = recovering()
              ? recover(frame)
              : frame.showRunning && state.showState !== "RUNNING"
                ? { kind: "refuse", reason: unidentifiedHold?.reason ?? "noJournal" }
                : { kind: "resync" };
            if (decision.kind === "resync") unidentifiedHold = null;
            if (decision.kind === "refuse") holdUnidentified(session, () => waiting.handshake(false), decision.reason, frame);
            else waiting.handshake(decision.kind === "adopt");
          }
          if (session.render !== null) noteHello(session.render, frame);
        } else if (frame.kind === "resyncRequest") {
          // SPEC §5.9.4 rule 5: the engine lost continuity; hand it the
          // snapshot, and never fail silently (v0.4.8 row 7).
          resyncOnRequest(frame.reason);
        } else if (frame.kind === "audioLevelCrossing") {
          // B1: a crossing is an event for a consumer, not a state to apply.
          // Handed over synchronously — the evaluator's one-frame budget
          // (AC-25 #1) starts when this frame arrives.
          for (const listener of engineEventListeners) listener(frame);
          // The audioLevel adapter: the crossing is observed as it arrives;
          // the engine computed it on the block it happened in (v0.4.7).
          automation.fire(
            {
              kind: "audioLevel",
              bus: frame.bus,
              thresholdDbfs: frame.thresholdDbfs,
              direction: frame.direction,
              levelDbfs: frame.levelDbfs,
              masterFrame: frame.masterFrame,
              ts: frame.ts,
            },
            NO_CAUSE,
            automation.now(),
          );
        } else if (frame.kind === "itemEvent") {
          const before = state.stateVersion;
          const snapBefore = snapshot(state);
          if (frame.event === "end") state.markDone(frame.itemRef);
          else if (frame.event === "missing") state.markMissing(frame.itemRef);
          // A clip that failed mid-play while the View holds its last frame
          // (review4 step 8): ERROR without the fallback flag, and the alert.
          else if (frame.event === "decodeError" && frame.detail?.startsWith(HELD_DETAIL_PREFIX)) {
            state.markErrorHeld(frame.itemRef, frame.detail);
            signals.evaluate();
          } else state.markError(frame.itemRef);
          state.bump();
          opts.persistence.onDirty();
          // Review4 step 5: a failed row must not skip what follows (the
          // item's automation, autoFollow's advance).
          audit.tryRecord({
            kind: "command",
            outcome: "ok",
            role: session.role,
            tokenId: session.tokenId,
            command: `engine:${frame.event}`,
            stateVersionBefore: before,
            stateVersionAfter: state.stateVersion,
          });
          // The engine's item transitions are state changes too, and
          // `PLAYING → DONE` is mediaEnd — the only point completion is true
          // (a stopped item's late `end` is dropped by `markDone`; §13.4.1).
          const observedAt = automation.now();
          const snapAfter = snapshot(state);
          for (const e of stateChanges(snapBefore, snapAfter)) automation.fire(e, NO_CAUSE, observedAt);
          if (
            frame.event === "end" &&
            snapBefore.itemStates.get(frame.itemRef) === "PLAYING" &&
            snapAfter.itemStates.get(frame.itemRef) === "DONE"
          ) {
            automation.fire({ kind: "mediaEnd", itemRef: frame.itemRef }, NO_CAUSE, observedAt);
            // WU4: autoFollow — the item completed; advance to the next item
            // in the rundown (§3.1), through the evaluator's queue so hold
            // governs it (§13.5 #2).
            const pkg = state.pkg;
            if (pkg?.items.get(frame.itemRef)?.autoFollow) {
              const order = [...pkg.items.keys()];
              const next = order[order.indexOf(frame.itemRef) + 1];
              automation.autoFollow(frame.itemRef, next, NO_CAUSE, observedAt);
            }
          }
        }
        return;
      }

      const env = EnvelopeSchema.safeParse(parsed);
      if (!env.success) {
        ws.send(
          JSON.stringify(errorResponse(randomUUID(), state.stateVersion, "E_BAD_PAYLOAD", "invalid envelope")),
        );
        return;
      }
      const envelope = env.data;
      const r = await runCommand({
        envelope,
        role: session.role,
        tokenId: session.tokenId,
        connectionId: connId,
        intentSource,
        systemHooks: { onTelemetrySubscribe: startTelemetry, onTelemetryUnsubscribe: stopTelemetry },
      });
      if (r.ok) {
        // Command responses are never dropped (addendum §2.8).
        ws.send(JSON.stringify(okResponse(envelope.id, r.stateVersion, r.data)));
      } else {
        const e = r.error;
        ws.send(JSON.stringify(errorResponse(envelope.id, state.stateVersion, e.code, e.message, e.details)));
      }
    }
  });

  const port = opts.port ?? 0;
  const host = opts.host ?? "127.0.0.1";
  await new Promise<void>((resolve, reject) => {
    http.once("error", reject);
    http.listen(port, host, () => resolve());
  });

  return {
    http,
    port: (http.address() as { port: number }).port,
    bridge,
    wsBridge,
    awaitApplied: (version: number, ms: number) => waitForGrace(ms, version),
    onEngineEvent(listener) {
      engineEventListeners.add(listener);
      return () => engineEventListeners.delete(listener);
    },
    automation,
    async close() {
      for (const s of clients.values()) {
        s.closed = true;
        if (s.telemetryTimer) clearInterval(s.telemetryTimer);
        s.render?.registration.unregister();
        // Without this, http.close() never resolves: an open WebSocket is an
        // open connection, and the server waits for it indefinitely.
        s.terminate();
      }
      clients.clear();
      for (const waiter of ackWaiters) waiter.resolve(false);
      ackWaiters.clear();
      clearInterval(signalTimer);
      clearInterval(tickerTimer);
      signals.close();
      automation.close();
      wss.close();
      await new Promise<void>((resolve) => http.close(() => resolve()));
    },
  };
}
