//! `nbe run`'s wiring: what the engine is given from the supervisor's
//! environment (review4 step 6; an automated security review's finding).

import { test } from "node:test";
import assert from "node:assert/strict";
import { ENGINE_ENV, engineEnv } from "./run.js";

test("the engine gets the variables it reads, and nothing else from the supervisor's environment", () => {
  const env = {
    NBE_TOKENS: '{"admin-secret":"admin"}',
    NBE_AUDIT_LOG: "/var/log/nbe/audit.jsonl",
    NBE_SOME_FUTURE_SECRET: "not for the engine",
    NBE_AUDIO_SINK: "null",
    NBE_CAMERAS: "main=FaceTime",
    RUST_LOG: "info",
    HOME: "/Users/someone",
  };
  assert.deepEqual(engineEnv(env), { NBE_AUDIO_SINK: "null", NBE_CAMERAS: "main=FaceTime", RUST_LOG: "info" });
  for (const k of ENGINE_ENV) assert.notEqual(k, "NBE_TOKENS");
});
