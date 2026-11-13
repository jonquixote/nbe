//! Companion bridge: a loopback-only HTTP + WebSocket front for Bitfocus
//! Companion (sprint item F, the user's ruling of 2026-11-12, 22:55).
//!
//! Companion's generic HTTP and WebSocket modules cannot send the control
//! plane's auth headers (`Authorization: Bearer`, `X-NBE-Role`). This process
//! holds the operator token, accepts button presses from Companion on
//! 127.0.0.1, resolves each press through the existing Companion adapter
//! (`findCompanionEntry` -> `resolveIntent` -> `intentToEnvelope`, with
//! `companionIntentSource` as the audit identity), and sends the envelope
//! over ONE authenticated WebSocket to the control plane. Control-plane auth
//! is unchanged: the bridge is an ordinary token-holding client, and the
//! control plane still decides what its role may do.
//!
//! Surface (all on 127.0.0.1 only):
//!   POST /press/<page>/<bank>/<key>   press one button (POST only: a GET can
//!                                     be forged by any web page's <img>)
//!   GET  /deck                        the deck JSON (`generateDeck`)
//!   GET  /health                      control-plane link state
//!   WS   /ws                          a message `<page>/<bank>/<key>` or
//!                                     `{"page":..,"bank":..,"key":..}`
//!
//! Every request is refused unless the peer is loopback, the Host header names
//! a loopback host, and any Origin header is a loopback origin. That keeps a
//! web page in the operator's browser (cross-site POST, DNS rebinding,
//! cross-site WebSocket) from pressing buttons. Companion's HTTP module sends
//! no Origin; its WebSocket module sends `Origin: http://<target host>`, which
//! is `http://127.0.0.1` here.
//!
//! Run: node --import tsx src/companion-bridge.ts --token-file <file>
//!        [--port 8470] [--cp ws://127.0.0.1:8462/nbe/v0.3] [--role operator]
//!        [--package <dir>] [--profile bridge]
//! The token file holds the token alone and must not be readable by group or
//! others (chmod 600).

