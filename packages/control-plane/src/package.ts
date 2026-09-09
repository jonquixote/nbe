//! Show package handling (Addendum 02a §1.4):
//! - All validation decisions come from the `nbe-preflight` binary (its exit
//!   code + preflight_report.json). The control plane NEVER re-implements
//!   manifest validation.
//! - The manifest's TS view is generated from schemas/manifest.v0.4.json
//!   (`./generated/manifest-schema.js`); here we only index structure needed
//!   for the state machine (items, scenes, elements, assets, …).

import { existsSync, readFileSync, statSync } from "node:fs";
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
 * Decode cost per MB of referenced media, measured — the term that cannot be
 * absent.
 *
 * Timed on this repo's fixtures, four copies each, both profiles:
 *
 *   `A1.mp4`      1080p, a real encode      23.4 s/MB release   204.7 debug
 *   `cfr_30.mp4`  640x360, synthetic         63.3 s/MB release   204.6 debug
 *   `av_tone.mp4` audio-heavy                 7.7 s/MB release    29.2 debug
 *
 * The **worst** row is the one that matters, not the typical: cost per MB rises
 * as content compresses, because the same bytes carry more frames. These are
 * the worst observed, rounded up. Real broadcast media at 5-50 Mbps is far
 * denser than any of these, so for real packages this term is loose — which is
 * the point. It exists to keep an undeclared package off the floor, not to be
 * precise.
 *
 * **Consequence, stated deliberately:** on a large package this term dominates
 * `max()` and the bound becomes generous. That is the trade taken knowingly. A
 * wedged process never finishes, so a generous bound still catches it; a tight
 * bound that refuses a legitimate load is the defect this whole sequence has
 * been chasing.
 */
const MS_PER_MB_RELEASE = 80_000;
const MS_PER_MB_DEBUG = 250_000;

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
 * The ceiling. A bound with a floor and no ceiling is only half a bound.
 *
 * **This one is operator-facing, and deliberately not a decode estimate.** The
 * control plane executes a connection's commands strictly in arrival order —
 * `server.ts` chains them so `show.load`'s subprocess cannot race the next
 * command — so the bound is also how long one load may leave that channel
 * answering nothing. Without a ceiling, a 100 MB package with no declared
 * durations derived 20.8 hours on debug and 6.7 on release, and a realistic
 * 1.8 GB bulletin put it in days. That is a worse outcome than the flat 600 s
 * this derivation replaced, which at least capped the stall at ten minutes.
 *
 * Derivation, measured on the §0.3 reference target (`hardware-baseline.txt`:
 * 6-core Intel i7 @ 2.6 GHz):
 *
 *   release decode           15.0-15.9 ms/frame
 *   one hour of 1080p/30     108,000 frames -> ~28.6 min of decode
 *   a 30-minute show          54,000 frames -> ~14.3 min
 *
 * One hour is ~2x the decode of a full hour of 1080p footage, and ~4x a
 * half-hour show. §20 caps MVP complexity at three preloaded clips, one
 * background loop and one alpha loop, so a conformant package sits far inside
 * that. Beyond it the operator raises `NBE_PREFLIGHT_TIMEOUT_MS` knowingly —
 * which is the override's purpose now, and what the refusal message says.
 *
 * Note the ceiling does **not** scale with the build profile, on purpose: it
 * bounds an operator's experience, not a decoder's throughput. A debug-built
 * preflight on an hour of footage costs ~3.9 h and will be refused by it. Debug
 * is the developer fallback (`preflightBin`), and being told why in one hour
 * beats a silent four.
 */
export const PREFLIGHT_CEILING_MS = 3_600_000;

