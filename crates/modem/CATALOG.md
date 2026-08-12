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
| DMR current chain (`Fsk4Demod`) | 48 kHz, 4800 baud, 4FSK ±1944 Hz outer deviation, RRC α=0.2 span 8 | discriminator + slicer, sync-anchored levels, TDMA gating | steady curve `crates/channels/baselines/dmr/dmr_steady_uncoded.json`; burst-model curve `crates/channels/baselines/dmr/dmr_burst_uncoded.json`; level-2 E2E + smoke gates in `crates/channels/tests/dmr_baseline.rs`; recorded fixture `decodes_a_recorded_call`; phase-1 soft-BPTC gate on the same chain — coded curves `crates/channels/baselines/dmr/dmr_bptc_hard.json` and `crates/channels/baselines/dmr/dmr_bptc_soft.json` (post-FEC BER over accepted frames, FER, undetected-error rate, each its own labelled curve), headline gain at BER 1e-3 in `crates/channels/baselines/dmr/dmr_bptc_gain.json`, measured and guarded by `crates/channels/tests/dmr_soft_gain.rs` | `crates/channels/baselines/dmr/dmr_limits.json` — every row carries a documented §4.3 override criterion (the chain's continuous-mode timing floor sits above the 1e-3 default; see the test's module docs) | `crates/modem/baselines/perf_phase0.json` (`fsk4_dmr_48k` row) | `dv/dmr` | provisional — pre-migration reference, superseded when phase 3 migrates DMR onto `cpm/` |

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
| M-ary CPFSK, M ∈ {2,4,8,…,64} | M, h or deviation set, mapping table, pulse, known-symbol hook | planned: discriminator+slicer; MLSE | — | — | — | dv/*, pocsag, rtty, navtex, subghz | pending (phase 3) |
| GMSK / GFSK (partial response) | BT, h, L | planned: discriminator; coherent-as-OQPSK; MLSE | — | — | — | dstar, ais | pending (phase 3) |
| MSK | — | planned: coherent; differential; discriminator | — | — | — | (via acars) | pending (phase 3) |
| Audio-domain FSK / AFSK | tone pair, baud, real input | planned: tone filterbank; discriminator on analytic signal | — | — | — | aprs, acars | pending (phase 3) |
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