import { readFileSync, statSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { join } from "node:path";
import type { Duplex } from "node:stream";
import { pathToFileURL } from "node:url";
import WebSocket, { WebSocketServer } from "ws";

import {
  companionIntentSource,
  CompanionButtonSchema,
  findCompanionEntry,
  generateDeck,
  type CompanionButton,
  type Deck,
} from "./companion.js";
import type { ControlBinding } from "./generated/manifest-schema.js";
import { intentToEnvelope, profileFromBindings, resolveIntent, type IntentProfile } from "./intent.js";
import { CpError, DEFAULT_PORT, RoleSchema, WS_PATH, type Role } from "./protocol.js";

export const DEFAULT_BRIDGE_PORT = 8470;
const BRIDGE_HOST = "127.0.0.1";

// ---------------------------------------------------------------------------
// Deck and profile
// ---------------------------------------------------------------------------

/**
 * The profile the bridge presses through: the manifest's bindings, plus the
 * built-in default buttons (`generateDeck`'s default page) as fully placed
 * companionKey bindings, so every button on the deck resolves through
 * `findCompanionEntry` like a manifest key does. A default button's intent id
 * is `default-<key>`.
 */
export function bridgeProfile(profileId: string, bindings: ControlBinding[]): { profile: IntentProfile; deck: Deck } {
  const deck = generateDeck(bindings);
  const defaults: ControlBinding[] = [];
  for (const page of deck.pages) {
    for (const bank of page.banks) {
      for (const b of bank.buttons) {
        if (b.bindingId !== null) continue;
        defaults.push({
          id: `default-${b.key}`,
          description: b.label,
          trigger: { kind: "companionKey", page: b.page, bank: b.bank, key: b.key },
          action: b.command,
          payload: b.payload,
        });
      }
    }
  }
  const ids = new Set(bindings.map((b) => b.id));
  for (const d of defaults) {
    if (ids.has(d.id)) throw new Error(`binding id ${d.id} collides with a default button's intent id`);
  }
  return { profile: profileFromBindings(profileId, "companion", [...bindings, ...defaults]), deck };
}

/** The `control.bindings` of a package's manifest.json (none when absent). */
export function packageBindings(packageDir: string): ControlBinding[] {
  const manifest = JSON.parse(readFileSync(join(packageDir, "manifest.json"), "utf8")) as {
    control?: { bindings?: ControlBinding[] };
  };
  return manifest.control?.bindings ?? [];
}

// ---------------------------------------------------------------------------
// The control-plane link: one authenticated WebSocket, reconnecting
// ---------------------------------------------------------------------------

/**
 * The bridge's own failures. Their codes start with `BRIDGE_` so they can
 * never be read as §16 registry codes; a control-plane refusal is passed
 * through with the control plane's own code.
 */
export class BridgeError extends Error {
  constructor(
    readonly code: "BRIDGE_NOT_CONNECTED" | "BRIDGE_TIMEOUT",
    message: string,
  ) {
    super(message);
    this.name = "BridgeError";
  }
}

interface CpResponse {
  requestId: string;
  status: "ok" | "error";
  stateVersion: number;
  data?: Record<string, unknown>;
  error?: { code: string; message: string };
}

class CpLink {
  private ws: WebSocket | null = null;
  private readonly pending = new Map<string, { resolve: (r: CpResponse) => void; reject: (e: Error) => void; timer: NodeJS.Timeout }>();
  private retryMs = 250;
  private retryTimer: NodeJS.Timeout | null = null;
  private closed = false;
  connected = false;
  lastError: string | null = null;

  constructor(
    private readonly url: string,
    private readonly token: string,
    private readonly role: Role,
    private readonly timeoutMs: number,
    private readonly log: (line: string) => void,
  ) {}

  start(): void {
    if (this.closed) return;
    const ws = new WebSocket(this.url, {
      headers: { authorization: `Bearer ${this.token}`, "x-nbe-role": this.role },
    });
    this.ws = ws;
    ws.on("open", () => {
      this.connected = true;
      this.lastError = null;
      this.retryMs = 250;
      this.log(`control plane connected: ${this.url} as ${this.role}`);
    });
    ws.on("unexpected-response", (_req, res) => {
      this.lastError = `control plane refused the connection: HTTP ${res.statusCode}`;
      res.resume();
      ws.terminate();
    });
    ws.on("message", (buf: Buffer) => {
      let m: CpResponse;
      try {
        m = JSON.parse(buf.toString("utf8")) as CpResponse;
      } catch {
        return;
      }
      if (typeof m.requestId !== "string") return; // telemetry / stateChange
      const p = this.pending.get(m.requestId);
      if (!p) return;
      clearTimeout(p.timer);
      this.pending.delete(m.requestId);
      p.resolve(m);
    });
    ws.on("error", (err) => {
      if (!this.lastError) this.lastError = String(err.message || err);
    });
    ws.on("close", () => {
      if (this.connected) this.log("control plane disconnected");
      this.connected = false;
      this.ws = null;
      for (const [id, p] of this.pending) {
        clearTimeout(p.timer);
        p.reject(new BridgeError("BRIDGE_NOT_CONNECTED", "control plane connection closed"));
        this.pending.delete(id);
      }
      if (this.closed) return;
      if (!this.lastError) this.lastError = "control plane connection closed";
      this.retryTimer = setTimeout(() => this.start(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 5000);
    });
  }

  send(frame: Record<string, unknown> & { id: string }): Promise<CpResponse> {
    const ws = this.ws;
    if (!ws || !this.connected || ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new BridgeError("BRIDGE_NOT_CONNECTED", this.lastError ?? "control plane not connected"));
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(frame.id);
        reject(new BridgeError("BRIDGE_TIMEOUT", `no control-plane response in ${this.timeoutMs} ms`));
      }, this.timeoutMs);
      this.pending.set(frame.id, { resolve, reject, timer });
      ws.send(JSON.stringify(frame));
    });
  }

  close(): void {
    this.closed = true;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.ws?.terminate();
  }
}

// ---------------------------------------------------------------------------
// Request guard: loopback peer, loopback Host, loopback-or-absent Origin
// ---------------------------------------------------------------------------

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);

function isLoopbackAddress(addr: string | undefined): boolean {
  return addr === "127.0.0.1" || addr === "::1" || addr === "::ffff:127.0.0.1";
}

function loopbackHostname(hostHeader: string): boolean {
  // `127.0.0.1:8470`, `localhost`, `[::1]:8470`
  const m = /^(\[[^\]]+\]|[^:]+)(?::\d+)?$/.exec(hostHeader.toLowerCase());
  return m !== null && LOOPBACK_HOSTS.has(m[1]!);
}