/**
 * Frames preflight will decode for this package, from the manifest.
 *
 * Defensive by construction: this runs *before* preflight has judged the
 * manifest, so anything unreadable, missing or malformed contributes zero and
 * the caller falls back to the floor. Deciding validity is preflight's job, and
 * this must not pre-empt it — it only needs a size.
 *
 * **This input is optional and may be absent.** `expectedDurationFrames` and
 * `loop.periodFrames` are both optional — the schema requires only
 * `id`/`kind`/`source`, and §12.10 says "if both present" — so a perfectly
 * valid package can declare neither. See `expectedDecodeBytes` for the term
 * that covers that case.
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

/**
 * Bytes of referenced video media, from the filesystem.
 *
 * The input that **cannot be absent**: a package that references media has
 * bytes on disk whether or not it says anything about them, and `stat` costs
 * nothing before a decode. A missing or unreadable file contributes zero — its
 * absence is preflight's finding to report, not this function's to pre-empt.
 */
export function expectedDecodeBytes(packagePath: string): number {
  try {
    const manifest = JSON.parse(readFileSync(join(packagePath, "manifest.json"), "utf8")) as {
      assets?: Array<{ kind?: string; source?: string }>;
    };
    let bytes = 0;
    for (const a of manifest.assets ?? []) {
      if (a.kind !== "video" && a.kind !== "alphaVideo") continue;
      if (typeof a.source !== "string") continue;
      try {
        bytes += statSync(join(packagePath, a.source)).size;
      } catch {
        // Missing asset: preflight's error to raise, not a reason to guess.
      }
    }
    return bytes;
  } catch {
    return 0;
  }
}

/**
 * A ceiling decision, recorded on every path (SPEC §10.7).
 *
 * The refusal message already says all of this in prose. A string is what an
 * operator reads; this is what a log query answers. Both, because the two
 * audiences are different — and because "how long is too long" has been the
 * longest-running defect class on this branch, so its decisions should be
 * countable rather than greppable.
 */
export interface BoundDecision {
  event: "preflight.bound_decision";
  packagePath: string;
  derivedMs: number;
  ceilingMs: number;
  floorMs: number;
  basis: PreflightBound["basis"];
  /** The override was passed AND the derivation exceeded the ceiling. */
  refusalBypassed: boolean;
  /** The bound actually applied. */
  appliedMs: number;
  outcome: "refused" | "ran";
}

export function boundDecision(
  packagePath: string,
  bound: PreflightBound,
  outcome: BoundDecision["outcome"],
): BoundDecision {
  return {
    event: "preflight.bound_decision",
    packagePath,
    derivedMs: bound.derivedMs,
    ceilingMs: PREFLIGHT_CEILING_MS,
    floorMs: PREFLIGHT_FLOOR_MS,
    basis: bound.basis,
    // Not merely "an override was set": an override below the ceiling changes
    // nothing about the ceiling, and counting it as used would make the field
    // answer a different question than the one it is named for.
    refusalBypassed: bound.basis === "override" && bound.derivedMs > PREFLIGHT_CEILING_MS,
    appliedMs: bound.ms,
    outcome,
  };
}

export interface PreflightBound {
  ms: number;
  frames: number;
  msPerFrame: number;
  bytes: number;
  msPerMb: number;
  derived: boolean;
  /** Which term set the bound. */
  basis: "frames" | "bytes" | "floor" | "ceiling" | "override";
  /** What the terms derived before the ceiling was applied. */
  derivedMs: number;
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
  const release = bin.includes(`${sep}release${sep}`);
  const msPerFrame = release ? MS_PER_FRAME_RELEASE : MS_PER_FRAME_DEBUG;
  const msPerMb = release ? MS_PER_MB_RELEASE : MS_PER_MB_DEBUG;
  const frames = packagePath ? expectedDecodeFrames(packagePath) : 0;
  const bytes = packagePath ? expectedDecodeBytes(packagePath) : 0;

  // Two terms and a floor, and `max()` so neither can undercut the other. The
  // declaration is tighter when it is there; the file sizes are there even when
  // the declaration is not, and they also cover a declaration that under-states
  // the truth — which is not a rejection today, only a warning, so such a
  // package still has to load.
  const framesMs = frames * msPerFrame * PREFLIGHT_SAFETY_FACTOR;
  const bytesMs = (bytes / (1024 * 1024)) * msPerMb * PREFLIGHT_SAFETY_FACTOR;

  const derivedMs = Math.round(Math.max(PREFLIGHT_FLOOR_MS, framesMs, bytesMs));

