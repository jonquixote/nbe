//! Determinism list item 5: the show.stop harness counts a take's bytes by
//! listing the record directory and stating each entry. The engine's
//! telemetry tick creates and removes a write probe there every second, so a
//! listing can name a file that is gone by its stat: ENOENT on #119's CI.
//! The count skips the probe and treats a vanished entry as 0 bytes.

import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { recordedBytes, sizeOrZero, WRITE_PROBE_PREFIX } from "./show-stop.harness.js";

test("item 5: the take's bytes count the media and not the engine's write probe", () => {
  const dir = mkdtempSync(join(tmpdir(), "nbe-record-dir-"));
  try {
    writeFileSync(join(dir, "take.mp4"), Buffer.alloc(1000));
    writeFileSync(join(dir, `${WRITE_PROBE_PREFIX}4242`), Buffer.alloc(7));
    assert.equal(recordedBytes(dir), 1000);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("item 5: an entry that vanished after the listing counts 0, and other errors still throw", () => {
  const dir = mkdtempSync(join(tmpdir(), "nbe-record-dir-"));
  try {
    assert.equal(sizeOrZero(join(dir, "gone.mp4")), 0, "a vanished entry is 0 bytes, not ENOENT");
    // Not a vanished file: a path through a regular file is ENOTDIR.
    writeFileSync(join(dir, "file"), "x");
    assert.throws(() => sizeOrZero(join(dir, "file", "below")), /ENOTDIR/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
