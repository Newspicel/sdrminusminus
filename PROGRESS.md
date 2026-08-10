# Implementation progress

Checklist tracked against `PLAN.md` §16 milestones. `PLAN.md` remains the source of truth for
the idea, the binding rules and everything still unbuilt; this file is the record of what
shipped and how it was verified, and the plan points here rather than repeating it. Tick items
as they land with tests green.

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

### Field sessions (real hardware on the native backends)
Run against the built release artifact, not in CI.

**RTL-SDR — Nooelec NESDR SMArt v5 (R820T):**
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
- [x] **Driver bench-off against the real dongle** (PLAN §17, post-M5 re-evaluation). The
  earlier survey passed `rtlsdr-pure` over on a claim that had never been run: that its
  one-transfer-at-a-time read starves the RTL2832U FIFO at 2.4 MS/s. Measured with the
  RTL2832U's test-mode counter ramp (every lost byte is a countable discontinuity), the claim
  is **wrong on an idle machine** — 8 breaks in 131 MB at 2.4 MS/s, full rate delivered — and
  **right under load**, which is the condition that matters: with 16 spinning threads it drops
  ~2.2% of the stream (241–254 breaks per 131 MB, ~1 discontinuity per 0.1 s). rs-rtl on the
  same machine, load, rates and byte count delivered 100.0% with 0 dropped chunks. The
  recommendation is unchanged — stay — but it now rests on a measurement instead of a README

**HackRF One (serial 230865dc3170af47):**
- [x] Enumerated, opened and streamed through the native backend with no libhackrf present.
  The capability model is the per-stage one Soapy's collapsed "overall gain" hides: LNA
  0–40 dB in 8 dB steps, VGA 0–62 in 2 dB steps, `amp` and `bias_tee`, 1 MHz–6 GHz, a
  *continuous* 2–20 Msps range
- [x] **20 Msps with zero overruns** — the whole 88–108 MHz broadcast band in one window, six
  stations resolved. Gain writes verified against the radio rather than the API: sweeping VGA
  0→62 dB moved the measured noise floor −100.7 → −42.9 dB (the peak barely moves because a
  local broadcast station compresses the front end, which is physics, not a bug)
- [x] The scanner swept 88–108 MHz (201 targets) at 8 Msps, held on a real station at
  91.100 MHz, and parked the WFM channel exactly on it — the whole M5 chain (native backend →
  DDC → scanner → hold channel) on real RF, zero overruns
- [x] **Found a real bug this session, fixed and regression-tested:** asking for 13 dB of LNA
  gain returned `204` and reported 13 dB back, but the HackRF's grid has no 13 — the radio was
  at 16. `Engine::patch_device` recorded the *request* (`merge_from(&delta)`) instead of what
  the device holds, so every backend-driven control could show a value the hardware never took.
  The device's own `settings()` is now laid over the request after a successful apply (both are
  needed: a backend that reports a field must win, one that reports nothing must not erase it).
  Verified on the radio afterwards: 13 → 16, 7 → 8, out-of-range still rejected

**Off-air RDS: a real gap, precisely bounded.** Five strong stations (88.0, 93.6, 97.8, 99.9,
107.5 MHz) plus the −31 dBFS station at 100.976 produced *no* RDS in tens of seconds each, on
both radios. The control run rules the plumbing out: the same binary decodes the synthesized
`rds_station_960k` fixture immediately and completely (PI D3C2, PS "SDR-M4", PTY, RadioText).
So the decoder is right against the specification and wrong against the air — exactly the risk
PLAN §14 names, now with evidence. Leading hypothesis, not yet proven: real broadcast is
*stereo*, and the 23–53 kHz L−R subcarrier sits directly against RDS at 57 kHz, while the
fixture is mono by construction (WFM stereo is deliberately unbuilt, PLAN §18). An 8 s off-air
SigMF capture of the 100.976 MHz station (HackRF, 2 Msps) reproduces the failure deterministically
with no hardware attached; it is 129 MB, so it is kept in `captures/` (gitignored) rather than
committed.

### Known gaps (honest, not deferred silently)
- The other M4 decoders (POCSAG, ADS-B, AIS, APRS, RTTY, Morse) still have no off-air proof at
  all; only RDS has been tried against the air.
- Workspaces/tabs (dockview) were never built in M0–M2 despite PLAN §16's parenthetical; the
  plan is corrected in the same change and the shell is deferred to M6.
- RDS does not decode off air (see the field sessions above). The decoder is proven against the
  specification and reproduced failing against a real capture; the cause is unresolved and the
  stereo-subcarrier hypothesis is untested. This is the M4 "no off-air proof" gap turning into a
  concrete defect, and it is the first thing to pick up.
- The native drivers expose no direct sampling, and the HackRF driver no independent
  baseband-filter bandwidth or hardware sweep; those settings are *rejected* rather than
  advertised and faked. Soapy still covers them, which is what the `soapy` feature is for.
  **Direct sampling is now unblocked but unbuilt** — it means bypassing the tuner, which was
  impossible while `rs-rtl` held its `R82xx` privately and is a small change now that the driver
  is ours. It is the next gap, deliberately out of scope of the vendoring change.
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

---

## Native drivers taken in-tree (post-M5)

Both native backends now own their radio driver, on one shared transport. The trigger was not
maintenance but correctness: each of the two crates this project depended on hand-rolled its own
USB transfer-error policy and both were wrong, in different ways, and the divergence is why one
of them went unnoticed for a whole milestone.

### The two bugs this closes
- **RTL-SDR, transient stall → nine seconds of dead air (was a known gap, now shipped fixed).**
  `rs-rtl` incremented one counter on *every* errored completion and tripped at
  `MAX_CONSECUTIVE_ERRORS = 5` with fifteen transfers in flight — so one stalled pipe (one real
  error plus four `Cancelled` completions from the transfers aborted behind it) read as
  "dongle disconnected", faulted the set, and cost a full teardown and re-open.
- **HackRF, one errored transfer killed the stream — newly found, never in this file before.**
  `hackrf-nusb` 0.3.0's `checked_completion` did `completion.status?` and `accept_completion`
  then called `close()`: no counting, no threshold, and no `Cancelled` exemption anywhere in the
  crate. Strictly worse than the RTL case, and it had simply never been provoked.

