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

## M2 — Listen ✅

Goal (PLAN §16): DDC channels · NFM/AM/SSB/WFM · squelch/AGC · Opus audio in browser+Tauri ·
presets · bookmarks · phone-usable UI.

**Status: complete.** `cargo xtask check` + `cargo xtask test` green (136 Rust + 37 web tests);
end-to-end listen path verified hardware-free: virtual modulated carriers → DDC → demod →
Opus → WS → browser decode/playback.

### Wire (single source of truth, PLAN §4)
- [x] Typed per-mode channel settings: `ChannelParams` tagged enum (`nfm`/`am`/`ssb`/`wfm`) with
  serde-default'd `NfmParams`/`AmParams`/`SsbParams`/`WfmParams` → TS discriminated union
- [x] `ChannelSettings { offset_hz, squelch_db: Option, params }`; `ChannelDescriptor` gains
  `input_rate_hz`; contract tests lock tagging + defaults
- [x] `AudioFrame` binary codec (16-byte header · ch_layout · Opus payload) + roundtrip test
- [x] WS: `SubscribeAudio`/`UnsubscribeAudio`, `AudioStreamStarted`; `StateScope::Presets`/`Bookmarks`
- [x] REST DTOs: channel types, presets (versioned `PresetSnapshot`), bookmarks

### DSP (`crates/dsp`, all pure + analytic golden tests)
- [x] Windowed-sinc FIR design (Blackman, unity DC gain) + shared streaming FIR core
- [x] Polyphase `Decimator`/`RealDecimator` (ragged-block bit-exact vs one-shot)
- [x] `FracResampler` — 128-phase polyphase bank, arbitrary ratio, ±2 samples long-run
- [x] `Ddc` — NCO mix → staged decimation → fractional resampler, exact output rate,
  >50 dB alias suppression, cheap `set_offset` retune
- [x] `FmDemod` quadrature discriminator · `Agc` (attack/release) · `Squelch`
  (power + hysteresis + hold) · `Deemphasis`/`DcBlocker` · `FirC` complex-tap FIR (SSB)

### Channels (`crates/channels`)
- [x] NFM, AM, SSB (USB/LSB), WFM (mono, de-emphasis, 240k→48k) demods → 48 kHz mono PCM
- [x] Registry: descriptors + `create()` from one per-module table (cannot drift); params-variant
  and input-rate validation; `apply()` reconfigures mode params in place
- [x] Per-demod analytic tests: 1 kHz tone recovery over ragged blocks, USB/LSB rejection,
  WFM exact 5:1, AGC convergence

### Engine (`crates/engine`)
- [x] Channel hosting on the DSP thread via command queue (Add/Remove/Retune/ApplySettings) —
  hot path keeps zero locks/steady-state allocation; closed squelch emits duration-exact silence
- [x] Per-channel Opus encoder threads (libopus vendored via `opus`, mono 48 kHz, 20 ms frames)
  → `AudioPacket` broadcast; clean join on remove/stop
- [x] `patch_channel` (delta retune/apply, or rebuild swap on type change that keeps audio
  subscribers), `subscribe_audio`, `channel_types`; offset+bandwidth validated against device rate
- [x] Device sample-rate change rebuilds all hosted channels (pre-validated, ids + audio preserved)
- [x] `device-virtual`: phase-continuous NFM/AM/WFM modulated test carriers (pub-const layout)
- [x] e2e listen tests: NFM/AM/WFM tone recovery from decoded Opus, squelch gating + reopen via
  patch, offset retune, type change with live subscription, rate-change rebuild, seq/timestamp
  contiguity

### Server (`crates/server`)
- [x] WS audio: per-connection stream ids, `AudioStreamStarted`/`StreamStopped`, drop-oldest
  forwarding; resubscribe replaces; teardown on close
- [x] REST: `GET /api/channeltypes`, `PATCH /api/devicesets/{ds}/channels/{ch}`; create/delete
  channel moved onto the blocking pool
- [x] SQLite persistence (`rusqlite` bundled, `user_version` migrations): presets
  (capture → list → apply → delete; apply = retune + replace channels) + bookmarks CRUD;
  `Config.db_path` (None = in-memory); `sdrmm --db` flag; desktop uses Tauri app-data dir
