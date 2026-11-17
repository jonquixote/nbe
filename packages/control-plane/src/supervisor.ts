//! The show's supervisor (review4 step 6; the user's ruling of 2026-11-16
//! 23:45, work order 5). It starts the show's processes in order, each waited
//! for, restarts the ones that may be restarted, keeps the Mac awake, and
//! leaves nothing behind when it stops.
//!
//! - Start order is the order given; each child is waited for (its `ready`
//!   check, every 100 ms, at most `readyMs`, 20 s by default) and a child
//!   that is not ready in time fails the start by name.
//! - Restart: at once, then after 0.5 s doubling to 5 s. Five failures within
//!   60 s stop the retries (`gaveUp`).
//! - NOT restarted until review4 step 7 (adoption) lands: the control plane.
//!   Restarted today, it would resync a running engine to UNLOADED and stop
//!   the show (review3 N1): a survivable crash would become an automatic
//!   off-air. Its spec says `restart: false`.
//! - An engine restart does NOT resume the recording or the stream: the new
//!   engine comes back idle while the control plane still believes
//!   "recording". Resuming outputs is review4 step 7 (case E).
//! - Each child runs in its own process group. Stopping sends SIGTERM to the
//!   group, then SIGKILL after 2 s, so a child's own children (the engine's
//!   ffmpeg readers) go with it.
//! - On macOS it holds `caffeinate -dimsu -w <its pid>` for its lifetime
//!   (R14: a running show must not idle-sleep).
//! - Each child's output goes to `<logDir>/<name>.log`, rotated at 50 MB,
//!   five old files kept.

import { spawn, type ChildProcess } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, renameSync, rmSync, writeSync } from "node:fs";
import { join } from "node:path";

export interface ChildSpec {
  name: string;
  command: string;
  args?: string[];
  /** The child's own variables. A child gets these and `BASE_ENV` only,
   * never the supervisor's whole environment: the supervisor holds every
   * role's token (NBE_TOKENS), and the engine and the media server need none
   * of them. */
  env?: Record<string, string>;
  cwd?: string;
  /** Restart when it exits. The control plane's is false until review4 step 7. */
  restart: boolean;
  /** Ready to depend on (polled every 100 ms during start). */
  ready?: () => Promise<boolean>;
  readyMs?: number;
}

export type SupervisorEvent =
  | { kind: "started"; name: string; pid: number; attempt: number }
  | { kind: "ready"; name: string; afterMs: number }
  | { kind: "exited"; name: string; code: number | null; signal: NodeJS.Signals | null }
  | { kind: "restarting"; name: string; attempt: number; afterMs: number }
  | { kind: "gaveUp"; name: string; failures: number; windowMs: number }
  | { kind: "notRestarted"; name: string; why: string };

export interface SupervisorOptions {
  logDir: string;
  rotateBytes?: number;
  keepLogs?: number;
  /** Delay before restart `attempt` (1-based). */
  backoffMs?: (attempt: number) => number;
  maxFailures?: number;
  failureWindowMs?: number;
  killGraceMs?: number;
  /** Hold `caffeinate` (macOS). Default: on macOS. */
  caffeinate?: boolean;
  onEvent?: (e: SupervisorEvent) => void;
}

/** What every child inherits from the supervisor: the basics a process
 * needs to run, and nothing that could carry a secret. */
export const BASE_ENV = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "TZ"] as const;

/** The base variables from `from` that are set. */
export function baseEnv(from: NodeJS.ProcessEnv = process.env): Record<string, string> {
  const out: Record<string, string> = {};
  for (const k of BASE_ENV) {
    const v = from[k];
    if (v !== undefined) out[k] = v;
  }
  return out;
}

/** At once, then 0.5 s doubling to 5 s. */
export function defaultBackoffMs(attempt: number): number {
  if (attempt <= 1) return 0;
  return Math.min(5000, 500 * 2 ** (attempt - 2));
}

