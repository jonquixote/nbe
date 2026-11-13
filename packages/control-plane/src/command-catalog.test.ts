//! The operator shell's copy of the §16 catalog (Prompt 13 step 3: bindings
//! tested against the command schema) must match a fresh export of
//! `CommandPayloadSchemas` and the §16.0 matrix, so a changed command breaks
//! here, in `npm test`, and the shell's parity tests then break against it.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { exportCommands } from "./command-catalog.js";

const SHELL_COPY = fileURLToPath(
  new URL("../../../apps/nbe-macos/Sources/NBEProtocol/Resources/commands.v0.4.json", import.meta.url),
);

test("the operator shell's command catalog matches a fresh export (run src/command-catalog.ts --out to refresh)", () => {
  const committed = JSON.parse(readFileSync(SHELL_COPY, "utf8")) as unknown;
  assert.deepEqual(committed, JSON.parse(JSON.stringify(exportCommands())));
});

test("the catalog carries §16.0's deliberate three", () => {
  const byName = new Map(exportCommands().commands.map((c) => [c.name, c]));
  assert.deepEqual(byName.get("show.start")?.roles, ["admin"]);
  assert.ok(byName.get("show.stop")?.roles.includes("operator"));
  assert.deepEqual(byName.get("plugin.reload")?.roles, ["admin"]);
});
