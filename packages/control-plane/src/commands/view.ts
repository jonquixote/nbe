//! Section 16.2 — view/preview commands.
//! view.take resolves presets and the Section 16.2 audio-default rule
//! (audio.durationFrames defaults to video durationFrames on mix); the
//! directive sent to the render node carries the RESOLVED transition
//! (never a preset name).

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function viewHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("preview.set", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const itemRef = String(payload.itemRef);
      const state = ctx.state;
      const item = state.requireItem(itemRef);
      const cur = state.itemStateOf(itemRef);
      if (cur !== "READY" && cur !== "ARMED") {
        throw new CpError("E_FORBIDDEN_STATE", `item ${itemRef} is ${cur}`);
      }
      if (item.assetId) {
        const pkg = state.requirePackage();
        if (!pkg.assets.has(item.assetId)) {
          throw new CpError("E_ASSET_MISSING", `asset ${item.assetId} not declared in manifest`);
        }
      }
      if (cur === "READY") state.armItem(itemRef);
      if (state.previewItem && state.previewItem !== itemRef) {
        const prevState = state.itemStateOf(state.previewItem);
        if (prevState === "ARMED") state.itemStates.set(state.previewItem, "READY");
      }
      state.previewItem = itemRef;
      return {};
    },
  });

  reg.set("view.take", {
    forward: false, // directives here are the resolved extraDirectives only
    handler: (ctx, payload): HandlerOutput => {
      const state = ctx.state;
      const preview = state.previewItem;
      if (!preview) throw new CpError("E_FORBIDDEN_STATE", "no preview item armed");
      const item = state.requireItem(preview);

      // Resolve the transition: explicit payload fields override preset.
      const resolved = resolveTransition(state, payload);
      const next = state.take(preview);
      return {
        data: { item: preview, state: next },
        extraDirectives: [
          {
            command: "view.take",
            target: { itemRef: preview },
            payload: resolved,
          },
        ],
      };
    },
  });

  reg.set("view.cut", {
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      const state = ctx.state;
      const itemRef = String(payload.itemRef);
      state.requireItem(itemRef);
      const cur = state.itemStateOf(itemRef);
      if (cur === "LIVE" || cur === "PLAYING") {
        return { data: { item: itemRef, state: cur } }; // already on view
      }
      if (cur !== "ARMED") state.armItem(itemRef); // cut implies arm+take
      const next = state.take(itemRef);
      return {
        data: { item: itemRef, state: next },
        extraDirectives: [
          { command: "view.take", target: { itemRef }, payload: { transition: "cut" } },
        ],
      };
    },
  });

  reg.set("view.fallback", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      ctx.state.fallbackActive = true;
      return { data: { fallbackActive: true } };
    },
  });
}

function resolveTransition(
  state: import("../state.js").ControlPlaneState,
  payload: Record<string, unknown>,
): Record<string, unknown> {
  // Preset lookup (payload fields win over preset, spec 16.2).
  let base: Record<string, unknown> = {};
  if (typeof payload.preset === "string") {
    const preset = state.requirePackage().transitionPresets.get(payload.preset);
    if (!preset) throw new CpError("E_NOT_FOUND", `no such transition preset: ${payload.preset}`);
    base = preset;
  }
  const transition = payload.transition ?? base.kind ?? "cut";
  const durationFrames = payload.durationFrames ?? base.durationFrames;
  const audio = (payload.audio ?? base.audio ?? {}) as Record<string, unknown>;
  const audioTransition = audio.transition ?? "follow";
  const resolved: Record<string, unknown> = { transition, audio: { ...audio, transition: audioTransition } };
  if (durationFrames !== undefined) resolved.durationFrames = durationFrames;
  // Section 16.2: mix without audio.durationFrames uses video durationFrames.
  if (transition === "mix" && !(resolved.audio as Record<string, unknown>).durationFrames) {
    if (durationFrames !== undefined) {
      (resolved.audio as Record<string, unknown>).durationFrames = durationFrames;
    }
  }
  return resolved;
}

