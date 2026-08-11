# sdr-- — Feature List

Everything sdr-- does, will do, or has deliberately decided not to do — collapsed into one
list. Three states only: **[shipped]** works today, **[planned]** is intended, **[skipped]**
is a deliberate no. Within each section, shipped comes first.

---

## 1. Platform & deployment

- **[shipped]** Client–server split — Rust server does all DSP/decoding, React client renders
- **[shipped]** Server is authoritative — every client (browser, desktop window, MCP agent) sees the same state and converges over one WebSocket
- **[shipped]** One frontend, two hosts — served by the server for browser access, and bundled into the Tauri v2 desktop shell
- **[shipped]** Desktop app spawns an embedded server on an ephemeral loopback port (loopback-only, unauthenticated by design)
- **[shipped]** Release archives for Linux (x86_64 + aarch64), macOS (arm64 + x86_64) and Windows (x86_64), a multi-arch ghcr.io image, and desktop installers (`.dmg`, `.deb`, `.AppImage`, `.msi`, `.exe`), all built by a tag-triggered workflow
- **[shipped]** Release artifacts just run — `xtask dist` produces a ~25 MB binary linking only IOKit/CoreFoundation/libiconv/libSystem: no libusb, no libSoapySDR, no libopus, no libsqlite
- **[shipped]** One version, one place — `[workspace.package] version` is the only copy; `tauri.conf.json` omits `version` so Tauri inherits it, `xtask dist` names archives from it, and the release workflow stamps it from the git tag with `xtask set-version`. Each built artifact is then run and asserted to report the version it is named after
- **[shipped]** The Tauri shell and the Dockerfile are pull-request gates (`xtask desktop`, plus an image build that boots the container and asserts the UI is really embedded) — both used to be built for the first time on release day, since `apps/desktop` sits outside the workspace's `default-members`
- **[shipped]** Pull requests are Linux-only by design; macOS tests run on `main` and on tags, where a platform break is still caught before it ships
- **[shipped]** `sdrmm --doctor` and `GET /api/doctor` — compiled backends, devices found, Linux udev/USB permissions with the fix, database and recordings-path writability, one shared report so CLI and UI cannot disagree
- **[shipped]** mdBook docs site + Pages deploy
- **[shipped]** RustSec advisories are a CI gate (`xtask audit`, policy in `deny.toml`) covering the whole graph, Tauri shell included. It runs as its own job because a new advisory lands on RustSec's schedule, not on a pull request's. Standing exception: `RUSTSEC-2024-0429` (`glib` `VariantStrIter` unsoundness), unreachable here and unfixable below gtk4 — see `deny.toml`
- **[planned]** Desktop app connecting to a *remote* server, and saved remote connections — the shell only ever spawns its own local one
- **[shipped]** Signed and notarised macOS bundles — a Developer ID Application certificate and the App Store notary service, driven by six repository secrets the release workflow passes through to Tauri
- **[planned]** A verified Raspberry Pi run — the Pi 4 is the stated performance floor and no field session has been on one
- **[skipped]** Mobile/phone layouts — every mobile path was deleted with the M6 shell; pointer, keyboard and laptop-class viewport are assumed everywhere

## 2. Device support

