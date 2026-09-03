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
  });

  console.log(`nbe control plane listening on ws://${process.env.NBE_HOST ?? "127.0.0.1"}:${server.port}/nbe/v0.3`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  void main();
}
