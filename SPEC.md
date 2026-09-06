# NBE SPEC — News Broadcasting Engine

**Current normative version: v0.4** → [`docs/spec.v0.4.md`](docs/spec.v0.4.md)

SPEC.md is the signpost; the versioned documents in `docs/` are the artifacts.

| Version | File | Status |
|---|---|---|
| v0.4 | `docs/spec.v0.4.md` | **current** — the midpoint review's revision: target-hardware correction, package resource model, `sequenceRef` retired |
| v0.3.3 | `docs/spec.v0.3.md` | superseded — self-contained composable broadcast language |
| v0.2.5 | git history (`SPEC.md` @ `f0071faf`) | superseded — consolidated v0.1+v0.2 |
| v0.2 | `docs/spec.v0.2.md` | historical amendments |
| v0.1 | `docs/spec.v0.1.md` | historical base |

Normative schema: `schemas/manifest.v0.4.json` (v0.4 removed the `sequenceRef` hook; `manifestVersion` accepts `"0.3"` and `"0.4"`). Vocabulary: `VOCABULARY.md`.

Each version's own header table lists what it changed. v0.4 corrected §0.1's target hardware to the Intel Mac with discrete graphics, added the package resource model (§12.11), `viewItemStartFrame` (§5.9.4) and `showState` (§10.1), made contradictory items a preflight failure (§17.5) and house-rate mismatch a `show.load` rejection (§7.15), and retired `sequenceRef`.
