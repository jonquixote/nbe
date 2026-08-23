# Test fixtures

`valid_show/` is a minimal schema-valid show package. Media files are text placeholders: preflight at this stage checks existence only (SPEC section 5.6 decode checks arrive with later agent prompts). Seeded-failure fixtures (missing asset, VFR clip, bad loudness, etc. per SPEC section 16.3) are added by the preflight implementation prompts.
