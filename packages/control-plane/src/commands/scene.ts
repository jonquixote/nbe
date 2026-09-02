//! Section 16.3 — scene commands.

import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function sceneHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("scene.arm", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const sceneId = String(payload.sceneId);
      ctx.state.armScene(sceneId);
      return {};
    },
  });

  reg.set("scene.apply", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const sceneId = String(payload.sceneId);
      const target = payload.target as "view" | "preview";
      ctx.state.applyScene(sceneId, target);
      return {};
    },
  });
}
