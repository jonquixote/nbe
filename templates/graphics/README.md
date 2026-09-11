# Graphics templates

**Packaged fonts and template layouts are package-resident. This directory does
not ship them.**

The founding scaffold created this directory with a note reading "Template JSON
layouts plus packaged font assets", which described an intent the implementation
did not take. Preflight resolves a template by `templateId` against the show
package's own `templates` array, and a template's fonts by `fontAssetIds`
against assets that package declares (`crates/nbe-preflight/src/main.rs`). A
font that is not inside the show package is a font preflight cannot see, and
§0.1 assumption 11 forbids falling back to a host-system face.

That was recorded as an open question in `agents/prompts/07b-delta.md` and is
now **decided: the package model won.** A show carries its own fonts and its own
template layouts, which is what makes a package reproducible on a machine that
has never seen it.

## What this directory is for

Engine-development assets only — sample layouts and scratch faces used while
working on the graphics layer, never loaded by a show at runtime. Nothing here
is normative and nothing here ships. The reference example of a real,
package-resident font lives in `tests/fixtures/overlay_show/media/`
(`Amiri-Regular.ttf`, OFL-1.1, with its licence beside it).

There is no HTML/browser render path in the engine (SPEC §6.5).
