# `sdrmm-modem` catalog

One row per modulation entry: the §5 definition-of-done bundle as a table (MODEM-PLAN §1.2 —
no modulation without a measurement; §5 item 8 — a missing row is a CI failure). The grouping
below mirrors MODEM-PLAN §6 exactly, one section per engine family, so later phases fill rows
in place instead of restructuring the file.

## Columns

- **Entry** — the modulation, named as the harness knows it where a runner exists
  (`cargo xtask ber <entry>`).
- **Parameters** — the parameter ranges / configuration the committed measurements were taken
  at (§5 item 8: "parameter ranges").
- **Tiers** — detection tiers implemented; the first listed is the one that gated the entry,
  later tiers are separate merges measured against it (§5 item 2). Planned tiers on pending
  rows are prefixed `planned:`.
- **References / fixtures** — correctness references per §4.1: the closed-form oracle or the
  committed-and-guarded curve, golden vectors, recorded fixtures.
- **Limits table** — the committed §4.3 resistance table.
- **Perf baseline** — the committed §4.2 performance baseline (with the row named where the
  file is shared between entries).
- **Consumers** — repo consumers per §6; "—" means harness-and-TX-only, which changes nothing
  about the required bundle.
- **Status** — `measured` (bundle complete for the tiers listed), `provisional` (committed,
  scheduled to be superseded), `pending (phase N)` (committed scope, no code yet).

## The docs-row rule

`cargo xtask check` enforces this file against the tree in both directions: every file under
`crates/modem/baselines/` and `crates/channels/baselines/` must be named here, and every
baseline path named here must exist on disk. Artifact paths are therefore written
workspace-relative (`crates/…`), exactly as committed — the checker resolves them from the
workspace root. An artifact no row names is a measurement nobody can find, and it stops
gating anything the moment it is forgotten; a row pointing at a deleted artifact is a claim
with nothing behind it.

## Harness calibration (phase 0)

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| Ideal BPSK (`bpsk-ideal`) | RRC α=0.35 span 8, 8 sps, genie timing/carrier, uncoded BER | coherent matched filter | exact ½·erfc(√γ) oracle (`ber::theory`); committed curve `crates/modem/baselines/bpsk_ideal_awgn.json` | — (calibration entry: its AWGN sweep *is* the sensitivity axis; §4.3 tables start with the engines) | `crates/modem/baselines/perf_phase0.json` (`symbol_sync_8sps` row) | the harness itself — the 0.2 dB calibration gate every later measurement inherits (§4.1); `cargo xtask ber bpsk-ideal` | measured |

## Pre-migration protocol baselines (phase 0)

