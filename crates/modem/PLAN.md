# `sdrmm-modem` — Modulation Library Plan

**Status:** in progress (phase 0)
**Audience:** implementer working in the `sdrmm` workspace

---

## 1. Goal

Build `crates/modem` as a complete, standalone modulation library: every practically
defined modulation family, implemented once, parameterised, and characterised. The
existing protocol channels (DMR, TETRA, BLE, …) become thin attachments — parameters
plus framing on top of a library entry — and are one consumer among several. The
library itself is the product.

Concretely:

1. **Catalog completeness.** Analog, ASK/PAM, PSK, QAM, APSK, FSK/CPM (full response,
   partial response, multi-h), orthogonal M-FSK, PPM, line/symbol codes, OFDM and its
   relatives, DSSS/CCK, CSS, FHSS, and the newer multicarrier waveforms. The full list
   is §6. Everything in it is committed scope; nothing is second-class.
2. **Every entry is thorough.** Modulator, demodulator(s), soft output where meaningful,
   and a full measured characterisation: correctness against a reference, performance
   (throughput/latency), and resistance (the exact impairment levels at which it fails).
   §4 defines the test regime; §5 defines "done" per entry.
3. **Minimal duplication.** A modulation is implemented exactly once. Shared substrate
   (pulses, constellations, demapping, timing/carrier recovery, symbol codes) lives
   below the engines; protocols contribute only tables and framing.
4. **End-to-end verified.** Not just unit-level DSP correctness: complete
   payload-to-payload chains, synthetic and recorded, at both the modem level and the
   full engine/runtime level.

### 1.1 Scope boundaries

- Single-stream reception: one coherent receive path. MIMO and multi-antenna
  processing are out of scope.
- Source-coding and storage formats (PCM, PDM, PFM) are not RF modulations and are
  excluded. Line and symbol codes (Manchester, NRZI, differential encoding) *are*
  included — they sit under `symbolcode/`.
- Trellis-coded modulation is channel coding fused with a mapper; it belongs beside
  the FEC in `sdrmm-dsp`, not here.
- Cryptographic layers of any protocol are out of scope; the library ends at recovered
  bits/frames.

### 1.2 The governing rule

A library full of modulations with no protocol behind them rots unless something
exercises every entry continuously. The rule that prevents this:

> **No modulation without a measurement.** The test harness is the universal consumer.
> An entry that lacks its committed correctness curve, resistance limits table,
> performance baseline, and docs row does not merge. An entry whose measurements
> regress fails CI exactly as a protocol regression would — whether or not any
> protocol uses it.

Two built-in consumers reinforce this: the transmit path (`tx.rs`) can drive every
modulator as a signal generator, and `testgen` builds every demodulator's test signals
from the library's own modulators through the shared pulse implementations — so
modulator and demodulator can never drift apart.

---

## 2. Current state

### 2.1 Workspace layout

```
crates/
  channels/src/
    dv/{dmr,dpmr,dstar,m17,nxdn,p25,ysf}.rs    7 digital-voice protocols
    dv/mod.rs                                   SymbolWindow, bit packing
    testgen/                                    per-protocol signal generators
    {acars,adsb,ais,am,aprs,atv,morse,navtex,nfm,pocsag,rds,rtty,ssb,subghz,wfm}.rs
    {tone_squelch,tx,testutil,lib}.rs
  dsp/    → sdrmm_dsp    fsk4.rs (Fsk4Demod), sync.rs (SymbolSync: Gardner + Farrow),
                         pll.rs (LoopFilter, PLL, Costas), agc.rs, fir.rs (design_rrc),
                         resamp.rs, ddc.rs, nco.rs, fec/ (Bptc196, Bptc128, CyclicCode,
                         conv.rs Viterbi, rs129_parity, crc16_msb)
  wire/   → sdrmm_wire   ChannelDescriptor, ChannelParams, DvFrame, DecoderEvent, …
  engine/                runtime: per-channel DDC construction, scheduling
```