### Crates
- [x] `crates/usb-stream` (`sdrmm-usb-stream`) — the bulk-IN transport both drivers share.
  Bytes in, bytes out; no register access, no sample conversion, no device knowledge. Keeps
  `hackrf-nusb`'s `BulkIn` trait as the seam so the whole queue is exercised against a scripted
  mock, and `rs-rtl`'s pump thread with a bounded blocking hand-off — the shape that measured
  100.0% delivery under load. 16 transfers in flight, buffers recycled so a steady stream
  allocates nothing, live received/processed/dropped counters
- [x] **One error policy, librtlsdr's** (`src/librtlsdr.c:3814`), pure and unit-tested: a
  cancellation never counts, only genuine errors do, the threshold *is* the queue depth, any
  success clears it. Two deliberate differences from librtlsdr: an errored transfer is
  resubmitted rather than retired (rs-rtl's habit, strictly better), and a stop flag separates
  the cancellations we asked for from the fallout of a fault, which is a distinction the
  completion itself cannot carry
- [x] `crates/device-rtlsdr/src/driver/` — the RTL2832U radio, one crate with the backend that
  drives it. `regs.rs` (registers, I2C bridge) and `tuner.rs` (R82xx programming) are the
  valuable, tedious part and are kept. Dropped rather than carried over:
  `StreamControl` and the in-thread retune machinery (fire-and-forget, so a rejected retune
  looked applied — the backend already bypassed them), `read_gain`, `open_first`, and the
  descriptor accessors nothing called. A short control response is now an error instead of a
  silently-zero register
- [x] `crates/device-hackrf/src/driver/` — the HackRF radio, one crate with the backend that
  drives it. Blocking only: the `MaybeFuture` layer, the async stream and the `wasm32` paths are
  gone
- [x] **The HackRF's transmit path is in.** The first cut dropped it as unused; it is back,
  because a driver that omits half its hardware has to be rewritten when the gated TX phase
  arrives. Bulk-OUT queue of 16, libhackrf's zero-filled end-of-burst marker (owed and paid
  across a timed-out write), transmit VGA control, and half-duplex arbitration — the radio takes
  one direction at a time and `start_tx` refuses while a capture is running, and the reverse.
  It uses the *same* `TransferPolicy` as the receive side, with one deliberate difference:
  transmit never re-sends a failed transfer, because those samples were meant for a moment that
  has passed and putting them back behind the queue would corrupt the burst worse than the gap.
  **It stops at this crate's API:** `SdrDevice` has no transmit method, `Capabilities` still
  reports `tx_capable: false`, nothing in `engine`/`server`/the UI can reach it, and the
  transmit VGA is written to 0 dB on open (PLAN §12a)
- [x] `rs-rtl` and `hackrf-nusb` are out of the workspace manifest and `Cargo.lock`; `nusb`
  0.2.7 is the one direct dependency underneath all of it. Where the code started is recorded
  once, in `PLAN.md` §18, because it explains why "always newest versions" does not apply — the
  code is no longer a fork of anything, so there is no upstream version to track
- [x] Taking the drivers in-tree paid for itself immediately: the crate boundary had been
  keeping a slice of API alive that nothing used. Tightening `pub` to `pub(crate)` exposed and
  removed a `DeviceId` selector enum whose serial variant was never constructed, two descriptor
  fields nothing read, a `DeviceInfo` stored and never looked at, and an `is_disconnected` helper
  with no caller — plus the getter/setter pair on the HackRF `Config` that existed only so the
  backend could build one across the boundary

### Two-tier recovery
- [x] The engine's fault path is one-shot and destructive by design (`RxSink::fail` →
  `mark_device_fault` → `CaptureRuntime::stop`, which *takes and drops* the device so a replug
  can re-open it), so a cheap restart cannot live above that seam. Tier 1 is now inside the
  capture supervisor and runs *before* `fail`; tier 2 is the engine's existing path, unchanged
- [x] The policy is device-agnostic, pure and unit-tested, in `crates/device`: three attempts,
  20 ms doubling to 80 ms, and an uptime rule — a stream that stayed up for five seconds earns a
  fresh budget, so stalls minutes apart never accumulate, while a stream that keeps dying
  immediately burns its attempts and faults. That rule is the termination argument
- [x] The restart *primitive* stays in each backend, because reinitialisation differs: the
  RTL2832U's `USB_EPA_CTL` FIFO reset, the HackRF's transceiver mode. `clear_halt` is on both
  restart paths — the plan left it as an open question and it is now in and measured
- [x] **A silent-stall detector on both backends.** A wedged board reports no error at all, so
  the capture thread would park forever behind a waterfall the set still advertises as Running.
  `hackrf-nusb` had this (50 empty reads); `rs-rtl` never did. Both have it now, at five seconds,
  and it feeds the same restart path — a wedge is the best case for an in-place restart
- [x] A disconnect skips tier 1 entirely: the endpoint, the interface claim and the device handle
  left with the radio, so re-arming cannot help and burning three attempts on it only delays the
  reconnect
- [x] Both converters gained `reset()`, called on every restart: a stalled transfer can complete
  on an *odd* length, and a carried half sample prepended to the fresh stream would swap I and Q
  for the rest of the session
- [x] **Tier-1 restarts stay off the wire, deliberately.** The device-set model carries
  `status` + `error`, and `error` means "this set is not working"; a restart that succeeded in
  milliseconds means it *is* working, and there is no counters/warnings surface to put it on
  without a wire change well beyond this plan. The report is `tracing::warn!` with the attempt
  count, the delay and the reason, plus an `info!` when the stream comes back. A restart that
  *fails* still reaches the wire as a fault, exactly as before
- [x] **`device-soapy` keeps tier 2 only, and this is a judgement, not an oversight.** Its
  transient stalls are already absorbed inside the C driver — librtlsdr has had the correct
  policy for fifteen years — so a read error surfacing through Soapy means that driver already
  gave up, and re-activating the same `RxStream` afterwards is unverifiable here. Its capture
  loop already tells a quiet stream from a vanished device by re-enumerating. `device-virtual`
  gets none of it: no transport that can stall

