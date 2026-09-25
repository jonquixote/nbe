//! Temporary directories for tests that remove themselves.
//!
//! Every test that needed scratch space called `mkdtempSync(join(tmpdir(), …))`
//! and never removed it; by 2026-09-25 the system temp directory held 8,597
//! `nbe-*` directories (168 MB) from control-plane, rehearsal and preflight
//! test runs since 2026-09-17. `tempDir` makes the same directory and removes
//! it when the test process exits. `node --test` runs each file in its own
//! process, so each file cleans up after itself; a crashed run still leaks,
//! which is the price of never deleting a directory a test is still using.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const made: string[] = [];
let hooked = false;

/** `mkdtempSync(join(tmpdir(), prefix))`, removed when the process exits. */
export function tempDir(prefix: string): string {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  made.push(dir);
  if (!hooked) {
    hooked = true;
    process.on("exit", () => {
      for (const d of made) {
        try {
          rmSync(d, { recursive: true, force: true });
        } catch {
          // Best effort: a directory already gone, or held open, is not a
          // test failure.
        }
      }
    });
  }
  return dir;
}
