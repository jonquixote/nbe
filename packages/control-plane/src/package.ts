//! Show package handling (Addendum 02a §1.4):
//! - All validation decisions come from the `nbe-preflight` binary (its exit
//!   code + preflight_report.json). The control plane NEVER re-implements
//!   manifest validation.
//! - The manifest's TS view is generated from schemas/manifest.v0.4.json
//!   (`./generated/manifest-schema.js`); here we only index structure needed
//!   for the state machine (items, scenes, elements, assets, …).

import { existsSync, readFileSync } from "node:fs";
import { execFile } from "node:child_process";
import { dirname, join } from "node:path";
import { promisify } from "node:util";

import { CpError } from "./protocol.js";
import type { Manifest, Item, Scene, Asset } from "./generated/manifest-schema.js";
import type { PackageInfo, PackageItem, PackageElement } from "./state.js";

const execFileP = promisify(execFile);

/** Path to the preflight binary: env override, else the workspace debug build
 *  found by walking upward from cwd (tests run from the package dir). */
export function preflightBin(): string {
  const fromEnv = process.env.NBE_PREFLIGHT_BIN;
  if (fromEnv) return fromEnv;
  let dir = process.cwd();
  for (let i = 0; i < 8; i++) {
    const candidate = join(dir, "target", "debug", "nbe-preflight");
    if (existsSync(candidate)) return candidate;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return join(process.cwd(), "target", "debug", "nbe-preflight");
}

/**
 * How long `nbe-preflight` may take before the control plane stops waiting.
 *
 * The bound is for **wedged, not slow**. Preflight legitimately takes 46 s on a
 * five-second 1080p package in a debug build and has been measured past 180 s
 * under CPU contention, so a tight bound would refuse packages that were only
 * working hard. Ten minutes is far outside any measured run and far inside "the
 * operator has been staring at an unanswered command".
 *
 * Override with `NBE_PREFLIGHT_TIMEOUT_MS` — the test suite sets it low so a
 * wedged binary produces a named failure in seconds rather than in minutes.
 * Parsed strictly: anything that is not a positive integer takes the default,
 * because a mistyped bound silently disabling the bound is the failure mode
 * this whole finding is about.
 */
export const DEFAULT_PREFLIGHT_TIMEOUT_MS = 600_000;

export function preflightTimeoutMs(): number {
  const raw = process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  if (raw === undefined || !/^\+?\d+$/.test(raw)) return DEFAULT_PREFLIGHT_TIMEOUT_MS;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n > 0 ? n : DEFAULT_PREFLIGHT_TIMEOUT_MS;
}

export interface PreflightResult {
  exitCode: number;
  report: PreflightReportShape | null;
  stderr: string;
  /** The binary was killed for exceeding `preflightTimeoutMs()`. */
  timedOut: boolean;
}

export interface PreflightReportShape {
  manifestValid: boolean;
  airReady: boolean;
  errors: string[];
  warnings: string[];
}

/**
 * Run nbe-preflight over a package. Never throws; errors arrive via exitCode.
 *
 * **The child is bounded.** It was not: `execFile` was called with no
 * `timeout`, no `killSignal` and no `AbortSignal`, so a preflight binary that
 * never answered left `show.load` waiting forever — the command was accepted
 * and never resolved, and the audit log recorded no terminal state for it. The
 * same unbounded handle also kept Node's event loop open, which is why the
 * render-channel suite reported its failures and then hung instead of exiting.
 * A timeout here fixes both, because both were one missing option.
 */
export async function runPreflight(packagePath: string, opts: { allowWarnings?: boolean } = {}): Promise<PreflightResult> {
  const args = ["--package-path", packagePath];
  if (opts.allowWarnings) args.push("--allow-warnings");
  const timeout = preflightTimeoutMs();
  try {
    const { stdout, stderr } = await execFileP(preflightBin(), args, {
      cwd: process.cwd(),
      timeout,
      // SIGTERM can be ignored; a wedged process must not survive its bound.
      killSignal: "SIGKILL",
    });
    void stdout;
    const report = readReport(packagePath);
    return { exitCode: 0, report, stderr, timedOut: false };
  } catch (e) {
    // execFile rejects on non-zero exit AND on the timeout kill. The two are
    // different outcomes: one is a verdict, the other is the absence of one.
    const err = e as { code?: unknown; stderr?: string; killed?: boolean; signal?: string | null };
    const timedOut = err.killed === true && err.signal === "SIGKILL";
    const code = typeof err.code === "number" ? err.code : 127;
    return {
      exitCode: timedOut ? 124 : code,
      report: timedOut ? null : readReport(packagePath),
      stderr: err.stderr ?? "",
      timedOut,
    };
  }
}

function readReport(packagePath: string): PreflightReportShape | null {
  const p = join(packagePath, "preflight_report.json");
  if (!existsSync(p)) return null;
  try {
    return JSON.parse(readFileSync(p, "utf8")) as PreflightReportShape;
  } catch {
    return null;
  }
}

/**
 * show.load: preflight (exit 2 => E_PREFLIGHT_FAILED), then index structure.
 * Exit 0 or 1 => package loads (warnings are surfaced, not fatal, per SPEC
 * 19.1 — e.g. loudness approaching tolerance must not block a load).
 */
export interface LoadedPackage {
  pkg: PackageInfo;
  /** The preflight exit code: 0 air-ready, 1 warnings only (SPEC 19.1). */
  exitCode: number;
  warnings: string[];
}

export async function loadPackage(packagePath: string, opts: { allowWarnings?: boolean } = {}): Promise<LoadedPackage> {
  const pre = await runPreflight(packagePath, opts);
  // A preflight that never answered is not a verdict, and "accepted, never
  // resolved" is the behaviour being abolished: the load fails by name, the
  // dispatcher's audit record gets its terminal state, and nothing is
  // half-loaded because this throws before any state is touched.
  if (pre.timedOut) {
    throw new CpError(
      "E_PREFLIGHT_FAILED",
      `preflight did not answer within ${preflightTimeoutMs()} ms for ${packagePath}; ` +
        `the binary was killed. It is wedged, not slow — raise ` +
        `NBE_PREFLIGHT_TIMEOUT_MS only if a real run legitimately takes longer`,
    );
  }
  if (pre.exitCode === 2) {
    const why = pre.report?.errors.join("; ") || pre.stderr || "preflight failed";
    throw new CpError("E_PREFLIGHT_FAILED", `preflight failed for ${packagePath}: ${why}`);
  }
  if (pre.exitCode !== 0 && pre.exitCode !== 1) {
    throw new CpError("E_PREFLIGHT_FAILED", `preflight could not run (exit ${pre.exitCode}): ${pre.stderr || "no stderr"}`);
  }

  const manifestPath = join(packagePath, "manifest.json");
  if (!existsSync(manifestPath)) {
    throw new CpError("E_NOT_FOUND", `no manifest.json in package: ${packagePath}`);
  }
  // Structural read only — validity was decided by preflight above.
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as Manifest;

  const items = new Map<string, PackageItem>();
  const sequences = new Set<string>();
  indexSequence(manifest.rundown.id, manifest.rundown.items, items, sequences);

  const scenes = new Map<string, { elements: PackageElement[] }>();
  const elements = new Map<string, PackageElement>();
  for (const s of manifest.scenes as Scene[]) {
    scenes.set(s.id, { elements: s.elements });
    for (const el of s.elements) elements.set(el.id, { id: el.id, kind: el.kind });
  }
  const overlays = new Set<string>();
  for (const o of manifest.overlays ?? []) {
    overlays.add(o.id);
    for (const el of o.elements) elements.set(el.id, { id: el.id, kind: el.kind });
  }

  const allElements = [...elements.values()];
  const templates = (manifest.templates ?? []).map((t) => t.id);
  const breakingTemplates = new Set(
    (manifest.templates ?? []).filter((t) => t.kind === "breakingBanner").map((t) => t.id),
  );
  const tickerExists =
    allElements.some((el) => el.kind === "ticker") || (manifest.templates ?? []).some((t) => t.kind === "ticker");
  const clockElements = new Set(allElements.filter((el) => el.kind === "clock").map((el) => el.id));

  const pkg: PackageInfo = {
    packagePath,
    showId: manifest.show.id,
    // SPEC §7.15: the rate the package was authored at. `show.load` compares
    // it with the engine's, because only the control plane knows both.
    houseRate: manifest.show.video.frameRate,
    manifestVersion: manifest.manifestVersion,
    items,
    sequences,
    scenes,
    elements,
    overlays,
    templates: new Set(templates),
    breakingTemplates,
    tickerExists,
    clockElements,
    plugins: new Set((manifest.plugins ?? []).map((p) => p.id)),
    automationRules: new Set((manifest.automation ?? []).map((r) => r.id)),
    assets: new Map((manifest.assets as Asset[]).map((a) => [a.id, a.source])),
    transitionPresets: new Map(
      (manifest.transitions ?? []).map((t) => [
        t.id,
        Object.fromEntries(
          ([
            ["kind", t.kind],
            ["durationFrames", t.durationFrames],
            ["easing", t.easing],
          ] as const).filter(([, v]) => v !== undefined),
        ) as Record<string, unknown>,
      ]),
    ),
    fallbackAssetId: manifest.show.fallbackAssetId,
    qualityProfile: manifest.qualityProfile,
  };

  return { pkg, exitCode: pre.exitCode, warnings: pre.report?.warnings ?? [] };
}

function indexSequence(
  seqId: string,
  seqItems: Item[],
  items: Map<string, PackageItem>,
  sequences: Set<string>,
): void {
  sequences.add(seqId);
  for (const item of seqItems) {
    const pkgItem: PackageItem = {
      id: item.id,
      kind: item.kind,
      sceneRef: item.sceneRef,
      assetId: item.assetId,
      sourceId: item.sourceId,
      durationFrames: item.durationFrames,
      autoFollow: item.autoFollow,
      audioPolicy: item.audioPolicy,
    };
    items.set(item.id, pkgItem);
    // SPEC v0.4 retired `sequenceRef`. The gap the 02a addendum recorded here
    // — a reserved kind with no registry to resolve against — was closed by
    // deleting the hook rather than by inventing a nesting convention.
  }
}
