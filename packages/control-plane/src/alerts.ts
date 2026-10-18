//! The operator-signal lane (SPEC §10.1.2, v0.4.8 row 8; R17 spine part A).
//!
//! One list of alerts on the shared status snapshot, so `stateChange` frames,
//! `system.status`, `GET /status` and the telemetry tick all carry it. An alert
//! says "something needs you"; the state fields beside it say what. The lane is
//! a live view: it is rebuilt from current state, never persisted, and every
//! alert but one is audited on its edges (one row when it raises, one when it
//! clears, never one per evaluation). The exception is the audit sink's own
//! failure, which by definition cannot be written to the sink (R18).
//!
//! Part A is additive: nothing here changes a belief field, an enum, or an
//! automation rule's input. It only announces.
//!
//! Part B (SPEC §10.1.3–§10.1.4, v0.4.8 rows 10 and 11) adds the belief's
//! side, without changing what the lane announces: a command's claim on the
//! stream belief ends at its acknowledgement or at its window's
//! unacknowledged end, and with no live claim a fresh report that postdates
//! the anchor corrects the belief through `correct` (a detection transaction,
//! the server's). The held alert's clearing follows §10.1.2's rule for
//! `outputs.held`: a real acknowledgement, or the show's lifecycle.

import { unreachable, type ControlPlaneState, type StreamState } from "./state.js";
import type { EngineReport } from "./telemetry.js";

export type AlertSeverity = "info" | "warning" | "critical";

export interface Alert {
  /** Stable for the episode: the same condition never raises twice at once. */
  id: string;
  severity: AlertSeverity;
  /** What the control plane saw, in words an operator can act on. */
  reason: string;
  /** Unix ms when this episode raised. */
  since: number;
  /** The audit event its edge rows carry; `null` only for the audit sink's own failure. */
  auditEvent: string | null;
}

/** One audit row per edge; `cause` says why a clear happened. */
export type AlertAudit = (edge: "raised" | "cleared", alert: Alert, cause: string | null) => void;

/** The alert ids, and the audit event each mirrors (SPEC §10.1.2's table). */
export const ALERT = {
  engineNotReporting: { id: "engine.notReporting", auditEvent: "engine.notReporting" },
  streamUnacknowledged: { id: "command.unacknowledged.stream", auditEvent: "command.unacknowledged" },
  recordUnacknowledged: { id: "command.unacknowledged.record", auditEvent: "command.unacknowledged" },
  streamDivergence: { id: "stream.divergence", auditEvent: "stream.divergence" },
  outputsHeld: { id: "outputs.held", auditEvent: "show.stopHeld" },
  auditSinkFailed: { id: "audit.sinkFailed", auditEvent: null },
} as const;

/** The list itself, kept on the state so every snapshot reads one source. */
export class AlertLane {
  constructor(
    private readonly state: Pick<ControlPlaneState, "alerts">,
    private readonly audit: AlertAudit,
    private readonly now: () => number,
  ) {}

  has(id: string): boolean {
    return this.state.alerts.some((a) => a.id === id);
  }

  /** Raise once per episode: a raise while the id is active changes nothing. */
  raise(spec: { id: string; auditEvent: string | null }, severity: AlertSeverity, reason: string): void {
    if (this.has(spec.id)) return;
    const alert: Alert = { id: spec.id, severity, reason, since: this.now(), auditEvent: spec.auditEvent };
    this.state.alerts.push(alert);
    this.edge("raised", alert, null);
  }

  clear(id: string, cause: string): void {
    const i = this.state.alerts.findIndex((a) => a.id === id);
    if (i < 0) return;
    const [alert] = this.state.alerts.splice(i, 1);
    this.edge("cleared", alert!, cause);
  }

  private edge(edge: "raised" | "cleared", alert: Alert, cause: string | null): void {
    // The audit sink's own failure has no row (R18): attempting one would
    // fail on exactly the case the alert exists for.
    if (alert.auditEvent === null) return;
    try {
      this.audit(edge, alert, cause);
    } catch {
      // A broken sink is the sink-failure alert's to announce (the
      // evaluator reads `auditSinkOk`); the lane itself never throws.
    }
  }
}

type Output = "stream" | "record";

interface Pending {
  command: string;
  version: number;
  /** When an acknowledgement at or above `version` (never a resync's) arrived. */
  processedAt: number | null;
  exact: boolean;
  /** When the window ended with no acknowledgement: the claim's expiry (§10.1.3). */
  expiredAt: number | null;
  timer: ReturnType<typeof setTimeout> | null;
}

/** A belief correction (§10.1.4): the server lands it as a detection transaction. */
export interface BeliefCorrection {
  field: "streamState";
  from: StreamState;
  to: StreamState;
  /** The engine report's `streamTransportState`. */
  token: string;
  /** How old the report was when it corrected the belief, in ms. */
  ageMs: number;
}

