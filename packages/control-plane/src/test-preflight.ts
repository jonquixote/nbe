//! The preflight-gated tests' skip (R28).
//!
//! Many control-plane tests load a real package, which shells out to
//! nbe-preflight. Without a built binary they skip by option, not by a runtime
//! `t.skip()`: node:test runs `beforeEach` for a test that skips itself but
//! never its `afterEach`, so a runtime skip leaked whatever `beforeEach` opened
//! (a listening server), and the suite never exited (R28: exit 124 under a
//! bound, `npm test` with it). The reason names the missing binary.

import { existsSync } from "node:fs";

import { preflightBin } from "./package.js";

/** `false` when nbe-preflight is built; otherwise the skip reason, naming it. */
export const NO_PREFLIGHT: string | false = existsSync(preflightBin())
  ? false
  : `nbe-preflight not built: ${preflightBin()} does not exist`;
