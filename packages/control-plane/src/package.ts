//! Show package handling (Addendum 02a §1.4):
//! - All validation decisions come from the `nbe-preflight` binary (its exit
//!   code + preflight_report.json). The control plane NEVER re-implements
//!   manifest validation.
//! - The manifest's TS view is generated from schemas/manifest.v0.4.json
//!   (`./generated/manifest-schema.js`); here we only index structure needed
//!   for the state machine (items, scenes, elements, assets, …).

import { existsSync, readFileSync } from "node:fs";
import { execFile } from "node:child_process";
import { dirname, join, sep } from "node:path";
import { promisify } from "node:util";

import { CpError } from "./protocol.js";
import type { Manifest, Item, Scene, Asset } from "./generated/manifest-schema.js";
import type { PackageInfo, PackageItem, PackageElement } from "./state.js";

const execFileP = promisify(execFile);

/**
 * Path to the preflight binary: env override, else the workspace build found by
 * walking upward from cwd (tests run from the package dir).
 *
 * **Release is preferred, and the order is load-bearing — do not "simplify" it
 * back.** Measured on this repo's own 1080p fixture: the debug binary decodes
 * at 130 ms/frame and the release binary at 16.7 ms/frame, an 8x difference.
 * That is the difference between a two-and-a-half-minute rundown taking ten
 * minutes to preflight and taking one, and it is the same cost the rehearsal
 * has carried since the midpoint review as "show.load takes 46 s". Debug stays
 * as the developer fallback, because a contributor who has only ever run
 * `cargo build` should still get a working control plane — just a slower one.
 */