### PPM correction, closed
- [x] `RtlSdr::set_freq_correction(ppm)`, librtlsdr-shaped, doing **both** halves, which are
  different mechanisms: the RTL2832U resampler has dedicated correction registers (demod page 1,
  0x3f/0x3e) and the tuner PLL has none, so it is corrected by telling the tuner its crystal is
  somewhere else and re-tuning. Doing only the first leaves every received frequency off by
  `ppm`; doing only the second leaves the sample rate wrong, which mistimes every decoder
  downstream. A rate change does not carry the correction registers over, so `set_sample_rate`
  re-writes them, as librtlsdr does
- [x] **No wire change was needed**, correcting the plan: `DeviceSettings.ppm` has been a
  first-class field since M1 (it is how `device-soapy` drives the `"CORR"` component) and the web
  UI already renders the control. `device-rtlsdr` simply stops rejecting it. Advertised range
  ±200 ppm, inside the demodulator's own ±488 register limit; whole ppm is the registers' only
  granularity, so a fractional request is rounded and `settings()` reports the hardware's value
  back
- [x] This replaces the caller-side approximation the previous "known gaps" entry proposed —
  owning the driver made the real thing available

### Gates
- [x] `cargo xtask check` green: fmt · clippy `-D warnings` · Soapy-free build · release-shaped
  native build · `biome ci` · `oxlint --type-aware` · `tsgo` · web build · codegen drift (none)
- [x] `cargo xtask test` green: 635 Rust tests (up from 564) + 171 web tests
- [x] `cargo xtask dist` produces a 25 MB `dist/sdrmm` linking only IOKit, CoreFoundation,
  libiconv and libSystem — no libusb, no libSoapySDR, no C radio library at all

### Field session (both radios attached)
Run against the built release artifact, not in CI.

**RTL-SDR — Nooelec NESDR SMArt v5 (`rtlsdr:89084597`):**
- [x] Enumerated, opened, streamed and decoded through the vendored driver: tuning, rate,
  gain-table snapping (30.0 dB → 29.7, the nearest step), tuner AGC, bias tee, a live WFM
  channel, and both validation rejections (ppm 500, rate 500 kHz) refused with reasons
- [x] **Load regression, the check the driver bench-off set:** 45 s at 2.048 MS/s and 45 s at
  2.4 MS/s under 16 spinning threads — **0 ring overruns, 0 dropped USB transfers, 0 restarts**
  at both rates. The ported path still delivers what `rs-rtl` measured at 100.0%
- [x] **PPM verified against a real carrier.** With the device on a broadcast station at
  101.296 MHz, the station's power centroid moved by **+22 to +23 kHz at +200 ppm** and **−17 to
  −21 kHz at −200 ppm** across two runs, against a predicted ±20.3 kHz, returning to within
  1.5–2.4 kHz of its starting point at 0 ppm — which is also the measurement floor (16 kHz bins
  and a wandering FM centroid). Direction and magnitude both as predicted. The *resampler* half is verified only by the register writes matching librtlsdr and
  `rtlsdr-pure`: a 200 ppm error in a 2.048 MHz span is 410 Hz, which no measurement available
  here can separate from noise
- [x] **Tier-1 restart timed on the dongle** (scratch binary outside the workspace, never
  `cargo add` into it): three consecutive in-place restarts took **6.1 / 7.6 / 6.2 ms**
  *including* `clear_halt`, each delivering real samples immediately, with centre frequency and
  sample rate intact. The full teardown + re-open + first block on the same device cost
  **1576 ms** — and that is bare driver time, before the engine's probe and reconnect backoff
  turn it into the ~9 s of dead air the M5 field session saw. ~250× cheaper

**HackRF One (serial 230865dc3170af47):**
- [x] Enumerated, opened and streamed through the vendored driver at **20 Msps with zero
  overruns and zero dropped transfers**, peak exactly at the commanded 100.976 MHz
- [x] Per-stage gains verified against the radio rather than the API: sweeping VGA 0 → 62 dB
  moved the measured noise floor **−89.9 → −37.5 dB** (the peak barely moves because a local
  broadcast station compresses the front end, which is physics, not a bug). An off-grid LNA
  request of 13 dB is still snapped to 16 and *reported* as 16, the M5 regression holding
- [x] Amp and bias-tee toggles, and the rate rejection at 25 Msps, all behave
- [x] A live WFM channel on the 101.296 MHz station at 8 Msps ran through the full native
  backend → DDC → demod chain with the set Running, 0 ring overruns and 0 dropped transfers
  (no RDS off air, which is the separate open gap above, unchanged by this work)