  // The override may exceed the ceiling — that is its purpose now. An operator
  // with a legitimately enormous package raises it knowingly; the refusal
  // message below names the remedy.
  const raw = process.env.NBE_PREFLIGHT_TIMEOUT_MS;
  if (raw !== undefined && /^\+?\d+$/.test(raw)) {
    const n = Number(raw);
    if (Number.isSafeInteger(n) && n > 0) {
      return {
        ms: n, frames, msPerFrame, bytes, msPerMb,
        derived: false, basis: "override", derivedMs,
      };
    }
  }

  const ms = Math.min(PREFLIGHT_CEILING_MS, derivedMs);
  const basis =
    ms < derivedMs
      ? "ceiling"
      : derivedMs === Math.round(framesMs)
        ? "frames"
        : derivedMs === Math.round(bytesMs)
          ? "bytes"
          : "floor";
  return { ms, frames, msPerFrame, bytes, msPerMb, derived: true, basis, derivedMs };
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

export async function loadPackage(
  packagePath: string,
  opts: {
    allowWarnings?: boolean;
    /**
     * Sink for the ceiling decision, called on EVERY path — refused, and ran.
     * Optional so the many tests that call `loadPackage` directly need no
     * wiring; the command handler supplies the audit log.
     */
    onBoundDecision?: (d: BoundDecision) => void;
  } = {},
): Promise<LoadedPackage> {
  const bound = preflightBound(packagePath);
  const pre = await runPreflight(packagePath, opts);
  // A preflight that never answered is not a verdict, and "accepted, never
  // resolved" is the behaviour being abolished: the load fails by name, the
  // dispatcher's audit record gets its terminal state, and nothing is
  // half-loaded because this throws before any state is touched.
  if (pre.timedOut) {
    const b = bound;
    const how =
      b.basis === "override"
        ? "NBE_PREFLIGHT_TIMEOUT_MS override"
        : b.basis === "ceiling"
          ? `capped at the ${PREFLIGHT_CEILING_MS} ms ceiling; this package derives ` +
            `${b.derivedMs} ms from ${(b.bytes / (1024 * 1024)).toFixed(1)} MB of media and ` +
            `${b.frames} declared frames. One load may not leave this connection ` +
            `unanswered for longer — commands run in arrival order. If the package ` +
            `genuinely needs it, raise NBE_PREFLIGHT_TIMEOUT_MS past the ceiling`
          : b.basis === "frames"
            ? `${b.frames} declared frames at ${b.msPerFrame} ms/frame x ${PREFLIGHT_SAFETY_FACTOR} safety`
            : b.basis === "bytes"
              ? `${(b.bytes / (1024 * 1024)).toFixed(1)} MB of media at ${b.msPerMb} ms/MB x ${PREFLIGHT_SAFETY_FACTOR} safety`
              : `floor, from ${b.frames} declared frames and ${b.bytes} bytes of media`;
    const decision = boundDecision(packagePath, b, "refused");
    opts.onBoundDecision?.(decision);
    const remedy =
      "Raise NBE_PREFLIGHT_TIMEOUT_MS only if a real run legitimately exceeds that" +
      (b.basis === "ceiling" ? "; past the ceiling if the package genuinely needs it" : "");
    throw new CpError(
      "E_PREFLIGHT_FAILED",
      `preflight produced no verdict within ${b.ms} ms for ${packagePath} (${how}); ` +
        `the binary was killed. Raise NBE_PREFLIGHT_TIMEOUT_MS only if a real run ` +
        `legitimately exceeds that`,
      // Additive: the §5.4 envelope defines `error` as {code, message}. A
      // caller that must decide what to do — raise the override, split the
      // package — needs the numbers, not the sentence.
      {
        derivedMs: decision.derivedMs,
        ceilingMs: decision.ceilingMs,
        basis: decision.basis,
        refusalBypassed: decision.refusalBypassed,
        remedy,
      },
    );
  }
  // Preflight answered. Whatever it decided, the bound did not refuse it.
  opts.onBoundDecision?.(boundDecision(packagePath, bound, "ran"));

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