Committed floors the phase-3 migrations must beat (§7). Rows here characterise the *current*
channel chains, not library entries; each moves into its engine's section when the channel
migrates, and its artifacts become the migration's regression gate.

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| DMR phase-0 chain (`Fsk4Demod`) | 48 kHz, 4800 baud, 4FSK ±1944 Hz outer deviation, RRC α=0.2 span 8 | discriminator + slicer, sync-anchored levels, TDMA gating | steady curve `crates/channels/baselines/dmr/dmr_steady_uncoded.json`; burst-model curve `crates/channels/baselines/dmr/dmr_burst_uncoded.json`; phase-1 soft-BPTC measurement on the same chain — coded curves `crates/channels/baselines/dmr/dmr_bptc_hard.json` and `crates/channels/baselines/dmr/dmr_bptc_soft.json`, headline gain `crates/channels/baselines/dmr/dmr_bptc_gain.json` (1.60 dB at BER 1e-3) | `crates/channels/baselines/dmr/dmr_limits.json` — every row carries a documented §4.3 override criterion (the chain's continuous-mode timing floor sits above the 1e-3 default) | `crates/modem/baselines/perf_phase0.json` (`fsk4_dmr_48k` row) | none (front end deleted in phase 3) | historical — the phase-0 pre-migration reference the §7 migration rule is measured against; artifacts never regenerated. The migrated chain's row is “DMR attachment” under the CPM engine, and `crates/channels/tests/dmr_baseline.rs` / `dmr_soft_gain.rs` hold the two generations' artifacts to the §7 rule (within 0.5 dB, limits no worse, gain kept) |

## Shared infrastructure (not catalog entries)

The §5 bundle applies to modulations; the substrate below the engines is measured through
the entries that consume it, not row by row.

- **`pulse/`** (phase 2) — the one place a pulse shape is defined: rect/LREC/LRC, raised
  cosine and root-raised cosine, the Gaussian premod filter and the GMSK frequency pulse
  (rect ⊗ Gaussian per Murota & Hirade), half-sine, plus the CPM phase-pulse helper
  (q(∞) = ½, so a unit-area frequency pulse steps the carrier phase by exactly π·h — the
  phase-3 engine contract). Every constructor takes an explicit normalisation
  (`Norm::Energy`, Σh² = 1, for amplitude pulses and matched filters; `Norm::Area`, Σh = 1,
  for level-preserving filters and CPM frequency pulses); the wrapped `sdrmm_dsp` designs
  (`design_rrc`, `design_gaussian`) pass through bit-identical under `Norm::Area`, pinned by
  test, as is the TX-RRC ⊗ RX-RRC Nyquist-ISI cascade. Consumers today: the live AIS and
  D-STAR matched filters, all C4FM/GMSK `testgen` shaping, and `ber::reference`'s calibrated
  BPSK link; the phase-3 engines draw from it next. No baseline artifacts of its own — its
  acceptance tests are exact identities, and its measurements arrive through every entry
  shaped by it.

## CPM / FSK engine (`cpm/`)

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| M-ary CPFSK, M ∈ {2,4,8,…,64} (`mfsk`) | M, h or deviation set, mapping table, pulse, known-symbol hook — measured at 48 kHz / 4800 baud / 10 sps: M=2 h=½ rect (the POCSAG/RTTY base); M=4 ETSI dibit table ±1944 Hz (h=0.27) RRC α=0.2 (the DMR-shaped reference configuration); M=8 natural map h=0.3 rect with the §3.4 known-symbol hook as level reference (blind normalisation's measured M ≤ 4 boundary); timing bandwidth per entry (0.003 continuous / 0.015 burst), per-entry channel-selection lowpass stated in the curve labels | discriminator+slicer; planned: MLSE (phase-3 follow-on, gated by its own measured gain) | M=2: noncoherent orthogonal 2-FSK oracle (`ber::theory::mfsk_noncoherent_ser`) + documented chain offset +1.29 dB at BER 1e-3 (≈1.0 dB of it is the stated framing overhead charged to Eb), gated within ±0.4 dB; M=4 and M=8: committed-and-guarded. Curves `crates/modem/baselines/cpm/mfsk2_cpfsk_awgn.json`, `crates/modem/baselines/cpm/mfsk4_cpfsk_awgn.json`, `crates/modem/baselines/cpm/mfsk8_cpfsk_awgn.json`; smoke prefix guards, full regeneration gates and level-1 E2E loopbacks at stated margins in `crates/modem/tests/mfsk_cpfsk.rs` (chains in `tests/mfsk_common/`) | `crates/modem/baselines/cpm/mfsk4_limits.json` — the *default* §4.3 criterion (the phase-0 DMR chain needed a documented 3e-2 override; this engine's continuous 1e-3/1e-4 sensitivities exist: 13.66 / 18.03 dB, 1e-2 at 10.18 dB vs the old chain's 16.89), with CFO/drift/clock/timing axes, the three §4.3 burst-survival rows on the TDMA chain, and the static-indoor composite profile | `crates/modem/baselines/cpm/mfsk_perf.json` (`cpm_demod_m4_48k`: 24.4 Msamples/s, ≈508× real time at 48 kHz — parity with the phase-0 `fsk4_dmr_48k` row); steady-state zero-alloc asserted in `cpm::demod` tests for both input domains | receivers *and* testgen: dv/* (all seven), pocsag; testgen/TX side only: rtty, navtex, subghz — their receivers stay off `CpmDemod` deliberately (asynchronous start-stop framing, a constant-ratio alphabet's static symbol-mean bias, and per-frame measured symbol rates; reasons in those modules and in PLAN §7's phase-3 follow-ons) | measured (discriminator+slicer tier; M ∈ {2,4,8} bundles committed) |
| DMR attachment (4-level CPFSK, RRC, TDMA) | 48 kHz, 4800 baud, ETSI dibit table (TS 102 361-1 §4.2.2), ±1944 Hz outer deviation → h = 0.27, RRC α=0.2 span 8 as frequency pulse and matched filter, burst timing operating point, known-symbol hook on the 48-bit syncs | discriminator + slicer, burst gate + `KnownSymbols` | steady curve `crates/channels/baselines/dmr/dmr_steady_uncoded_cpm.json`; burst-model curve `crates/channels/baselines/dmr/dmr_burst_uncoded_cpm.json`; soft-BPTC coded curves `crates/channels/baselines/dmr/dmr_bptc_hard_cpm.json` and `crates/channels/baselines/dmr/dmr_bptc_soft_cpm.json`, headline gain `crates/channels/baselines/dmr/dmr_bptc_gain_cpm.json` (1.34 dB at BER 1e-3; phase 0 measured 1.60 dB, the two within the crossings' ~±0.25 dB intervals); recorded fixture `decodes_a_recorded_call`; level-2 E2E, continuous-floor guard and phase-0 delta gates in `crates/channels/tests/dmr_baseline.rs` and `crates/channels/tests/dmr_soft_gain.rs` | `crates/channels/baselines/dmr/dmr_limits_cpm.json` — same axes and documented criterion as the phase-0 table, row-for-row comparable (sensitivity 1e-2 improved 16.89 → 14.55 dB; CFO/drift/burst-shortening better, clock/timing/dead-time unchanged, level step 7.31 → 6.75 dB inside the 20% tolerance) | `crates/modem/baselines/cpm/mfsk_perf.json` (`cpm_demod_m4_48k` row — the DMR-shaped reference configuration) | `dv/dmr` (shared dv 4FSK front end: `dv::c4fm_params` / `dv::c4fm_demod`) | measured (phase 3) |
| GMSK / GFSK (partial response) (`gmsk`) | BT ∈ {0.3, 0.5} (Bluetooth BR/D-STAR's 0.5; 3GPP TS 45.004's 0.3), h = ½, `gaussian_freq` pulse (L = 3 at BT 0.5, L = 4 at BT 0.3) — measured at 48 kHz / 4800 baud / 10 sps behind a ±6 kHz channel-selection lowpass shared verbatim with the MSK row so the BT comparison reads the pulse alone; receive filter is measured per-BT data: pulse-matched at 0.5, a BT-0.5 Gaussian at 0.3 (the matched filter's 3-symbol ISI closes the eye — six candidates measured, see the test's `gmsk_rx` docs); burst timing operating point (0.015) | discriminator + slicer; planned: coherent-as-OQPSK; MLSE (the measured route to BT = 0.3's ~7 dB ISI penalty) | committed-and-guarded curves `crates/modem/baselines/cpm/gmsk_bt05_awgn.json` (1e-3 at 13.96 dB) and `crates/modem/baselines/cpm/gmsk_bt03_awgn.json` (1e-3 at 20.95 dB); sanity vs MSK gated: BT 0.5 costs +1.34 dB at 1e-3 (partial response stays cheap; a hard-slicing discriminator pays the eye closure the coherent textbook number does not); smoke prefix guards, full regeneration gates and +6 dB-margin level-1 E2E (both BTs) in `crates/modem/tests/gmsk_msk_afsk_bundles.rs` | `crates/modem/baselines/cpm/gmsk_limits.json` at BT 0.5, default §4.3 criterion — CFO 1500 Hz, drift 8750 Hz/s, sample clock 19 922 ppm, timing bracket-bound, plus the §4.3 burst-survival rows on an AIS-shaped 152/104-symbol TDMA chain with per-burst `KnownSymbols` anchoring: dead time bracket-bound at 1024 symbols, minimum burst 24-sync + 53 payload symbols (75 of 128 removable), 8.1 dB alternate-burst level step | `crates/modem/baselines/cpm/gmsk_perf.json` (`gmsk_bt05_demod`: 103.6 Msamples/s, ≈2158× real time at 48 kHz) | dstar, ais (phase-3 migrations) | measured (discriminator tier, BT ∈ {0.3, 0.5} bundles committed) |
| MSK (`msk`) | The LREC(1) h = ½ case: rect frequency pulse, integrate-and-dump receive filter (its matched filter), 48 kHz / 4800 baud / 10 sps behind the same ±6 kHz lowpass as the GMSK row, burst timing operating point; the half-sine amplitude pulse is this waveform's linear-OQPSK reading and belongs to the planned coherent tier | discriminator + slicer; planned: coherent (as OQPSK); differential | committed-and-guarded curve `crates/modem/baselines/cpm/msk_awgn.json` (1e-3 at 12.62 dB, 1e-4 at 13.95 dB) — the reference the GMSK BT 0.5 sanity gate compares against; smoke prefix guard, full regeneration gate and +6 dB-margin level-1 E2E in `crates/modem/tests/gmsk_msk_afsk_bundles.rs` | `crates/modem/baselines/cpm/msk_limits.json`, default §4.3 criterion — CFO 2273 Hz, drift 13 438 Hz/s, sample clock 19 922 ppm, timing bracket-bound | `crates/modem/baselines/cpm/msk_perf.json` (`msk_demod`: 131.8 Msamples/s, ≈2747× real time at 48 kHz) | acars (real audio through `CpmDemod::real`, analytic discriminator about the 1800 Hz subcarrier — the same LREC(1) h = ½ data at 2400 baud) | measured (discriminator tier) |
| Audio-domain FSK / AFSK (`afsk`) | Bell-202-like: mark 1200 Hz = bit 1, space 2200 Hz = bit 0 (the inverted mapping table carries the assignment — mark sits *below* the 1700 Hz centre), 1200 baud, real audio at 12 kHz / 10 sps through `CpmDemod::real`; both `RealDetector` options measured with per-detector receive filters (half-symbol rect behind the 1.2-symbol tone correlators; full-symbol rect behind the discriminator) | tone filterbank (tier-1 by measurement: 2.1 dB ahead at 1e-3 — the correlators integrate exactly the tone split the discriminator's click noise smears); analytic-signal discriminator | committed-and-guarded curves `crates/modem/baselines/cpm/afsk_filterbank_awgn.json` (1e-3 at 13.42 dB) and `crates/modem/baselines/cpm/afsk_discriminator_awgn.json` (1e-3 at 15.53 dB), detector ordering gated; smoke prefix guards, full regeneration gates and +6 dB-margin level-1 E2E through both detectors in `crates/modem/tests/gmsk_msk_afsk_bundles.rs` | `crates/modem/baselines/cpm/afsk_limits.json` on the filterbank tier, default §4.3 criterion — CFO 332 Hz (audio-tone shift; the tones are only 1000 Hz apart), drift 453 Hz/s, sample clock 19 531 ppm, timing bracket-bound | `crates/modem/baselines/cpm/afsk_perf.json` (`afsk_filterbank_12k`: 76.3 Msamples/s, ≈6362× real time at 12 kHz) | acars (on the MSK row's parameterisation, real-input discriminator tier). **aprs is measured but not landed**: the Bell-202 entry data is complete and its loopback is 42/42 green with centre tracking frozen, but the engine's centre tracker is fixed policy, and HDLC/NRZI's legal 192-symbol biased preamble outlasts it — the migration waits on a per-entry centre-tracking axis (PLAN §7 phase-3 follow-on) | measured (both real detectors; filterbank is tier 1) |
| Multi-h CPM / SOQPSK | h sequence, pulse | planned: MLSE | — | — | — | — | pending (phase 3 follow-on) |

## Linear engine (`linear/` + `constellation/`)

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| M-PAM / M-ASK / OOK | M, pulse | planned: coherent; noncoherent envelope + adaptive threshold | — | — | — | morse, subghz | pending (phase 4) |
| BPSK / QPSK / M-PSK | M, mapping, pulse | planned: coherent (Costas/FLL/DD/pilot); differential | — | — | — | rds | pending (phase 4) |
| DPSK family (DBPSK/DQPSK/8DPSK) | M | planned: differential | — | — | — | — | pending (phase 4) |
| OQPSK / π/2-BPSK | offset handling | planned: coherent | — | — | — | — | pending (phase 4) |
| π/4-DQPSK | RRC α | planned: differential first; coherent second | — | — | — | tetra (new) | pending (phase 4) |
| Square QAM 16/64/256/1024 | M, Gray map | planned: coherent, pilot- or decision-directed | — | — | — | — | pending (phase 4) |
| Cross QAM 32/128 | point table | planned: coherent | — | — | — | — | pending (phase 4) |
| Star-QAM | rings | planned: differential; coherent | — | — | — | — | pending (phase 4) |
| Non-uniform / hierarchical QAM | spacing table | planned: coherent | — | — | — | — | pending (phase 4) |
| APSK 16/32 | ring radii/counts | planned: coherent | — | — | — | — | pending (phase 4) |

## Orthogonal & pulse-position (`orthogonal/`, `ppm/`)

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| Noncoherent M-FSK | M, tone spacing | planned: filterbank energies → LLR | — | — | — | — | pending (phase 5) |
| M-PPM | slots, slot width | planned: matched filter + argmax | — | — | — | adsb | pending (phase 5) |

## Analog (`analog/`)

Analog rows use SINAD/THD vs input SNR in place of BER references (§5 item 4).

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| AM / DSB-SC | — | planned: envelope; synchronous | — | — | — | am | pending (phase 8) |
| SSB (USB/LSB) | — | planned: Weaver; Hilbert | — | — | — | ssb | pending (phase 8) |
| VSB | AM engine + filter config | planned: (configuration, not a new type) | — | — | — | atv | pending (phase 8) |
| FM (narrow/wide) / PM | — | planned: discriminator; PLL | — | — | — | nfm, wfm | pending (phase 8) |

## Symbol codes (`symbolcode/`)

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| NRZ / NRZI / Manchester / bi-phase / differential | NRZI transition-on-zero (AX.25/HDLC/USB) and -on-one (FDDI); Manchester IEEE 802.3 and G. E. Thomas; bi-phase Mark/FM1 (AES3, S/PDIF, LTC) and Space/FM0 (EPC Gen2); differential binary XOR and M-ary symbol-index (the DPSK/π/4-DQPSK rule), any M ≥ 2 | codec round-trip (no detection tiers of their own); bit-pair slice decoders report per-bit coding violations and a half-bit alignment verdict | round-trip, slip-detection and corruption-flagging tests in `crates/modem/src/symbolcode/`; bit-exact cross-validation against `sdrmm_dsp::bits` (NRZI, differential, Manchester) on seeded random streams — the phase-3+ call-site swap contract | — (codes, not modulations: resistance is measured through the engine carrying them) | — | subghz, testgen, framings; dsp call sites swap in phase 3+ | measured (phase 2; no baseline artifacts — the acceptance tests are exact round-trips, not curves) |

## Frameworks (`ofdm/`, `multicarrier/`, `spread/`)

| Entry | Parameters | Tiers | References / fixtures | Limits table | Perf baseline | Consumers | Status |
|---|---|---|---|---|---|---|---|
| CP-OFDM | FFT size, CP, pilot pattern, subcarrier map | planned: preamble autocorr → CFO → channel est → one-tap EQ → demap | — | — | — | 802.11 (new) | pending (phase 6) |
| DMT | Hermitian flag on CP-OFDM | planned: same engine | — | — | — | — | pending (phase 6) |
| DSSS | PN sequence, chip rate | planned: correlator; processing gain vs 10·log₁₀(N) | — | — | — | 802.11b (new) | pending (phase 7) |
| CCK | codebooks | planned: codeword correlator bank | — | — | — | 802.11b (new) | pending (phase 7) |
| CSS (chirp) | SF, BW, chirp map | planned: dechirp + FFT peak | — | — | — | lora (new) | pending (phase 7) |
| FHSS | hop sequencer + underlying entry | planned: framework over any catalog entry | — | — | — | — | pending (phase 7) |
| OTFS / FBMC / GFDM / UFMC | per waveform | planned: per waveform | — | — | — | — | pending (phase 9) |

## Protocol attachments planned against the catalog

Attachments are parameters + framing on a library entry (§1); they earn level-2 (and level-3
when captured) E2E on top of the entry's bundle. The phase-3 CPM migrations move the existing
channels here row by row.

| Attachment | Library entry | Status |
|---|---|---|
| TETRA downlink | π/4-DQPSK | pending (phase 4, staged: burst sync and MAC first) |
| BLE 1M/2M advertising | GMSK/GFSK | pending (entry lands phase 3; attachment unscheduled) |
| 802.11a/g | CP-OFDM | pending (phase 6, staged: SIGNAL at BPSK-1/2 first) |
| 802.11b | DSSS + CCK | pending (phase 7) |
| LoRa RX | CSS | pending (phase 7, preamble/header first) |
| Bluetooth EDR modulation | DPSK family (+ FHSS if hop-following is pursued) | pending (entries land phases 4/7; attachment unscheduled) |