- [x] **Tier-1 restart timed on the radio:** three consecutive mode-off/mode-on restarts took
  **0.83 / 1.18 / 0.77 ms**, each delivering real samples immediately, with frequency, rate and
  gains intact; a full re-open cost 31 ms of driver time (plus the engine's ~9 s above it)

**What is still the owner's to run.** The one real regression test for the RTL stall — streaming
while physically touching the antenna — has no harness. Nothing here reproduces a stalled pipe on
demand, so the tier-1 supervisor is proven in three pieces (the policy under unit tests, the
transport's error handling against a scripted mock, and the restart primitive timed on both
radios) rather than end-to-end against a real stall. Every restart timing above measured the
*clean* path: `clear_halt` is now on the restart path, which is the strongest medicine short of
a re-open, but it is unproven against a genuinely halted pipe. Replug recovery is likewise
untested here.

---

## Review follow-up: the abstraction absorbs what the backends were repeating

A review of the vendoring PR found the change had fixed its own thesis in one place and violated
it in another: the transfer-error policy was shared, and then each backend hand-rolled the
supervisor that drives it. The two copies were ~105 identical lines apart from the restart
primitive, the block size and the word in the log line — and had already diverged on a real bug.

**Moved into `crates/device`, the abstract layer, once each:**

| what | was | now |
|---|---|---|
| half-duplex arbitration | private `Direction`/`claim`/`select` in the HackRF driver | `Duplex` + `DuplexState`, pure and unit-tested, for any radio |
| the capture thread and tier-1 supervisor | one copy per native backend | `Capture` + `CaptureRadio`/`CaptureStream`; a backend writes `arm`/`disarm` |
| 8-bit IQ conversion | `IqConverter` twice, identical but for the table | `LutConverter`; a backend supplies 256 floats |
| thread/stop-flag/join plumbing | five copies (rtlsdr, hackrf, siggen, playback, soapy) | `Worker` |
| the bulk-OUT transfer queue | `device-hackrf` only | `crates/usb-stream`, beside bulk-IN, same policy |
| the scripted bulk-OUT endpoint | a second mock in `device-hackrf`'s tests | `usb-stream`'s `test-util` feature |

`crates/device` stays I/O-free by default: the `CaptureStream` impl for the USB transport sits
behind its `usb` feature, so a Soapy-only or virtual-only build still compiles no USB stack.

**The abstraction now carries TX, both ways.** `SdrDevice::duplex()` reports `RxOnly`, `TxOnly`,
`Half` or `Full`; `SdrDevice::tx_start()` hands back a `TxStream`. RX-only backends inherit the
defaults and change nothing. The HackRF declares `Half` and implements both, so its restored
transmit path is reachable through the trait instead of only through its concrete type. Nothing
above the device layer moved: `tx_capable` is false on every backend, no wire type can request a
transmit, and no `engine`/`server`/MCP/UI path calls `tx_start` (PLAN §12a).

**Two lifecycle bugs the split fixed by construction, both found by the review:**

- **`rx_stop` could silence a live transmit burst.** A `TxStream` holds its own handle on the
  radio, so the device could be stopped or dropped while transmitting — and `rx_stop`
  unconditionally switched the transceiver off and cleared "the active direction". Now each
  direction releases only its own claim, and a `Capture` that is not running holds no radio at
  all, so a stray `rx_stop` reaches neither the mode register nor the transmit claim.
- **A stop racing a tier-1 restart could strand the radio armed.** If `rx_stop` ran while the
  capture thread was inside its restart backoff, the thread re-armed the radio, saw the stop flag
  and returned without switching it off — leaving the device permanently `AlreadyStreaming` until
  it was re-opened. The control thread now disarms *after* joining, which no interleaving can
  get behind. Asserted as an invariant over 32 randomly-raced starts and stops.

Neither bug was reachable from the running application (the transmit API is gated, and the race
needed a stop inside a 20–80 ms window on a failing stream), which is why they had not been seen.

**Gates:** `cargo xtask check` green; `cargo xtask test` green with **667 Rust tests**, up from
615 — the new ones are the arbitration rules, the supervisor's restart/teardown behaviour against
a scripted radio, the converter's carry, and the transmit queue split across its two layers. No
test touches USB. Nothing was re-verified on hardware: this change moves code between crates
without altering the register writes, the transfer policy or the restart timings the field
session measured, and the field evidence above stands unamended.

---

## M6 — The UI shell ✅

Goal (PLAN §16): workspaces → tabs → dockview panel layouts, server-persisted (§10); templates
gain layouts. M0–M5 shipped a fixed arrangement, so this is a shell change: every panel it hosts
already existed.

**Status: complete.** `cargo xtask check` + `cargo xtask test` green (76 server tests, 182 web
tests); verified live in a browser against `device-virtual` — layout restore, tab switch, panel
add, reload, and the phone breakpoint (screenshots in the session; the behaviours are listed
under *Verified live* below).

### Wire (single source of truth, PLAN §4)
- [x] `crates/wire/src/workspace.rs` — `PanelKind`, `PanelSpec`/`PanelGroup`, `LayoutNode`
  (`split`/`group`, adjacently tagged like `ChannelParams`), `SplitNode`/`LayoutChild`,
  `FloatingGroup`, `TabSpec`, `WorkspaceSnapshot` + `WorkspaceInfo`/`WorkspaceDetail`/requests,
  `StateScope::Workspaces`
- [x] **The layout tree is ours, not dockview's** — templates author layouts in Rust, and a dock
  major cannot invalidate stored workspaces (PLAN §10 decision)
- [x] Weights in **permille (`u16`)**, not fractions: a float drifts across load→save cycles and
  serializes `null` for NaN, which would fail the *whole* snapshot on the way back in
- [x] Ids, never indices, for "which panel/tab is active" — an index goes stale the moment a
  panel closes
- [x] `#[schema(no_recursion)]` on the `LayoutChild → LayoutNode` back edge; without it utoipa
  recurses forever and `cargo xtask codegen` never finishes
- [x] Pure `WorkspaceSnapshot::validate()` in `wire` (one rejection point, no I/O): version,
  tab/panel caps, nesting depth, degenerate splits, zero weights, duplicate ids, dangling
  `active`, floating geometry outside the dock
- [x] `WorkspaceSnapshot::station_default()` — the M0–M5 arrangement expressed as two tabs, so a
  first run lands on a working station and nothing reachable before M6 became unreachable
- [x] `TemplateInfo.layout` + contract tests for every new tag, default and absent-field case

### Server (`crates/server`)
- [x] Migration 4: `workspaces` (one JSON snapshot per row, like presets — written atomically,
  read whole, never queried by inner field) + a single-row `active_workspace` table, because
  "exactly one is active" belongs in the schema rather than in a per-row flag someone will
  eventually set twice
- [x] `tabs` denormalized onto the row so the switcher never parses a layout: a snapshot this
  build cannot read breaks opening *that* workspace, never the list that lets you leave it
- [x] `active_workspace` is maintained in the delete transaction, not by a foreign key —
  rusqlite leaves `PRAGMA foreign_keys` off, so `ON DELETE SET NULL` would be inert
  documentation and the pointer would dangle
- [x] Deleting the active workspace promotes the lowest-id survivor; deleting the last one
  reports `active: null` honestly instead of 500-ing
- [x] Revision-checked updates: an update carrying a revision the caller no longer holds is a
  409, so an idle browser cannot overwrite the layout someone is arranging
- [x] REST: `GET`/`POST /api/workspaces`, `GET`/`PUT`/`DELETE /api/workspaces/{id}`,
  `POST /api/workspaces/{id}/activate`, all emitting `StateScope::Workspaces`
- [x] Seeding runs on every `Store::open` and only acts on an empty table
- [x] Templates gained layouts: applying one also upserts a `template:<slug>` tab into the active
  workspace and activates it, so re-applying replaces that tab instead of stacking copies. The
  device configuration is what the user asked for, so a workspace that cannot take the tab
  (none active, or another client wrote first) does not turn a successful apply into an error
- [x] Tests: store CRUD + seeding + stale-revision + name-collision + bad-layout rejection;
  handler tests for the whole REST surface and for the template tab (twice-applied, one tab)

### Web (`web/src/shell`, PLAN §10)
- [x] `dockview-react` 7.0.4 — v7 moved the React flavour out of the `dockview` package, which
  is now vanilla; one dependency, styles from `dockview-react/dist/styles/dockview.css`
- [x] `dockLayout.ts` — the two mappers, pure and unit-tested: our tree ⇄ `SerializedDockview`.
  It carries the two shape differences: dockview's grid **alternates axis at every depth** (so
  same-direction nesting is collapsed on the way in, which makes alternation an invariant), and
  it stores **pixels** where we store permille
