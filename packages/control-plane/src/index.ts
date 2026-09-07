//! Index entry (Prompt 02): boots the server from env config.

import { createControlPlaneServer } from "./server.js";
import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { StatePersistence } from "./persistence.js";
import { DEFAULT_PORT, RoleSchema, type Role } from "./protocol.js";
import { join } from "node:path";
import { tmpdir } from "node:os";

async function main(): Promise<void> {
  const tokensRaw = process.env.NBE_TOKENS;
  if (!tokensRaw) {
    console.error("NBE_TOKENS env required: JSON map of token -> role");
    process.exit(1);
  }
  const rawTokens = JSON.parse(tokensRaw) as Record<string, unknown>;
  const tokens: Record<string, Role> = {};
  for (const [token, role] of Object.entries(rawTokens)) {
    const parsed = RoleSchema.safeParse(role);
    if (!parsed.success) {
      console.error(`NBE_TOKENS: "${String(role)}" is not a valid role`);
      process.exit(1);
    }
    tokens[token] = parsed.data;
  }

  // SPEC §10.7.1: refuse to start unaudited.
  const audit = new AuditLog(process.env.NBE_AUDIT_LOG);
  try {
    audit.assertConfigured();
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }

  const state = new ControlPlaneState();
  const stateFile = join(process.env.NBE_STATE_DIR ?? join(tmpdir(), "nbe"), "control-plane-state.json");
  const persistence = new StatePersistence(state, stateFile);
  if (persistence.restore()) {
    console.log("recovered prior control-plane state version", state.stateVersion);
  }

  const server = await createControlPlaneServer({
    port: Number(process.env.NBE_PORT ?? DEFAULT_PORT),
    host: process.env.NBE_HOST ?? "127.0.0.1",
    auth: { tokens },
    audit,
    state,
    persistence,
    // SPEC §7.15: the control plane is the side that knows BOTH the package
    // and the running engine, so it is the side that refuses. It reads the
    // same variable the engine does, because a control plane that guessed the
    // engine's rate would be guessing at exactly the thing this rule exists to
    // stop being guessed.
    houseRate: parseHouseRate(process.env.NBE_HOUSE_RATE),
  });

  console.log(`nbe control plane listening on ws://${process.env.NBE_HOST ?? "127.0.0.1"}:${server.port}/nbe/v0.3`);
}

/**
 * SPEC §7.15. Mirrors the engine's `.parse().ok().unwrap_or(30)` exactly.
 *
 * `Number(env ?? 30)` produced `NaN` for any non-numeric value, and every
 * comparison against NaN is false — so `NBE_HOUSE_RATE="abc"` did not fall
 * back to 30, it made `declared !== engineRate` true for EVERY package and
 * rejected all of them, including a matching 30 fps one, with "runs at NaN
 * fps". A one-character misconfiguration was a total outage.
 *
 * `Number.parseInt` is NOT that mirror either: it stops at the first
 * non-digit, so `"60abc"` yields 60 here and 30 in the engine — the two sides
 * would disagree about the rate they exist to reconcile. Rust's
 * `u32::from_str` accepts an optional `+` and digits, nothing else, so that is
 * the grammar checked here.
 *
 * The mirror is exact, including where it is unhelpful: `NBE_HOUSE_RATE="0"`
 * parses, so both sides run at 0 and reject every package. That is a shared
 * hazard, not a divergence — narrowing it here would put the two mirrors out
 * of step, which is the failure this function exists to prevent.
 */
export function parseHouseRate(raw: string | undefined): number {
  if (raw === undefined || !/^\+?\d+$/.test(raw)) return 30;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n <= 0xffffffff ? n : 30;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  void main();
}
