//! Section 16.6 overlay, 16.10–16.13 — automation, snapshot/marker, plugin,
//! clock. Small state-surface commands grouped here.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function stateHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  // overlay
  reg.set("overlay.show", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const overlayId = String(payload.overlayId);
      ctx.state.requireOverlay(overlayId);
      ctx.state.visibleOverlays.add(overlayId);
      return {};
    },
  });

  reg.set("overlay.hide", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const overlayId = String(payload.overlayId);
      ctx.state.requireOverlay(overlayId);
      ctx.state.visibleOverlays.delete(overlayId);
      return {};
    },
  });

  // automation
  reg.set("automation.enable", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      setRule(ctx, String(payload.ruleId), true);
      return {};
    },
  });

  reg.set("automation.disable", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      setRule(ctx, String(payload.ruleId), false);
      return {};
    },
  });

  reg.set("automation.hold", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.automationHold = payload.hold as boolean;
      return {};
    },
  });

  // snapshot/marker
  reg.set("snapshot.save", {
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.saveSnapshot(String(payload.name));
      return {};
    },
  });

  reg.set("snapshot.recall", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.recallSnapshot(String(payload.name));
      return {};
    },
  });

  reg.set("marker.add", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      if (ctx.state.showState !== "RUNNING") {
        throw new CpError("E_FORBIDDEN_STATE", "show is not running");
      }
      ctx.state.markers.push({
        name: String(payload.name),
        timecode: payload.timecode as string | undefined,
      });
      return {};
    },
  });

  // plugin
  reg.set("plugin.reload", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const pluginId = String(payload.pluginId);
      if (!ctx.state.requirePackage().plugins.has(pluginId)) {
        throw new CpError("E_NOT_FOUND", `no such plugin: ${pluginId}`);
      }
      return {};
    },
  });

  // clock
  reg.set("clock.configure", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const elementId = String(payload.elementId);
      if (!ctx.state.requirePackage().clockElements.has(elementId)) {
        throw new CpError("E_NOT_FOUND", `no clock element: ${elementId}`);
      }
      ctx.state.elementOverrides.set(elementId, { clock: payload.clock });
      return {};
    },
  });
}

function setRule(
  ctx: { state: { automationRules: Map<string, boolean>; requirePackage: () => { automationRules: Set<string> } } },
  ruleId: string,
  enabled: boolean,
): void {
  if (!ctx.state.requirePackage().automationRules.has(ruleId)) {
    throw new CpError("E_NOT_FOUND", `no such automation rule: ${ruleId}`);
  }
  ctx.state.automationRules.set(ruleId, enabled);
}
