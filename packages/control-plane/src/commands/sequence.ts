//! Section 16.4 — sequence/item commands.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function sequenceHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("sequence.arm", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const id = String(payload.sequenceId);
      const pkg = ctx.state.requirePackage();
      if (pkg.sequences.has(id) || pkg.items.has(id)) {
        for (const itemId of collectItemIds(pkg, id)) {
          const cur = ctx.state.itemStateOf(itemId);
          if (cur === "READY" || cur === "DONE") ctx.state.armItem(itemId);
        }
        return {};
      }
      throw new CpError("E_NOT_FOUND", `no such sequence: ${id}`);
    },
  });

  reg.set("sequence.unarm", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const id = String(payload.sequenceId);
      const pkg = ctx.state.requirePackage();
      if (!pkg.sequences.has(id) && !pkg.items.has(id)) {
        throw new CpError("E_NOT_FOUND", `no such sequence: ${id}`);
      }
      for (const itemId of collectItemIds(pkg, id)) {
        if (ctx.state.itemStateOf(itemId) === "ARMED") ctx.state.unarmItem(itemId);
      }
      return {};
    },
  });

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
