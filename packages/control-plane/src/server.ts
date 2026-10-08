//! Server (addendum 02a §1.1/1.3, §2.5-2.8): ONE node HTTP server owns the
//! WebSocket upgrade and `GET /nbe/v0.3/status`. Prompt 08 is WS-only:
//! Companion speaks the §5.4/§16 WS bus; the HTTP listener is §10.4's health
//! endpoint, full stop. No second transport.

import { createServer, type Server, type IncomingMessage, type ServerResponse } from "node:http";
import { randomUUID, createHash, timingSafeEqual } from "node:crypto";
import { WebSocketServer, type WebSocket } from "ws";
import type { Duplex } from "node:stream";

import { AuditLog } from "./audit.js";
import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import {
  EngineFrameSchema,
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
import type { ControlPlaneState, DeprecationRecord } from "./state.js";
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
import type { PersistenceHooks } from "./persistence.js";
import { buildTick, ingestEngineFrame, newWorldTelemetry, type WorldTelemetry } from "./telemetry.js";

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

interface ClientSession {
  role: Role;
  tokenId: string;
  telemetryTimer: ReturnType<typeof setInterval> | null;
  closed: boolean;
  /** Commands are processed strictly in arrival order. */
  tail: Promise<void>;
  /** Pushes a server-initiated frame (SPEC §5.4.1); false when dropped. */
  push: (frame: unknown) => boolean;
  /** Render sessions only: this session's own directive channel. */
  render: { registration: RenderRegistration; lastApplied: number | null } | null;
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
  const wsBridge = new WsRenderBridge();
  const bridge: RenderBridge = opts.bridge ?? wsBridge;
  const rateLimiter = new RateLimiter();

  const clients = new Map<string, ClientSession>();
  const engineEventListeners = new Set<(frame: AudioLevelCrossingFrame) => void>();

  // -- SPEC §5.9.5: the quiescence acknowledgement -------------------------
  // `show.stop` waits for a render node to confirm it applied the stop
  // directives. Waiters resolve on the ack, or false on timeout / no node.
  const ackWaiters = new Set<{ version: number; resolve: (acked: boolean) => void }>();

  function noteApplied(session: ClientSession, version: number): void {
    if (!session.render) return;
    const prev = session.render.lastApplied;
    if (prev !== null && version <= prev) return; // stale ack: log and ignore
    session.render.lastApplied = version;
    for (const waiter of [...ackWaiters]) {
      if (version >= waiter.version) {
        ackWaiters.delete(waiter);
        waiter.resolve(true);
      }
    }
  }

  function renderSessions(): ClientSession[] {
    return [...clients.values()].filter((c) => c.render !== null && !c.closed);
  }

  async function waitForGrace(ms: number, version: number): Promise<boolean> {
    const nodes = renderSessions();
    if (nodes.length === 0) return false; // nothing can acknowledge; force it
    if (nodes.some((n) => (n.render!.lastApplied ?? -1) >= version)) return true;
    return new Promise<boolean>((resolve) => {
      const waiter = { version, resolve };
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
    waitForGrace,
    emitDirectivesNow: (directives, stateVersion) => {
      for (const d of directives) {
        bridge.send({ command: d.command, target: d.target ?? {}, payload: d.payload, stateVersion });
      }
    },
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
    try {
      const out = await dispatch(run.automation ? automationDeps : deps, registry, {
        connectionId: run.connectionId,
        role: run.role,
        envelope,
        ...(run.systemHooks ? { systemHooks: run.systemHooks } : {}),
      });
      const alias = resolveCommand(envelope.command);
      const command = alias?.command ?? envelope.command;
      audit.record({
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
      hotkeyFired(run.intentSource);
      return { ok: true, stateVersion: out.stateVersion, data: out.data };
    } catch (err) {
      const e = err instanceof CpError ? err : new CpError("E_ENGINE", String(err));
      audit.record({
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
    audit: (rec) =>
      audit.record({
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
  function sendResync(session: ClientSession): void {
    const now = Date.now();
    const payload = state.resyncSnapshot(now);
    session.render?.registration.sendDirect({
      command: RESYNC_COMMAND,
      target: {},
      payload,
      stateVersion: state.stateVersion,
    });
    // SPEC v0.4.8 row 3: a resync that re-establishes the on-air item's end
    // says what it did. An item whose duration elapsed during the outage ends
    // on receipt, late by `overdueMs`, on purpose.
    const end = payload.viewItemEnd as ResyncViewItemEnd | undefined;
    if (end !== undefined) {
      const durationFrames = state.pkg?.items.get(end.itemRef)?.durationFrames ?? null;
      const rate = state.pkg?.houseRate ?? null;
      const elapsedMs = state.viewItemTakenAtMs === null ? null : now - state.viewItemTakenAtMs;
      const durationMs = durationFrames === null || rate === null ? null : (durationFrames * 1000) / rate;
      audit.record({
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
      audit.record({
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
    audit.record({
      kind: "auth",
      outcome: "ok",
      role: ar.role!,
      tokenId: ar.tokenId!,
      remote: req.socket.remoteAddress ?? null,
    });
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

    // Render-role sessions receive directives: register a fan-out sender.
    if (session.role === "render") {
      const renderSender = (frame: RenderDirective): boolean => {
        if (session.closed) return false;
        if (ws.bufferedAmount > 256 * 1024) return false; // backpressure: drop, never block dispatch
        ws.send(JSON.stringify(frame));
        return true;
      };
      session.render = { registration: wsBridge.register(renderSender), lastApplied: null };
      // SPEC §5.9.4: show.resync goes out before any other directive on this
      // connection. Directives issued while no node was connected are never
      // replayed — the snapshot is the recovery mechanism.
      sendResync(session);
    }

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

    ws.on("close", () => {
      session.closed = true;
      stopTelemetry();
      session.render?.registration.unregister();
      clients.delete(connId);
    });
    ws.on("error", () => {
      session.closed = true;
      stopTelemetry();
      session.render?.registration.unregister();
      clients.delete(connId);
    });

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
        } else if (frame.kind === "resyncRequest") {
          // SPEC §5.9.4: the engine lost continuity; hand it the snapshot.
          sendResync(session);
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
          else state.markError(frame.itemRef);
          state.bump();
          opts.persistence.onDirty();
          audit.record({
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
      automation.close();
      wss.close();
      await new Promise<void>((resolve) => http.close(() => resolve()));
    },
  };
}
