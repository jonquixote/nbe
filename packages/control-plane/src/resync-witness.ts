/**
 * R32's test support (test-only; no production module imports it): what the
 * control plane stamped a render session's `show.resync` with at send time,
 * and a wait for the engine to apply that resync.
 *
 * Why: the control plane's current version is not a version any directive
 * carries. An item end the engine reports (`itemEvent`) moves it with no
 * directive to apply (`server.ts`, the `itemEvent` branch), so a wait for
 * "the engine applied the current version" can wait for a version no
 * directive will ever bring. Production never waits that way: the
 * acknowledgement waiters (`server.ts`) and the operator signals
 * (`alerts.ts`) compare with the version a directive was stamped with at
 * send time, or `>=`. `package-reconcile.e2e.ts`'s `reconnect()` did, and
 * that is R32's candidate mechanism (`docs/soak-protocol.md`, R32).
 */
import type { RenderSender, WsRenderBridge } from "./render-bridge.js";
import { RESYNC_COMMAND } from "./protocol.js";

/** Every `show.resync` the bridge's sessions were sent, in send order. */
export interface ResyncWitness {
  /** The version each `show.resync` was stamped with when it was sent. */
  readonly stamped: number[];
  /** Puts the bridge's own `register` back. */
  restore(): void;
}

/**
 * Wraps `bridge.register` so every session registered from now on records
 * the `stateVersion` of each `show.resync` frame it is handed, as it is
 * handed, before the socket sees it. The frame and the send's result are
 * untouched.
 */
export function witnessResyncs(bridge: WsRenderBridge): ResyncWitness {
  const stamped: number[] = [];
  const register = bridge.register;
  bridge.register = function (this: WsRenderBridge, send: RenderSender) {
    const witnessed: RenderSender = (frame) => {
      if (frame.command === RESYNC_COMMAND) stamped.push(frame.stateVersion);
      return send(frame);
    };
    return register.call(this, witnessed);
  };
  return {
    stamped,
    restore: () => {
      bridge.register = register;
    },
  };
}

/** How often, and at most how many times, the applied version is read. */
export const POLL_MS = 25;
/**
 * A count, never a deadline (R39; R48's form): a stall lengthens the wait and
 * never ends it early. 2,400 reads 25 ms apart is a minute or more.
 */
export const APPLY_POLLS = 2400;

export interface ResyncApplied {
  /** True once the engine reported a version at or past the resync's. */
  applied: boolean;
  /** The engine's last applied version at the final read. */
  last: number | null;
  /** How many reads were made. */
  polls: number;
}

/**
 * Waits until the engine's last applied version is at or past `resyncAt`,
 * the version its `show.resync` was stamped with at send time. Reads
 * `lastApplied` at most `polls` times, `POLL_MS` apart.
 */
export async function awaitResyncApplied(
  lastApplied: () => Promise<number | null>,
  resyncAt: number,
  polls: number = APPLY_POLLS,
): Promise<ResyncApplied> {
  let last: number | null = null;
  for (let i = 1; i <= polls; i++) {
    last = await lastApplied();
    if (last !== null && last >= resyncAt) return { applied: true, last, polls: i };
    if (i < polls) await new Promise((r) => setTimeout(r, POLL_MS));
  }
  return { applied: false, last, polls };
}

export interface HandshakeResync extends ResyncApplied {
  /** The version the session's handshake `show.resync` was stamped with, if one was sent. */
  resyncAt: number | undefined;
  /** The control plane's current version at the final read (for a failure's message only). */
  current: number;
}

/**
 * After a render node registers again: waits for it to apply the
 * `show.resync` its handshake was sent, the first one witnessed after
 * `since` (the witness's count before the drop). Never compares with
 * `current()`: that is read for the failure's message alone.
 */
export async function awaitHandshakeResync(o: {
  witness: ResyncWitness;
  since: number;
  lastApplied: () => Promise<number | null>;
  current: () => number;
  polls?: number;
}): Promise<HandshakeResync> {
  const resyncAt = o.witness.stamped[o.since];
  if (resyncAt === undefined) return { applied: false, resyncAt, last: await o.lastApplied(), polls: 0, current: o.current() };
  const r = await awaitResyncApplied(o.lastApplied, resyncAt, o.polls);
  return { ...r, resyncAt, current: o.current() };
}
