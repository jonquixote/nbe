//! Render bridge (Prompt 02 Step 8, addendum 02a §1.1): the interface the
//! render node consumes, plus two implementations:
//!  - `MockRenderBridge`: loopback with the ordered test hook required by CI.
//!  - `WsRenderBridge`: fans a directive out to every connected `render`-role
//!    WebSocket session. This is the production delivery path.

import { PROTOCOL_VERSION } from "./protocol.js";

/**
 * A directive handed to the render node. Matches the pinned frame:
 * a separate protocol from the Section 5.4 command envelope, layered on the
 * same WebSocket connection. `seq` is a per-connection monotonic counter and
 * resets to 0 when a connection is (re)established, so gap detection is
 * meaningful within a single connection only.
 */
export interface RenderDirective {
  v: typeof PROTOCOL_VERSION;
  kind: "directive";
  /** Per-connection monotonic sequence; resets to 0 on (re)connect. */
  seq: number;
  /** stateVersion the directive was issued at. */
  stateVersion: number;
  command: string;
  /** Resolved target references (always present; empty object when none). */
  target: Record<string, unknown>;
  payload: Record<string, unknown>;
}

/** The command fields a bridge implementation supplies; v/kind/seq are added by the bridge. */
export type PartialDirective = Pick<RenderDirective, "command" | "target" | "payload" | "stateVersion">;

export type RenderBridge = {
  send(directive: PartialDirective): void;
};

/** Builds a full frame when a bridge only supplies the command fields. */
export function makeDirective(partial: PartialDirective, seq: number): RenderDirective {
  return { v: PROTOCOL_VERSION, kind: "directive", seq, ...partial };
}

// ---------------------------------------------------------------------------
// Mock (loopback, test hook)
// ---------------------------------------------------------------------------

/**
 * The loopback/mock bridge. Records every directive in receipt order.
 * `drain()` is the test hook the CI gate uses: returns (and clears) the
 * recorded directives in order, with their stateVersions.
 */
export class MockRenderBridge implements RenderBridge {
  private queue: RenderDirective[] = [];
  private dropped = 0;
  private seqCounter = 0;

  constructor(private readonly capacity = 1024) {}

  send(directive: PartialDirective): void {
    if (this.queue.length >= this.capacity) {
      this.dropped += 1;
      console.warn(`render-bridge: queue full (${this.capacity}), dropping directive ${directive.command}`);
      return;
    }
    this.queue.push(makeDirective(directive, this.seqCounter++));
  }

  /** Test hook: returns all directives in receive order, clears the queue. */
  drain(): RenderDirective[] {
    const out = this.queue;
    this.queue = [];
    return out;
  }

  droppedCount(): number {
    return this.dropped;
  }

  pending(): number {
    return this.queue.length;
  }
}

// ---------------------------------------------------------------------------
// WebSocket fan-out (production)
// ---------------------------------------------------------------------------

/**
 * A registered render session's outbound hook. Returns true when the frame was
 * handed to the socket, false when it was dropped (backpressure) or errored.
 */
export type RenderSender = (frame: RenderDirective) => boolean;

interface Session {
  send: RenderSender;
  seq: number;
  dropped: number;
}

/**
 * Fan-out bridge: sends each directive to every registered render-role
 * session. Each session keeps its OWN `seq` counter (resetting to 0 on
 * connect), so a node that joins mid-show starts at seq 0 and gap detection
 * never conflates "joined late" with "lost directives".
 *
 * Bounded/backpressure: the underlying sender (server.ts) checks
 * `ws.bufferedAmount` and returns false when the socket is too far behind;
 * the bridge counts those drops and logs. Overflow never blocks dispatch.
 */
export class WsRenderBridge implements RenderBridge {
  private sessions = new Map<symbol, Session>();
  // counters for bookkeeping only; per-session seq lives in Session
  private droppedTotal = 0;

  register(send: RenderSender): () => void {
    const id = Symbol("render-session");
    this.sessions.set(id, { send, seq: 0, dropped: 0 });
    let done = false;
    return () => {
      if (done) return;
      done = true;
      const removed = this.sessions.get(id);
      if (removed) this.droppedTotal += removed.dropped;
      this.sessions.delete(id);
    };
  }

  send(directive: PartialDirective): void {
    if (this.sessions.size === 0) {
      this.droppedTotal += 1;
      return; // no render node connected; dropping is intentional, never blocks
    }
    for (const session of this.sessions.values()) {
      const frame = makeDirective(directive, session.seq++);
      let ok = false;
      try {
        ok = session.send(frame);
      } catch (e) {
        console.warn(`render-bridge: send failed: ${String(e)}`);
      }
      if (!ok) {
        session.dropped += 1;
        this.droppedTotal += 1;
        console.warn(`render-bridge: dropping directive ${directive.command} (backpressure)`);
      }
    }
  }

  renderNodeCount(): number {
    return this.sessions.size;
  }

  droppedCount(): number {
    return this.droppedTotal;
  }
}
