//! Section 16.7 — ticker commands.

import { CpError, TICKER_MAX_ITEMS } from "../protocol.js";
import type { ControlPlaneState } from "../state.js";
import type { CommandRegistry, DispatchDeps, HandlerCtx, HandlerOutput } from "../dispatch.js";

function requireTicker(ctx: HandlerCtx): void {
  if (!ctx.state.requirePackage().tickerExists) {
    throw new CpError("E_NOT_FOUND", "no ticker declared in this package");
  }
}

interface TickerItem {
  text: string;
  language?: string;
  priority: number;
  ttlSec?: number;
  /** Acceptance + ttlSec, on the control plane's clock. */
  expiresAtMs?: number;
}

/** The queue as the engine gets it: text, language and priority, never a TTL
 * (review4 step 2; §5.9.1: the engine draws what it is told). */
export function tickerDirectiveItems(items: TickerItem[]): Array<{ text: string; language?: string; priority: number }> {
  return items.map(({ text, language, priority }) => (language === undefined ? { text, priority } : { text, language, priority }));
}

/** Remove the items whose TTL has run out at `nowMs`. Returns how many went. */
export function expireTickerItems(state: ControlPlaneState, nowMs: number): number {
  const before = state.tickerItems.length;
  state.tickerItems = state.tickerItems.filter((it) => it.expiresAtMs === undefined || it.expiresAtMs > nowMs);
  return before - state.tickerItems.length;
}

export function tickerHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("ticker.setSource", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      requireTicker(ctx);
      ctx.state.tickerSource = payload.source as "manual" | "rss" | "mixed";
      return {};
    },
  });

  reg.set("ticker.override", {
    // The engine gets the queue RESOLVED (SPEC §5.9.1: directives carry the
    // control plane's resolution, never re-resolved by the engine): the
    // ordered items after this command's mode and the priority sort, sent as
    // a `replace`. Forwarding the raw payload would make the engine repeat
    // the merge and the sort, and a second implementation of one rule is two
    // chances to disagree.
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      requireTicker(ctx);
      const accepted = Date.now();
      // A TTL counts from acceptance, on the control plane's clock.
      const items = (payload.items as TickerItem[]).map((it) =>
        it.ttlSec === undefined ? it : { ...it, expiresAtMs: accepted + it.ttlSec * 1000 },
      );
      const mode = payload.mode as "replace" | "prepend" | "append";
      const existing = ctx.state.tickerItems;
      let next: TickerItem[];
      if (mode === "replace") next = items;
      else if (mode === "prepend") next = [...items, ...existing];
      else next = [...existing, ...items];
      if (next.length > TICKER_MAX_ITEMS) {
        throw new CpError(
          "E_BAD_PAYLOAD",
          `the ticker holds at most ${TICKER_MAX_ITEMS} items; this ${mode} would make ${next.length}`,
        );
      }
      // Section 16.7 ordering: priority desc, insertion order preserved (stable sort).
      next = stablePriorityOrder(next);
      ctx.state.tickerItems = next;
      return {
        data: { count: next.length },
        extraDirectives: [{ command: "ticker.override", payload: { items: tickerDirectiveItems(next), mode: "replace" } }],
      };
    },
  });

  reg.set("ticker.clearOverride", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      requireTicker(ctx);
      ctx.state.tickerItems = [];
      return {};
    },
  });

  reg.set("ticker.refreshRss", {
    forward: false, // network fetch is async; the render node gets pushed via override
    handler: (ctx): HandlerOutput => {
      requireTicker(ctx);
      return { data: { refreshed: true } };
    },
  });
}

function stablePriorityOrder(items: TickerItem[]): TickerItem[] {
  return items
    .map((it, idx) => ({ it, idx }))
    .sort((a, b) => b.it.priority - a.it.priority || a.idx - b.idx)
    .map(({ it }) => it);
}
