//! Section 16.8 — soundboard/audio commands.

import { CpError } from "../protocol.js";
import { randomUUID } from "node:crypto";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function audioHandlers(reg: CommandRegistry, deps: DispatchDeps): void {
  reg.set("soundboard.play", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const assetId = String(payload.assetId);
      if (!ctx.state.requirePackage().assets.has(assetId)) {
        throw new CpError("E_NOT_FOUND", `no such asset: ${assetId}`);
      }
      const playbackId = randomUUID();
      ctx.state.soundboardPings.push({ playbackId, assetId, gainDb: payload.gainDb as number | undefined });
      return { data: { playbackId } };
    },
  });

  reg.set("soundboard.stop", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const playbackId = payload.playbackId as string | undefined;
      const assetId = payload.assetId as string | undefined;
      const before = ctx.state.soundboardPings.length;
      ctx.state.soundboardPings = ctx.state.soundboardPings.filter(
        (p) => p.playbackId !== playbackId && p.assetId !== assetId,
      );
      if (ctx.state.soundboardPings.length === before) {
        throw new CpError("E_NOT_FOUND", "no matching active playback");
      }
      return {};
    },
  });

  reg.set("soundboard.stopAll", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      ctx.state.soundboardPings = [];
      return {};
    },
  });

  reg.set("audio.bus.set", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const bus = String(payload.bus);
      const guestId = payload.guestId as string | undefined;
      if (bus === "guestReturn" && !guestId) {
        throw new CpError("E_BAD_PAYLOAD", "guestId required for guestReturn bus");
      }
      if (bus !== "guestReturn" && guestId) {
        // spec: ignored
      }
      const key = bus === "guestReturn" ? `guestReturn:${guestId}` : bus;
      const existing = ctx.state.audioBuses.get(key) ?? { muted: false };
      ctx.state.audioBuses.set(key, {
        ...existing,
        ...(payload.gainDb !== undefined ? { gainDb: payload.gainDb as number } : {}),
        ...(payload.muted !== undefined ? { muted: payload.muted as boolean } : {}),
      });
      return {};
    },
  });

  reg.set("audio.duck", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      ctx.state.ducking = {
        enabled: payload.enabled as boolean,
        depthDb: (payload.depthDb as number | undefined) ?? -6,
        attackMs: (payload.attackMs as number | undefined) ?? 10,
        releaseMs: (payload.releaseMs as number | undefined) ?? 250,
      };
      return {};
    },
  });

  reg.set("guest.mute", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const guest = ctx.state.guests.get(String(payload.guestId));
      if (!guest) throw new CpError("E_NOT_FOUND", `no such guest: ${String(payload.guestId)}`);
      guest.muted = payload.muted as boolean;
      return {};
    },
  });

  void deps;
}
