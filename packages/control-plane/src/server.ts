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
  type Role,
} from "./protocol.js";
import { MockRenderBridge, WsRenderBridge, type RenderBridge, type RenderDirective } from "./render-bridge.js";
import type { ControlPlaneState } from "./state.js";
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
  /**

   * Inject a specific bridge (tests use MockRenderBridge); defaults to the
   * production WsRenderBridge fan-out.
   */
  bridge?: RenderBridge;
}

export async function createControlPlaneServer(opts: ServerOptions): Promise<ControlPlaneServer> {
  const { state, audit, auth } = opts;
  const world: WorldTelemetry = newWorldTelemetry();
  const wsBridge = new WsRenderBridge();
  const bridge: RenderBridge = opts.bridge ?? wsBridge;
  const rateLimiter = new RateLimiter();

  const deps: DispatchDeps = { state, bridge, persistence: opts.persistence, rateLimiter };
  const registry = buildRegistry(deps);
  const clients = new Map<string, ClientSession>();

  const http = createServer((req: IncomingMessage, res: ServerResponse) => {
    if (req.method === "GET" && req.url === `${WS_PATH}/status`) {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ v: PROTOCOL_VERSION, ok: true, ...state.statusSnapshot() }));
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
      });
      const frame = JSON.stringify(
        errorResponse(randomUUID(), state.stateVersion, "E_AUTH", ar.reason ?? "auth failed"),
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
    };
    clients.set(connId, session);

    // Render-role sessions receive directives: register a fan-out sender.
    let unregisterRender: (() => void) | null = null;
    if (session.role === "render") {
      const renderSender = (frame: RenderDirective): void => {
        if (session.closed) return;
        ws.send(JSON.stringify(frame));
      };
      unregisterRender = wsBridge.register(renderSender);
    }

    const startTelemetry = (intervalMs: number): void => {
      if (session.telemetryTimer) clearInterval(session.telemetryTimer);
      session.telemetryTimer = setInterval(() => {
        if (session.closed) return;
        if (ws.bufferedAmount > 256 * 1024) return; // backpressure: coalesce
        ws.send(JSON.stringify({ v: PROTOCOL_VERSION, kind: "telemetry", data: buildTick(state, world, Date.now()) }));
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
      unregisterRender?.();
      clients.delete(connId);
    });
    ws.on("error", () => {
      session.closed = true;
      stopTelemetry();
      unregisterRender?.();
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
      }
      wss.close();
      await new Promise<void>((resolve) => http.close(() => resolve()));
    },
  };
}
