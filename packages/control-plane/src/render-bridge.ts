//! Render bridge (Prompt 02 Step 8): the interface the render node consumes,
//! plus the loopback/mock with the ordered test hook required by the CI gate.

/** A directive handed to the render node (fire-and-forget). */
export interface RenderDirective {
  command: string;
  /** Resolved target references (always present; empty object when none). */
  target: Record<string, unknown>;
  payload: Record<string, unknown>;
  /** stateVersion the directive was issued at. */
  stateVersion: number;
}

export interface RenderBridge {
  send(directive: RenderDirective): void;
}

/**
 * The loopback/mock bridge. Records every directive in order. `drain()`
 * is the test hook the CI gate uses: it returns (and clears) the recorded
 * directives in receipt order, with their stateVersions.
 */
export class MockRenderBridge implements RenderBridge {
  private queue: RenderDirective[] = [];
  private dropped = 0;

  constructor(private readonly capacity = 1024) {}

  send(directive: RenderDirective): void {
    if (this.queue.length >= this.capacity) {
      this.dropped += 1;
      // bounded queue: overflow is logged, never blocks the caller
      console.warn(`render-bridge: queue full (${this.capacity}), dropping directive ${directive.command}`);
      return;
    }
    this.queue.push(directive);
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
