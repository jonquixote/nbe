# Vendored: wgpu-core 30.0.1 plus gfx-rs/wgpu#9958

**Drop this at wgpu 31.** Remove `vendor/`, the `[patch.crates-io]` table and the `exclude` line in the root `Cargo.toml`, and loosen the `=30.0.1` pins in `crates/nbe-engine/Cargo.toml` and `crates/nbe-decode/Cargo.toml`. First confirm the new release carries the fix: `crates/nbe-engine/tests/r42_two_pollers.rs` must stay green without the patch.

**Why.** R42 in `docs/soak-protocol.md`. `Device::maintain` reads the fence, then with no lock calls `queue.maintain`. A sibling poll can empty the queue between the two, and the stale read meets the assert at `src/device/resource.rs:948`. The panic kills the encode thread that polled.

**What this is.** The crates.io source of wgpu-core 30.0.1, checksum `14c018fce9b6270aa203c2fdd56f3cce996713534bd757e4ea58c8560b121f14`, with one change: the gate from upstream's commit `385520f72f5bbb614fec957ed31f3f6d19076a3f`, applied to `Device::maintain` in `src/device/resource.rs`. Upstream #9958 adds no test (it changed only CHANGELOG.md and wgpu-core/src/device/resource.rs); nothing is omitted from this copy. Left out of the copy: `Cargo.lock`, `Cargo.toml.orig`, `.cargo_vcs_info.json`.

**Status upstream.** On trunk since 2026-08-07, under Unreleased. Not in 30.0.1, not on the `v30` branch.