/**
 * §10.1.4's mapping, from the engine's transport token to the belief (D1).
 * `reconnecting` reads `live`: a redial is §9.5's survival shape, and the
 * belief stays stable through a blip, as the lane does. An unknown token
 * corrects nothing.
 */
export function beliefFromReport(token: string): StreamState | null {
  switch (token) {
    case "live":
    case "reconnecting":
      return "live";
    case "none":
    case "closed":
      return "idle";
    default:
      return null;
  }
}

/** The stops whose own acknowledgement witnesses an actual stop (§10.1.2, `outputs.held`). */
const STOPS = new Set(["stream.stop", "record.stop", "show.stop"]);

export interface SignalInputs {
  state: ControlPlaneState;
  lane: AlertLane;
  now: () => number;
  /** §5.9.5's window: an output command unacknowledged within it raises. */
  windowMs: number;
  /** §10.1.1's freshness threshold for an engine report. */
  ttlMs: number;
  engineReport: () => EngineReport | null;
  auditSinkOk: () => boolean;
  /** Lands a belief correction (§10.1.4): one bump, one audit row, no frame, no automation. */
  correct: (c: BeliefCorrection) => void;
}

const OUTPUT_OF: Record<string, Output | undefined> = {
  "stream.start": "stream",
  "stream.stop": "stream",
  "record.start": "record",
  "record.stop": "record",
};

/**
 * The evaluator (SPEC §10.1.2). Two mechanisms, one question each:
 * - **(b) command confirmation.** An output command (`stream.start`,
 *   `stream.stop`, `record.start`, `record.stop`) with no exact
 *   acknowledgement within the window raises "<command> unacknowledged". A
 *   refused or skipped directive is never acknowledged (the engine's `apply`
 *   returns before its ack), so the absence is the signal. `show.stop` has its
 *   own exact wait, and the held alert is its surface.
 * - **(a) standing contradictions.** With no command pending, a fresh engine
 *   report that postdates the anchor (the latest of the relevant command's
 *   processing and the connect/resync) is compared with the control plane's
 *   belief, stream only (the engine reports no record state).
 */
export class OperatorSignals {
  private readonly pending = new Map<Output, Pending>();
  /**
   * `show.stop` sent and not yet processed: both outputs are in motion. It
   * lapses after the window: with outputs held, the held alert owns them;
   * without, (a) compares again.
   */
  private showStopPending: number | null = null;
  private showStopTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * The unacknowledged `show.stop` whose outputs are held: its own late
   * acknowledgement clears the held alert (§10.1.2's clearing rule for
   * `outputs.held`, path (i)). It survives the hold: ~~`held()` drops the
   * pending entry~~ (§2c, v0.4.8 row 11).
   */
  private heldStopVersion: number | null = null;
  /** The latest moment the engine is known to have caught up with the stream's commands. */
  private streamAnchor = 0;
  private lastRegisteredAt: number | null = null;
  private readonly startedAt: number;

  constructor(private readonly i: SignalInputs) {
    this.startedAt = i.now();
  }

  /**
   * A render session registered: the connect is an anchor (a new engine's
   * reports describe its own outputs from here on), and its first report gets
   * a TTL's grace before the engine counts as not reporting.
   */
  onRegistered(): void {
    const now = this.i.now();
    this.lastRegisteredAt = now;
    this.streamAnchor = Math.max(this.streamAnchor, now);
  }

  /** An output command (or `show.stop`) was accepted at `version`. */
  onAccepted(command: string, version: number): void {
    // §10.1.2's clearing rule for `outputs.held`, path (iii): the show's
    // lifecycle ends an unresolved hold, the cause named in the clear row.
    if (command === "show.load" || command === "show.unload") {
      this.heldStopVersion = null;
      this.i.lane.clear(ALERT.outputsHeld.id, `cleared by ${command} (stateVersion ${version})`);
      return;
    }
    if (command === "show.stop") {
      this.showStopPending = version;
      if (this.showStopTimer) clearTimeout(this.showStopTimer);
      this.showStopTimer = setTimeout(() => {
        if (this.showStopPending === version) this.showStopPending = null;
      }, this.i.windowMs);
      this.showStopTimer.unref?.();
      return;
    }
    const output = OUTPUT_OF[command];
    if (!output) return;
    const previous = this.pending.get(output);
    if (previous?.timer) clearTimeout(previous.timer);
    // The operator's explicit action supersedes the last episode.
    const unacked = output === "stream" ? ALERT.streamUnacknowledged : ALERT.recordUnacknowledged;
    this.i.lane.clear(unacked.id, `superseded by ${command} (stateVersion ${version})`);
    if (output === "stream") this.i.lane.clear(ALERT.streamDivergence.id, `superseded by ${command} (stateVersion ${version})`);
    const entry: Pending = { command, version, processedAt: null, exact: false, expiredAt: null, timer: null };
    entry.timer = setTimeout(() => this.windowElapsed(output, entry), this.i.windowMs);
    entry.timer.unref?.();
    this.pending.set(output, entry);
  }