/** null when the request may proceed, else the reason it is refused. */
export function refusal(req: IncomingMessage): string | null {
  if (!isLoopbackAddress(req.socket.remoteAddress)) return "peer is not loopback";
  const host = req.headers.host;
  if (!host || !loopbackHostname(host)) return "Host is not a loopback host";
  const origin = req.headers.origin;
  if (origin !== undefined) {
    let u: URL;
    try {
      u = new URL(origin);
    } catch {
      return "Origin is not a loopback origin";
    }
    if ((u.protocol !== "http:" && u.protocol !== "https:") || !LOOPBACK_HOSTS.has(u.hostname.toLowerCase())) {
      return "Origin is not a loopback origin";
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

export interface BridgeOptions {
  /** 0 picks a free port. */
  port: number;
  cpUrl: string;
  token: string;
  role?: Role;
  profileId?: string;
  bindings?: ControlBinding[];
  requestTimeoutMs?: number;
  log?: (line: string) => void;
}

export interface PressResult {
  httpStatus: number;
  body: Record<string, unknown>;
}

export interface CompanionBridge {
  port: number;
  press(button: CompanionButton): Promise<PressResult>;
  close(): Promise<void>;
}

const HTTP_STATUS: Record<string, number> = {
  BRIDGE_NOT_CONNECTED: 503,
  BRIDGE_TIMEOUT: 504,
  // resolveIntent: the button's action or payload is not a valid command
  E_UNSUPPORTED: 422,
  E_BAD_PAYLOAD: 422,
};

export async function startCompanionBridge(opts: BridgeOptions): Promise<CompanionBridge> {
  const log = opts.log ?? ((line: string) => console.log(line));
  const role = opts.role ?? "operator";
  const profileId = opts.profileId ?? "bridge";
  const { profile, deck } = bridgeProfile(profileId, opts.bindings ?? []);
  const link = new CpLink(opts.cpUrl, opts.token, role, opts.requestTimeoutMs ?? 5000, log);
  link.start();

  async function press(button: CompanionButton): Promise<PressResult> {
    const label = `${button.page}/${button.bank}/${button.key}`;
    const entry = findCompanionEntry(profile, button);
    if (!entry) {
      log(`press ${label}: no button`);
      return { httpStatus: 404, body: { ok: false, error: { code: "BRIDGE_NO_BUTTON", message: `no button ${label}` } } };
    }
    const intentSource = companionIntentSource(profileId, entry.intentId);
    let command = entry.action;
    try {
      const resolved = resolveIntent(entry);
      command = resolved.command;
      const envelope = intentToEnvelope(resolved);
      const r = await link.send({ ...envelope, intentSource });
      if (r.status === "ok") {
        log(`press ${label}: ${command} ok, stateVersion ${r.stateVersion}`);
        return { httpStatus: 200, body: { ok: true, command, intentSource, stateVersion: r.stateVersion, data: r.data ?? {} } };
      }
      log(`press ${label}: ${command} refused, ${r.error?.code}`);
      return { httpStatus: 409, body: { ok: false, command, intentSource, stateVersion: r.stateVersion, error: r.error } };
    } catch (err) {
      const code = err instanceof CpError || err instanceof BridgeError ? err.code : "BRIDGE_INTERNAL";
      const message = err instanceof Error ? err.message : String(err);
      log(`press ${label}: ${command} failed, ${code}: ${message}`);
      return {
        httpStatus: HTTP_STATUS[code] ?? 500,
        body: { ok: false, command, intentSource, error: { code, message } },
      };
    }
  }

  function json(res: ServerResponse, status: number, body: unknown, extra: Record<string, string> = {}): void {
    const text = JSON.stringify(body);
    res.writeHead(status, { "content-type": "application/json", "content-length": Buffer.byteLength(text), ...extra });
    res.end(text);
  }

  const http: Server = createServer((req, res) => {
    req.resume(); // the body is never read: the path names the button
    const why = refusal(req);
    if (why) {
      log(`refused ${req.method} ${req.url}: ${why}`);
      json(res, 403, { ok: false, error: { code: "BRIDGE_FORBIDDEN", message: why } });
      return;
    }
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const parts = url.pathname.split("/").filter((p) => p !== "");
    if (parts[0] === "press") {
      if (req.method !== "POST") {
        json(res, 405, { ok: false, error: { code: "BRIDGE_METHOD", message: "press is POST only" } }, { allow: "POST" });
        return;
      }
      const button = parseButton(parts.slice(1));
      if (!button) {
        json(res, 400, { ok: false, error: { code: "BRIDGE_BAD_PRESS", message: "expected /press/<page>/<bank>/<key>" } });
        return;
      }
      void press(button).then((r) => json(res, r.httpStatus, r.body));
      return;
    }
    if (req.method === "GET" && url.pathname === "/deck") {
      json(res, 200, deck);
      return;
    }
    if (req.method === "GET" && url.pathname === "/health") {
      json(res, 200, { ok: true, profileId, role, controlPlane: { url: opts.cpUrl, connected: link.connected, lastError: link.lastError } });
      return;
    }
    json(res, 404, { ok: false, error: { code: "BRIDGE_NO_PATH", message: "unknown path" } });
  });

  const wss = new WebSocketServer({ noServer: true });
  http.on("upgrade", (req: IncomingMessage, socket: Duplex, head: Buffer) => {
    const why = refusal(req);
    const path = new URL(req.url ?? "/", "http://127.0.0.1").pathname;
    if (why || path !== "/ws") {
      log(`refused WebSocket ${req.url}: ${why ?? "unknown path"}`);
      socket.write("HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n");
      socket.destroy();
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => {
      ws.on("message", (buf: Buffer) => {
        const button = parseWsPress(buf.toString("utf8"));
        if (!button) {
          ws.send(JSON.stringify({ httpStatus: 400, ok: false, error: { code: "BRIDGE_BAD_PRESS", message: "expected <page>/<bank>/<key>" } }));
          return;
        }
        void press(button).then((r) => {
          if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ httpStatus: r.httpStatus, ...r.body }));
        });
      });
    });
  });

  await new Promise<void>((resolve, reject) => {
    http.once("error", reject);
    http.listen(opts.port, BRIDGE_HOST, () => resolve());
  });
  const addr = http.address();
  const port = typeof addr === "object" && addr ? addr.port : opts.port;
  log(`companion bridge listening on http://${BRIDGE_HOST}:${port} (profile ${profileId}, ${profile.entries.length} buttons)`);

  return {
    port,
    press,
    close: async () => {
      link.close();
      for (const c of wss.clients) c.terminate();
      await new Promise<void>((resolve) => {
        http.close(() => resolve());
        http.closeAllConnections();
      });
    },
  };
}