Facts that shape the plan:

- **Sample rates are already per-channel.** `ChannelDescriptor` carries a processing
  rate (`Resampled { output_hz } | Native { min_hz, max_hz }`); the engine builds a
  per-channel DDC (mix, decimate, fractional resample); ADS-B already runs on the
  native-rate path. No global-rate migration is needed — only documentation of the
  device-rate / occupied-bandwidth / processing-rate distinction, a DMR-through-DDC
  regression test, and policy extensions if a future protocol needs one.
- **Timing and carrier recovery are already shared.** `SymbolSync` (Gardner detector
  driving a Farrow interpolator, with TDMA coast behaviour), the PLL/Costas loops, and
  AGC live in `sdrmm-dsp` and are rate-agnostic. `Fsk4Demod` composes them; it does not
  own a timing loop.
- **What `Fsk4Demod` uniquely owns** is the burst-mode glue: a carrier gate, level
  estimation driven by known sync patterns ("a burst's own sync pattern knows the
  levels better than any loop can"), attack-limited level tracking, and the policy for
  when loops may learn. This is the hard-won part; the generalised engine must preserve
  it (§3.4).
- **Soft decisions are partially present.** The Viterbi API defines **positive =
  logical 1**; `SymbolWindow` already preserves soft metrics for M17/YSF. The block
  codes (`Bptc196` etc.) are hard-input. The task is completing soft support, not
  introducing it — and the existing sign convention is kept.
- **The most valuable existing test** is `decodes_a_recorded_call`: a recorded off-air
  DMR fixture with real timing drift and real dead time. It stays green through every
  phase; a failure there is blocking.

### 2.2 Modulation census of the existing channels

| Family | Channels | Count |
|---|---|---|
| CPM, 4-ary | dmr, dpmr, nxdn, p25, m17, ysf | 6 |
| CPM, 2-ary (incl. audio-domain AFSK/MSK) | dstar, ais, pocsag, acars, aprs, navtex, rtty, part of subghz | 8 |
| OOK/ASK | morse, part of subghz | 2 |
| PPM | adsb | 1 |
| Linear (BPSK) | rds | 1 |
| Analog | am, ssb, nfm, wfm, atv | 5 |

Fourteen of twenty-one channels sit on one generalised CPM chain — that engine is the
highest-leverage single item and is built first among the families.

---

## 3. Architecture

### 3.1 Crate layout

```
crates/modem/src/
  constellation/   Point sets as data, never match arms: PSK/QAM/APSK/PAM generators
                   plus an arbitrary-table constructor; Gray maps; hard slice; one
                   generic max-log LLR demapper (exact log-sum optional) taking a
                   noise-variance input. Covers every linear modulation with one
                   demapper implementation.
  pulse/           Filter design only: rect, RC, RRC, Gaussian phase pulse, half-sine,
                   LREC/LRC partial-response pulses. Unit-energy normalisation asserted
                   by test. Shared by modulators, demodulators, and testgen.
  symbolcode/      NRZ, NRZI, Manchester, bi-phase, differential encode/decode.
  cpm/             The M-ary CPM/CPFSK engine: symbol→frequency mapping tables;
                   modulation index as one canonical internal form (converted from
                   h or deviation-set at construction); pluggable timing-error
                   detector; known-symbol hook (§3.4); real- and complex-valued input
                   entry points (§3.5). Detectors: quadrature discriminator + slicer,
                   coherent (MSK-as-OQPSK), MLSE over the phase trellis, multi-h.
  linear/          Mapper/demapper over constellation/; coherent recovery (Costas,
                   FLL, decision-directed, pilot-aided); differential detection;
                   OQPSK and π/2 offset handling; noncoherent envelope tier for
                   ASK/OOK with adaptive thresholding.
  orthogonal/      Noncoherent M-FSK: tone filterbank (FFT/Goertzel), energies to
                   per-bit LLRs.
  ppm/             M-PPM matched-filter detection, slot timing.
  analog/          AM / DSB-SC / SSB (Weaver + Hilbert) / VSB (a filter configuration
                   of the AM engine, not a separate type) / FM / PM.
  ofdm/            Framework: FFT framing, cyclic prefix, preamble autocorrelation
                   detection, coarse+fine CFO, LS channel estimation + interpolation,
                   per-subcarrier one-tap equalisation, pilot tracking, pluggable
                   pilot patterns and subcarrier maps. A Hermitian-symmetry flag
                   yields real-baseband DMT from the same engine.
  multicarrier/    OTFS, FBMC, GFDM, UFMC — same architectural slot as ofdm/
                   (frameworks consuming constellation/), scheduled last (§7 phase 9)
                   because their references are commit-and-guard rather than
                   closed-form.
  spread/          DSSS (arbitrary PN + correlator, Barker-11 canonical), CCK codeword
                   correlators, CSS (chirp: SF/BW parameterised, dechirp + FFT peak),
                   FHSS framework (hop sequencer + any underlying catalog entry).
  ber/
    theory/        Closed-form oracle curves: BPSK/QPSK exact, M-PAM/M-QAM/M-PSK,
                   DPSK, noncoherent M-FSK. Acceptance references.
    impair/        Calibrated impairment models: AWGN, static CFO, frequency drift,
                   phase noise, timing offset and jitter, sample-clock ppm error,
                   IQ gain/phase imbalance, DC offset, multipath (named static
                   profiles: two-ray, exponential PDP), adjacent- and co-channel
                   interference, clipping/saturation, input quantisation to N bits,
                   burst/TDMA model (gaps, keying transients, level steps).
                   Every impairment is unit-calibrated: applied value == measured value.
    limits/        Resistance-sweep runner (§4.3): binary-searches each impairment
                   axis to the failure threshold and emits a limits table.
    perf/          Criterion benchmarks + committed baselines (§4.2).
    e2e/           End-to-end scaffolding (§4.4).
    Sweep runner, seeded RNG throughout, `cargo xtask ber <entry>` → CSV/JSON.
  CATALOG.md       One row per entry: parameters, detectors, references, limits table,
                   perf baseline, consumers. CI fails on missing/stale rows.
```

### 3.2 Dependency graph (locked before any code)

```
channels → modem → dsp
channels → wire        modem → wire
xtask / integration tests → channels + modem + engine
```

Numerical primitives (FIR/FFT/resampling/DDC/NCO, loop filters, `SymbolSync`,
PLL/Costas, AGC) stay in `sdrmm-dsp`. `modem` composes them into modulation-level
engines. One AGC, one Gardner — never two parallel sync stacks. `testgen` splits:
waveform modulators move to `modem` (they *are* the library's modulators); protocol
frame builders (BPTC encoding, CSBK/LC construction, …) stay in `channels::testgen`.

### 3.3 Locked design decisions

- **Soft boundary.** `IQ → demod → soft symbols → demapper → LLRs → FEC`. Convention:
  **positive = logical 1** (matches the existing Viterbi). A `SoftBit`/`Llr` newtype
  distinguishes arbitrary-scale confidence from true LLR (which requires the noise
  variance the sync stage estimates). Sign convention and pulse normalisation are
  documented at the crate root before the second dependent module exists.
- **No universal `Demodulator` trait.** CPM emits real soft symbols, linear emits
  complex, OFDM emits symbol vectors. `Cpm`, `Linear`, `Ofdm`, … are concrete types
  with fitting APIs; sharing happens below them (pulses, loops, demapper). With a
  catalog this size the discipline matters more, not less.
- **Frameworks are not peers of mappers.** `ofdm/`, `multicarrier/`, `spread/` consume
  `constellation/` and the sync primitives. They contain a mapper; they never sit
  beside one.
- **Two processing shapes, one channel interface.** Continuous/streaming and
  burst/packet (energy or preamble detect → bounded burst buffer → block decode) both
  live *behind* the existing `ChannelRx` streaming interface — the engine keeps one
  scheduling/retuning/output model. ADS-B already demonstrates packet-style processing
  behind the streaming API.
- **Constellations are tables.** Cross-QAM, star-QAM, non-uniform QAM, and APSK exist
  precisely to prove the demapper is table-driven. Any code path that special-cases
  "the" constellation is a defect.

### 3.4 The known-symbol hook

Every burst-oriented standard embeds known symbols (sync words, training sequences,
preambles, pilots). The engines expose a uniform hook: *"positions i..j carry known
sequence S"* — used for level estimation (CPM), carrier phase/frequency reference
(linear), channel estimation (OFDM), and timing refinement everywhere. This is how the
existing demodulator's burst robustness generalises instead of being lost, and it is a
required parameter axis, not an optional extra.

### 3.5 Real- and complex-valued domains

Several existing channels are nested chains: APRS is AFSK inside NFM, ACARS is MSK
inside AM, RDS is BPSK on a 57 kHz subcarrier whose reference is the 19 kHz pilot.
Therefore: `analog/` outputs feed the digital engines; `cpm/` and `linear/` accept
real-valued input (internally analytic-signal or subcarrier-shifted) as first-class,
not as a wrapper hack. Protocol-specific carrier references (RDS's pilot lock) stay in
the protocol attachment; the mapper/slicer/demapper come from the library.

---

## 4. Test regime

Four measurement classes. All four are built in phase 0 and applied to every entry;
they are the plan's spine, not an afterthought.

### 4.1 Correctness

- **Theory oracles.** Wherever a closed form exists, the measured curve must sit
  within a stated, test-encoded tolerance of it at high Eb/N0. The harness itself is
  trusted only after measured BPSK matches the exact erfc curve within 0.2 dB across
  0–10 dB — the calibration gate that everything else inherits.
- **Committed references.** Where no closed form exists (partial-response CPM through a
  discriminator, CCK, the multicarrier set), the first measured curve is reviewed,
  committed, and guarded against regression.
- **Golden vectors.** Where standards publish test vectors, they become fixtures.
- **Metric definitions are explicit:** Eb per information bit vs per coded bit;
  TDMA dead time excluded from Eb/N0 accounting; uncoded SER/BER, post-FEC BER,
  frame/burst error rate, and undetected-error rate reported separately; minimum
  error counts (or confidence intervals) per sweep point; fixed seeds everywhere.

### 4.2 Performance

Every engine carries a committed performance baseline:

- **Throughput:** Msamples/s per core per configuration (M, pulse span, detector
  tier), measured by Criterion benches.
- **Real-time factor:** throughput divided by the entry's required processing rate at
  its reference configuration — the number that says how many channels of this type
  one core sustains.
- **Steady-state allocation:** the hot `process()` path performs zero heap
  allocations, asserted with a counting allocator in tests.
- **Latency:** group delay from sample in to symbol/event out, documented per entry.
- **Concurrency scaling:** an N-instances benchmark for the engines protocols run
  many-at-once.

Baselines are committed as JSON; the nightly run fails on >10 % regression against
baseline (threshold adjustable per entry with justification in the commit).

### 4.3 Resistance — where things fail, measured

For every (entry, detector) pair, the limits runner sweeps each impairment axis to its
failure threshold and commits the result as the entry's **limits table**. Default
failure criterion: post-detection BER exceeds 1e-2 while operating 3 dB above the
entry's measured 1e-3 sensitivity, or loss of lock/sync — entries may override with a
documented criterion where that default is meaningless (analog, acquisition metrics).

Axes and the recorded numbers:

| Axis | Recorded threshold |
|---|---|
| Sensitivity | Eb/N0 at BER 1e-2 / 1e-3 / 1e-4 (uncoded and post-FEC) |
| Static CFO | max offset (absolute and as fraction of symbol rate) for ≤1 dB penalty; absolute pull-in range |
| Frequency drift | max Hz/s tracked for ≤1 dB |
| Sample-clock error | max ppm for ≤1 dB |
| Phase noise | max integrated phase noise (° RMS, stated mask shape) for ≤1 dB |
| Timing | static offset tolerance (fraction of symbol); jitter tolerance |
| IQ imbalance | gain (dB) / phase (°) at which the error floor reaches a stated BER |
| DC offset | max relative DC for ≤1 dB |
| Multipath | max delay spread per named profile for ≤1 dB or floor level |
| Co-channel interference | minimum C/I for ≤1 dB |
| Adjacent-channel | rejection (dB) vs offset |
| Clipping | max input overdrive (dB above full scale) for ≤1 dB |
| Quantisation | minimum input bit depth for ≤0.5 dB |
| Burst survival (burst-mode entries) | minimum burst length; maximum dead time; level-step size recovered within one sync period; keying-transient tolerance |
| Acquisition | SNR for 90 % / 99 % sync probability; mean time-to-lock; false-sync rate on pure noise per hour |

Combined stress: a small set of named composite profiles (e.g. "mobile urban" =
multipath profile + CFO + drift + phase noise at realistic levels) run per entry, with
the resulting degradation committed. Limits tables regress like curves: a threshold
moving worse than its tolerance fails CI. The limits table *is* the deliverable behind
"robust across different types and uses" — robustness as numbers, not adjectives.

### 4.4 End-to-end

Five levels, all seeded and deterministic:

1. **Modem loopback:** random payload → modulator → impairment channel at a stated
   margin above sensitivity → demodulator → demapper → FEC → payload equality.
   Property-style, per entry. This is also the TX-side correctness test, since the
   same modulators drive `tx.rs`.
2. **Protocol E2E, synthetic:** protocol frame builders (`channels::testgen`) construct
   a complete transmission (e.g. a full DMR call: headers, voice bursts, terminator) →
   library modulator → impairment channel → the actual channel implementation →
   decoded events and payloads asserted field-by-field. One per protocol attachment.
3. **Recorded-fixture E2E:** off-air SigMF recordings with expected decoded output,
   kept short (hundreds of milliseconds). `decodes_a_recorded_call` is the model; each
   newly attached protocol gets one when a capture becomes available. These are the
   only tests with real drift, real level steps, and real dead time — a failure is
   blocking.
4. **Engine integration E2E:** raw sample stream at a native device rate → engine
   runtime (DDC construction, per-channel resampling, scheduling) → `DecoderEvent`
   stream asserted. Includes a multi-channel case: several protocols decoded
   concurrently from one wideband synthetic stream.
5. **Cross-validation:** where an independent reference implementation exists,
   spot-check agreement on identical input.

CI policy: PR runs = unit tests + a smoke subset (few curve points per family, level-1
and level-2 E2E for touched entries, recorded fixtures). Nightly = full sweeps, full
limits tables, perf baselines, all E2E levels. Full runs also invokable locally via
`cargo xtask`.

---

## 5. Definition of done — per catalog entry

An entry merges when all of these exist; the bundle is the uniform merge gate for
every row in §6:

1. Modulator in `modem`, built on `pulse/`, driving testgen and usable by `tx.rs`.
2. Demodulator(s) at the detection tier(s) listed in the catalog. Where several tiers
   are listed, the first gates the entry; later tiers are separate merges measured
   against the first (e.g. coherent PSK must demonstrably beat differential).
3. Soft output through the shared demapper wherever the entry can feed FEC.
4. Correctness fixture per §4.1 (oracle-matched or committed reference; analog entries
   use SINAD/THD vs input SNR instead of BER).
5. Limits table per §4.3, including the burst rows for burst-capable entries.
6. Performance baseline per §4.2.
7. Level-1 E2E loopback test. Protocol attachments additionally require level-2, and
   level-3 when a capture exists.
8. `CATALOG.md` row: parameter ranges, tiers implemented, standards using it,
   references to its fixtures. Missing row = CI failure.

---

## 6. Catalog

One catalog. Grouped by shared engine purely so rows map onto §3.1 modules — the
grouping implies no priority, and every row is committed scope. "Repo consumer" names
the channel that attaches; "—" means harness-and-TX-only until a protocol adopts it,
which changes nothing about its required §5 bundle.

### CPM / FSK engine (`cpm/`)

| Entry | Key parameters | Detectors | Reference | Standards examples | Repo consumer |
|---|---|---|---|---|---|
| M-ary CPFSK, M ∈ {2,4,8,…,64} | M, h or deviation set, mapping table, pulse, known-symbol hook | discriminator+slicer; MLSE | theory (orthogonal/noncoh. forms) + committed | DMR, dPMR, NXDN, P25, M17, YSF, POCSAG, RTTY, navtex | dv/*, pocsag, rtty, navtex, subghz |
| GMSK / GFSK (partial response) | BT, h, L | discriminator; coherent-as-OQPSK; MLSE | committed | D-STAR, AIS, BLE, Bluetooth BR | dstar, ais |
| MSK | — | coherent; differential; discriminator | theory | AIS variants, ACARS payload | (via acars) |
| Audio-domain FSK / AFSK | tone pair, baud, real input | tone filterbank; discriminator on analytic signal | committed | APRS (AFSK-in-NFM), ACARS (MSK-in-AM) | aprs, acars |
| Multi-h CPM / SOQPSK | h sequence, pulse | MLSE | committed | aeronautical telemetry | — |

### Linear engine (`linear/` + `constellation/`)

| Entry | Key parameters | Detectors | Reference | Standards examples | Repo consumer |
|---|---|---|---|---|---|
| M-PAM / M-ASK / OOK | M, pulse | coherent; noncoherent envelope + adaptive threshold | theory | Morse, ISM remotes | morse, subghz |
| BPSK / QPSK / M-PSK | M, mapping, pulse | coherent (Costas/FLL/DD/pilot); differential | theory | RDS, countless | rds |
| DPSK family (DBPSK/DQPSK/8DPSK) | M | differential | theory | Bluetooth EDR | — |
| OQPSK / π/2-BPSK | offset handling | coherent | theory | satellite, IoT PHYs | — |
| π/4-DQPSK | RRC α | differential first; coherent second | theory (DPSK) | TETRA, EDR | tetra (new) |
| Square QAM 16/64/256/1024 | M, Gray map | coherent, pilot- or decision-directed | theory | WLAN, DVB, microwave links | — |
| Cross QAM 32/128 | point table | coherent | theory | DVB-C | — |
| Star-QAM | rings | differential; coherent | committed | legacy mobile | — |
| Non-uniform / hierarchical QAM | spacing table | coherent | committed | DVB-T hierarchical | — |
| APSK 16/32 | ring radii/counts | coherent | committed | DVB-S2 | — |

### Orthogonal & pulse-position (`orthogonal/`, `ppm/`)

| Entry | Key parameters | Detectors | Reference | Standards examples | Repo consumer |
|---|---|---|---|---|---|
| Noncoherent M-FSK | M, tone spacing | filterbank energies → LLR | theory | WSJT modes (FT8 = 8-GFSK) | — |
| M-PPM | slots, slot width | matched filter + argmax | theory | ADS-B (2-PPM), optical | adsb |

### Analog (`analog/`)

| Entry | Detectors | Reference | Repo consumer |
|---|---|---|---|
| AM / DSB-SC | envelope; synchronous | SINAD vs SNR | am |
| SSB (USB/LSB) | Weaver; Hilbert | SINAD vs SNR | ssb |
| VSB | AM engine + filter config | SINAD vs SNR | atv |
| FM (narrow/wide) / PM | discriminator; PLL | SINAD vs SNR | nfm, wfm |

### Symbol codes (`symbolcode/`)

NRZ, NRZI, Manchester, bi-phase, differential — codec round-trip tested; consumed by
subghz, testgen, and several framings. Small, but in the catalog so they are
implemented once.

### Frameworks (`ofdm/`, `multicarrier/`, `spread/`)

| Entry | Key parameters | Detection chain | Reference | Standards examples | Repo consumer |
|---|---|---|---|---|---|
| CP-OFDM | FFT size, CP, pilot pattern, subcarrier map | preamble autocorr → CFO → channel est → one-tap EQ → demap | theory per subcarrier | 802.11a/g, DAB-like, DRM-like | 802.11 (new) |
| DMT | Hermitian flag on CP-OFDM | same | theory | wireline | — |
| OTFS / FBMC / GFDM / UFMC | per waveform | per waveform | committed (+ published curves where available) | research / 5G-adjacent | — |
| DSSS | PN sequence, chip rate | correlator; processing gain measured vs 10·log₁₀(N) | theory | 802.11b 1/2 Mbps, GPS-like | 802.11b (new) |
| CCK | codebooks | codeword correlator bank | committed | 802.11b 5.5/11 Mbps | 802.11b (new) |
| CSS (chirp) | SF, BW, chirp map | dechirp + FFT peak | committed | LoRa | lora (new) |
| FHSS | hop sequencer + underlying entry | framework | committed | Bluetooth BR, ISM | — |

### Protocol attachments planned against the catalog

TETRA downlink (π/4-DQPSK entry; scoped in stages — burst sync and MAC first, then
coding/interleaving/logical channels: TETRA is substantially more than parameters),
BLE 1M/2M advertising (GFSK entry + packet shape), 802.11a/g (CP-OFDM entry; SIGNAL
field at BPSK-1/2 first, higher rates via the existing demapper), 802.11b (DSSS/CCK),
LoRa RX (CSS), Bluetooth EDR modulation (DPSK entries; hop-following is a hopping
problem, not a modulation problem, and lives with the FHSS framework if pursued).

---

## 7. Phases

One phase per branch; the acceptance list is the merge gate; refactor steps must not
move any committed measurement. `decodes_a_recorded_call` is green at every merge.

**Phase 0 — Test infrastructure.** Lock the dependency graph (§3.2). Build `ber/`
complete: theory oracles, calibrated impairments, limits runner, perf scaffold, E2E
scaffold, xtask commands, CI wiring (PR smoke + nightly full).
*Accept:* BPSK-vs-erfc within 0.2 dB over 0–10 dB; every impairment unit-calibrated;
all runs seeded and reproducible; the **current** DMR chain baselined in full —
steady-state curve, burst-model curve, limits table, perf baseline, and a level-2 E2E
using the existing testgen — committed as the pre-migration reference.

**Phase 1 — Soft substrate.** `SoftBit`/`Llr` newtype (positive = 1); the generic
max-log demapper over arbitrary point sets; M-FSK energy-LLR path; noise-variance
plumbing. Soft block decoding for `Bptc196`/`Bptc128` (Chase-2 per component code,
optionally Pyndiah-iterated), with `errors_corrected` defined as Hamming distance to
the hard slice, plus rejection/false-positive tests.
*Accept:* demapper output matches hand-computed LLRs for 2- and 4-level cases; a
genie-LLR bound in the harness separates concept failures from LLR-quality failures;
soft-BPTC gain measured and recorded on the phase-0 DMR curves (expected order 1–2 dB
at 1e-3; the measured number becomes the regression gate).

**Phase 2 — `pulse/` + `symbolcode/`.**
*Accept:* unit-energy tests; TX-RRC ⊗ RX-RRC Nyquist-ISI test; all codecs round-trip;
testgen and `tx.rs` consume `pulse/`.

**Phase 3 — CPM engine and migrations.** Build the general M-ary engine (§3.1),
migrate in probe order — POCSAG (base 2FSK), DMR (4-ary + RRC + the recorded fixture),
D-STAR (Gaussian), AIS (parameterisation), APRS or ACARS (real-valued input) — each
probe chosen to stress a different axis; if any needs special-case code inside `cpm/`,
stop and fix the parameter space. Then the remaining CPM channels mechanically.
*Accept:* synthetic M ∈ {2, 4, 8} curves match `theory/` within stated tolerance —
8-ary gates the engine's generality even with no protocol attached; no per-protocol
branches inside `cpm/`; every migrated channel within 0.5 dB of its phase-0
steady-state **and** burst-model baselines, with limits tables no worse than baseline;
`Fsk4Demod` deleted. Follow-on merges, each justified by its own measured gain: MLSE
tier, multi-h/SOQPSK.

**Phase 4 — Linear engine and the full constellation catalog.** Coherent recovery
loops, differential tiers, offset handling, envelope tier; every linear-family row in
§6 lands with its §5 bundle (the exotic tables — cross/star/non-uniform QAM, APSK —
are the proof the demapper is table-driven). Migrations: morse and subghz-OOK onto the
envelope tier; RDS mapper (pilot-locked carrier stays protocol-side).
Attachment: **TETRA downlink**, staged as scoped in §6.
*Accept:* all linear rows bundled; DPSK curves match theory; coherent beats
differential by the measured, recorded margin; TETRA synthetic level-2 E2E decodes
burst structure and MAC; recorded TETRA fixture added when captured.

**Phase 5 — Orthogonal M-FSK + PPM.** Filterbank detector; generalised M-PPM; ADS-B
migrates onto `ppm/`.
*Accept:* noncoherent M-FSK matches theory; ADS-B regression byte-identical; FT8 tone
demodulation validated against published WSJT test vectors as the orthogonal entry's
golden-vector test.

**Phase 6 — OFDM framework.** Built and validated synthetically end-to-end before any
protocol: per-subcarrier BPSK→64-QAM loopback under AWGN + CFO + named multipath
profiles; channel-estimation error curves; DMT flag.
Attachment: **802.11a/g**, staged — preamble detect, LTF sync, SIGNAL at BPSK-1/2,
then higher rates through the phase-4 demapper. Golden vectors from published example
frames; recorded fixture when captured.
*Accept:* synthetic OFDM bundle complete with limits tables (notably CFO,
sample-clock ppm, and multipath rows); 802.11 SIGNAL decode on golden vectors.

**Phase 7 — Spread, chirp, hopping.** DSSS with processing gain measured against
10·log₁₀(chips/symbol); CCK codebooks; CSS across SF7–SF12; FHSS framework driving an
underlying entry through a synthetic hop schedule.
Attachments: 802.11b (DSSS/CCK), LoRa preamble/header RX.
*Accept:* processing-gain and CSS detection curves committed; hop-framework level-1
E2E (payload survives a hopping channel with the sequencer known); attachment E2Es on
synthetic vectors.

**Phase 8 — Analog consolidation.** The five analog channels migrate onto `analog/`
engines; VSB as configuration; SINAD-based correctness and limits (co-channel,
adjacent-channel, clipping rows especially).
*Accept:* all analog rows bundled; channel behaviour unchanged on existing fixtures.

**Phase 9 — Multicarrier completion.** OTFS, FBMC, GFDM, UFMC through the framework
slot, each with commit-and-guard references (cross-checked against published curves
where available) and the full §5 bundle.
*Accept:* every row of §6 shows a complete bundle in `CATALOG.md`; CI enforces the
docs-row rule; the nightly full run (all sweeps, all limits, all perf, all E2E levels)
is green.

---

## 8. Standing rules for the implementer

- One phase per branch; acceptance lists are merge gates.
- Refactors never move a committed measurement — curve, limits threshold, or perf
  baseline. If one moves, something broke, even with green unit tests.
- `decodes_a_recorded_call` failing is blocking, always.
- Sign convention (positive = 1) and pulse normalisation are documented at the crate
  root before the second module depending on either is written.
- Constellations, mappings, and deviation sets are data. A `match` on a specific
  standard inside an engine is a defect.
- Every new capability enters through the harness first: no detector tier, equaliser,
  or waveform merges ahead of the measurement that would prove it works — the
  measurement is the consumer.
