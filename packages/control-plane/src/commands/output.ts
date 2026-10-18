//! Section 16.14 — output commands, Section 16.10–16.13 automation/snapshot/
//! marker/plugin/clock grouped here as state-surface commands.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";
import { unreachable } from "../state.js";

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
      // SPEC §16, v0.4.8 row 11 (D3): a stream starts only from `idle`. A
      // `held` stream's state is unknown, so a start is refused by name.
      switch (state.streamState) {
        case "idle":
          break;
        case "live":
          throw new CpError("E_FORBIDDEN_STATE", "stream already live");
        case "held":
          throw new CpError("E_FORBIDDEN_STATE", "stream is held: its stop was not acknowledged");
        default:
          return unreachable(state.streamState);
      }
      state.streamState = "live";
      return {};
    },
  });

  reg.set("stream.stop", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      // SPEC §16, v0.4.8 row 11 (D3): a stream is stopped from `live` or from
      // `held`, which is the operator's only way out of a hold. From `idle`
      // there is nothing to stop, and the refusal names the state.
      switch (ctx.state.streamState) {
        case "live":
        case "held":
        case "idle": // FALSIFICATION (D6 idle refusal): stream.stop accepts idle
          break;
        default:
          return unreachable(ctx.state.streamState);
      }
      ctx.state.streamState = "idle";
      return {};
    },
  });
}
