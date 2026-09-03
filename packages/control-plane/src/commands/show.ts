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
      const loaded = await loadPackage(String(payload.packagePath), {});
      state.loadPackage(loaded.pkg);
      // SPEC §16.1: loading a warnings-only package is fine; going to air on
      // one is an explicit decision. `airReady` stays true only at exit 0.
      state.preflightPassed = loaded.exitCode === 0;
      state.preflightWarnings = loaded.warnings;
      return {
        data: {
          packagePath: loaded.pkg.packagePath,
          showId: loaded.pkg.showId,
          airReady: loaded.exitCode === 0,
          warnings: loaded.warnings,
        },
      };
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
      ctx.state.preflightPassed = result.exitCode === 0;
      ctx.state.preflightWarnings = result.report?.warnings ?? [];
      ctx.state.lastError = null;
      return { data: { airReady: result.exitCode === 0, warnings: ctx.state.preflightWarnings } };
    },
  });

  reg.set("show.start", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const state = ctx.state;
      if (state.showState === "RUNNING") throw new CpError("E_FORBIDDEN_STATE", "show already running");
      if (state.showState !== "LOADED") throw new CpError("E_FORBIDDEN_STATE", "no package loaded");
      // SPEC §16.1 warnings policy: exit 0 starts; exit 1 (warnings only)
      // starts only with an explicit allowWarnings; exit 2 never loaded.
      if (!state.preflightPassed) {
        if (!state.preflightWarnings.length) {
          throw new CpError("E_FORBIDDEN_STATE", "preflight has not passed for the loaded package");
        }
        if (payload.allowWarnings !== true) {
          throw new CpError(
            "E_FORBIDDEN_STATE",
            `preflight passed with ${state.preflightWarnings.length} warning(s); pass allowWarnings: true to go to air anyway`,
          );
        }
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
      // SPEC §16.1 step order: stop the outputs first, then the clock. The
      // stop directives are emitted at this command's stateVersion, which is
      // also the version the engine acknowledges (SPEC §5.9.5).
      const stopVersion = ctx.stateVersion;
      const extraDirectives = outputsWereActive
        ? [
            { command: "record.stop", payload: {} },
            { command: "stream.stop", payload: {} },
          ]
        : [];
      if (outputsWereActive) deps.emitDirectivesNow?.(extraDirectives, stopVersion);

      if (outputsWereActive) {
        if (force) {
          warnings.push(SHOW_STOP_FORCE_WARNING);
          deps.warn?.(SHOW_STOP_FORCE_WARNING);
        } else {
          // The graceful window is a wait for the engine's appliedStateVersion
          // acknowledgement (SPEC §5.9.5), not a sleep. Timing out — or having
          // no render node at all — forces the stop and logs.
          const graceMs = deps.showStopGraceMs ?? 2000;
          const acked = deps.waitForGrace
            ? await deps.waitForGrace(graceMs, stopVersion)
            : false;
          if (!acked) {
            warnings.push(SHOW_STOP_FORCE_WARNING);
            deps.warn?.(SHOW_STOP_FORCE_WARNING);
          }
        }
      }

      state.recordState = "idle";
      state.streamState = "idle";
      state.showState = "STOPPED";
      deps.persistence.flushNow();

      // Already emitted above, at the same stateVersion, before the wait.
      return { warnings };
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