- [x] Round-trip tests: the default layout unchanged, idempotent across repeated passes (or every
  load would rewrite the layout and fan a state change for nothing), pixel sizes → permille
  shares, floating groups through every corner anchor, same-direction collapse, empty groups and
  degenerate splits dropped, unknown component names ignored, single-group root wrapped (dockview
  refuses a leaf root: *"root must be of type branch"*)
- [x] `WorkspaceDock.tsx` — applies the stored layout, maps every gesture back, and breaks the
  echo loop three ways: writes are suppressed while a layout is being applied, a layout that maps
  back to what is already stored is not written, and an incoming tab equal to the one just
  emitted is not re-applied. Writes debounce to the end of a gesture
- [x] `defaultRenderer: "always"` — panels are never detached, so the waterfall's GL context, the
  map's camera and every scroll position survive a tab switch (a detached element also measures
  0×0, which is how canvas code gets resized to nothing)
- [x] Narrow viewports (< `md`) lay every panel out as one stack and **never persist**: dockview
  clamps panels to their minimum size there, and writing the clamp back would flatten the layout
  a desktop client authored
- [x] `WorkspaceBar.tsx` — workspace switcher, create/remove, tab strip with inline rename, add
  tab, add panel (offering only kinds the tab lacks)
- [x] `panels.tsx` — the `PanelKind → component` registry; `context.tsx` carries the socket, the
  active set and the selection, because dockview serializes panel params into the layout and a
  socket or a setter cannot travel that way
- [x] Selection is scoped to the active device set — channel ids are per set, so the old shared
  selection silently matched a different channel after a set switch
