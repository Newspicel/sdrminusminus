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

### Field hardening (first real-hardware sessions, post-M2)
Found live with an RTL-SDR — none reachable in CI (no hardware there, PLAN §14):
- [x] **Server SIGSEGV during USB churn** (critical): concurrent `soapysdr::enumerate` calls
  (hotplug prober · capture liveness probe · REST device probe) race SoapyHackRF's
  `hackrf_init`/`hackrf_exit` refcount and tear down its libusb context mid-find — all
  enumerates now serialize behind a process-wide lock (`device-soapy`)
- [x] `xtask dev` orphaned Vite on server exit (killed pnpm, not the tree) — own process
  group + group kill + null stdin; no more prompt spam / `EIO` crash
- [x] Dead backend read as success in the web client: openapi-fetch yields
  `{ error: undefined }` for empty error bodies (proxy 502), crashing on `data.id` and
  faking successful deletes — REST helpers now gate on `response.ok` (vitest-covered)

## M3 — Record & replay ✅

Goal (PLAN §16): SigMF recording · file playback device · recordings browser · start the
decoder fixture library.

**Status: complete.** `cargo xtask check` + `cargo xtask test` green (224 Rust + 72 web
tests); record→replay verified hardware-free end to end: virtual siggen → lossless recorder
tap → SigMF pair → probed playback device → NFM channel → 1 kHz tone recovered from decoded
Opus.

### Recorder (`crates/recorder`, new)
- [x] SigMF v1.2.6 meta model (`core:`-prefixed serde, cf32_le), streaming `SigmfWriter` +
  `SigmfReader` (torn-tail tolerant, rewind for looping), stem scan keyed on final `.sigmf-meta`
- [x] Crash-safe lifecycle: `.sigmf-meta.tmp` breadcrumb at create; atomic finalize (synced
  tmp rewrite → `fs::rename`) — breadcrumb-or-complete, never a listed-but-unparseable meta
- [x] Atomic stem claiming (`create_new` on data + breadcrumb, `StemTaken` → suffix retry);
  abort paths delete only files they created
- [x] `core:sample_rate` optional per spec (writer always emits; consumers reject rate-less
  files honestly)

### Engine recording
- [x] Lossless DSP-thread tap (PLAN §5/§7): per-slice Arc-copy → bounded queue → `sdrmm-rec`
  writer thread; overflow/writer death disarms the tap and surfaces a hard error — never a
  silent drop
- [x] Sample-count-exact `start_sample`; center retunes recorded as SigMF capture segments;
  ring overruns during recording counted into `RecordingStatus.overruns`
- [x] Start/stop under the anti-zombie ordering (files opened control-side, commit re-verified
  under `inner`, writer joined outside); implicit finalize on device fault, set removal, and
  `Engine::shutdown()`/`Drop` — both binaries finalize live recordings on exit
- [x] Sample-rate changes rejected while recording; both orders of the patch-vs-record race
  lose cleanly (in-flight counter + merge re-check, deterministic interleaving test)
- [x] Recording counters/errors ride the hotplug tick like overruns; writer faults land in
  `DeviceSet.recording.error` + `StateChanged`
- [x] e2e: record→replay roundtrip, playback spectrum frames, fault/shutdown finalization

### Playback (`device-virtual`)
- [x] `VirtualDriver::with_recordings(dir)`: probe lists one device per finalized recording
  (`virtual:file:<stem>`, cheap scan — hotplug-prober safe)
- [x] `FilePlayback`: real-time paced worker, capabilities pinned to the recording (center
  min==max, single rate), `loop` extra setting (live toggle, EOF park when off), I/O errors
  → `sink.fail`
- [x] Deterministic `render()` exposed for fixtures/tests

### Wire + server
- [x] `RecordingStatus` (on `DeviceSet`), `RecordRequest`/`RecordAction`, `RecordingInfo`,
  `StateScope::Recordings` + contract tests; OpenAPI codegen regenerated, zero drift
- [x] `POST /api/devicesets/{ds}/record` · `GET /api/recordings` · `DELETE
  /api/recordings/{id}`; SQLite `recordings` migration; disk↔index reconcile (files are the
  source of truth, PLAN §11) serialized against delete/stop — no ghost rows or 404 races
- [x] Error honesty: record I/O failures 500, validation 400, missing set 404;
  `--recordings-dir` (default platform data dir); desktop uses its app-data dir
- [x] Playback needs zero new endpoints: recordings probe as devices, "play" = the normal
  device-set open flow

