//! Index entry (Prompt 02): boots the server from env config.

import { createControlPlaneServer } from "./server.js";
import { AuditLog } from "./audit.js";
import { ControlPlaneState } from "./state.js";
import { StatePersistence } from "./persistence.js";
import { DEFAULT_PORT } from "./protocol.js";
import { join } from "node:path";
import { tmpdir } from "node:os";

async function main(): Promise<void> {
  const tokensRaw = process.env.NBE_TOKENS;
  if (!tokensRaw) {
    console.error("NBE_TOKENS env required: JSON map of token -> role");
    process.exit(1);
  }
  const tokens = JSON.parse(tokensRaw) as Record<string, string>;

  const state = new ControlPlaneState();
  const stateFile = join(process.env.NBE_STATE_DIR ?? join(tmpdir(), "nbe"), "control-plane-state.json");
  const persistence = new StatePersistence(state, stateFile);
  if (persistence.restore()) {
    console.log("recovered prior control-plane state version", state.stateVersion);
  }

  const server = await createControlPlaneServer({
    port: Number(process.env.NBE_PORT ?? DEFAULT_PORT),
    host: process.env.NBE_HOST ?? "127.0.0.1",
    auth: { tokens: tokens as never },
    audit: new AuditLog(process.env.NBE_AUDIT_LOG),
    state,
    persistence,
  });

  console.log(`nbe control plane listening on ws://${process.env.NBE_HOST ?? "127.0.0.1"}:${server.port}/nbe/v0.3`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  void main();
}
