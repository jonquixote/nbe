//! Section 16.15 — system commands.

import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export interface SystemHooks {
  onTelemetrySubscribe(intervalMs: number): void;
  onTelemetryUnsubscribe(): void;
}

export function systemHandlers(
  reg: CommandRegistry,
  _deps: DispatchDeps,
): void {
  reg.set("system.status", {
    forward: false,
    handler: (ctx): HandlerOutput => {
      return { data: ctx.state.statusSnapshot() };
    },
  });

  reg.set("system.telemetry.subscribe", {
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      ctx.systemHooks?.onTelemetrySubscribe((payload.intervalMs as number) ?? 1000);
      return {};
    },
  });

  reg.set("system.telemetry.unsubscribe", {
    forward: false,
    handler: (ctx): HandlerOutput => {
      ctx.systemHooks?.onTelemetryUnsubscribe();
      return {};
    },
  });
}
