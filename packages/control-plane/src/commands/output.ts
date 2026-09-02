//! Section 16.14 — output commands, Section 16.10–16.13 automation/snapshot/
//! marker/plugin/clock grouped here as state-surface commands.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function outputHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("record.start", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      const state = ctx.state;
      if (state.showState !== "RUNNING") throw new CpError("E_FORBIDDEN_STATE", "show is not running");
      if (state.recordState === "recording") throw new CpError("E_FORBIDDEN_STATE", "already recording");
      state.recordState = "recording";
      return {};
    },
  });

  reg.set("record.stop", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      if (ctx.state.recordState !== "recording") {
        throw new CpError("E_FORBIDDEN_STATE", "not recording");
      }
      ctx.state.recordState = "idle";
      return {};
    },
  });

  reg.set("stream.start", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      const state = ctx.state;
      if (state.showState !== "RUNNING") throw new CpError("E_FORBIDDEN_STATE", "show is not running");
      if (state.streamState === "live") throw new CpError("E_FORBIDDEN_STATE", "stream already live");
      state.streamState = "live";
      return {};
    },
  });

  reg.set("stream.stop", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      if (ctx.state.streamState !== "live") throw new CpError("E_FORBIDDEN_STATE", "stream not live");
      ctx.state.streamState = "idle";
      return {};
    },
  });
}
