//! Render bridge (Prompt 02 Step 8, addendum 02a §1.1): the interface the
//! render node consumes, plus two implementations:
//!  - `MockRenderBridge`: loopback with the ordered test hook required by CI.
//!  - `WsRenderBridge`: fans a directive out to every connected `render`-role
//!    WebSocket session. This is the production delivery path.

import { PROTOCOL_VERSION } from "./protocol.js";

/**
 * A directive handed to the render node. Matches the pinned frame B4.EXACTLY:
 * a separate protocol from the Section 5.4 command envelope, layered on the
 * same WebSocket connection.
 */
export interface RenderDirective {
  v: typeof PROTOCOL_VERSION;
  kind: "directive";
  /** Monotonic per-connection sequence (the WsRenderBridge increments it). */
  seq: number;
  /** stateVersion the directive was issued at. */
  stateVersion: number;
  command: string;
  /** Resolved target references (always present; empty object when none). */
  target: Record<string, unknown>;
  payload: Record<string, unknown>;
}

export type RenderBridge = {
  send(directive: Omit<RenderDirective, "v" | "kind" | "seq">): void;
};

/** Builds a full frame when a bridge only supplies the command fields. */
export function makeDirective(partial: Omit<RenderDirective, "v" | "kind" | "seq">, seq: number): RenderDirective {
  return { v: PROTOCOL_VERSION, kind: "directive", seq, ...partial };
}

// ---------------------------------------------------------------------------
// Mock (loopback, test hook)
// ---------------------------------------------------------------------------

/** Minimal partial-frame type used by callers for clarity. */
export type PartialDirective = Omit<RenderDirective, "v" | "kind" | "seq">;

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
 * Fan-out bridge: sends each directive to every registered render-role
 * session. Sessions register (on connect) and unregister (on close); each
 * gets a monotonically increasing `seq` so a render node can detect loss.
 */
export class WsRenderBridge implements RenderBridge {
  private senders = new Set<(frame: RenderDirective) => void>();
  private seqCounter = 0;
  private dropped = 0;

  /** Register a render session's send function; returns an unregister fn. */
  register(send: (frame: RenderDirective) => void): () => void {
    this.senders.add(send);
    let done = false;
    return () => {
      if (done) return;
      done = true;
      this.senders.delete(send);
    };
  }

  send(directive: PartialDirective): void {
    const frame = makeDirective(directive, this.seqCounter++);
    if (this.senders.size === 0) {
      this.dropped += 1;
      return; // no render node connected; dropping is intentional, never blocks
    }
    for (const send of this.senders) {
      try {
        send(frame);
      } catch (e) {
        console.warn(`render-bridge: send failed: ${String(e)}`);
      }
    }
  }

  renderNodeCount(): number {
    return this.senders.size;
  }

  droppedCount(): number {
    return this.dropped;
  }
}
