# Agent Prompt 14 — Packaging & Release Pipeline

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — Section 21 (hardware tiers; the named floor device) and, loosely, Section 26. **§26 is a twelve-item implementation-order list whose item 12 names an OBS baseline benchmark harness; it contains no build, release, packaging, notarization, or distribution content whatsoever.** NOTE: this prompt previously cited "Section 22" for build/release requirements; §22 is Acceptance criteria and always was. **v0.4 has no build, release, packaging, notarization, or distribution section at all** — this prompt's contract does not exist in the spec. That gap is a v0.5 candidate; until it is filled, everything this prompt specifies is prompt-authored rather than spec-derived, and must say so. Prerequisites: Agent Prompt 13 merged (there is an app to ship).**

You are a senior macOS release engineer building the `nbe` shipping pipeline: a tagged commit in, a signed, notarized, stapled app out, with checksums, that launches clean under Gatekeeper on a fresh Mac.

Read these first:

- `docs/spec.v0.4.md` — no section is your contract; see the note above. §26 is handoff notes, not requirements, and its twelve items are an implementation order — it says nothing about building, signing, or shipping.
- `agents/prompts/13-operator-shell.md` — the artifact being packaged.

## Step 0: Scope discipline

Allowed now: build, sign, notarize, staple, publish. Forbidden: shipping unsigned nightlies as releases, manual release steps that are not in the pipeline, and any entitlement the app does not justify (camera, microphone, screen capture — declared and explained).

## Step 1: The pipeline

- GitHub Actions, macos-14: tag → build the workspace release binaries and the Swift app → sign with the Developer ID certificate → notarize with Apple → staple the ticket → produce the `.dmg` → publish a GitHub Release with checksums.
- **Intel x86_64 target with discrete AMD graphics** — v0.4 §0.1 item 1 *corrects* the earlier Apple Silicon assumption: "The reference machine is an Intel MacBook Pro with discrete AMD graphics, and that is the mission rather than a fallback." §21 names the 2019 dual-GPU Intel/Radeon MacBook Pro (i7, 16 GB) as the floor device. Apple Silicon is supported but MUST NOT be assumed (§0.1 item 2). Hardened runtime on; entitlements minimal and documented.

## Step 2: Reproducibility

- The release build is reproducible from the tag: pinned toolchain, locked dependencies, and the version string derived from the tag, never from a hand edit.
- The pipeline fails loudly on any signing or notarization error — a half-signed release must be impossible to publish.

## Step 3: Tests

1. **Gatekeeper**: `spctl -a -vv` accepts the stapled app on a clean machine profile.
2. **Notarization**: the ticket validates (`stapler validate`) after stapling.
3. **Launch**: the released app cold-starts to ready on the **§21 floor device** (2019 dual-GPU Intel/Radeon MacBook Pro). **There is no §20.5** — §20 is the MVP scope hard ceiling and has no subsections; the nearest normative bar is **AC-5** (30-minute zero-drop). The cold-start budget itself is this prompt's to set.
4. **Dry run**: the pipeline runs end-to-end in a dry-run mode on every merge to main, publishing nothing.

## Constraints

- One pipeline, no manual steps. A release is a tag, not a ritual.
- Vocabulary discipline: release notes speak `View`, `Element`, `Sequence`, `Item`.