function parseButton(parts: string[]): CompanionButton | null {
  if (parts.length !== 3) return null;
  const [page, bank, key] = parts as [string, string, string];
  if (!/^\d+$/.test(page) || !/^\d+$/.test(bank)) return null;
  let k: string;
  try {
    k = decodeURIComponent(key);
  } catch {
    return null;
  }
  const parsed = CompanionButtonSchema.safeParse({ page: Number(page), bank: Number(bank), key: k });
  return parsed.success ? parsed.data : null;
}

function parseWsPress(text: string): CompanionButton | null {
  const t = text.trim();
  if (t.startsWith("{")) {
    try {
      const parsed = CompanionButtonSchema.safeParse(JSON.parse(t));
      return parsed.success ? parsed.data : null;
    } catch {
      return null;
    }
  }
  return parseButton(t.split("/"));
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function arg(argv: string[], name: string): string | undefined {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv[i + 1] : undefined;
}

/** The token from a file only the owner can read; never from argv or env. */
export function readTokenFile(path: string): string {
  const mode = statSync(path).mode & 0o777;
  if ((mode & 0o077) !== 0) {
    throw new Error(`token file ${path} is readable by group or others (mode ${mode.toString(8)}); chmod 600 it`);
  }
  const token = readFileSync(path, "utf8").trim();
  if (!token) throw new Error(`token file ${path} is empty`);
  return token;
}

async function main(argv: string[]): Promise<void> {
  const tokenFile = arg(argv, "token-file");
  if (!tokenFile) {
    console.error("companion bridge: --token-file <file> is required (the operator token, chmod 600)");
    process.exit(2);
  }
  const roleRaw = arg(argv, "role") ?? "operator";
  const role = RoleSchema.safeParse(roleRaw);
  if (!role.success || role.data === "render") {
    console.error(`companion bridge: --role ${roleRaw} is not a session role`);
    process.exit(2);
  }
  const port = Number(arg(argv, "port") ?? DEFAULT_BRIDGE_PORT);
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    console.error(`companion bridge: --port ${arg(argv, "port")} is not a port`);
    process.exit(2);
  }
  let token: string;
  let bindings: ControlBinding[] = [];
  try {
    token = readTokenFile(tokenFile);
    const pkg = arg(argv, "package");
    if (pkg) bindings = packageBindings(pkg);
  } catch (err) {
    console.error(`companion bridge: ${(err as Error).message}`);
    process.exit(2);
  }
  const bridge = await startCompanionBridge({
    port,
    cpUrl: arg(argv, "cp") ?? `ws://127.0.0.1:${DEFAULT_PORT}${WS_PATH}`,
    token,
    role: role.data,
    profileId: arg(argv, "profile") ?? "bridge",
    bindings,
  });
  const stop = (): void => {
    void bridge.close().then(() => process.exit(0));
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main(process.argv.slice(2)).catch((err: unknown) => {
    console.error(`companion bridge: ${String(err)}`);
    process.exit(1);
  });
}
