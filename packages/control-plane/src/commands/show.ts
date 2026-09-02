//! Section 16.1 — show commands.
//! show.load / show.preflight shell out to nbe-preflight (addendum §1.4);
//! show.stop implements the Section 16.1 quiescence truth table.

import { CpError } from "../protocol.js";
import { loadPackage, runPreflight, type PreflightResult } from "../package.js";
import type { CommandRegistry, DispatchDeps, HandlerCtx, HandlerOutput } from "../dispatch.js";
import { SHOW_STOP_FORCE_WARNING } from "../dispatch.js";

export function showHandlers(reg: CommandRegistry, deps: DispatchDeps): void {
  reg.set("show.load", {
    forward: true,
    handler: async (ctx: HandlerCtx, payload): Promise<HandlerOutput> => {
      const state = ctx.state;
      if (state.showState === "RUNNING") {
        throw new CpError("E_FORBIDDEN_STATE", "cannot load a package while the view is live");
      }
      if (state.pkg && payload.mode !== "reload") {
        throw new CpError("E_FORBIDDEN_STATE", "a package is already loaded; pass mode: reload");
      }
      const pkg = await loadPackage(String(payload.packagePath), {});
      state.loadPackage(pkg);
      state.preflightPassed = true; // preflight ran green (0) or warnings-only (1) in loadPackage
      return { data: { packagePath: pkg.packagePath, showId: pkg.showId } };
    },
  });

  reg.set("show.preflight", {
    forward: false,
    handler: async (ctx): Promise<HandlerOutput> => {
      const pkg = ctx.state.pkg;
      if (!pkg) throw new CpError("E_FORBIDDEN_STATE", "no show loaded; load a package first");
      const result: PreflightResult = await runPreflight(pkg.packagePath, {});
      if (result.exitCode === 2) {
        ctx.state.preflightPassed = false;
        ctx.state.lastError = "preflight failed";
        throw new CpError("E_PREFLIGHT_FAILED", result.report?.errors.join("; ") ?? "preflight failed");
      }
      if (result.exitCode !== 0 && result.exitCode !== 1) {
        throw new CpError("E_PREFLIGHT_FAILED", `preflight could not run (exit ${result.exitCode})`);
      }
      ctx.state.preflightPassed = true;
      ctx.state.lastError = null;
      return { data: { airReady: result.exitCode === 0, warnings: result.report?.warnings ?? [] } };
    },
  });

  reg.set("show.start", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      const state = ctx.state;
      if (state.showState === "RUNNING") throw new CpError("E_FORBIDDEN_STATE", "show already running");
      if (state.showState !== "LOADED") throw new CpError("E_FORBIDDEN_STATE", "no package loaded");
      if (!state.preflightPassed) {
        throw new CpError("E_FORBIDDEN_STATE", "preflight has not passed for the loaded package");
      }
      state.showState = "RUNNING";
      deps.persistence.flushNow();
      return {};
    },
  });

  reg.set("show.stop", {
    forward: true,
    handler: async (ctx, payload): Promise<HandlerOutput> => {
      const state = ctx.state;
      const quiesce = payload.quiesceOutputs as boolean;
      const force = payload.force as boolean;
      if (state.showState !== "RUNNING" && !force) {
        throw new CpError("E_FORBIDDEN_STATE", "show is not running");
      }
      const outputsWereActive = state.outputsActive();
      if (outputsWereActive && !quiesce && !force) {
        throw new CpError("E_FORBIDDEN_STATE", "outputs active; pass quiesceOutputs or force");
      }

      const warnings: string[] = [];
      if (outputsWereActive) {
        if (force) {
          warnings.push(SHOW_STOP_FORCE_WARNING);
          deps.warn?.(SHOW_STOP_FORCE_WARNING);
        } else {
          // 2-second graceful window: wait for the render node to confirm
          // the stop directives (via appliedStateVersion) before forcing.
          const graceMs = deps.showStopGraceMs ?? 2000;
          if (deps.waitForGrace) await deps.waitForGrace(graceMs);
        }
      }

      state.recordState = "idle";
      state.streamState = "idle";
      state.showState = "STOPPED";
      deps.persistence.flushNow();

      const extraDirectives = outputsWereActive
        ? [
            { command: "record.stop", payload: {} },
            { command: "stream.stop", payload: {} },
          ]
        : [];
      return { warnings, extraDirectives };
    },
  });

  reg.set("show.unload", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      const state = ctx.state;
      if (state.showState === "RUNNING") throw new CpError("E_FORBIDDEN_STATE", "cannot unload while live");
      state.unloadPackage();
      deps.persistence.flushNow();
      return {};
    },
  });
}
