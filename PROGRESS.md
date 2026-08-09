# Implementation progress

Checklist tracked against `PLAN.md` §16 milestones. `PLAN.md` remains the source of truth;
this file only records what is built. Tick items as they land with tests green.

## M0 — Walking skeleton ✅

Goal (PLAN §16): workspace + wire + codegen green · axum serving embedded UI · WS hub ·
`device-virtual` siggen → spectrum frames → WebGL waterfall in the browser · Tauri shell boots
the same UI via embedded server · state snapshot + `StateChanged` invalidation.

**Status: complete.** `cargo xtask check` + `cargo xtask test` green; live smoke test confirms
siggen → ring → DSP FFT → WS binary spectrum → decoded client-side with visible tones, plus
REST CRUD, retune, and embedded-UI serving.

### Foundation
- [x] Pinned nightly toolchain (`rust-toolchain.toml`, `nightly-2026-08-01`) + `-Zpolonius=next`
- [x] Cargo workspace (`Cargo.toml`) with M0 crate members + `default-members` (desktop excluded from the fast gate)
- [x] `rustfmt.toml`, `.cargo/config.toml`, `clippy.toml` (no-unwrap discipline, tests exempt)
- [x] `Cargo.lock` committed; `.gitignore` corrected

### Crates
- [x] `crates/wire` — DTOs, WS `ServerEvent`/`ClientCommand` enums, binary frame codec, settings, OpenAPI schemas (contract tests lock the JSON shape)
- [x] `crates/dsp` — spectrum analyzer (Hann + complex FFT → dBFS → max-decimate → u8), NCO, windows + golden tests
- [x] `crates/device` — `SdrDevice`/`DeviceDriver` traits, `RxSink`, capability model, driver registry (serial-merge)
- [x] `crates/device-virtual` — siggen (tones + drifting sweep + noise) implementing `SdrDevice`, real-time paced
- [x] `crates/channels` — `ChannelRx` trait + output/registry scaffold (no demods at M0)
- [x] `crates/engine` — device thread → rtrb ring → DSP thread → spectrum tap → broadcast; authoritative state + events (e2e tested)
- [x] `crates/server` — axum REST + WS hub + binary spectrum frames + `rust-embed` static + OpenAPI + Swagger (handler + snapshot tests)
- [x] `apps/sdrmm` — headless server binary
- [x] `apps/desktop` — Tauri v2 shell embedding `crates/server` on an ephemeral loopback port
- [x] `xtask` — `codegen` · `dev` · `check` · `test`

### Web
- [x] Vite 8 + React 19 + TS7 (`tsgo`) + Tailwind v4 + Biome + Oxlint (type-aware) scaffold
- [x] Generated OpenAPI types + `openapi-fetch` client + TanStack Query adapter (keyed by path)
- [x] WS client (JSON events + binary spectrum decode mirroring `wire/frame.rs`)
- [x] WebGL2 waterfall (magma colormap, scrolling R8 texture ring) + spectrum line panel
- [x] `StateChanged` → query invalidation wired (no polling)

### Gates
- [x] `cargo fmt --check` + `cargo clippy -D warnings` clean
- [x] `cargo test` (26 tests: dsp golden, engine e2e via `device-virtual`, server handlers, wire contract) green
- [x] `biome ci` + `oxlint` (type-aware) + `tsgo` typecheck + web build clean
- [x] OpenAPI codegen regenerated, no drift
- [x] CI workflow (GitHub Actions) mirroring `cargo xtask check` + `cargo xtask test`

### Post-review hardening (adversarial multi-agent review, all verified)
- [x] **Waterfall renderer never created** (critical): canvases now always mount so the renderer attaches — the WebGL2 waterfall renders (verified in-browser)
- [x] **Stream frozen after WS reconnect** (high): subscription gated on/re-run by connection state
- [x] Unknown `/api/*` returns JSON 404 instead of the SPA shell
- [x] Tune buttons accumulate rapid clicks (synchronous optimistic cache update)
- [x] WS `Hello` subscribes before snapshotting revision (no lost-event gap)
- [x] `StreamStopped` emitted when a streamed device set is removed (verified in-browser)
- [x] `stream_id` u16 range guarded (no silent aliasing)
- [x] NCO phase step normalized (exact for any frequency, incl. aliased)
- [x] Waterfall shader bottom-edge seam fixed; GL VAO/buffer freed in `dispose`
- [x] `decimate_max` upsample path hardened (nearest-neighbor)

## M1 — Real hardware ✅