  /**
   * The engine acknowledged `version`. A resync's acknowledgement proves
   * nothing about a command (the resync carries no output state) but is the
   * connect/resync anchor. Any other acknowledgement at or above a pending
   * command's version proves the engine processed it: the engine applies in
   * arrival order, and the control plane sends at increasing versions.
   */
  onApplied(version: number, viaResync: boolean): void {
    const now = this.i.now();
    if (viaResync) {
      this.streamAnchor = now;
      return;
    }
    if (this.showStopPending !== null && version >= this.showStopPending) {
      this.showStopPending = null;
      this.streamAnchor = now;
    }
    // Path (i): the held stop's own acknowledgement, however late. It is
    // inert to belief (§10.1.3's late-acknowledgement rule): it clears the
    // alert and moves nothing else.
    if (this.heldStopVersion !== null && version === this.heldStopVersion) {
      this.heldStopVersion = null;
      this.i.lane.clear(ALERT.outputsHeld.id, `show.stop acknowledged late (stateVersion ${version})`);
    }
    for (const [output, entry] of this.pending) {
      if (entry.processedAt !== null || version < entry.version) continue;
      entry.processedAt = now;
      entry.exact = version === entry.version;
      // Path (ii): a later stop's own acknowledgement at or above the held
      // stop's version. The engine acknowledges a stop only while that output
      // is live, so the acknowledgement witnesses an actual stop.
      if (entry.exact && STOPS.has(entry.command) && this.heldStopVersion !== null && entry.version >= this.heldStopVersion) {
        this.heldStopVersion = null;
        this.i.lane.clear(ALERT.outputsHeld.id, `${entry.command} acknowledged (stateVersion ${version}): the stop is witnessed`);
      }
      if (entry.timer) clearTimeout(entry.timer);
      if (output === "stream") this.streamAnchor = now;
      const unacked = output === "stream" ? ALERT.streamUnacknowledged : ALERT.recordUnacknowledged;
      this.i.lane.clear(
        unacked.id,
        entry.exact
          ? `${entry.command} acknowledged late (stateVersion ${version})`
          : `a later acknowledgement (stateVersion ${version}) proves ${entry.command} was processed`,
      );
    }
  }

  private windowElapsed(output: Output, entry: Pending): void {
    // A superseded command's claim passed to the newer one at its acceptance;
    // a confirmed one did not expire. Both are outside §10.1.3's clause.
    if (this.pending.get(output) !== entry || entry.processedAt !== null) return;
    // The claim expires (§10.1.3): it ends the claim and raises the expiry
    // alert where (b) raises one, and does nothing else. For the stream, only
    // a report that postdates the expiry may correct the belief.
    entry.expiredAt = this.i.now();
    if (output === "stream") this.streamAnchor = Math.max(this.streamAnchor, entry.expiredAt);
    if (this.i.lane.has(ALERT.engineNotReporting.id)) return; // that alert owns it
    const spec = output === "stream" ? ALERT.streamUnacknowledged : ALERT.recordUnacknowledged;
    const report = this.freshReport();
    const context =
      output === "stream" && report
        ? `; the engine's latest report reads the stream ${report.frame.streamTransportState} and the control plane believes it ${this.i.state.streamState}`
        : "";
    this.i.lane.raise(
      spec,
      "warning",
      `${entry.command} (stateVersion ${entry.version}) was not acknowledged within ${this.i.windowMs} ms: refused, skipped or lost${context}`,
    );
  }

  private freshReport(): EngineReport | null {
    const r = this.i.engineReport();
    return r !== null && this.i.now() - r.receivedAt <= this.i.ttlMs ? r : null;
  }