- [x] Instrumentation dock theme from the existing palette via `--dv-*` variables (no fork of
  dockview's stylesheet); square corners, 1px separators, 28px tab strip
- [x] `PanelSection` deleted — a dock tab *is* the header it drew, and its collapse was
  mobile-only

### Fixed where found (CLAUDE.md #4)
- [x] **A failing waterfall took the whole UI down.** `WaterfallRenderer` throws on a missing
  WebGL2 context or a rejected shader, and the throw escaped a dock panel's mount. It is caught
  now: the spectrum line, the controls and every other panel keep working, with the reason shown
  on the panel. Found live — headless Chrome's GL refused the shader
- [x] `WaterfallRenderer::dispose` releases the GL *context* (`WEBGL_lose_context`), not just its
  objects: browsers cap contexts per document (~16), and a dock creates and destroys panels for
  the life of a session — the browser would drop the oldest context, blacking out some *other*
  canvas
- [x] The renderer skips frames at 0×0 instead of burning GPU on a panel nobody can see
- [x] Fixed heights that fight a resizable panel: the decoder log's `max-h-80` table and the
  transcript's `h-48` now fill their panel; the map fills instead of `h-72`

### Verified live (browser, `device-virtual`)
- [x] Seeded workspace restores as two tabs; the Station tab streams spectrum from the signal
  generator through the docked panel
- [x] Tab switch, add panel and a dock tab click each persist exactly one revision and then
  **settle** — no echo loop between the save and the `StateChanged` it triggers
- [x] Reload is a fixed point: the restored layout produces no write
- [x] At phone width every panel becomes one stack, nothing is persisted, and returning to
  desktop width restores the split layout with the revision untouched
- [x] Two bugs this found and fixed: a single-group layout was refused by dockview (leaf root),
  and the layout listener closed over the narrow-mode flag captured at mount — a dock first
  opened on a phone kept discarding writes after the viewport widened

### Gates
- [x] `cargo fmt` + `cargo clippy -D warnings` clean; Soapy-free and release-shaped builds green
- [x] `cargo xtask test` green; `biome ci` + `oxlint` (type-aware) + `tsgo` + web build clean
- [x] OpenAPI regenerated, TS aliases added, zero drift

### Known gaps (honest, not deferred silently)
- **Per-channel decoder panels.** One `Decoders` panel renders every decoder channel of the
  active set, as the fixed layout did. A panel *per channel* needs channel identity that survives
  a restart, which the engine does not have — the same reason panels carry no device-set binding.
- **Panels do not pin a radio.** With two receivers open, every panel follows the one selected in
  the device bar. Pinning needs the stable `driver:key` a preset uses, plus UI to choose it.
- **Floating windows hosting more than one group** are dropped by the reverse mapper rather than
  half-placed; dockview writes those as a nested grid the wire model does not describe. A single
  floating group round-trips. **Popout windows** are not persisted at all — an OS window position
  is per-machine state, and the plan does not name popouts.
- **A layout written by a newer build is refused, not migrated.** `WorkspaceSnapshot.version`
  gates the write; `PresetSnapshot` has had the same unfilled promise since M3, and neither has a
  migration path yet.
- **A stored snapshot naming an unknown `PanelKind` fails the whole workspace open** (loudly —
  the row is never rewritten without it). Only reachable by downgrading the binary or editing the
  database by hand, since the UI ships inside the server.
- **One spectrum panel at a time.** `SdrSocket.onSpectrum` is a single handler, so two spectrum
  panels in one tab would fight; the add-panel menu only offers kinds the tab lacks, which makes
  it unreachable rather than fixed.
- **No Playwright smoke flow** still (PLAN §14): the live verification above was manual, and the
  mappers carry the automated coverage.

### Post-review hardening (adversarial multi-agent review: 12 findings, 5 refuted, 7 confirmed & fixed)
- [x] **The dock was rebuilt after every gesture** (high). `TabSpec.floating` is
  `skip_serializing_if` on the wire, so a tab with no floating groups comes back *without* the
  key — while the mapper always emitted `floating: []`. The echo-suppression comparison could
  therefore never succeed: one round trip after every drag, the whole dock was torn down and
  rebuilt, which is exactly what `renderer: "always"` exists to prevent. The mapper now emits the
  canonical wire shape (as it already did for `active` and `title`), with a test that asserts the
  key is absent on both sides
- [x] **`dispose()` poisoned the canvas it was about to reuse** (high). Losing a WebGL context
  poisons the *canvas*, not the context object: `getContext` keeps returning the dead one, and
  every later shader compile fails with an empty log. React StrictMode runs mount→unmount→mount
  on the same canvas, so the release meant the waterfall never came back — which is what the
  live check had blamed on headless GL. The release is now deferred to the next macrotask and
  skipped unless the canvas has actually left the document
- [x] **The 0×0 render guard could never fire** (low). It tested `canvas.width`, which
  `resizeToDisplay` deliberately leaves at its last non-zero value; the *display* size is what
  goes to zero
- [x] **A pending layout write was dropped when the dock unmounted** (medium). Switching tab or
  workspace inside the 400 ms debounce lost the rearrangement. The layout is now mapped when the
  gesture lands rather than when the timer fires, so the unmount can flush it — by unmount time
  React has already disposed the dock and its `toJSON` describes nothing
- [x] **Two writes inside one round trip fought over the revision** (medium). Both carried the
  revision they were rendered with, so the server — correctly — refused the second as stale and
  the change was lost. Writes are now serialized and expressed as *edits of the current
  snapshot*: each reads the revision the previous produced (folded in from the write's own
  response), and no caller can build on a snapshot it merely happened to be rendering
- [x] **The debounced write did not re-check the mode** (medium). Crossing the narrow breakpoint
  mid-debounce would have persisted the phone-flattened layout
- [x] **`create_workspace` did not bound the name** while `update_workspace` did — one shared
  `validate_name` now guards both, with the rejection tested
- [x] Fixed while verifying live: the waterfall canvas needed `min-h-0`. A canvas has an
  intrinsic size from its backing store and a flex item defaults to `min-height: auto`, so it
  refused to shrink below the height it was last given and overflowed its dock panel
- Refuted after verification (recorded so they are not re-found): the template-tab apply
  swallowing non-conflict store errors (the doc comment scopes exactly the two cases it
  handles), a cross-process seeding race (the two binaries cannot share a database, and a
  restart re-reads a non-empty table), unbounded floating `x_frac`/`y_frac` (the only producer
  clamps, and dockview re-clamps on restore so the row self-heals), and a redundant assertion in
  a wire test

---

## The design pass — `DESIGN.md`, the top bar, the dial and the plot ✅

PLAN §10 makes a design pass part of every UI milestone's definition of done, and M6 shipped the
dock without one. Five rows of chrome stood between the operator and the plot; the plot itself
could only be looked at; and the "large, digit-scrollable frequency readout" the plan names was a
label. This closes that, and writes the rules down so the next pass has something to be checked
against.

### The rulebook
- [x] **`DESIGN.md` is binding, like `CLAUDE.md`.** An OKLCH role table with both themes derived
  from it (never a second hand-picked hex set), the type scale, the spacing and separation
  ladders, the density floor, the motion budget, the keyboard map, and the spectrum's gesture
  contract — all in numbers a reviewer can check. It also names what this pass deliberately does
  not do
- [x] **A palette that is a choice, not a default.** Warm graphite surfaces and one lamp-amber
  accent — the anodized front panel the plan's "pro audio and lab equipment" reference points to
  — replacing teal on blue-black, which is the generic dark-dashboard look. Contrast measured for
  every role in both themes (dark: ink 14.7:1, ink-dim 7.0, ink-faint 4.7, accent 9.8; light:
  13.2 / 6.0 / 4.6 / 4.2), and every accent gamut-checked in sRGB
- [x] **The plot never inverts and its overlays are achromatic.** Inside the plot rectangle the
  colormap owns hue, so cursors, markers and axis text separate by luminance and shape alone — an
  isoluminant cursor over a colormap is invisible and a coloured one is a conjunction search. The
  plot therefore has its own theme-independent token set
- [x] **The light theme PLAN §10 promised**, as the same role table re-anchored: surfaces to
  L .92–1.0, ink to L .27–.53, accents darkened and de-chromatised. Three states (auto / dark /
  light) in `localStorage` — a theme belongs to the eye looking at the screen, not to the station,
  so unlike workspaces it does not sync between clients

### Two rows of chrome instead of five
- [x] `TopBar.tsx` — the radio: dial, tune step, receiver popover, capture, link state. Nothing
  that changes what you are *looking* at
- [x] `TabBar.tsx` — the view: workspace menu, tab strip, add-panel, theme, shortcuts. Nothing
  that changes what the radio is doing
- [x] The device-settings strip became `RadioSettings` inside the receiver popover; the first-run
  banner became `OpenRadio`, the spectrum's empty state (where someone with no radio open is
  already looking, and costing nothing when one is); the workspace name/add/remove row became a
  menu; the template gallery became the Channels panel's empty state
- [x] **Errors are toasts, not a banner.** A banner that appears and disappears at the top of the
  shell moves every panel under it, which is layout instability the operator did not cause. One
  shared stack (`lib/toasts.ts`), one surface, dismissible and auto-expiring; five panels' private
  "Rejected:" banners and `useDevicePatch`'s private error store all fold into it
- [x] `useChannelPatch` extracted — three surfaces now edit channels (the panel, a dragged marker,
  the keyboard) and all three go through one optimistic pipeline instead of the panel's private copy

### The dial (PLAN §10's digit-scrollable readout)
- [x] Ten place-value targets: wheel over a digit, ←/→ between them, ↑/↓ and PageUp/PageDown to
  step, type digits over them, Enter or `f` for direct entry (`145.5`, `433800k`, `2.4g`). One
  tab stop, roving focus, clamped to the device's range
- [x] The arithmetic is `dial.ts` — 15 tests over place widths, leading-zero dimming, stepping,
  digit writes, clamping and free-text parsing. The component only routes events

### The plot answers gestures
- [x] Frequency and dB axes on the 1-2-5 nice-number ladder, drawn from the frame's own metadata,
  refining as the view zooms
- [x] Wheel zoom about the cursor as a fixed point, drag to pan (4px slop before a click becomes a
  drag), click to tune the selected channel (or the radio when none is selected), double-click to
  re-centre, marker drag to move a channel, and zoom buttons that appear only on coarse pointers,
  which have no wheel