Goal (PLAN §16): Soapy backend · probe/open/capability UI · RTL-SDR + HackRF fully
controllable (gains, rate, PPM, bias-T…) · hotplug robustness · seify-vs-own-FFI decision.

**Status: complete.** `cargo xtask check` + `cargo xtask test` green; Soapy-free
`--no-default-features` build verified; no hardware in CI (device-soapy logic is tested
against fabricated capability data).

### Device layer
- [x] `crates/device-soapy` — `SoapyDriver` probe/open (stable serial-or-args keys, probe-merge ready) + `SoapyDevice` on the `soapysdr` 0.5 binding
- [x] Capabilities mapped from hardware queries: freq ranges, discrete/continuous sample rates, per-stage gains, antennas, discrete bandwidths
- [x] RTL-SDR/HackRF extra settings via per-driver tables (`biastee`, `direct_samp`, `offset_tune`, `digital_agc`; `bias_tx`) — the binding lacks `getSettingInfo`
- [x] PPM via the `"CORR"` frequency component (binding lacks `setFrequencyCorrection`); `Unsupported` when the tuner has no CORR
- [x] Settings validated against capabilities before any hardware setter runs (freq, rate, bandwidth, gain stages *and* values, antenna, extras)
- [x] Extra-setting writes read back and verified — a driver silently ignoring a key (e.g. bias-tee-less librtlsdr build) is an error, not a lie
- [x] Feature-gated default-on: `--no-default-features` builds a Soapy-free binary (checked in `xtask check` + CI)
- [x] `DeviceRegistry::open` returns the probed `DeviceInfo` (real label/serial in state)
- [x] `DeviceSettings.bandwidth` + `DeviceSettings::merge_from` (per-stage gain / per-name extra merge) in `wire`, shared by engine, virtual and Soapy backends

### Hotplug robustness
- [x] `RxSink::fail` fatal path: capture threads report unrecoverable stream errors; engine fault drainer marks the set `error` + `StateChanged` (no deadlock against removal; create-race faults stashed and applied on insert)
- [x] Capture-thread unplug detection by filtered re-enumeration (SoapyRTLSDR/HackRF `hardware_key` does no live I/O — endless-timeout + enumerate-absence is the reliable signal)
- [x] Engine hotplug prober: periodic probe diff → `StateChanged{Devices}`; running sets whose device vanishes from probe on two consecutive ticks are faulted
- [x] Device I/O moved off the engine-wide lock (per-set runtime mutex) and off tokio workers (`spawn_blocking` in handlers)

### Web
- [x] Capability-driven device settings panel: per-stage gain sliders, antenna, PPM, bandwidth, generic extra-setting rendering (bool/enum/range) — zero per-device frontend code
- [x] Continuous-rate devices: MS/s input clamped to `sample_rate_range` (discrete list still a select)
- [x] Device-fault banner (status `error`) + rejected-PATCH banner; optimistic cache merge mirrors `merge_from` (vitest-tested), in-flight refetches cancelled before optimistic writes
- [x] Header badge: `real hardware · M1`

### Gates
- [x] `cargo fmt` + `clippy -D warnings` + full test suite green (engine fault/hotplug races, device-soapy mapping/validation, wire merge, TS merge mirror)
- [x] `biome ci` + `oxlint --type-aware` + `tsgo` + web build + vitest green; `xtask test` runs web tests too
- [x] OpenAPI codegen regenerated (bandwidth), no drift
- [x] CI: libsoapysdr installed (apt/brew), `--no-default-features` check, macOS job runs web tests

### Post-review hardening (adversarial multi-agent review, all verified)
- [x] **Unplug detection was a no-op** (high): `hardware_key()` liveness probe returned cached strings with zero USB I/O on both flagship drivers — replaced with filtered re-enumeration + engine probe cross-check
- [x] Fault racing device-set creation no longer dropped (pending-fault stash; instant-fail devices surface as `error`)
- [x] Blocking USB I/O no longer stalls the whole engine (per-set lock) or tokio workers (`spawn_blocking`)
- [x] Partial hardware applies resync settings from the device on mid-batch setter failure
- [x] Silently-ignored driver settings detected via read-back verification
- [x] Optimistic-cache race with in-flight refetches fixed (`cancelQueries` before write)
- [x] Rejected PATCHes surfaced in the UI; off-list bandwidth rendered honestly; pending gain commit flushed on unmount
- [x] Seify decision recorded in `PLAN.md` §18 (soapysdr 0.5 adopted; gaps documented)

## M2 — Listen (not started)
DDC channels · NFM/AM/SSB/WFM · squelch/AGC · Opus audio · presets · bookmarks · phone-usable UI.
See `PLAN.md` §16.