### Web
- [x] Recordings panel — browsable with zero sets open (device-independent library): rate ·
  duration · size rows, Play (opens a playback set), guarded Delete
- [x] Record ●/■ control with live elapsed/size/overruns readout; faulted recordings freeze
  at the captured duration and surface the error; badge `record & replay · M3`
- [x] vitest: record-control state machine, duration/size formatting, fault freeze

### Fixtures (library started, PLAN §14)
- [x] `cargo xtask fixtures` renders deterministic SigMF pairs (`siggen_2m4_1s`) via
  `render()` + recorder; `fixtures/README.md` conventions (synthesized = regenerated, never
  committed; recorded off-air captures land with their M4+ decoders)
- [x] Decoder golden-test pattern proven now by the record→replay e2e

### Gates
- [x] `cargo fmt` + `clippy -D warnings` + full Rust suite green (224: recorder 11 ·
  device-virtual 22 · engine 37+8+2 · server 25 · wire 17 · dsp 45 · channels 26 · device 2 ·
  soapy 25 · sdrmm 4)
- [x] `biome ci` + `oxlint --type-aware` + `tsgo` + web build + vitest (72) green
- [x] OpenAPI regenerated, no drift; PLAN §5 record-endpoint sketch reconciled in the same
  change (format field deferred with reason)

### Post-review hardening (adversarial multi-agent review, all verified & fixed)
- [x] **Rate-patch TOCTOU** (high): a patch committing under a concurrently-started recording
  could silently mix sample rates under one SigMF meta — closed from both sides, tested with
  a blocking-apply mock device
- [x] **Recordings lost at shutdown** (high): the writer thread was never joined on process
  exit, stranding a breadcrumb-only pair — `Engine::shutdown()` + `Drop` + both binaries'
  exit paths finalize live recordings
- [x] Stem-claim TOCTOU let a losing concurrent start truncate/delete the winner's live files
  (atomic `create_new` claims); non-atomic finalize could crash into a listed-but-unopenable
  phantom (rename-based finalize)
- [x] Reconcile racing DELETE/stop 404'd successful deletes and churned row ids (server-side
  gate); implicit stops never invalidated the recordings list (`StateScope::Recordings`
  emitted on fault/remove/shutdown)
- [x] Web: recordings panel unreachable with no open set; record mutation missed STATE
  invalidation; unguarded Delete double-click (fixed in Presets/Bookmarks too); elapsed
  readout ticking past a fault
- [x] Lows: SigMF-optional `sample_rate`, 500-vs-400 honesty for record I/O, 404-before-400
  ordering, writer-fault-glue test via injected shared error, honest queue-cap comment

## M4 — Decoders wave 1 ✅

Goal (PLAN §16): RDS · POCSAG · ADS-B + map · AIS · APRS/AX.25 · RTTY · Morse ·
decoder-log database + export.

**Status: complete.**

### Wire (single source of truth, PLAN §4)
- [x] `crates/wire/src/decode.rs` — `DecoderEvent` tagged union (rds/pocsag/adsb/ais/aprs/rtty/
  morse) with one typed payload per decoder, plus `kind()`/`summary()`/`position()`/`station()`
  so the log table, CSV export, map and panels share one rendering
- [x] `DecodedRecord` (device set · channel · RFC3339 `at` · absolute `freq_hz` · event);
  `ServerEvent::Decoded` + `ServerEvent::DecodedLost`; `StateScope::DecoderLog`
- [x] Per-decoder `ChannelParams` variants + defaults; `WfmParams.rds`;
  `ChannelDescriptor.has_audio` / `decoder_kind` (decoders advertise no audio, the UI hides
  the transport); contract tests lock every tag and default
- [x] Decoder-log REST DTOs (`DecoderLogEntry`/`Response`/`Query`, `ExportFormat`, `DeletedCount`)

### Engine
- [x] Typed decoder frames leave the DSP plane through a bounded `DecodedSink` (drops counted,
  never silent); a pump thread stamps wall-clock time off the hot path and fans out on a
  broadcast separate from the control-event stream
- [x] `Engine::subscribe_decoded()` / `decoded_dropped()`; channel ids reserved before the host
  is built so a frame always knows which channel produced it

### DSP (`crates/dsp`, all pure + analytic golden tests)
- [x] `bits` — NRZI · differential · HDLC deframer (flag sync, zero de-stuffing, abort, shared
  flags, length bounds) · G3RUH scrambler/descrambler · sliding sync-word correlator with an
  error tolerance · bit packing/field extraction/Manchester