- [x] The view transform is `spectrumView.ts` — 16 tests including the fixed point surviving a
  chain of wheel notches, edge clamping that does not change the zoom level mid-pan, and tick
  refinement
- [x] A draggable trace/waterfall split, max-hold, five colormaps (magma, inferno, plasma,
  viridis, gray — all monotone in luminance; jet and its relatives are excluded on purpose), and
  a per-column peak decimation so a narrow carrier is never sampled away
- [x] **The waterfall advanced one row per backing-store pixel**, so on a 2× display every second
  arriving row was dropped and the history took twice as long to reach the bottom. It is one row
  per CSS pixel now, which is also what makes the scroll rate the frame rate

### Keyboard (PLAN §10 "keyboard-first")
- [x] Tune, tune step, mode, squelch, audio, channel and tab switching all bound, with the `?`
  overlay rendering the same table the handler switches on — a shortcut nobody can find is not a
  feature. Handlers stand down for text fields, for the dial (which claims the arrows itself), and
  for the activation keys of a focused button

### Verified live (browser, `device-virtual`)
- [x] Wheel zoom holds the cursor's frequency still and refines the axis; the reset control
  appears only once there is something to reset
- [x] A click on a marker selects it and leaves its offset alone; a click on empty plot at 72%
  moved the channel to exactly +528 kHz of a 2.4 MHz span
- [x] `←` tuned one step, `]` walked the step ladder, `?` opened the overlay and `Esc` closed it;
  Space over a focused button still presses the button
- [x] Dark, light and auto themes; the phone breakpoint at 390×844 with no horizontal overflow

### Gates
- [x] `cargo xtask check` green (fmt, clippy `-D warnings`, Soapy-free and native-driver builds,
  `biome ci`, type-aware `oxlint`, `tsgo`, web build, zero codegen drift)
- [x] `cargo xtask test` green — 610 Rust tests, 210 web tests (31 of them new)

### Known gaps (honest, not deferred silently)
- **Pinch-zoom is not implemented.** A touch pointer pans by drag and zooms with the two buttons
  the toolbar shows on coarse pointers; a two-finger pinch does nothing. It needs multi-pointer
  tracking that cannot be verified in this environment
- **Zoom magnifies, it does not resolve.** The server streams a fixed span, so zooming re-frames
  bins that have already arrived. The readout discloses it by reporting the *visible* span rather
  than the device's; a server-side zoom would be a protocol change
- **One spectrum panel at a time**, unchanged from M6: `SdrSocket.onSpectrum` is still a single
  handler, and the add-panel menu only offers kinds the tab lacks, which keeps it unreachable
  rather than fixed
- **Per-channel decoder panels and radio-pinned panels** stay out, for the same reason M6 left
  them out: both need identity that survives an engine restart, which is a server change
- **The band-plan explorer** (PLAN §8a) is still open. The dial and the plot are built so it can
  hang off them without rework
- **Still no Playwright smoke flow** (PLAN §14). The pure transforms — dial arithmetic, view
  transform, axis ticks — carry unit tests; the composition above was verified by hand

---

## Decoders wave 2 — NAVTEX, ACARS, sub-GHz ✅

The three decoder rows PLAN §13 Phase 2 still had open, minus WEFAX (which is a transport
problem, not a DSP one — see below). Each is one module in `channels`, one settings struct and
one event in `wire`, one reference modulator in `channels::testgen`, and one React view. The
registry, the "add channel" menu, the decoder log, the CSV/JSON export and the MCP tools all
picked them up without a line of per-decoder plumbing — which was the point of §8.

### NAVTEX / SITOR-B (`crates/channels/src/navtex.rs`, PLAN §13 P2)
100 baud FSK at a 170 Hz shift carrying CCIR 476 with mode-B time diversity (ITU-R M.540,
M.625). Three layers over the discriminator:

- **The alphabet detects.** CCIR 476 is a constant-ratio code — exactly four of seven bits are
  mark — so a corrupted character is recognisable without a checksum. The chart is stored as
  *CCIR code → ITA2 code*, so the alphabet itself stays defined once, in `rtty`, and NAVTEX
  inherits the LTRS/FIGS tables RTTY already proves. A test transcribes all 35 code points from
  the standard, checks the four-of-seven property across the whole 128-entry space, and asserts
  the ITA2 side is a bijection onto codes 1–31 — a mistyped constant fails, it does not vanish.
- **The diversity corrects.** Every character is sent twice, five character periods apart. The
  decoder decides at the repeat slot, where both copies are in hand: the RX copy if it is legal,
  else the DX copy, else the two soft-value sets summed and re-sliced — a character neither copy
  carries on its own. `BitSync` gained `push_soft` for exactly that (the hard `push` is now a
  one-line wrapper, and a test pins the two to the same slicing instants).
- **The framing selects.** Only text between `ZCZC B1B2B3B4` and `NNNN` is emitted. A station
  idles for minutes; logging everything sliced would bury the messages. A broadcast cut short is
  emitted with `complete: false` rather than dropped.

Phasing lock needs the REP/ALPHA pattern twice at exactly the slot spacing — one hit on noise is
not proof at 100 baud. The B2 subject table is transcribed against ITU-R M.540 with the
unassigned letters explicitly left unnamed.

### ACARS (`crates/channels/src/acars.rs`, PLAN §13 P2)
MSK at 2400 bit/s amplitude-modulated onto a VHF carrier, ARINC 618 block framing. The data
rides on the carrier's amplitude, so the magnitude *is* the audio: envelope → DC block → mix the
1200/2400 Hz pair down about its 1800 Hz centre and decimate by 2 → discriminator → one-symbol
matched filter → bit clock → byte framing. At h = 0.5 the bit is carried by frequency alone, so
no phase reference is needed, and a mirrored spectrum costs nothing — the sync character is
recognised in both polarities and every later bit is corrected on the way in (`acarsdec`'s
trick; a test decodes a conjugated transmission to the same message).

Validation is strict and repairs nothing: odd parity on every character *and* the CRC-16 over
the block, or it is dropped. An ACARS message is free text, and a plausible-but-wrong one is
worse than a missing one. `dsp::fec` gained `crc16_ccitt` (reflected 0x1021, init 0, the
KERMIT parameters) beside `crc16_x25`, both now sharing one register loop.