- **[shipped]** RTL-SDR — in-tree driver (RTL2832U registers, I²C bridge, R82xx tuner) over the shared pure-Rust USB transport. Verified on a Nooelec NESDR SMArt v5: enumerate, tune, rate, 29-step gain snapping, tuner AGC, bias-T, 45 s at 2.048 and 2.4 MS/s under 16 spinning threads with zero overruns and zero dropped transfers
- **[shipped]** HackRF One — in-tree driver, both directions. Verified at 20 Msps with zero overruns, per-stage LNA (8 dB) / VGA (2 dB) gains checked against the radio's own noise floor rather than the API, amp and bias-T, off-grid gain requests snapped *and reported* at the snapped value
- **[shipped]** PPM frequency correction — both halves (resampler registers and the tuner's crystal re-tune), verified against a real carrier at ±200 ppm
- **[shipped]** SoapySDR backend as optional extra coverage — airspy, airspyhf, bladeRF 1/2, FUNcube Pro(+), Fobos, LimeSDR, Perseus, PlutoSDR, SDRplay (v3), USRP, XTRX, Aaronia RTSA. The contract is tested against fabricated capability data; no radio but RTL-SDR and HackRF has been attached
- **[shipped]** Virtual devices — signal generator (tones, drifting sweep, noise, phase-continuous NFM/AM/WFM test carriers), file playback, SigMF file playback
- **[shipped]** Auto-rendered device UI — frequency ranges, discrete or continuous sample rates, per-stage gains, antennas, bandwidths and typed extra settings all come from the capability model; a new setting needs zero frontend code
- **[shipped]** Native drivers rank above Soapy in the serial merge; duplicates collapse by serial
- **[shipped]** Hotplug detection by filtered re-enumeration + engine probe cross-check, with the fault path releasing the device so a replug can re-open it
- **[shipped]** Auto-reconnect on replug — a faulted set whose radio re-enumerates is re-opened, its tuning re-applied and its channels rebuilt with ids, PCM identity and live audio subscriptions preserved
- **[shipped]** Two-tier recovery — an in-place stream restart (measured 6.1–7.6 ms on the RTL-SDR, 0.8–1.2 ms on the HackRF, against ~1.6 s for a re-open) with a silent-stall detector on both radios, falling back to the engine's destructive fault path only when the restart budget is spent. Proven in three pieces (policy, transport, primitive); never yet driven by a genuinely halted pipe
- **[shipped]** Soapy-free builds are a CI gate (`--no-default-features --features rtl-native,hackrf-native`)
- **[planned]** Direct sampling (HF via RTL-SDR)
- **[planned]** HackRF independent baseband-filter bandwidth and hardware sweep mode
- **[planned]** rtl_tcp / SpyServer client device
- **[planned]** KiwiSDR client device
- **[planned]** Remote source/sink between sdr-- instances; local routing between device sets
- **[planned]** Audio-input device (`cpal`) — soundcard as a receiver
- **[planned]** rtl_tcp *server* (remote TCP sink)
- **[skipped]** Android SDR driver input

## 3. Many radios at once & coherent arrays

- **[shipped]** Unlimited simultaneous device sets, each with its own DSP thread, channels and recorder; both radios have been run together on the bench
- **[shipped]** Spatial identity for them — a device node names `backend + serial` (with a key tie-break only where a backend exposes no serial), and an absent radio is a visibly disconnected node, never a silent rebind
- **[planned]** Cross-device features: a scanner spanning devices, multi-VOR fix, diversity
- **[planned]** `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output
- **[planned]** KrakenSDR support — via its Heimdall DAQ network stream first, direct hardware drive later
- **[planned]** Generic synced bank — any N receivers on a shared reference clock
- **[planned]** Network coherent source — aligned multi-lane IQ from another sdr-- node or a DAQ
- **[planned]** Direction finding (MUSIC/ESPRIT) with bearings on the map; multi-station triangulation
- **[planned]** Passive radar (range-Doppler)
- **[planned]** Beamforming and diversity combine
- **[planned]** Interferometer
- **[planned]** TDoA geolocation across distributed sdr-- nodes

## 4. Spectrum, tuning & navigation

- **[shipped]** Live spectrum + waterfall (WebGL2, one shared context for every scope face, off-screen views skipped, zoom-adjusted DPR), per-connection throttling
- **[shipped]** Several scope faces at once — spectrum subscriptions are refcounted per device set and the socket fans to a listener set
- **[shipped]** Plot gestures — wheel zoom about the cursor as a fixed point, drag to pan, click to tune, double-click to re-centre, marker drag to move a channel, frequency and dB axes on a 1-2-5 ladder that refines as you zoom
- **[shipped]** Max-hold, a draggable trace/waterfall split, and five luminance-monotone colormaps (magma, inferno, plasma, viridis, gray)
- **[shipped]** Digit-scrollable frequency dial — ten place-value targets, wheel/arrows/typing/direct entry (`145.5`, `433800k`, `2.4g`), clamped to the radio's range
- **[shipped]** Keyboard-first operation — tune, tune step, mode, squelch, audio, channel and view switching, with a `?` overlay rendering the same table the handler switches on
- **[shipped]** Frequency manager: presets and bookmarks
- **[shipped]** Frequency scanner — targets grouped into passband-sized tunings so one dwell measures every target in the passband, peak-hold over the dwell, post-retune settle and drain, hold-and-resume that parks a channel on the hit, and exclusive ownership of the set's centre frequency while it runs. Swept 88–108 MHz (201 targets) on both radios and held on real stations
- **[planned]** Hardware-assisted wideband sweep — today's scanner sweeps by retuning; the HackRF's own sweep mode is not driven
- **[planned]** Strongest-signal "close-call" finder
- **[planned]** Signal-strength **hunt mode** — Geiger-style audio/visual feedback as you close on a transmitter
- **[planned]** Percentile-anchored waterfall colour range — the range is the frame's own min…max today, so a high noise floor washes the display out
- **[planned]** Server-side zoom — zooming re-frames bins that already arrived rather than resolving finer; the readout is honest about it
- **[planned]** Pinch-zoom on touch pointers
- **[planned]** 3D spectrogram view
- **[planned]** Band occupancy analytics over time

## 5. Frequency-allocation database — "what is this frequency?"

Nothing here is built; the dial and the plot were built so it can hang off them without rework.

- **[planned]** Band-plan / allocation layer overlaid on the spectrum and searchable
- **[planned]** Layered scopes, most-specific-wins: **World** (ITU Regions 1/2/3 + global services) → **Germany** (BNetzA Frequenzplan) → **US** (FCC), **UK** (Ofcom), EU CEPT and more as pluggable importers
- **[planned]** Region chosen in settings or auto-selected from GPS
- **[planned]** Band ruler with colored allocation blocks; click-to-identify popover (service, allocation, suggested mode, channel step, notes)
- **[planned]** Searchable band explorer ("show me marine VHF", "70 cm ham")
- **[planned]** One-click "tune here with the suggested mode"
- **[planned]** Amateur band plans (IARU R1) overlay
- **[planned]** User-extendable and override-able entries; re-runnable importers with per-row provenance
- **[planned]** Community overlays, "band plan of the day"

## 6. Recording, capture & replay

- **[shipped]** Device-level SigMF v1.2.6 recorder — lossless DSP-thread tap, crash-safe breadcrumb-then-atomic-finalize lifecycle, atomic stem claiming, sample-count-exact `start_sample`, centre retunes recorded as capture segments, ring overruns counted into the status
- **[shipped]** Recordings finalized on device fault, set removal and process exit; a writer fault surfaces as a hard error instead of a silent drop
- **[shipped]** Recordings browser — rate, duration, size, guarded delete, and Play as the ordinary device-open flow (a finalized recording probes as a device, so replay needed no new endpoints)
- **[shipped]** Files on disk are the source of truth; the SQLite index reconciles against them, serialized against delete and stop
- **[shipped]** Decoder log persisted server-side — indexed, composable filters (kind, set, time window, free text, limit), batched writer with a retry queue and periodic prune, and lag/overflow reported as a visible `dropped` count
- **[shipped]** Decoder log export as a real CSV/JSON download with RFC4180 quoting
- **[shipped]** Recorder as a node face, gated on its receiver running, with a live elapsed/size/overruns readout
- **[planned]** Per-channel sinks — audio recording, baseband file, UDP out to external tools
- **[planned]** RF replay-capture workflow — record a burst, annotate it, analyze it
- **[planned]** **IQ time machine** — rolling ring buffer, retro-record the last N seconds after the fact
- **[planned]** Inspectrum-style offline IQ viewer in the browser
- **[planned]** Annotated recordings; recording scheduler + unattended satellite-pass automation
- **[planned]** Wideband recording + offline re-channelization
- **[planned]** Session/replay sharing as one openable bundle

## 7. UI, workspaces & onboarding

- **[shipped]** Patch-graph canvas — every radio, channel, scope, map, speaker, log, recorder, export and scanner is a node; wiring is the UI; the palette and its ports are served from the server's catalog so a new node kind needs no frontend edit
- **[shipped]** Node faces sized to their instrument, opened framed (`fitView`), active-on-click so the wheel tunes the dial *or* pans the patch but never both
- **[shipped]** Pin-board rack (12×8) — pin a face, drag a boundary and neighbours give up exactly what it takes, drop a face on another and they trade places
- **[shipped]** Right-click menu: pin, reset size, cut a wire, fit the patch
- **[shipped]** Workspaces that apply **additively and idempotently** — loading a workspace opens the radios it names and creates the channels it draws, never closing or deleting anyone else's work; what it cannot satisfy is reported (`absent`, `refused` with the engine's reason) rather than skipped
- **[shipped]** Revision-checked workspace writes with serialized edits, so an idle browser cannot overwrite the layout someone is arranging
- **[shipped]** The canvas refuses invalid wiring where it is drawn — an ADS-B wire onto a 2.4 Msps receiver names the rate that works, using the same rule the engine enforces rather than a second copy of it
- **[shipped]** A radio's left side is what is done *to* it — `control` in (the scanner owns the tuning, one wire), `tx` in (reserved, inert, refuses every wire with the server's own reason), `iq` out
- **[shipped]** Template gallery — eight built-in workspaces (FM·RDS, airband, ADS-B, AIS, APRS, POCSAG, 2 m, marine VHF) with explainers, each validated to fit its own passband, re-applying replaces rather than stacks
- **[shipped]** Library drawer for the things that are not nodes — presets, bookmarks, templates, recordings — scoped to the radios this patch binds
- **[shipped]** Generic schema-rendered settings forms for anything without a dedicated face; decoder output renders on the channel's own face
- **[shipped]** MapLibre map (OpenFreeMap, no API key) with a themed fallback, plotting only the decoders wired into it, GeoJSON updated on a throttled tick
- **[shipped]** `DESIGN.md` as a binding rulebook — OKLCH role table, contrast measured in both themes, achromatic plot overlays, type/spacing/density/motion ladders
- **[shipped]** Dark, light and auto themes (per browser, not synced — a theme belongs to the eye, not the workspace)
- **[shipped]** Errors as a dismissible toast stack rather than a banner that shoves every panel down
- **[shipped]** Playwright smoke flow (`xtask smoke`) driving the built UI against a real server
- **[shipped]** Channel settings surviving a restart — apply recreates channels at their type's defaults, so offsets and squelch come back neutral unless a preset carries them
- **[planned]** A first-run wizard — the canvas has no guided first run
- **[planned]** Band-plan explorer (§5)
- **[planned]** Node kinds whose backends do not exist yet: GPS source, UDP sink, WAV sink, and the `iq-tap`/`position` port types that go with them
- **[planned]** A scope on a channel tap — a scope only takes a device today
- **[planned]** Theme/skin system and a layout marketplace
- **[planned]** Localization (DE/EN first)

## 8. Voice & analog channels

- **[shipped]** AM, NFM, SSB (USB/LSB), WFM mono — DDC → mode-aware complex channel filter → squelch → demod → 48 kHz PCM
- **[shipped]** Squelch (power + hysteresis + hold, measured on the filtered channel so a threshold means the same thing across modes), AGC, de-emphasis, DC blocking
- **[shipped]** Mode changed in place on a live channel, keeping audio subscribers
- **[shipped]** RDS — 19 kHz pilot PLL → 3rd harmonic → symbol sync → differential decode → offset-word block sync; groups 0A/0B (PS, TP/TA/MS, AF), 2A/2B (RadioText), PTY; emitted only when a field changes. **It decodes the synthesized fixture completely and has never decoded off air** — six real stations on two radios produced nothing, reproduced deterministically from a committed-out 8 s capture; the leading (untested) hypothesis is the stereo L−R subcarrier sitting against 57 kHz
- **[planned]** WFM **stereo** — the audio path becomes two-channel end to end (PCM, Opus, frame layout, worklet)
- **[planned]** ATV (analog TV)
- **[planned]** Notch and audio filters per channel
- **[shipped]** CTCSS/DCS on NFM — the subaudible band decimated off the discriminator, a bank of 50 sliding correlators (half-second window, because the closest pair of standard tones is 2.3 Hz apart) and a DCS reader: Golay(23,12) at 134.4 bit/s, sliced against a tracked baseline so a carrier offset is not a decision threshold. Detect names what a repeater uses without gating; CTCSS and DCS gate on it, muting rather than skipping so the client's jitter buffer keeps its samples, and a 300 Hz highpass keeps the tone out of the audio it lets through. **The 83 standard DCS codes are part of the decoder, not a dropdown**: the code is cyclic, so a sliding window finds a valid word at all 23 alignments, and only that set reads back unambiguously — which is also why an inverted transmission comes out as the code's inverse-pair partner (023 ↔ 047) instead of needing a polarity switch
- **[planned]** Selcall (CCIR/ZVEI)

## 9. Digital voice

- **[planned]** DMR, D-Star, YSF, NXDN, P25, dPMR
- **[planned]** M17, FreeDV
- **[planned]** Trunking following — P25 / DMR Tier III control channel decode with auto-steered voice channels
- **[planned]** Hardware AMBE dongle/server support

## 10. Aviation & marine

- **[shipped]** ADS-B + map — level-relative preamble correlation, Mode S CRC-24 with single-bit repair, identification, airborne/surface CPR position, velocity, Gillham and 25 ft altitude, bounded per-ICAO CPR cache
- **[shipped]** Mode S beyond the extended squitter — DF11 all-call replies, DF4/20 altitude and DF5/21 identity (squawk) replies, plus the BDS 2,0 callsign a Comm-B reply may carry. A roll-call reply keys its address onto the parity and so proves nothing by itself: it is decoded only when that address was proved in the clear (DF11/17/18) within the last minute, and single-bit repair is confined to the bare-parity formats where it cannot invent a different aircraft
- **[shipped]** ADS-B at **any receiver rate 2–4 MHz** — the decoder meets the radio instead of the radio meeting the decoder: per-chip half-chip boundaries, eight sub-sample phase tables arbitrated by the CRC, and overlap-weighted energy per half-chip. Measured 0% → 100% at 2.048 Msps off-grid and band-limited (98% at 34 dB SNR); 2.000 Msps keeps a physical half-sample blind spot that real 2.048 receivers do not have
- **[shipped]** AIS + map — GMSK via discriminator and Gaussian matched filter, NRZI + HDLC + CRC-16/X-25, types 1/2/3/5/18/24, `!AIVDM` output
- **[shipped]** ACARS — MSK on an AM carrier, mirrored-spectrum tolerant, strict validation: character parity *and* the ARINC 618 CRC both pass or the block is dropped, uplink/downlink field layouts distinguished
- **[shipped]** NAVTEX / SITOR-B — CCIR 476 constant-ratio alphabet as a code→ITA2 map over the RTTY tables, mode-B time diversity repairing a character neither copy carries alone, `ZCZC`…`NNNN` framing so idle phasing never reaches the log
- **[planned]** VOR, VOR localizer (multi-VOR fix), ILS, DSC
- **[planned]** Inmarsat STD-C / AERO
- **[planned]** VDL Mode 2; HFDL; Iridium bursts
- **[planned]** ADS-B / AIS log enrichment against offline aircraft and ship databases
- **[planned]** **Off-air proof for all four shipped decoders** — every one is verified against its specification via a reference modulator (independently written, or the mode's own transmitter where it has one), and none has yet decoded a real signal

## 11. Data, text & paging

- **[shipped]** POCSAG — per-candidate-rate bit clocks where the rate that finds frame sync takes the lock, BCH corrections counted, numeric and alphanumeric bodies, 512/1200/2400 detected per transmission
- **[shipped]** RTTY — ITA2 with LTRS/FIGS and unshift-on-space, start/stop framing with stop-bit rejection, 45.45/50/75 baud, 170/450/850 Hz shifts
- **[shipped]** Morse — envelope + adaptive keying slicer, element/gap clustering that tracks sending speed, unknown sequences surface as `*` rather than vanishing, pure noise decodes to nothing
- **[shipped]** APRS / AX.25 — AFSK1200 and 9600 G3RUH, SSIDs and the has-been-repeated flag, TNC2 line, uncompressed and base-91 compressed positions, course/speed, `/A=` altitude
- **[shipped]** Mic-E — the one APRS form that is not a text format: six latitude digits and three indicator bits unpacked from the destination *callsign*, longitude/course/speed/symbol from an information field offset by 28, all 15 message codes named (and the standard/custom mixture the spec itself refuses to name), position ambiguity carried from the latitude into the longitude, telemetry told apart from status text, and the base-91 `xxx}` altitude
- **[planned]** APRS *feature* — station/position collection, distinct from the channel
- **[planned]** FLEX and further pager formats, ERMES
- **[planned]** CW skimmer — every CW signal in the passband at once
- **[planned]** Tetrapol, STANAG modem ID, GSM downlink analysis, OsmocomBB-style monitoring
- **[planned]** Off-air proof — as above, all four are specification-proven only

## 12. Sub-GHz, ISM & IoT

- **[shipped]** **Generic sub-GHz OOK/ASK/FSK capture-and-decode channel** — 250 kHz channel 150 kHz flat by default (these transmitters sit tens of kHz off nominal), OOK through an adaptive envelope slicer and FSK through a discriminator against a tracked level, then shared debounced edge timing, base-period estimation and classification. Pulse-width and Manchester recognized; unknown signals come back as raw edge timings you can still look at
- **[shipped]** No chip is named — a 24-bit frame carries both the EV1527 reading (address + button) and the PT2262 tri-state string where every bit pair is a legal symbol
- **[shipped]** Repeats inside 500 ms collapse into one counted event, and a better-classified frame supersedes a held one only while that one is a single sighting — which is what stops a capture that started mid-burst from logging its fragment
- **[planned]** Rolling-code *analysis* — a KeeLoq-style remote decodes today as a structureless 66-bit PWM frame; analysis is gated TX-phase work (§20)
- **[planned]** Protocol library — the encoding is classified and the bits handed back; a table of known payload layouts (weather stations, TPMS, meters) is data work, not DSP
- **[planned]** ISM sensor suite (rtl_433-style); escape hatch is a UDP sink into the rtl_433 binary
- **[planned]** ChirpChat / LoRa, Meshtastic, MeshCore
- **[planned]** End-of-Train (EOT) telemetry
- **[planned]** LoRaWAN frame parsing
- **[planned]** BLE advertisements, 2.4 GHz survey, Wi-Fi channel occupancy (energy only)
- **[planned]** Off-air proof — never yet tested against a real remote

## 13. Weather, satellite & imaging

- **[planned]** NOAA APT; Meteor M-2 LRPT
- **[planned]** Radiosonde (RS41 …) + map/log feature; later DFM, M10/M20, iMet
- **[planned]** HF WEFAX — the DSP is the easy half; it needs an `IMAGE` binary frame kind, a server-side page store and a canvas panel first, which is a transport decision and not a decoder
- **[planned]** SSTV RX; APRS weather aggregation

## 14. Broadcast & wideband digital

- **[planned]** DAB / DAB+
- **[planned]** DATV (DVB-S / S2)
- **[planned]** TETRA
- **[planned]** DRM30 / DRM+

## 15. Amateur & weak-signal

- **[planned]** FT8 / FT4
- **[planned]** PSK31 / PSK63; WSPR
- **[planned]** Radio clock (DCF77 / WWVB / MSF / JJY)

## 16. Analysis & measurement

- **[planned]** Channel analyzer (scope, constellation) — also the prerequisite for wiring a scope to a channel tap
- **[planned]** Demod analyzer; channel power meter; heat map channel
- **[planned]** Noise figure; PER tester; SID monitor
- **[planned]** Radio astronomy; star tracker; sky map
- **[planned]** Signal-ID assistant — match a spectrum/audio snapshot against a signal catalog, later an ML classifier
- **[planned]** GNSS educational decode

## 17. Audio processing

- **[shipped]** Opus audio to the browser and desktop — per-channel encoder threads, WebCodecs fast path with a WASM fallback, AudioWorklet jitter buffer (100 ms target, underrun rebuffer, 400 ms drop-oldest), per-channel gain, gesture-unlocked context, auto-resubscribe on reconnect, timestamp-gap loss detection
- **[shipped]** Speaker node — client-side mixing across every channel wired into it
- **[planned]** Spectral noise reduction, noise blanker, auto-notch, AGC as advanced processing inside **every** voice channel rather than a separate channel type
- **[planned]** Adaptive/auto DSP — auto-notch, auto-squelch, auto-gain, per-mode click and noise removal

## 18. Station services & hardware integration

- **[planned]** Satellite tracker (TLE fetch, pass prediction, Doppler-corrected channels)
- **[planned]** Rotator control (GS-232, rotctld); rigctld-compatible rig control server
- **[planned]** GPS position source (gpsd / NMEA) — station position for maps and trackers, geotagged mobile heat map, auto grid locator, geotagged recordings
- **[planned]** NanoVNA over USB serial — sweeps, SWR and Smith-chart panels, saved antenna profiles
- **[planned]** Antenna calculators
- **[planned]** Map layers — sondes, satellites, beacons, MUF
- **[planned]** TinySA import, bias-T presets, Hamlib CAT control

## 19. API, automation & access

- **[shipped]** REST control API with OpenAPI schema, generated typed client, and a codegen-drift gate — no hand-written frontend DTOs anywhere
- **[shipped]** One WebSocket: JSON state events plus binary spectrum, audio and decoder frames, with drop-oldest backpressure, per-connection throttling, and lag surfaced as a typed loss count instead of a silent gap
- **[shipped]** **MCP server** at `/mcp` — `rmcp` streamable HTTP, stateless, 13 tools over the same engine calls REST uses (state, devices, channel types, open/close, tune, add/remove channel, scan, record, decoder-log query, spectrum snapshot), with channel settings built through the wire enum so no parallel settings model exists
- **[shipped]** Optional shared token over REST, WS and MCP alike — Bearer header *or* `?token=` (the browser WS API cannot set headers), constant-time comparison, one middleware, and auth/OpenAPI/docs endpoints public because they describe the API's shape and never its data
- **[shipped]** Default LAN-trusted posture (bind `0.0.0.0`, no auth), same-origin CORS
- **[shipped]** Multi-client polish — connection count surfaced in the UI, decoder frames serialized once for the whole server, WS reconnect backoff 1 s → 30 s, a 401 forgetting a stale token and asking again
- **[shipped]** Config via `config.toml`, flags and env (`SDRMM_TOKEN` so a token need not appear in the process list)
- **[planned]** Scripting recipes on the existing REST + MCP surface (scanner bots, "ping me when this callsign appears")
- **[planned]** Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- **[planned]** Plugin SDK via WASM
- **[planned]** Multi-user roles; remote fleet management across several Pi nodes
- **[planned]** Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps
- **[skipped]** TLS termination — reverse-proxy or VPN it

## 20. Transmit & RF security research

A general-purpose, legitimate transmit and RF-research toolkit, behind a default-off
"controlled RF environment / authorized test" gate. Test instruments for *contained, authorized*
assessment — direct-connect, dummy load or shielded — against devices you are authorized to
test. No presets exist whose purpose is uncontrolled over-the-air disruption of third parties.

- **[shipped]** The device abstraction carries TX both ways — `Duplex` (`RxOnly`/`TxOnly`/`Half`/`Full`) and `tx_start` → `TxStream`; RX-only backends inherit the defaults and change nothing
- **[shipped]** The HackRF's transmit path — bulk-OUT queue of 16 on the shared transport, the firmware's zero-filled end-of-burst marker, transmit VGA control, half-duplex arbitration in both directions, and a transfer policy that deliberately never re-sends a failed transmit transfer
- **[shipped]** It is unreachable from outside the device layer, by construction — transmit VGA written to 0 dB on open, no wire type through which a client could ask, no `engine`/`server`/MCP/UI caller. `Capabilities.duplex` now states what the hardware *has* (a HackRF is `half`), which is what draws its reserved transmit input; the port emits nothing and accepts nothing. No node kind emits the `tx` port type, so no edge into it can validate, and a test fails the day one does
- **[shipped]** `ChannelTx`, the transmit half of the channel surface — payload in on the control plane, IQ out on the hot path, raised-cosine burst edges so keying does not splatter, a bounded queue that refuses a backlog instead of growing, and a short fill as the "burst is over" signal. Deliberately not the same trait as `ChannelRx`: the two directions share a mode's constants and (next) its framing, not their state. A modulator sits in the same registry row as the demodulator it pairs with, and `can_transmit` on the wire is *derived* from whether that column is filled, so the flag a UI would draw a port from cannot disagree with what `create_tx` will build. Reaching an antenna is still gated exactly as before — nothing in `engine` or `server` calls it, and the samples land in a buffer
- **[shipped]** NFM is the first mode with a modulator, round-tripped against its own demodulator in test at both channel spacings. Neither end carries pre- or de-emphasis, which is what makes the pair agree
- **[shipped]** AM, SSB and APRS / AX.25 followed, each round-tripped against its own demodulator: AM keys an 80 %-depth envelope normalized so the peak, not the carrier, is full scale; SSB is a phasing exciter (Hilbert transformer) against a receiver that filters one side and takes the real part, so the two share a passband and no code; AX.25 owns the framing in both directions — addresses, stuffing, CRC-16/X.25, NRZI and the G3RUH scrambler on the way out, `dsp`'s deframer on the way back — and keys either Bell 202 AFSK1200 or 9600 baud FSK. Queue bound and burst envelope are shared by all four
- **[shipped]** A mode with a modulator no longer has a reference encoder in `testgen`: the transmitter *is* the reference, and the fixture library and the end-to-end run key the mode's own transmitter instead of a stand-in — at its channel rate, resampled to whatever the device replays at. Receive-only modes keep their independently-written generators
- **[planned]** The authorized-use gate itself, and everything below it
- **[planned]** Signal generator / arbitrary waveform + IQ playback-to-air
- **[planned]** Modulators for the remaining modes, over the shared frame/bit codec each protocol module owns in both directions — for two-way, beacon and test use on licensed bands
- **[planned]** Sub-GHz capture → decode → replay; fixed-code analysis and generation including de Bruijn sequences; rolling-code capture and implementation analysis against your own DUT
- **[planned]** Interference / jam-susceptibility testing into a contained link
- **[planned]** Flood / spam / malformed-broadcast testing at a DUT over a contained link
- **[planned]** Targeted protocol fuzzing
- **[planned]** Bench loopback — TX into your own RX to validate decoders (note: this is the point at which the graph's no-cycle proof stops being sufficient)
- **[planned]** Offline frame workbench — dissect, mutate, re-analyze captured frames; encoding identification
- **[planned]** Simple PTT
- **[planned]** Beam-steering CW modulator (TX MIMO)

## 21. Cross-cutting engine capabilities

- **[shipped]** Any number of channels per device set, each with its own DDC, settings and face; hot path holds no locks and allocates nothing in steady state, settings arriving by command queue between blocks
- **[shipped]** Channels that need the radio's own rate get its samples mixed to their offset and nothing else, with the rule (`exact_rate_only`, derived once) shipped on the wire so the UI and the engine cannot disagree
- **[shipped]** A device rate change rebuilds every hosted channel with ids and audio subscriptions preserved; recording blocks a rate change rather than mixing rates under one SigMF header
- **[shipped]** Typed decoder events leave the DSP plane through a bounded sink with counted drops, are wall-clock stamped off the hot path, and are persisted by the server rather than the engine
- **[shipped]** Squelch feeds decoders duration-exact silence instead of splicing the stream — a decoder measures its bit clock in the time a skip would delete
- **[shipped]** Golden-vector tests on every DSP primitive, and a fixture library regenerated rather than committed. A receive-only decoder is tested against a reference modulator written independently of it; one whose mode ships a transmitter is tested against that transmitter, which shares the mode's constants but implements none of the same steps
- **[planned]** GPU spectrum path (wgpu) for very large FFTs or many channels
- **[planned]** Diversity combine / noise cancelling with a reference antenna