- [x] `fec` — CRC-16/X-25 + HDLC FCS · Mode S CRC-24 with single-bit repair · POCSAG BCH(31,21)
  + parity (2-bit correction) · RDS (26,16) syndrome + offset words, both directions
- [x] `pll` — second-order loop filter · tracking PLL with a pull-in clamp, harmonic output and
  a lock estimate · Costas loop for BPSK
- [x] `sync` — Gardner TED with a Farrow interpolator (`SymbolSync`) · zero-crossing bit clock
  (`BitSync`) that free-runs through a crossing-free stretch
- [x] `tone` — Goertzel · sliding-DFT tone correlator · attack/release envelope · adaptive
  keying slicer with an SNR estimate; `fir` gains bandpass and Gaussian (GMSK) designs

### Decoder log (`crates/server`, PLAN §11)
- [x] SQLite migration + indexed `decoder_log` (time · set · channel · kind · freq · station ·
  summary · verbatim typed event); filters compose (kind, device set, time window, free text,
  limit) with the total reported alongside the page
- [x] Batched writer task off the engine's decoded broadcast (one transaction per batch, retry
  queue, periodic prune to a bounded row count); lag and queue overflow are counted and
  reported as `dropped` on every list — loss is visible, never silent
- [x] `GET /api/decoderlog` · `DELETE /api/decoderlog` (filtered, emits `StateScope::DecoderLog`)
  · `GET /api/decoderlog/export/{csv|json}` as a real download with RFC4180 quoting
- [x] WS hub forwards `Decoded` on its own per-connection task; a lagging client gets
  `DecodedLost { count }` instead of a full-state resync storm

### Codegen
- [x] OpenAPI regenerated with the decoder-log paths and every decoder schema; `ExportFormat`
  force-registered (utoipa emits a bare `$ref` for a path-parameter enum, which
  `openapi-typescript` cannot resolve) — TS types generated, no hand-written mirrors

### Channels — decoders wave 1 (`crates/channels`, PLAN §13 P2)
- [x] **POCSAG** — discriminator → tracked slicing level → one bit clock per candidate rate;
  the rate that finds the frame sync word takes the lock and releases it when sync is lost, so
  512/1200/2400 are detected per transmission. BCH corrections counted into the event; numeric
  and alphanumeric bodies; `invert` honoured
- [x] **ADS-B** — level-relative preamble correlation (no fixed threshold: overhead and horizon
  aircraft differ by tens of dB), PPM slicing at 2 samples/bit, Mode S CRC with optional
  single-bit repair. DF17/18 only — every other downlink format overlays the address on the
  parity, so a zero syndrome there would invent aircraft. Identification, airborne/surface
  position (CPR, global pair and local against a reference), velocity, Gillham and 25 ft
  altitude; the per-ICAO CPR cache is bounded and age-limited
- [x] **AIS** — GMSK via discriminator + Gaussian matched filter, NRZI + HDLC + CRC-16/X-25,
  types 1/2/3/5/18/24, unavailable sentinels honoured, `!AIVDM` sentence with checksum
- [x] **APRS / AX.25** — AFSK1200 (mark/space correlators) and 9600 G3RUH (descrambled NRZI);
  address decoding with SSIDs and the has-been-repeated `*`, TNC2 line, uncompressed and
  base-91 compressed positions, course/speed and `/A=` altitude. Mic-E is out of scope for M4
  and yields a valid packet with no position rather than a wrong one
- [x] **RTTY** — ITA2 with LTRS/FIGS and `unshift_on_space`, start/stop framing with stop-bit
  rejection, 45.45/50/75 baud and 170/450/850 Hz shift, `invert`
- [x] **Morse** — envelope + adaptive keying slicer, element/gap clustering that tracks sending
  speed (or a fixed WPM tolerating ±30% sloppiness), international table, unknown sequences
  surface as `*` instead of vanishing; pure noise decodes to nothing
- [x] Reference modulators in `channels::testgen` behind the `test-signals` feature — one
  encoder per protocol, shared by the unit tests, the engine e2e and `xtask fixtures`. ADS-B's
  CPR/Gillham/callsign encoders are written independently from the decoder's (closed form vs
  table) so a mistyped constant fails a test instead of cancelling out

### Engine end-to-end (`crates/engine/tests/decode.rs`, PLAN §14)
- [x] Each decoder: reference transmission → SigMF pair → `virtual:file:` playback → DDC at a
  deliberately different device rate → decoder → the engine's decoded broadcast, asserting the
  exact message plus the device set, channel and absolute frequency the record is stamped with