interface Running {
  spec: ChildSpec;
  child: ChildProcess | null;
  attempt: number;
  failures: number[];
  log: RotatingLog;
  restartTimer: ReturnType<typeof setTimeout> | null;
  gaveUp: boolean;
}

export class Supervisor {
  private readonly running = new Map<string, Running>();
  private stopping = false;
  private caffeinateChild: ChildProcess | null = null;
  private readonly o: Required<Omit<SupervisorOptions, "onEvent">> & { onEvent: (e: SupervisorEvent) => void };

  constructor(
    private readonly specs: ChildSpec[],
    opts: SupervisorOptions,
  ) {
    this.o = {
      logDir: opts.logDir,
      rotateBytes: opts.rotateBytes ?? 50 * 1024 * 1024,
      keepLogs: opts.keepLogs ?? 5,
      backoffMs: opts.backoffMs ?? defaultBackoffMs,
      maxFailures: opts.maxFailures ?? 5,
      failureWindowMs: opts.failureWindowMs ?? 60_000,
      killGraceMs: opts.killGraceMs ?? 2000,
      caffeinate: opts.caffeinate ?? process.platform === "darwin",
      onEvent: opts.onEvent ?? (() => {}),
    };
  }

  /** The pid of each child now (undefined while it is down). */
  pids(): Record<string, number | undefined> {
    const out: Record<string, number | undefined> = {};
    for (const [name, r] of this.running) out[name] = r.child?.pid;
    return out;
  }

  get caffeinatePid(): number | undefined {
    return this.caffeinateChild?.pid;
  }

  /** Start every child in order, each waited for. Rejects, naming the child,
   * when one is not ready in time (the ones already started keep running;
   * call `stop`). */
  async start(): Promise<void> {
    mkdirSync(this.o.logDir, { recursive: true });
    if (this.o.caffeinate) {
      this.caffeinateChild = spawn("caffeinate", ["-dimsu", "-w", String(process.pid)], { stdio: "ignore" });
      this.caffeinateChild.on("error", () => {
        this.caffeinateChild = null;
      });
    }
    for (const spec of this.specs) {
      const r: Running = {
        spec,
        child: null,
        attempt: 0,
        failures: [],
        log: new RotatingLog(this.o.logDir, spec.name, this.o.rotateBytes, this.o.keepLogs),
        restartTimer: null,
        gaveUp: false,
      };
      this.running.set(spec.name, r);
      this.launch(r);
      if (spec.ready) await this.waitReady(spec);
    }
  }

  private async waitReady(spec: ChildSpec): Promise<void> {
    const limit = spec.readyMs ?? 20_000;
    const started = Date.now();
    while (Date.now() - started < limit) {
      if (this.stopping) return;
      if (await spec.ready!().catch(() => false)) {
        this.o.onEvent({ kind: "ready", name: spec.name, afterMs: Date.now() - started });
        return;
      }
      await new Promise((res) => setTimeout(res, 100));
    }
    throw new Error(`${spec.name}: not ready within ${limit} ms`);
  }

  private launch(r: Running): void {
    r.attempt += 1;
    const child = spawn(r.spec.command, r.spec.args ?? [], {
      cwd: r.spec.cwd,
      env: { ...baseEnv(), ...(r.spec.env ?? {}) },
      stdio: ["ignore", "pipe", "pipe"],
      // Its own process group, so a stop takes its children with it.
      detached: true,
    });
    r.child = child;
    child.stdout?.on("data", (b: Buffer) => r.log.write(b));
    child.stderr?.on("data", (b: Buffer) => r.log.write(b));
    child.on("error", (e) => r.log.write(Buffer.from(`supervisor: spawn failed: ${String(e)}\n`)));
    if (child.pid !== undefined) {
      this.o.onEvent({ kind: "started", name: r.spec.name, pid: child.pid, attempt: r.attempt });
    }
    child.on("exit", (code, signal) => {
      if (r.child === child) r.child = null;
      this.o.onEvent({ kind: "exited", name: r.spec.name, code, signal });
      if (this.stopping) return;
      this.onExit(r);
    });
  }

