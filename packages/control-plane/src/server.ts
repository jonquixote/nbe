//! Server (addendum 02a §1.1/1.3, §2.5-2.8): ONE node HTTP server owns the
//! WebSocket upgrade, `GET /nbe/v0.3/status`, and (Prompt 08) the future
//! Companion HTTP endpoint. No second transport.

import { createServer, type Server, type IncomingMessage, type ServerResponse } from "node:http";
import { randomUUID, createHash, timingSafeEqual } from "node:crypto";
import { WebSocketServer, type WebSocket } from "ws";
import type { Duplex } from "node:stream";

import { AuditLog } from "./audit.js";
import { buildRegistry, dispatch, type DispatchDeps } from "./dispatch.js";
import {
  EngineFrameSchema,
  EnvelopeSchema,
  WS_PATH,
  PROTOCOL_VERSION,
  CpError,
  errorResponse,
  okResponse,
  resolveCommand,
  RESYNC_COMMAND,
  type Role,
} from "./protocol.js";
import {
  WsRenderBridge,
  type RenderBridge,
  type RenderDirective,
  type RenderRegistration,
} from "./render-bridge.js";
import type { ControlPlaneState, DeprecationRecord } from "./state.js";
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
  /** Warning sink; defaults to console.warn. Tests assert exact strings. */
  warn?: (message: string) => void;
}

export async function createControlPlaneServer(opts: ServerOptions): Promise<ControlPlaneServer> {
  const { state, audit, auth } = opts;
  const world: WorldTelemetry = newWorldTelemetry();
  const wsBridge = new WsRenderBridge();
  const bridge: RenderBridge = opts.bridge ?? wsBridge;
  const rateLimiter = new RateLimiter();

  const clients = new Map<string, ClientSession>();

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
    waitForGrace,
    emitDirectivesNow: (directives, stateVersion) => {
      for (const d of directives) {
        bridge.send({ command: d.command, target: d.target ?? {}, payload: d.payload, stateVersion });
      }
    },
    warn: opts.warn ?? ((m) => console.warn(m)),
  };
  const registry = buildRegistry(deps);

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
    session.render?.registration.sendDirect({
      command: RESYNC_COMMAND,
      target: {},
      payload: state.resyncSnapshot(),
      stateVersion: state.stateVersion,
    });
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

      // Engine frames (render-role only)
      const engine = EngineFrameSchema.safeParse(parsed);
      if (engine.success) {
        if (session.role !== "render") return;
        const frame = engine.data;
        if (frame.kind === "engineTelemetry") {
          ingestEngineFrame(world, frame, Date.now());
        } else if (frame.kind === "appliedStateVersion") {
          // SPEC §5.9.5: the signal show.stop's grace window waits for.
          noteApplied(session, frame.stateVersion);
        } else if (frame.kind === "resyncRequest") {
          // SPEC §5.9.4: the engine lost continuity; hand it the snapshot.
          sendResync(session);
        } else if (frame.kind === "itemEvent") {
          const before = state.stateVersion;
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
      const before = state.stateVersion;

      try {
        const out = await dispatch(deps, registry, {
          connectionId: connId,
          role: session.role,
          envelope,
          systemHooks: { onTelemetrySubscribe: startTelemetry, onTelemetryUnsubscribe: stopTelemetry },
        });
        const alias = resolveCommand(envelope.command);
        audit.record({
          kind: "command",
          outcome: "ok",
          role: session.role,
          tokenId: session.tokenId,
          requestId: envelope.id,
          command: alias?.command ?? envelope.command,
          rawCommand: alias?.deprecated ? envelope.command : null,
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
        broadcastStateChange(out.stateVersion, [alias?.command ?? envelope.command]);
        // Command responses are never dropped (addendum §2.8).
        ws.send(JSON.stringify(okResponse(envelope.id, out.stateVersion, out.data)));
      } catch (err) {
        const e = err instanceof CpError ? err : new CpError("E_ENGINE", String(err));
        audit.record({
          kind: "command",
          outcome: "rejected",
          role: session.role,
          tokenId: session.tokenId,
          requestId: envelope.id,
          command: envelope.command,
          errorCode: e.code,
          stateVersionBefore: before,
          stateVersionAfter: state.stateVersion,
        });
        ws.send(JSON.stringify(errorResponse(envelope.id, state.stateVersion, e.code, e.message)));
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
      wss.close();
      await new Promise<void>((resolve) => http.close(() => resolve()));
    },
  };
}
