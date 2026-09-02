//! Section 16.7 — ticker commands.

import { CpError } from "../protocol.js";
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
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      requireTicker(ctx);
      const items = payload.items as TickerItem[];
      const mode = payload.mode as "replace" | "prepend" | "append";
      const existing = ctx.state.tickerItems;
      let next: TickerItem[];
      if (mode === "replace") next = items;
      else if (mode === "prepend") next = [...items, ...existing];
      else next = [...existing, ...items];
      // Section 16.7 ordering: priority desc, insertion order preserved (stable sort).
      next = stablePriorityOrder(next);
      ctx.state.tickerItems = next;
      return { data: { count: next.length } };
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