Fields follow the standard's own names — mode, registration, ack, label, block id, and on
downlinks the sequence number and flight number that prefix the text. An uplink carries neither,
and a decoder that skipped that check would eat ten characters out of every uplink's text; that
is its own test.

### Sub-GHz OOK/FSK (`crates/channels/src/subghz.rs`, PLAN §8b, §13 P2)
The Flipper "read Sub-GHz" experience. Two front ends produce the same keyed stream — OOK
through an adaptive slicer on the envelope, FSK through a discriminator sliced against a tracked
level and gated by the same carrier detector — and everything above is shared: debounced edge
timing, a base-period estimate, and a classifier.

- **250 kHz channel, 150 kHz wide by default.** These transmitters are SAW-controlled and sit
  tens of kHz off nominal; a filter narrow enough to look correct would not hear them. `dsp`
  gained `flat_bandwidth_hz` next to `resamplable_bandwidth_hz` for this: the alias limit is the
  wider number, but a decoder measuring pulse widths off an envelope needs the *flat* band.
- **No chip is named.** An EV1527's 24 data bits and a PT2262's 12 tri-state symbols are the
  same pulse train, so `encoding` says `pwm` and a 24-bit frame carries both readings — address
  and button, plus the tri-state string when every bit pair is a symbol a PT2262 can emit.
  Manchester is recognised too; anything else comes back as raw edge timings, which is what
  makes an unknown signal something you can still look at.
- **Repeats collapse.** Every one of these devices transmits its payload several times per
  press. Copies inside 500 ms become one event with a count, and a better-classified frame
  supersedes a held one *only* while that one is still a single sighting — which is what stops a
  capture that started mid-burst from logging its fragment.

`KeyingSlicer` gained a `KeyingTiming` value (`MORSE` / `BURST`) instead of a second copy of
itself: Morse elements are tens of milliseconds, remote symbols are hundreds of microseconds,
and the trackers' time constants were the only difference. `one_pole_coeff` moved from a private
`dsp` helper plus a copy in `pocsag` to one public primitive.

### Wire (single source of truth, PLAN §4)
`NavtexParams` / `AcarsParams` / `SubghzParams` and `NavtexMessage` / `AcarsMessage` /
`SubghzFrame`, plus `SubghzModulation` and `SubghzEncoding`. The typecheck did the routing work:
adding three `DecoderEvent` variants failed the build in `channelSettings.ts`, `decoderLog.ts`,
`decoded.ts` and `ChannelsPanel.tsx` until each had a label, a summary, a station rule and a
settings form — no hand-written TS anywhere.

### Tests
- **Channel unit tests, 221 in `channels`** — round trips, figures shifts, a 120 ms burst that
  wipes one NAVTEX copy and is repaired from the other, a conjugated ACARS transmission, a
  corrupted ACARS block dropped at four different offsets, a sub-GHz capture started mid-burst,
  additive noise through the real channel filter, pure noise decoding to nothing, ragged block
  splits matching one-shot decoding exactly, and retunes dropping what was in flight.
- **Engine end-to-end** (`crates/engine/tests/decode.rs`): three new cases through
  `device-virtual`, each at an offset so the DDC mixes and decimates. Sub-GHz is now the widest
  channel in the suite — 250 kHz out of a 500 kHz device while still timing 320 µs edges.
- **Fixtures**: `navtex_518_48k`, `acars_downlink_240k`, `subghz_ev1527_500k`, from the same
  modulators, so a fixture can never drift from what the decoders are tested against.
- **Web**: 11 new vitest cases over the view projections and the summary/station mirrors
  that have to read identically to `DecoderEvent::summary` in the same table.

### Verified live (browser, `device-virtual`)
- [x] All three fixtures played through the running server: the decoder log filled with
  `DA07 · Navigational warning · GALE WARNING GERMAN BIGHT`,
  `D-AIBC · LH0400 · [H1] · SDR-- FIXTURE` and `24 bit 0A1B23 · addr 0A1B2 · btn 3 · ×5`
- [x] The NAVTEX pane renders header, subject name and body; the sub-GHz pane renders
  modulation · encoding · payload · base period · repeat count with the address/button reading
  under it
- [x] Settings forms: NAVTEX's invert, ACARS's bandwidth, sub-GHz's modulation / bandwidth /
  min-pulse / frame-gap fields

### Gates
- [x] `cargo xtask check` green (fmt, clippy `-D warnings`, Soapy-free and native-driver builds,
  `biome ci`, type-aware `oxlint`, `tsgo`, web build, zero codegen drift)
- [x] `cargo xtask test` green — 744 Rust tests, 226 web tests

### Known gaps (honest, not deferred silently)
- **WEFAX is not built, and not for DSP reasons.** A fax page is an image; PLAN §5's binary frame
  kinds are spectrum, audio and IQ, and a decoder-log row is typed JSON. Shipping it needs an
  `IMAGE` frame kind, a server-side page store and a canvas panel — a transport decision that
  deserves its own change rather than a base64 blob smuggled into the log. Recorded in PLAN §13
  and §18
- **No off-air proof for any of the three.** Every fixture is synthesized, which proves the
  decoders against the *specification* and not against the world (PLAN §14). NAVTEX needs an HF
  receiver at 518 kHz, ACARS a VHF session near an airport, sub-GHz a remote to press — all
  three are bench sessions the owner can run against a release build, and none has happened yet
- **ACARS repairs nothing.** `acarsdec` recovers blocks with up to three parity errors using a
  syndrome table. Ours drops them. That trades sensitivity on a weak signal for never printing a
  wrong message; the syndrome table is the obvious follow-up if field sessions show it matters
- **Sub-GHz rolling codes are read, not analyzed.** A KeeLoq-style hopping remote decodes as a
  66-bit PWM frame with no structure attached. Rolling-code *analysis* is TX-phase work behind
  the §12a gate
- **No sub-GHz protocol library.** rtl_433 knows hundreds of device-specific framings; we
  classify the encoding and hand back the bits. The next step is a table of known payload
  layouts (weather stations, TPMS), which is data, not DSP
- **NAVTEX needs the header to be received.** A receiver that joins mid-broadcast keeps the text
  but reports no station or serial, and the message is marked incomplete when the carrier drops