- [x] OpenAPI: new paths + force-registered `PresetSnapshot`; snapshot test; codegen regenerated,
  no drift; handler tests for every new endpoint

### Web
- [x] Audio playback: `AudioFrame` decode (mirrors `wire/frame.rs`), WebCodecs `AudioDecoder`
  fast path + `opus-decoder` WASM fallback, AudioWorklet jitter buffer (100 ms target,
  underrun-rebuffer, 400 ms drop-oldest), per-channel gain, gesture-unlocked shared context,
  reconnect auto-resubscribe — vitest-covered (frame/jitter/engine state machine)
- [x] Channels panel: add-by-type, offset (kHz stepping), squelch toggle+slider, mode-specific
  forms from the generated union, play/volume, remove; optimistic PATCH mirroring M1 pattern
- [x] Spectrum channel markers (click-to-select, pointer-transparent overlay)
- [x] Presets + bookmarks panels (save/apply/tune/delete, inline error banners)
- [x] Phone-usable pass: <768px stacked collapsible sections, ≥40 px touch targets, no overflow
  at 390 px; header badge `listen · M2`

### Gates
- [x] `cargo fmt` + `clippy -D warnings` + full Rust suite green (dsp 36 · channels 21 ·
  engine 12+8 e2e · server 10 · wire 13 · device-virtual 9)
- [x] `biome ci` + `oxlint --type-aware` + `tsgo` + web build + vitest green
- [x] OpenAPI codegen regenerated, no drift; CI unchanged (same `xtask` gates)

### Post-review hardening (adversarial multi-agent review, all verified & fixed)
Critical/high:
- [x] **Stale-snapshot rebuild race** (critical): `patch_device`/`patch_channel` rebuilds re-check
  channel existence + settings under `inner` and send DSP commands under it (mpsc never blocks) —
  a concurrent `remove_channel` can no longer strand a zombie channel that hangs the encoder join
- [x] **Spectrum unsubscribe killed unrelated audio** (high): `StreamStopped` now carries
  `StreamKind`; audio stream ids allocated from a disjoint high range; client acts only on
  audio-kind stops
- [x] **NFM/AM had no channel IQ filtering** (high): pipeline is now DDC → mode-aware complex
  channel filter → squelch → demod; squelch measures filtered channel power (mode-comparable)
- [x] **`apply_preset` validated against soon-deleted channels / non-atomic** (high): reordered
  remove→patch→add; honest partial-state detail in the error
- [x] **Capture-ring overruns silently corrupted output** (high): `DeviceSet.overruns` surfaced
  in state; hotplug tick emits `StateChanged` + rate-limited warn when it grows
- [x] **`ServerEvent::Error` unhandled + failed SubscribeAudio bricked Play** (high): errors shown
  in a banner; failed subscribe resets to a retryable stopped state
- [x] **Rapid Play→Stop→Play stale-event race** (high): generation-guarded subscribe binding so a
  superseded `AudioStreamStarted`/`StreamStopped` can't tear down the live stream

Medium/low:
- [x] DDC alias floor restored to ≥50 dB for hostile resample ratios; one NaN sample no longer
  latches Deemphasis/DcBlocker/AGC/Squelch state; `StreamFir` handles factor > taps without panic;
  `FmDemod` seeds its first sample (no startup pop)
- [x] Drop-oldest WS backpressure (was drop-newest); event forwarder synthesizes `StateChanged{All}`
  on broadcast lag; spectrum subscribe moved to `spawn_blocking`; fps throttle delivers the
  requested rate; REST extractor rejections return `ApiError` bodies
- [x] Encoder-lag PCM drops surface as timestamp gaps (client conceals/resets); NFM/SSB level
  clamped to Opus range (dropped SSB's 2× factor); `validate_channel` honors bandwidth + SSB
  one-sided occupancy
- [x] Web: sink teardown on decoder failure; suspended-`AudioContext` observed + resumed on gesture
  (no false "playing"); jitter buffer sheds back to target after a burst; timestamp-gap loss
  detection; device/channel errors surfaced; debounced PATCH cancelled on device change; spectrum
  meta reset on device-set switch; ≥40 px marker touch targets
- [x] Headless DB defaults to the platform data dir (`dirs`); virtual device rejects out-of-range
  `center_hz`; per-block hot-path zero-fill allocation removed (documented bounded-cost note on the
  remaining Arc/broadcast audio send)
