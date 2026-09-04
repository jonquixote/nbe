# Platform boundaries — `[RI-7]`

**Status: enumeration, not a port plan.** macOS-first is deliberate (VideoToolbox). No port work is v1. This file records every hard-coded platform assumption and the boundary that would absorb a future port, so the open-source future is served by a map rather than by rewriting from memory.

Produced by the midpoint integration review. See `docs/review-midpoint-report.md` §3A for the hardware measurement that reframed this document.

---

## The assumption that reframes everything else

`[RI-2]`'s probe found the reference machine is **Intel with discrete AMD graphics**, while SPEC §0.1 assumption 2 declares **Apple Silicon** the primary target. So the sharpest portability boundary in this project is not Linux-vs-macOS — it is **unified vs discrete memory**, and it is already live *inside* the declared target platform. Anything written here about "porting" applies equally to moving between the machine in the room and the machine in the spec.

---

## Hard-coded platform assumptions

| # | Assumption | Where | Boundary that absorbs a port |
|---|---|---|---|
| 1 | **VideoToolbox / AVFoundation** hardware decode | `crates/nbe-decode` (the only `unsafe` crate) | **`nbe-decode` is the platform isolate.** Its public surface — `decode_video`, `decode_audio`, `VideoAsset`, `AudioTrack` — is platform-neutral; a Linux build swaps the crate body (VA-API/NVDEC) and nothing above it changes. CI enforces the isolation: the unsafe-exception grep is scoped to `crates/nbe-decode/src`. |
| 2 | **Metal** as the wgpu backend | implicit via `wgpu` 30 | `wgpu` already abstracts Vulkan/DX12/Metal. The real coupling is not the API but the **memory model** (row 3). |
| 3 | **Unified memory** assumed by §12.6's clamp | `loop_cache.rs::effective_mib`, `recommended_working_set_mib` | The clamp is the boundary and it is **currently unwired** (`None` at every production site). Wiring it must query the adapter rather than assume the platform: discrete adapters report a real VRAM ceiling, unified ones report `recommendedMaxWorkingSetSize`. One code path, two answers. |
| 4 | **Adapter selection is implicit** | `gpu.rs` | On this machine `wgpu` picks between an Intel iGPU (1536 MB dynamic) and a Radeon Pro 555X (4 GB dedicated) and the engine neither chooses nor records which. **A port boundary and a bug in one:** the quality profile is probed from an adapter nobody named. Recording the chosen adapter in telemetry is the minimum. |
| 5 | **`macos-14` CI runner** | `.github/workflows/ci.yml` (both jobs) | Both jobs run on macOS because `nbe-preflight` depends on VideoToolbox. A Linux CI lane becomes possible only once row 1's crate has a second body; until then this is a hard dependency, not a preference. |
| 6 | **`cpal`'s future device sink** | recorded deferral, `06-audio-graph.md` | The `AudioSink` trait is already the boundary: `NullSink` in CI, a device sink later. Nothing above the sink — graph, drain, counters, drift — is platform-specific. |
| 7 | **Ticker font handling (Prompt 07)** | not yet built | §0.1 assumption 11 forbids host-system fonts; fonts are packaged. **This is a portability asset, not a risk** — the constraint that makes shows reproducible also makes text rendering platform-independent. 07 must not weaken it. |
| 8 | **Apple-specific pixel formats** (NV12, IOSurface) | `nbe-decode`, `loop_cache` texture formats | Behind row 1. The manifest's `textureFormat` enum (`nv12`, `rgba8`, `nv12Alpha`, `bc7`) is already a portable vocabulary; the mapping to platform formats is the isolate's job. |

---

## Known-good boundaries — record them so they are not accidentally broken

1. **The §16 command surface is device-independent.** Any input device — Companion, MIDI, keyboard, a phone — speaks the same WebSocket JSON. Prompt 08's Input Intent schema (v0.4 outline §6) makes that explicit rather than adding a second surface. Nothing about the command bus assumes macOS.
2. **`nbe-decode` is the platform isolate**, and CI enforces it: `unsafe` is denied workspace-wide with the exception scoped to that crate's `src/`.
3. **`nbe-preflight` is a binary any pipeline can shell.** Exit codes 0/1/2 and `preflight_report.json` are the contract; no caller needs to link Rust. This is what lets a future non-macOS CI validate packages even while decode stays macOS-only.
4. **The master clock is arithmetic, not a platform service.** `(F − t0)` and `sampleForMasterFrame` derive from integers, not from any OS timer, so determinism survives a port unchanged.

---

## What is explicitly not decided here

No port is scheduled. No second decode backend is designed. This document does not authorise work; it records where the work would go. Promotion follows the roadmap's §5 rules like anything else.
