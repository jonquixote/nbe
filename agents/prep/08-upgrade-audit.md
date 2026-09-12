# 08 Upgrade Audit — prep document

> Provenance: produced by the 08 planning pass (prepper A) on 2026-09-12; pasted
> by the user and committed verbatim. This is a PRE-decision audit: it records
> gaps and collisions as found. The governing rulings (WS-only transport, origin
> as spec-written identity, AC-12 correction) are recorded in the Prompt 08
> execution order and the upgraded mission document, not here.

**Verdict: GAPs — 08 is not ready as written.**

- **Step 1 — HTTP POST `/nbe/v0.3/command`: GAP.** `server.ts:281-289` serves
  only `GET /status`, else 404. `server.ts:1-3` marks HTTP "future". Spec §16
  (`spec.v0.4.md:2372-2374`) is WS-only.
- **Step 2 — generator: types HELD, code GAP.** Binding types exist at
  `manifest.rs:546-579` and schema `:777-827`; zero consumers; the `companion`
  field is free-form.
- **Step 3 — default deck: GAP.** No file, no test.
- **Step 4 — preflight binding check: GAP.** `main.rs` (790 lines) has zero
  binding references. The spec carries only two one-liners: §6.6 (:20) and
  §19.3 (:9).
- **Step 5 — origin audit: GAP.** §10.7.1 (:1806-1827) has no origin field;
  `audit.ts:13-40` mirrors the spec. No OSC/MIDI modules exist.
- **Step 6 — AC-12 drift:** spec (:3293-3295) says the WS bus; the prompt (:55)
  says an HTTP module. Predates this work.

## Process findings

- The #11 retarget pass did header-only correction (§5.3, §21→§24, §16.0,
  §10.7.1). Bodies were untouched by design.
- New collisions: the original 08 body contradicts `v0.4-outline.md` §6 and
  `prompt-map-07-13.md:500-502` (Input Intent schema; no second command surface;
  keyboard adapter with zero core changes). The old body invents an HTTP second
  surface against its own Assumption 6 (Companion emits WebSocket commands,
  spec assumption :66). The §19 three-part binding check is prompt-authored.
