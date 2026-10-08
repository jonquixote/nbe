//! Section 16.2 — view/preview commands.
//! view.take resolves presets and the Section 16.2 audio-default rule
//! (audio.durationFrames defaults to video durationFrames on mix); the
//! directive sent to the render node carries the RESOLVED transition
//! (never a preset name).

import { CpError, TakeDirectivePayloadSchema, type TakeDirectivePayload } from "../protocol.js";
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

      // Build and validate the directive BEFORE the take mutates anything
      // (the take invariant, on `takePayload` below). The transition resolves
      // first: explicit payload fields override the preset.
      const directive = {
        command: "view.take",
        target: { itemRef: preview },
        payload: takePayload(state, preview, resolveTransition(state, payload)),
      };
      const next = state.take(preview);
      return { data: { item: preview, state: next }, extraDirectives: [directive] };
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
      // Built and validated before the arm and the take mutate anything (the
      // take invariant, on `takePayload` below).
      const directive = {
        command: "view.take",
        target: { itemRef },
        payload: takePayload(state, itemRef, { transition: "cut" }),
      };
      if (cur !== "ARMED") state.armItem(itemRef); // cut implies arm+take
      const next = state.take(itemRef);
      return { data: { item: itemRef, state: next }, extraDirectives: [directive] };
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

/**
 * The payload of a directive that puts `itemRef` on the View: the resolved
 * transition `base`, plus the ITEM's own duration when the item is timed
 * (SPEC v0.4.8 row 2). The engine schedules the item's end from
 * `itemDurationFrames` and never from the transition's `durationFrames`. It
 * used to, so a cut never ended and a mix ended a timed item at the mix's
 * length. Parsed through `TakeDirectivePayloadSchema` before it is sent: a
 * payload the engine could not read is refused here, not there.
 *
 * **The take invariant (the user's word of 2026-10-06): no command path
 * mutates state before its directive validates.** The order is build →
 * validate → mutate → send. view.take, view.cut and snapshot.recall each call
 * this before their first state write, so a payload that does not validate is
 * refused with the control plane exactly where it was, and the engine never
 * hears of it. PR #37's two-key pass found the other order (P1): the parse ran
 * after `state.take`, so a refusal left the control plane's View moved and the
 * engine's not.
 */
export function takePayload(
  state: import("../state.js").ControlPlaneState,
  itemRef: string | null,
  base: Record<string, unknown>,
): TakeDirectivePayload {
  const durationFrames = itemRef === null ? undefined : state.requireItem(itemRef).durationFrames;
  return TakeDirectivePayloadSchema.parse(
    durationFrames == null ? base : { ...base, itemDurationFrames: durationFrames },
  );
}

/**
 * A take's transition, resolved — the directive payload the engine applies
 * (never a preset name). Exported for `snapshot.recall`, whose cut-class
 * application goes through the same resolution (`commands/state.ts`).
 */
export function resolveTransition(
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
  let transition = payload.transition ?? base.kind ?? "cut";
  let durationFrames = payload.durationFrames ?? base.durationFrames;
  // A 0-frame mix is a cut, and it resolves as one. §16.2 allows the 0
  // (`durationFrames` 0–600), but its audio-follows-video rule below would
  // copy it into `audio.durationFrames`, whose minimum is 1: a value the spec
  // forbids, which the take payload's parse then refused (PR #37's two-key
  // pass, P1). The command schema is unchanged; the resolution is where the 0
  // becomes honest.
  if (transition === "mix" && durationFrames === 0) {
    transition = "cut";
    durationFrames = undefined;
  }
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