  private onExit(r: Running): void {
    if (!r.spec.restart) {
      this.o.onEvent({
        kind: "notRestarted",
        name: r.spec.name,
        why: "restart is off for this process (the control plane's stays off until review4 step 7, adoption)",
      });
      return;
    }
    const now = Date.now();
    r.failures = r.failures.filter((t) => now - t < this.o.failureWindowMs);
    r.failures.push(now);
    if (r.failures.length >= this.o.maxFailures) {
      r.gaveUp = true;
      this.o.onEvent({ kind: "gaveUp", name: r.spec.name, failures: r.failures.length, windowMs: this.o.failureWindowMs });
      return;
    }
    const afterMs = this.o.backoffMs(r.failures.length);
    this.o.onEvent({ kind: "restarting", name: r.spec.name, attempt: r.failures.length, afterMs });
    r.restartTimer = setTimeout(() => {
      r.restartTimer = null;
      if (!this.stopping) this.launch(r);
    }, afterMs);
  }

  /** Stop every child: SIGTERM to its process group, SIGKILL after the grace.
   * Resolves when every child has exited. */
  async stop(): Promise<void> {
    this.stopping = true;
    const waits: Promise<void>[] = [];
    for (const r of this.running.values()) {
      if (r.restartTimer) clearTimeout(r.restartTimer);
      const child = r.child;
      if (!child || child.pid === undefined || child.exitCode !== null || child.signalCode !== null) continue;
      const pid = child.pid;
      waits.push(
        new Promise<void>((res) => {
          const done = (): void => {
            clearTimeout(kill);
            // The group may outlive its leader: make sure of it.
            killGroup(pid, "SIGKILL");
            res();
          };
          child.once("exit", done);
          killGroup(pid, "SIGTERM");
          const kill = setTimeout(() => killGroup(pid, "SIGKILL"), this.o.killGraceMs);
        }),
      );
    }
    await Promise.all(waits);
    for (const r of this.running.values()) r.log.close();
    this.caffeinateChild?.kill("SIGTERM");
    this.caffeinateChild = null;
  }
}

/** Signal a process group (a detached child leads its own). */
function killGroup(pid: number, signal: NodeJS.Signals): void {
  try {
    process.kill(-pid, signal);
  } catch {
    // Already gone.
  }
}

/** `<dir>/<name>.log`, rotated to `<name>.1.log` … `<name>.<keep>.log`.
 * Written synchronously: the file exists from construction, so a rotation
 * never races its own open, and a reader sees what was written. */
export class RotatingLog {
  private fd: number;
  private bytes = 0;

  constructor(
    private readonly dir: string,
    private readonly name: string,
    private readonly rotateBytes: number,
    private readonly keep: number,
  ) {
    this.fd = openSync(this.path(0), "a");
  }

  private path(n: number): string {
    return join(this.dir, n === 0 ? `${this.name}.log` : `${this.name}.${n}.log`);
  }

  write(b: Buffer): void {
    if (this.fd < 0) return;
    writeSync(this.fd, b);
    this.bytes += b.length;
    if (this.bytes >= this.rotateBytes) this.rotate();
  }

  private rotate(): void {
    closeSync(this.fd);
    const oldest = this.path(this.keep);
    if (existsSync(oldest)) rmSync(oldest);
    for (let n = this.keep - 1; n >= 0; n--) {
      if (existsSync(this.path(n))) renameSync(this.path(n), this.path(n + 1));
    }
    this.fd = openSync(this.path(0), "a");
    this.bytes = 0;
  }

  close(): void {
    if (this.fd < 0) return;
    closeSync(this.fd);
    this.fd = -1;
  }
}