- [x] **Wideband-channel rejection** (found by this run): the DDC delivers only 80% of the
  output rate flat, so ADS-B — which fills its whole 2 MHz channel — decoded *nothing* at
  2.4 Msps, indistinguishable from an empty sky. `validate_channel` now refuses any channel
  whose occupied band exceeds what a resampling DDC can deliver, naming the rate that works
- [x] **RDS** — 57 kHz DBPSK off the FM composite: 19 kHz pilot PLL → 3rd harmonic (a real
  receiver's path, and far steadier than locking 57 kHz directly) → symbol sync → differential
  decode → offset-word block sync that holds through bad blocks before re-hunting. Groups
  0A/0B (PS, TP/TA/MS, alternative frequencies), 2A/2B (RadioText with the A/B flag), PTY with
  its name. An event is emitted only when a field actually changed — RDS repeats endlessly and
  one event per group would flood the log. Lives on the WFM channel behind `WfmParams.rds`, so
  audio and RDS come off one demod chain

### Web (PLAN §10)
- [x] Live decoded-event store (Zustand, outside TanStack Query): per-kind ring buffers, a
  100 ms publish batch so an ADS-B burst cannot re-render at frame rate, station aggregation
  that merges partial ADS-B frames forward into one row, age-out, and a visible `lost` counter
- [x] Decoder log panel — filter by kind/set/text/limit through the query key (no manual
  refetch, no polling), live rows prepended and visually distinct, CSV/JSON export as real
  download links, guarded filtered clear
- [x] MapLibre map (OpenFreeMap, no API key) for ADS-B/AIS/APRS: GeoJSON sources updated on a
  throttled tick rather than a React element per marker, targets aged out, and a themed
  fallback when the basemap cannot load — a Pi in a field must still plot its targets
- [x] Per-decoder views: RDS station display, aircraft/ship target tables, rolling RTTY/Morse
  transcript, pager list, APRS packets — each scoped to its channel
- [x] Decoder settings forms generated from the params union (an unhandled variant fails
  typecheck); audio controls hidden for channels whose descriptor reports no audio
- [x] App shell: decoder frames routed into the store, one live panel per decoder channel, map
  shown only when a position-bearing decoder is running, `DecoderLog` scope invalidation

### Fixtures (PLAN §14)
- [x] `cargo xtask fixtures` writes one playable SigMF pair per decoder from the same
  `testgen` modulators the tests use, re-runnable, each printing what it should decode to;
  `fixtures/README.md` documents the channel and offset for every one
- [x] Off-air captures remain the honest gap — every decoder is currently proven against the
  specification via its reference modulator, not against the world

### Gates
- [x] `cargo xtask check` green: fmt · clippy `-D warnings` · Soapy-free build · `biome ci` ·
  `oxlint --type-aware` · `tsgo` · web build · codegen drift
- [x] `cargo xtask test` green: 465 Rust tests (dsp 113 · channels 167 · engine 37+8 decode+8
  listen+2 record · server 42 · wire 24 · recorder 11 · device-virtual 22 · soapy 25 · device 2
  · sdrmm 4) + 155 web tests

### Post-review hardening (adversarial multi-agent review: 37 findings, 27 refuted, 10 confirmed & fixed)
Critical/high:
- [x] **ADS-B geometric altitude was decoded as metres** (high): TC 20–22 select the altitude
  *source*, not its encoding — the AC12 field is the same Q-bit/Gillham feet code as TC 9–18.
  Every GNSS-altitude frame reported a wrong number, and 12 bits of metres cannot even express
  FL380, so the field could never have been metres. The reference modulator shared the mistake
  (its `.min(0xFFF)` clamp was the tell), which is exactly the generator/decoder cancellation
  the fixture strategy exists to prevent — both fixed, plus the absent-altitude sentinel
- [x] **A retune never reached the decoder** (high): the engine sends no settings command for
  an offset-only patch, so `WfmChannel`'s "a retune is a different station, reset RDS" rule was
  unreachable in production. `ChannelRx::retuned()` is now a first-class hook called from
  `DspCommand::Retune`, and an engine end-to-end test drives that exact path
- [x] **Decoder panels keyed on channel id alone** (high): channel ids are per device set, so
  two sets' frames poured into each other's panel
- [x] **Conditional React hook in `TextView`** (high): patching a channel from rtty to morse in
  place changed the hook count and tore down the app
- [x] **A test that asserted nothing** (high): the RDS retune test's 0.05 s probe was too short
  to close a group, so its `all()` passed over an empty list — and it exercised `apply`, an
  entry point the retune path never uses

Medium/low:
- [x] Squelch spliced the sample stream for decoders: skipping the gated span deletes time a
  decoder measures its bit clock in. Decoders are now fed silence of the same duration; audio
  demods keep the cheap skip, which is where the CPU saving matters
- [x] APRS compressed reports ignored the compression-type byte, fabricating a course and speed
  for a GGA-sourced packet and dropping its altitude
- [x] The client's station index grew without bound for any decoder whose panel does not drive
  `ageOut` (a POCSAG-only session leaked one entry per RIC) — now capped, least-recently-seen first
- [x] The RTTY alphabet guard spot-checked 9 of 30 code points while the modulator shared the
  same tables; it now transcribes the whole chart independently — which immediately caught that
  the transcription, not the decoder, had mixed ITA2 variants

## M5 — Ops & UX polish ✅

Goal (PLAN §16): frequency scanner · auto-reconnect on replug · multi-client polish · token
auth · MCP server · template gallery + first-run wizard · native `rtl-native`/`hackrf-native`
backends → self-contained binaries · Tauri packaging/signing · Docker/Pi image · docs site ·
`--doctor`.

**Status: complete.** `cargo xtask check` + `cargo xtask test` green; no hardware in CI.

### Wire (single source of truth, PLAN §4)
- [x] `crates/wire/src/scan.rs` — `ScanRange`/`ScanSettings`/`ScanState`/`ScannerStatus`,
  `ScanRequest`+`ScanAction`, `MAX_SCAN_TARGETS`; `DeviceSet.scanner` and
  `ServerEvent::ScannerUpdate` (its own event, not a `StateChanged` — a sweep retunes several
  times a second, PLAN §5)
- [x] `crates/wire/src/doctor.rs` — `DoctorReport`/`DoctorCheck`/`CheckStatus`, shared by
  `sdrmm --doctor` and `GET /api/doctor` so the CLI and the web UI cannot disagree
- [x] `TemplateInfo`/`TemplatesResponse`/`ApplyTemplateRequest`, `AuthInfo`, `ClientsResponse`,
  `StateScope::Clients`; contract tests lock every new tag, default and absent-field case
- [x] OpenAPI regenerated, TS aliases added, zero drift

### Frequency scanner (`crates/engine/src/scanner.rs`, PLAN §13 P2)
- [x] The unit of work is a *device tuning*, not a target: targets are grouped into
  passband-sized tunings and every target inside one is measured from the same spectrum
  frames, so a 2 MHz receiver sweeps a whole band per dwell. Costs retunes and a max over FFT
  bins — no extra DSP, which is what keeps it inside the Pi 4 budget (PLAN §14)
- [x] Peak-hold over the dwell (a burst mid-dwell counts), measurement over a configurable
  bandwidth by *max* bin so a threshold picked off the waterfall means the same thing, and a
  post-retune settle + drain so the ring backlog is never measured at the old frequency
- [x] Hold: parks the configured channel on the hit so audio/decoding follows, resumes after
  `resume_ms` of quiet; a hold channel the user deletes mid-scan drops the listening half
  instead of killing the scan
- [x] A running scan owns its set's centre frequency: client retunes are refused (`PatchOrigin`)
  rather than fought over, and the scan's own retunes are silent — progress rides
  `ScannerUpdate`, throttled to 5 Hz with state transitions exempt
- [x] Targets outside the device's tuning range, an unreachable hold channel, and unusable
  settings (zero/NaN step, inverted range, >20 000 targets) are all rejected at `start_scan`
  with a message naming the problem
- [x] Teardown ordering: `stop_scan`, device fault and set removal all take the handle under
  `inner` and join outside it (the scan thread takes that lock on every step)
- [x] Tests: plan expansion/dedup/rejection, tuning coverage, bin measurement; an end-to-end
  scan against a mock device synthesizing a carrier at a fixed *absolute* frequency — it finds
  it, holds, parks the channel on it, refuses a client retune, and tears down mid-sweep

### Auto-reconnect on replug (`crates/engine`, PLAN §16 M5)
- [x] A faulted set whose device re-enumerates is re-opened by the hotplug tick, its stored
  tuning re-applied and its channels rebuilt — ids, PCM identity and live audio subscriptions
  all preserved, so a listener never re-subscribes
- [x] Device I/O outside `inner`, swap under it, old runtime stopped before the replacements
  start (one channel is never hosted on two DSP threads)
- [x] A device that is present but unopenable keeps the set faulted with the live reason, and
  re-reports only when that reason changes — a retry every probe interval must not invalidate
  every client on a timer
- [x] Tests: reconnect restores channels and keeps a pre-fault audio subscription alive;
  repeated failure reports once

### Token auth (PLAN §12)
- [x] `crates/server/src/auth.rs` — one `route_layer` middleware over REST + WS + MCP, and
  deliberately *not* the SPA fallback (the login UI must load before a token can be typed, and
  an unmatched `/api/*` stays a typed 404 rather than a 401). CORS stays outermost so a
  preflight is never 401'd
- [x] `Authorization: Bearer` **or** `?token=` — the browser WebSocket API cannot set headers
  and the decoder-log export is a plain navigation; constant-time comparison; an empty token
  reads as "no auth" with a warning instead of half-enabling it
- [x] `/api/auth` (unauthenticated), `/api/openapi.json` and `/api/docs` stay public: they
  describe the API's shape, never its data
- [x] `sdrmm --token` (also `SDRMM_TOKEN`, so it need not appear in the process list); the
  desktop app stays loopback-only and unauthenticated
- [x] Web: fetch middleware, token in the WS URL and the export href, a login gate driven by
  `GET /api/auth` (the browser reports a rejected handshake as a plain close, so without the
  probe a wrong token is indistinguishable from an outage), reconnect backoff 1 s→30 s

### MCP server (`crates/server/src/mcp.rs`, PLAN §5, §18)
- [x] `rmcp` 3.1 streamable-HTTP at `/mcp`, stateless, behind the same token layer
- [x] 13 tools over the *same* engine/store calls REST uses — state, devices, channel types,
  open/close, tune, add/remove channel, start/stop scan, record, decoder-log query, spectrum
  snapshot; channel settings are built through the wire enum, so MCP accepts exactly what REST
  does and no parallel settings model exists
- [x] Tests: tool names locked, every tool described with an input schema, and an end-to-end
  `tools/list` over HTTP proving the mount and the token gate

### Templates + first-run wizard (PLAN §10)
- [x] `crates/server/src/templates.rs` — eight built-in stations (FM radio·RDS, airband, ADS-B,
  AIS, APRS, POCSAG, 2 m, marine VHF), each with a "what am I looking at" explainer; a static
  table, not seeded rows (a shipped template needs no migration and cannot be half-deleted)
- [x] `apply_configuration` extracted so presets and templates share one apply path, including
  the honest partial-application detail
- [x] Tests: unique slug ids, every channel inside the flat 80% of its own passband (a template
  that cannot be applied is worse than none), ADS-B pinned to its 2 Msps device rate
- [x] Web: gallery with out-of-range cards greyed against the open device's capabilities, and a
  wizard that appears only when the *server* is untouched and *this browser* has not dismissed
  it — pick a device (hardware ranked above the signal generator), then a template, with the
  `--doctor` report one click away when nothing shows up

### Native backends (PLAN §6, §15)
- [x] `crates/device-rtlsdr` over `rs-rtl` 0.4.2 — probe/open by serial (falling back to
  bus/address when dongles share the factory serial), tuner gain table with nearest-step
  snapping, tuner AGC, bias-T, IF bandwidth, u8→cf32 at the device edge with a 256-entry LUT
  and I/Q alignment carried across block boundaries
- [x] `crates/device-hackrf` over `hackrf-nusb` 0.3 — probe/open by serial, per-stage LNA/VGA
  gains (8 dB / 2 dB steps, the thing Soapy's collapsed gain hides), amp, bias-T, samples
  already `Complex32` at the crate boundary
- [x] Both registered above Soapy in the serial merge; `--no-default-features --features
  rtl-native,hackrf-native` builds a Soapy-free binary and is now a gate in `xtask check`
- [x] 53 hardware-free tests between them (conversion, gain snapping, capability construction,
  every validation rejection path); no test touches USB

### Diagnostics, packaging, docs (PLAN §15)
- [x] `sdrmm --doctor` + `GET /api/doctor`: compiled backends (derived from the registry, so it
  cannot drift), devices found, Linux udev/USB permissions with the fix, and writability of the
  database and recordings paths — `collect` does the I/O, `render` is pure and tested
- [x] `cargo xtask dist` — web build, an assertion that `web/dist/index.html` exists (the
  server's `build.rs` creates an empty `web/dist`, so a release without it would silently ship
  the "not built" page), then the release-shaped `--no-default-features --features
  rtl-native,hackrf-native` build
- [x] Multi-stage `Dockerfile` (+ `.dockerignore`, `docker-compose.yml` with USB passthrough and
  a data volume) and a tag-triggered `release.yml`: linux x86_64/aarch64 + macOS arm64
  tarballs, a multi-arch ghcr.io image, and Tauri bundles (unsigned without the Apple secrets)
- [x] mdBook docs site (`docs/`) + `docs.yml` Pages deploy, a real `README.md`, and the `LICENSE`
  file the manifests have claimed since M0

### Multi-client polish
- [x] Decoder frames are serialized **once** for the whole server and shared as `Utf8Bytes`;
  every connection used to re-serialize byte-identical JSON, which under ADS-B traffic cost N×
  the work for N browsers
- [x] `GET /api/clients` + `StateScope::Clients`: connections are counted on connect/disconnect
  and the header says so when more than one operator is on the radio
- [x] WS reconnect backoff (1 s → 30 s): a stopped server, or a wrong token, used to mean a
  1 Hz request flood for as long as the tab stayed open

### Gates
- [x] `cargo xtask check` green: fmt · clippy `-D warnings` · Soapy-free build · release-shaped
  native build · `biome ci` · `oxlint --type-aware` · `tsgo` · web build · codegen drift
- [x] `cargo xtask test` green: 564 Rust tests + 171 web tests
- [x] `cargo xtask dist` produces a 24 MB `dist/sdrmm` that links no libSoapySDR, libusb,
  libopus or libsqlite (checked with `otool -L`) — PLAN §15's "release artifacts just run"

### Field session (first real hardware on the native backend)
Run against the built release artifact and a Nooelec NESDR SMArt v5 (R820T), not in CI:
- [x] The native RTL-SDR backend enumerated, opened and streamed the dongle with no SoapySDR
  and no C library present: `rtlsdr:89084597`, 24 MHz–1.766 GHz, one TUNER stage of 29 steps,
  `bias_tee`/`agc` extras — and `sdrmm --doctor` reported exactly that
- [x] `GET /api/doctor` and the MCP `spectrum_snapshot` tool over real RF: a broadcast carrier
  at 100.976 MHz, 30 dB above the noise floor, through the full rs-rtl → ring → DSP → FFT path
- [x] The scanner swept 88–108 MHz (201 targets) and held on a real station at −21 dBFS
- [x] **Auto-reconnect earned its keep on its first live session.** The dongle stalled
  spontaneously after 40 s (`kIOReturnNotResponding`, then four cancelled bulk transfers, which
  rs-rtl reports as "dongle disconnected"); the set faulted, released the device, and the next
  probe re-opened it and restored the channel ~9 s later with no operator action. Confirmed
  *not* self-inflicted: eight rapid `GET /api/devices` probes during streaming (which
  re-enumerate and read string descriptors) disturbed nothing

### Known gaps (honest, not deferred silently)
- A *transient* USB stall costs a full teardown and re-open — about nine seconds of dead air —
  because rs-rtl turns five consecutive transfer errors into "disconnected" and the backend
  can only report that as a fault. **Root cause found by re-reading rs-rtl 0.4.2** (driver
  re-evaluation, PLAN §17): its counter cannot tell an independent failure from the fallout of
  one. `streaming_thread` keeps 15 transfers in flight and increments `consecutive_errors` on
  every errored completion, resetting it only on a *successful* one (`rtlsdr.rs:1022-1042`).
  When a pipe faults, the queued transfers behind it are aborted too, and nusb reports each as
  `TransferError::Cancelled` (`macos_iokit/mod.rs:35`) — so one stall delivers one real error
  plus four cancellations with no success in between, which is exactly the observed
  `kIOReturnNotResponding` + four cancelled transfers, and exactly `MAX_CONSECUTIVE_ERRORS = 5`.
  The fix is ours and needs no crate switch: `RtlSdr` latches no streaming flag, and
  `start_streaming_with` re-runs the `USB_EPA_CTL` FIFO reset and opens a fresh endpoint on
  every call (nusb frees the endpoint claim when the old one drops), so re-calling
  `start_streaming()` after the channel closes restarts in place — no re-open, no re-tune, no
  bias-tee reset. It needs `sdr` behind an `Arc<Mutex<…>>` so the capture thread can become a
  supervisor loop with bounded retries before it faults the set.
- RDS did not decode in the short live window on the station tested. Inconclusive rather than a
  regression — the station may carry no RDS, and the window was tens of seconds — but it means
  the M4 decoders still have no off-air proof, exactly as PROGRESS said at M4.
- Workspaces/tabs (dockview) were never built in M0–M2 despite PLAN §16's parenthetical; the
  plan is corrected in the same change and the shell is deferred to M6.
- The HackRF backend is proven against its crate's API and unit-tested hardware-free; unlike
  the RTL-SDR one it has had no live session (no HackRF to hand).
- rs-rtl exposes no PPM or direct sampling, and hackrf-nusb no independent baseband-filter
  bandwidth or hardware sweep; those settings are *rejected* rather than advertised and faked.
  Soapy still covers them, which is what the `soapy` feature is for.
- macOS signing/notarisation needs Apple secrets the repo does not have; the release workflow
  produces unsigned bundles until they are configured.
- The Docker image has not been built here (no daemon in this environment); the workflow builds
  it on every tag.
- Dependabot flags a moderate unsoundness in `glib` 0.18 (`VariantStrIter`), reached only
  through Tauri → gtk 0.18 → glib on Linux, in the desktop app. It cannot be resolved here:
  `gtk 0.18` requires `glib ^0.18`, so the fix has to come from a Tauri release. Nothing in
  this project calls the affected iterator, and the headless server does not depend on it.

### Post-review hardening (adversarial multi-agent review: 29 findings, 10 refuted, 19 confirmed & fixed)
High:
- [x] **The RTL-SDR backend mistuned the radio on any bandwidth change** (high): rs-rtl's
  `set_bandwidth` reassigns the tuner's IF and rewrites only the demodulator's IF register, so
  the PLL was left at `centre + old_IF` — the dongle received `centre + (old_IF − new_IF)` while
  `settings()`, the spectrum metadata, every channel offset and the recorder's SigMF centre all
  still said `centre`. A 300 kHz filter at 2.048 Msps mistunes by 500 kHz, an 8 MHz one by
  2.9 MHz. librtlsdr re-tunes after a width change for exactly this reason; the backend now does
- [x] **The release image name was never lowercased** (high): `ghcr.io/${{ github.repository }}`
  expands to `ghcr.io/Newspicel/…` and an OCI reference must be lowercase, so every tagged
  release would have failed at the push and published no GitHub Release at all
- [x] **Desktop bundles were uploaded under `dmg/`, `deb/`, `appimage/`** (high): the release job
  attaches every downloaded entry as a file, and would have handed `gh release create`
  directories; bundles are flattened before upload and the asset list is now explicit

Medium:
- [x] **A faulted set kept its device open, so no replug could ever recover it**: a USB backend
  holds its interface claim for as long as the handle lives, and `reconnect` re-opens the very
  same radio — it would have failed "busy" every time. `CaptureRuntime::stop` now drops the
  device, and a fault stops the runtime. Regression-tested with a driver that can only be
  opened once, which is what every real one is
- [x] **A fault from the fresh capture during a reconnect was discarded**, leaving a dead
  capture advertised as Running forever; such a fault is now parked and handed to the normal
  fault path once the swap lands
- [x] **Every client's waterfall froze permanently after a reconnect**: replacing the runtime
  closes the spectrum broadcast, and the per-connection task treated that as "the set is gone".
  It now re-subscribes and only stops when the set really is gone (the test fails without it)
- [x] **Unauthenticated remote panic**: `percent_decode` sliced a `&str` at `i+1..i+3`, which a
  `%` followed by a multi-byte character splits off a char boundary. Now sliced as bytes
- [x] **A wrong or stale token bricked the UI**: nothing ever cleared it, so every request 401'd
  behind a UI that looked fine. A 401 now forgets the token and the gate asks again, saying so
- [x] **A rejected preset or template destroyed the device set**: the apply deletes the channels
  before it can retune, so a configuration the device was always going to refuse left the
  operator with nothing. `Engine::validate_configuration` pre-flights the whole thing first
- [x] `docs/` claimed `--no-default-features` keeps the native backends (it drops them), and
  that auto-reconnect was unbuilt (it ships here)

Low:
- [x] `ScanPlan::build` cast the step count to `usize` before bounding it — `f64 as usize`
  saturates in release and panics in debug, so a 1 Hz step over a GHz behaved differently per
  profile; bounded before the cast now
- [x] `retryNow()` could leave two live sockets fanning duplicate frames into the same handlers
- [x] The MCP `record` stop never indexed the finished recording nor invalidated the recordings
  scope, so an agent-finalized recording vanished from the library
- [x] The doctor inferred file-vs-directory from the extension, so `--db /data/sdrmm` had a
  *directory* created where the database belongs; the caller says which it is now
- [x] MCP `query_decoder_log` reported database failures as invalid parameters
- [x] `SHA256SUMS` hashed itself, while empty, because the redirect created it before `find` ran