  /** Re-evaluated on a short interval; raises and clears only on edges. */
  evaluate(): void {
    const { state, lane } = this.i;
    const now = this.i.now();

    // R18: the audit sink's own failure, on the lane and the health flag.
    const sinkOk = this.i.auditSinkOk();
    state.health.auditSinkOk = sinkOk;
    if (!sinkOk) lane.raise(ALERT.auditSinkFailed, "critical", "the audit sink cannot be written: failures are not being recorded");
    else lane.clear(ALERT.auditSinkFailed.id, "the audit sink accepted a write");

    // Engine not reporting: stale beyond the TTL, measured from the latest of
    // its last report, its registration and this process's start, once an
    // engine has connected or a show is running.
    const report = this.i.engineReport();
    const lastHeard = Math.max(report?.receivedAt ?? 0, this.lastRegisteredAt ?? 0, this.startedAt);
    const expected = this.lastRegisteredAt !== null || state.showState === "RUNNING";
    const notReporting = expected && now - lastHeard > this.i.ttlMs;
    if (notReporting) {
      lane.raise(
        ALERT.engineNotReporting,
        "critical",
        `no engine report for over ${this.i.ttlMs} ms: output state is unconfirmed`,
      );
      lane.clear(ALERT.streamUnacknowledged.id, "the engine is not reporting");
      lane.clear(ALERT.recordUnacknowledged.id, "the engine is not reporting");
      lane.clear(ALERT.streamDivergence.id, "the engine is not reporting");
      return;
    }
    lane.clear(ALERT.engineNotReporting.id, "the engine is reporting again");

    // ~~The held stop heals when the operator's explicit stops land.~~ (§2c,
    // v0.4.8 row 11: the held alert clears only by §10.1.2's clearing rule
    // for `outputs.held`, a real acknowledgement or the show's lifecycle,
    // never by what the belief reads.)

    this.divergence();
    this.correction();
  }

  /** (a) The stream's standing contradiction (row 8, unchanged). */
  private divergence(): void {
    const { state, lane } = this.i;
    const fresh = this.freshReport();
    const streamEntry = this.pending.get("stream");
    const streamPending = (streamEntry !== undefined && streamEntry.processedAt === null) || this.showStopPending !== null;
    const owned = lane.has(ALERT.streamUnacknowledged.id) || lane.has(ALERT.outputsHeld.id);
    if (fresh === null || streamPending || owned || fresh.receivedAt <= this.streamAnchor) return;
    const transport = fresh.frame.streamTransportState;
    const believed = state.streamState;
    let divergent: boolean;
    switch (believed) {
      case "live":
        divergent = transport === "none" || transport === "closed";
        break;
      case "idle":
        divergent = transport === "live" || transport === "reconnecting";
        break;
      case "held":
        // §5.9.5's limbo: the state is unknown, so nothing contradicts it.
        // The held alert announces it, and a fresh report corrects it.
        divergent = false;
        break;
      default:
        unreachable(believed);
    }
    if (divergent) {
      lane.raise(
        ALERT.streamDivergence,
        "critical",
        believed === "live"
          ? `the control plane believes the stream live; the engine reports it ${transport}`
          : `the engine reports the stream ${transport} while the control plane believes it idle: it is on air`,
      );
    } else {
      lane.clear(ALERT.streamDivergence.id, `consistent again: believed ${believed}, reported ${transport}`);
    }
  }

  /**
   * §10.1.4: with no live claim on the stream, a fresh engine report that
   * postdates the anchor corrects the belief by the mapping. The server lands
   * it as a detection transaction. It clears no alert: corrections move the
   * belief, not the lane (§10.1.2's clearing rule for `outputs.held`).
   */
  private correction(): void {
    if (this.i.windowMs > 0) return; // FALSIFICATION (D6 bounded wait): the correction lands nothing
    const fresh = this.freshReport();
    if (fresh === null || this.streamClaimLive() || fresh.receivedAt <= this.streamAnchor) return;
    const token = fresh.frame.streamTransportState;
    if (token === undefined) return;
    const to = beliefFromReport(token);
    const from = this.i.state.streamState;
    if (to === null || to === from) return;
    this.i.correct({ field: "streamState", from, to, token, ageMs: this.i.now() - fresh.receivedAt });
  }

  /** Whether a command still holds the stream belief (§10.1.3's first tier). */
  private streamClaimLive(): boolean {
    const e = this.pending.get("stream");
    return (e !== undefined && e.processedAt === null && e.expiredAt === null) || this.showStopPending !== null;
  }

  /** `show.stop` returned with outputs still active: it was not acknowledged (SPEC §5.9.5). */
  held(stopVersion: number): void {
    const { state } = this.i;
    // The stop's claim expired with its window. Only a report that postdates
    // this expiry may correct the belief, and the stop's own late
    // acknowledgement is still awaited (path (i)).
    this.showStopPending = null;
    this.heldStopVersion = stopVersion;
    this.streamAnchor = Math.max(this.streamAnchor, this.i.now());
    this.i.lane.raise(
      ALERT.outputsHeld,
      "warning",
      `show.stop (stateVersion ${stopVersion}) was not acknowledged within ${this.i.windowMs} ms; outputs held: record ${state.recordState}, stream ${state.streamState}`,
    );
  }

  close(): void {
    for (const entry of this.pending.values()) if (entry.timer) clearTimeout(entry.timer);
    if (this.showStopTimer) clearTimeout(this.showStopTimer);
  }
}
