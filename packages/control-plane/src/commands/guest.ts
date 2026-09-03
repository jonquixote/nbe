//! Section 16.9 — guest commands.

import { CpError } from "../protocol.js";
import { createHmac } from "node:crypto";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function guestHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("guest.connect", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const guestId = String(payload.guestId);
      if (ctx.state.guests.has(guestId)) {
        throw new CpError("E_FORBIDDEN_STATE", `guest already connected: ${guestId}`);
      }
      ctx.state.guests.set(guestId, {
        guestId,
        displayName: payload.displayName as string | undefined,
        layout: "full",
        muted: false,
        returnConfig: { mode: "programMinusSelf", includeOtherGuests: true },
      });
      return {};
    },
  });

  reg.set("guest.disconnect", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const guestId = String(payload.guestId);
      if (!ctx.state.guests.delete(guestId)) {
        throw new CpError("E_NOT_FOUND", `no such guest: ${guestId}`);
      }
      return {};
    },
  });

  reg.set("guest.setLayout", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const g = mustGuest(ctxStateOf(ctx), String(payload.guestId));
      g.layout = payload.layout as "pip" | "full";
      return {};
    },
  });

  reg.set("guest.placeholder", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const g = mustGuest(ctxStateOf(ctx), String(payload.guestId));
      const assetId = payload.assetId as string | undefined;
      if (assetId && !ctx.state.requirePackage().assets.has(assetId)) {
        throw new CpError("E_NOT_FOUND", `no such asset: ${assetId}`);
      }
      g.placeholderAssetId = assetId;
      return {};
    },
  });

  reg.set("guest.configureReturn", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const g = mustGuest(ctxStateOf(ctx), String(payload.guestId));
      g.returnConfig = {
        mode: (payload.mode as "programMinusSelf" | "producerMix" | "mute") ?? "programMinusSelf",
        includeOtherGuests: (payload.includeOtherGuests as boolean) ?? true,
        gainDb: payload.gainDb as number | undefined,
        muted: payload.muted as boolean | undefined,
      };
      return {};
    },
  });

  reg.set("guest.getTurn", {
    forward: false,
    handler: (ctx, payload): HandlerOutput => {
      mustGuest(ctxStateOf(ctx), String(payload.guestId));
      const ttlSec = (payload.ttlSec as number) ?? 600;
      const secret = process.env.NBE_TURN_SECRET;
      const uris = (process.env.NBE_TURN_URIS ?? "").split(",").map((u) => u.trim()).filter(Boolean);
      // SPEC §9.6.2: without a configured shared secret there is no credential
      // that any TURN server would accept. Failing here is honest; vending a
      // placeholder defers the failure to ICE, mid-show, where it is
      // indistinguishable from a network fault.
      if (!secret || uris.length === 0) {
        throw new CpError(
          "E_UNSUPPORTED_FEATURE",
          "TURN vending is not configured (set NBE_TURN_SECRET and NBE_TURN_URIS)",
        );
      }
      // coturn long-term-credential REST convention.
      const expiry = Math.floor(Date.now() / 1000) + ttlSec;
      const username = `${expiry}:${String(payload.guestId)}`;
      const credential = createHmac("sha1", secret).update(username).digest("base64");
      return { data: { uris, username, credential, ttlSec } };
    },
  });
}

import type { ControlPlaneState } from "../state.js";
import type { HandlerCtx } from "../dispatch.js";

function ctxStateOf(ctx: HandlerCtx): ControlPlaneState {
  return ctx.state;
}

type GuestState = ControlPlaneState["guests"] extends Map<string, infer G> ? G : never;

function mustGuest(state: ControlPlaneState, guestId: string): GuestState {
  const g = state.guests.get(guestId);
  if (!g) throw new CpError("E_NOT_FOUND", `no such guest: ${guestId}`);
  return g;
}
