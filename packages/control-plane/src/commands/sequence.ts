//! Section 16.4 — item commands.
//!
//! `sequence.arm` / `sequence.unarm` were removed in SPEC v0.4 along with the
//! `sequenceRef` hook they served: the schema declares exactly one Sequence,
//! `rundown`, so they could only ever resolve that one id. See §16.4.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function sequenceHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("item.arm", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.armItem(String(payload.itemId));
      return {};
    },
  });

  reg.set("item.unarm", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.unarmItem(String(payload.itemId));
      return {};
    },
  });

  reg.set("item.stop", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.stopItem(String(payload.itemId));
      return {};
    },
  });

  // Section 16.4 (new in SPEC v0.3.2): the command that produces the
  // Section 17.3 `reset` event. Without it DONE/MISSING/ERROR are terminal
  // for every client and a single failed item forces a show reload.
  reg.set("item.reset", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const itemId = String(payload.itemId);
      ctx.state.resetItem(itemId);
      return { data: { itemId, state: "READY" } };
    },
  });
}

function collectItemIds(
  pkg: { sequences: Set<string>; items: Map<string, { id: string }> },
  id: string,
): string[] {
  if (pkg.items.has(id)) return [id];
  // sequence ids: fall back to all items (the v0.3 rundown is a single
  // non-recursive sequence; nested registries are 02a §3 out-of-scope).
  return [...pkg.items.keys()];
}