export function preflightBin(): string {
  const fromEnv = process.env.NBE_PREFLIGHT_BIN;
  if (fromEnv) return fromEnv;
  let dir = process.cwd();
  for (let i = 0; i < 8; i++) {
    for (const profile of ["release", "debug"] as const) {
      const candidate = join(dir, "target", profile, "nbe-preflight");
      if (existsSync(candidate)) return candidate;
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return join(process.cwd(), "target", "debug", "nbe-preflight");
}

/**
 * Per-frame decode cost, measured — not guessed.
 *
 * Timed on this repo's `tests/fixtures/dress_show/media/A1.mp4` (150 frames,
 * 1080p H.264) replicated into one package, at 300/600/1200/2400 frames: the
 * cost is linear across an 8x span with no curvature.
 *
 *   debug    130 ms/frame   (39.16 / 77.95 / 156.35 / 311.43 s)
 *   release  16.7 ms/frame  (20.00 s at 1200 frames)
 *
 * Both are rounded up here, because the measurement is one machine and a
 * contended CI runner is slower. The previous constant — a flat 600 s — was
 * crossed by 2 min 34 s of 1080p footage, ten ordinary clips, and told the
 * operator their binary was "wedged, not slow" when it was exactly slow.
 */
const MS_PER_FRAME_RELEASE = 25;
const MS_PER_FRAME_DEBUG = 200;

/**
 * How much longer than the estimate a legitimate run may take.
 *
 * The As-Built Ledger measured preflight at 46 s nominal and past 180 s under
 * CPU contention — roughly 4x. Three is inside that and still leaves the bound
 * meaning "no verdict is coming" rather than "this is taking a while".
 */
const PREFLIGHT_SAFETY_FACTOR = 3;

/**
 * The floor, for packages whose declared work is small or unreadable.
 *
 * A package with no declared durations still gets a minute: process spawn,
 * schema validation and asset hashing cost something the frame count does not
 * describe, and a manifest preflight is about to reject may not parse here at
 * all.
 */
export const PREFLIGHT_FLOOR_MS = 60_000;

/**
 * Frames preflight will decode for this package, from the manifest.
 *
 * Defensive by construction: this runs *before* preflight has judged the
 * manifest, so anything unreadable, missing or malformed contributes zero and
 * the caller falls back to the floor. Deciding validity is preflight's job, and
 * this must not pre-empt it — it only needs a size.
 */
export function expectedDecodeFrames(packagePath: string): number {
  try {
    const manifest = JSON.parse(readFileSync(join(packagePath, "manifest.json"), "utf8")) as {
      assets?: Array<{
        kind?: string;
        expectedDurationFrames?: number;
        loop?: { periodFrames?: number };
      }>;
    };
    let frames = 0;
    for (const a of manifest.assets ?? []) {
      if (a.kind !== "video" && a.kind !== "alphaVideo") continue;
      // Preflight decodes the asset; the manifest's own declaration is the
      // best estimate of how much there is. A loop period is a lower bound on
      // the same footage.
      const declared = Number(a.expectedDurationFrames ?? 0);
      const period = Number(a.loop?.periodFrames ?? 0);
      const n = Math.max(Number.isFinite(declared) ? declared : 0, Number.isFinite(period) ? period : 0);
      if (n > 0) frames += n;
    }
    return frames;
  } catch {
    return 0;
  }
}

export interface PreflightBound {
  ms: number;
  frames: number;
  msPerFrame: number;
  derived: boolean;
}

/**
 * How long `nbe-preflight` may take before the control plane stops waiting.
 *
 * **Derived from the package, not picked.** `frames x msPerFrame x safety`,
 * floored, where `msPerFrame` follows the binary that actually resolved —
 * release and debug are 8x apart and a bound sized for one is wrong for the
 * other. The bound is for **wedged, not slow**: a package that is merely large
 * gets a proportionally larger budget, which a flat constant could not do.
 *
 * `NBE_PREFLIGHT_TIMEOUT_MS` overrides it entirely, parsed strictly — anything
 * that is not a positive integer takes the derived value, because a mistyped
 * bound silently disabling the bound is the failure mode this began as. `"0"`
 * is refused for the same reason: Node reads `timeout: 0` as *no timeout*.
 */
export function preflightBound(packagePath?: string): PreflightBound {
  const bin = preflightBin();
  const msPerFrame = bin.includes(`${sep}release${sep}`) ? MS_PER_FRAME_RELEASE : MS_PER_FRAME_DEBUG;
  const frames = packagePath ? expectedDecodeFrames(packagePath) : 0;
  const derivedMs = frames * msPerFrame * PREFLIGHT_SAFETY_FACTOR;

  const raw = process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  if (raw !== undefined && /^\+?\d+$/.test(raw)) {
    const n = Number(raw);
    if (Number.isSafeInteger(n) && n > 0) {
      return { ms: n, frames, msPerFrame, derived: false };
    }
  }
  return {
    ms: Math.max(PREFLIGHT_FLOOR_MS, derivedMs),
    frames,
    msPerFrame,
    derived: true,
  };
}

export interface PreflightResult {
  exitCode: number;
  report: PreflightReportShape | null;
  stderr: string;
  /** The binary was killed for exceeding `preflightBound()`. */
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
  const bound = preflightBound(packagePath);
  try {
    const { stdout, stderr } = await execFileP(preflightBin(), args, {
      cwd: process.cwd(),
      timeout: bound.ms,
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
    const b = preflightBound(packagePath);
    const how = b.derived
      ? `${b.frames} frames at ${b.msPerFrame} ms/frame x ${PREFLIGHT_SAFETY_FACTOR} safety` +
        (b.ms === PREFLIGHT_FLOOR_MS ? `, floored at ${PREFLIGHT_FLOOR_MS} ms` : "")
      : "NBE_PREFLIGHT_TIMEOUT_MS override";
    throw new CpError(
      "E_PREFLIGHT_FAILED",
      `preflight produced no verdict within ${b.ms} ms for ${packagePath} (${how}); ` +
        `the binary was killed. Raise NBE_PREFLIGHT_TIMEOUT_MS only if a real run ` +
        `legitimately exceeds that`,
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
