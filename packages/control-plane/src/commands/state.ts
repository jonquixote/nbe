//! Section 16.6 overlay, 16.10–16.13 — automation, snapshot/marker, plugin,
//! clock. Small state-surface commands grouped here.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";
import { resolveTransition, takePayload } from "./view.js";

export function stateHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  // overlay
  reg.set("overlay.show", {
    // forward:false — the directive is produced explicitly below so it carries
    // the overlayId on `target` (mirroring view.take's `target: { itemRef }`),
    // which is what the engine's on_overlay reads.
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      const overlayId = String(payload.overlayId);
      ctx.state.requireOverlay(overlayId);
      // §16.6 + recorded assumption: show on an on-air overlay is an
      // idempotent success no-op, noted as `data.noop` for the change stream.
      if (ctx.state.visibleOverlays.has(overlayId)) {
        return { data: { noop: true } };
      }
      ctx.state.visibleOverlays.add(overlayId);
      ctx.state.overlayAnimation.set(overlayId, "enter");
      const directive: Record<string, unknown> = {};
      if (payload.animation !== undefined) directive.animation = payload.animation;
      return {
        extraDirectives: [{ command: "overlay.show", target: { overlayId }, payload: directive }],
      };
    },
  });

  reg.set("overlay.hide", {
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      const overlayId = String(payload.overlayId);
      ctx.state.requireOverlay(overlayId);
      // Mirror of show: hide on a hidden overlay is an idempotent no-op.
      if (!ctx.state.visibleOverlays.has(overlayId)) {
        return { data: { noop: true } };
      }
      ctx.state.visibleOverlays.delete(overlayId);
      ctx.state.overlayAnimation.delete(overlayId);
      return {
        extraDirectives: [{ command: "overlay.hide", target: { overlayId }, payload: {} }],
      };
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
    // forward:false — the engine gets the RESOLVED recall below, never the
    // snapshot's name (§5.9.1: the engine never resolves). Forwarding the
    // name was the Prompt 13 re-plan's P1 split brain: the engine routed
    // nothing for it, so a recall moved `viewItem` here while the old View
    // stayed on air.
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      const state = ctx.state;
      const name = String(payload.name);
      const snap = state.requireSnapshot(name);
      // The View the audience sees, applied as a cut through the take's own
      // resolution: `target.itemRef` only when the recalled item is not the
      // one on air (view.cut's "already on view" rule — a recall does not
      // restart the item it leaves on air), `null` for an empty View, and
      // the overlays wholesale. The preview half stays here: the engine
      // routes no preview writer yet.
      //
      // Built from the snapshot and validated BEFORE the recall mutates
      // anything (the take invariant, on `takePayload` in view.ts): the
      // directive used to be built from the state after `recallSnapshot`.
      const target: Record<string, unknown> = {};
      if (snap.viewItem !== state.viewItem) target.itemRef = snap.viewItem;
      const directive = {
        command: "snapshot.recall",
        target,
        // The recalled item's own duration rides with the cut when the item
        // is timed (v0.4.8 row 2): a recall starts the item now, so its end
        // is scheduled from now.
        payload: takePayload(state, typeof target.itemRef === "string" ? target.itemRef : null, {
          ...resolveTransition(state, { transition: "cut" }),
          // As `recallSnapshot` restores them: a set, in the snapshot's order.
          visibleOverlays: Array.from(new Set(snap.visibleOverlays)),
        }),
      };
      state.recallSnapshot(name);
      return { extraDirectives: [directive] };
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
