//! Show package handling (Addendum 02a §1.4):
//! - All validation decisions come from the `nbe-preflight` binary (its exit
//!   code + preflight_report.json). The control plane NEVER re-implements
//!   manifest validation.
//! - The manifest's TS view is generated from schemas/manifest.v0.3.json
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

export interface PreflightResult {
  exitCode: number;
  report: PreflightReportShape | null;
  stderr: string;
}

export interface PreflightReportShape {
  manifestValid: boolean;
  airReady: boolean;
  errors: string[];
  warnings: string[];
}

/** Run nbe-preflight over a package. Never throws; errors arrive via exitCode. */
export async function runPreflight(packagePath: string, opts: { allowWarnings?: boolean } = {}): Promise<PreflightResult> {
  const args = ["--package-path", packagePath];
  if (opts.allowWarnings) args.push("--allow-warnings");
  try {
    const { stdout, stderr } = await execFileP(preflightBin(), args, { cwd: process.cwd() });
    void stdout;
    const report = readReport(packagePath);
    return { exitCode: 0, report, stderr };
  } catch (e) {
    // execFile rejects on non-zero exit; surface the code.
    const err = e as { code?: unknown; stderr?: string };
    const code = typeof err.code === "number" ? err.code : 127;
    return { exitCode: code, report: readReport(packagePath), stderr: err.stderr ?? "" };
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
export async function loadPackage(packagePath: string, opts: { allowWarnings?: boolean } = {}): Promise<PackageInfo> {
  const pre = await runPreflight(packagePath, opts);
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

  return {
    packagePath,
    showId: manifest.show.id,
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
  };
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
      sequenceRef: item.sequenceRef,
      assetId: item.assetId,
      sourceId: item.sourceId,
      durationFrames: item.durationFrames,
      autoFollow: item.autoFollow,
      audioPolicy: item.audioPolicy,
    };
    items.set(item.id, pkgItem);
    // Note (02a §3 gap): kind "sequenceRef" has no nested registry in the
    // v0.3 manifest (single rundown Sequence) — such items are indexed but
    // their targets cannot be resolved in Prompt 02.
  }
}
