# Agent Prompt 14 — Packaging & Release Pipeline

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — Section 26 (implementation handoff notes) and Section 21 (hardware tiers, for what must be supported). NOTE: this prompt previously cited "Section 22" for build/release requirements; §22 is Acceptance criteria and always was. **v0.4 has no build, release, packaging, notarization, or distribution section at all** — this prompt's contract does not exist in the spec. That gap is a v0.5 candidate; until it is filled, everything this prompt specifies is prompt-authored rather than spec-derived, and must say so. Prerequisites: Agent Prompt 13 merged (there is an app to ship).**

You are a senior macOS release engineer building the `nbe` shipping pipeline: a tagged commit in, a signed, notarized, stapled app out, with checksums, that launches clean under Gatekeeper on a fresh Mac.

Read these first:

- `docs/spec.v0.4.md` — no section is your contract; see the note above. Section 26 is the closest thing and it is handoff notes, not requirements.
- `agents/prompts/13-operator-shell.md` — the artifact being packaged.

## Step 0: Scope discipline

Allowed now: build, sign, notarize, staple, publish. Forbidden: shipping unsigned nightlies as releases, manual release steps that are not in the pipeline, and any entitlement the app does not justify (camera, microphone, screen capture — declared and explained).

## Step 1: The pipeline

- GitHub Actions, macos-14: tag → build the workspace release binaries and the Swift app → sign with the Developer ID certificate → notarize with Apple → staple the ticket → produce the `.dmg` → publish a GitHub Release with checksums.
- Apple Silicon target per the spec's reference hardware; hardened runtime on; entitlements minimal and documented.

## Step 2: Reproducibility

- The release build is reproducible from the tag: pinned toolchain, locked dependencies, and the version string derived from the tag, never from a hand edit.
- The pipeline fails loudly on any signing or notarization error — a half-signed release must be impossible to publish.

## Step 3: Tests

1. **Gatekeeper**: `spctl -a -vv` accepts the stapled app on a clean machine profile.
2. **Notarization**: the ticket validates (`stapler validate`) after stapling.
3. **Launch**: the released app cold-starts to ready within the Section 20.5 bar on the reference Mac mini.
4. **Dry run**: the pipeline runs end-to-end in a dry-run mode on every merge to main, publishing nothing.

## Constraints

- One pipeline, no manual steps. A release is a tag, not a ritual.
- Vocabulary discipline: release notes speak `View`, `Element`, `Sequence`, `Item`.
