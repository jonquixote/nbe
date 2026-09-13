# 08 Execution Plan — prep document

> Provenance: produced by the 08 planning pass (prepper B) on 2026-09-12; pasted
> by the user and committed verbatim. This is a PRE-decision plan. Its Phase 1
> (the HTTP door) was REJECTED by user ruling — see the Prompt 08 execution
> order: the transport is WebSocket-only per Assumption 6 and portability
> boundary 1. The parity-test content of P1 survives translated to WS. All other
> phases stand as the skeleton the upgraded mission document elaborates.

Six phases, gates exact:

- **P0 scope lock:** verify prompts 01–07 merged.
- **P1 HTTP door** *(rejected — see header)*: `server.ts`, same
  `authenticate()` + `dispatch()`, `connectionId = http:uuid`. Test
  `command-http.test.ts`: take ok+bump; E_UNSUPPORTED/E_BAD_PAYLOAD no bump;
  E_AUTH 401 pre-state; render role deny; WS/HTTP parity. Gate: tsc + npm ≥30.
- **P2 generator:** `companion.ts` new — deterministic, sorted-by-id, stable
  JSON. Tests: round-trip; enum audit vs schema; byte-identical ×2; coverage of
  all ids. Gate: `gen:manifest-types` diff green.
- **P3 default deck always:** TAKE / CUT / arm-next / next / breaking /
  soundboard / record / stream / fallback. Empty bindings still drivable. No key
  collision.
- **P4 preflight:** `main.rs`, after existing checks — unknown action → exit 2,
  airReady false, names the binding id (E_PREFLIGHT_FAILED); bad payload →
  exit 2; incomplete trigger → exit 2; valid → exit 0. Gate: cargo
  check/clippy/test + exit-2 grep gate.
- **P5 audit origin:** additive — companion | ui | ws | http | automation |
  engine; no token logging. Same state change via UI vs Companion differs only
  in origin.
- **P6 AC-12:** HTTP view.take *(now WS — see header)*, no plugin; all gates;
  falsification table per §2a (delete the door, break determinism, drop TAKE,
  accept unknown, strip origin — each must go red). Forbids: plugin ever; a
  second protocol; schema edits.

**Decision needed (resolved):** HTTP second surface vs WS-only Assumption 6, and
the origin spec write (patch v0.4 or v0.5 before code). Ruled WS-only by the
user; origin is drafted as Input Intent spec text, unratified pending PR review.
