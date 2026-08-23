# fixtures/ — golden IQ fixture library

IQ samples for decoder golden tests (): every decoder ships with short fixtures plus
expected decoded output, and building this library *is* part of building each decoder. The
library starts at M3 (record & replay); the wave-1 decoders landed at M4 and wave 2 (NAVTEX,
ACARS, sub-GHz) after M6.

## What `cargo xtask fixtures` writes

One pair per decoder — and one per mode that scans out a picture — rendered by the same
modulators the unit tests and the engine end-to-end run use — a fixture can therefore never drift from what the decoders are tested
against. Most come from a `channels::testgen` encoder; a mode that ships a `ChannelTx` (APRS
today) is keyed by that transmitter instead, at its own channel rate, and resampled to the
device rate the fixture is written at. Each is meant to be *played*: open it as a `virtual:file:`
device, add the named channel at the stated offset, and the decoder log fills up.

| stem | rate | channel | expected |
|---|---|---|---|
| `siggen_2m4_1s` | 2.4 M | any demod | the virtual siggen's tones (record/replay fixture) |
| `pocsag_1200_240k` | 240 k | `pocsag` @ +50 kHz | address 1234567, `SDR-- FIXTURE` |
| `flex_1600_2_240k` | 240 k | `flex` @ +30 kHz | address 1234567, `SDR-- FLEX FIXTURE`, cycle 7 frame 83 |
| `ermes_alpha_240k` | 240 k | `ermes` @ −30 kHz | address 234567, urgent alert 5, `SDR-- ERMES FIXTURE` |
| `selcall_ccir1_48k` | 48 k | `selcall` @ +5 kHz, CCIR-1 | `12234`, including the repeat marker |
| `selcall_zvei1_48k` | 48 k | `selcall` @ −5 kHz, ZVEI-1 | `A11D0`, including group and repeat symbols |
| `ais_position_240k` | 240 k | `ais` @ +25 kHz | MMSI 211234560 at 53.5413, 9.9846 |
| `aprs_afsk1200_240k` | 240 k | `aprs` @ −40 kHz | `DL1ABC-9>APRS,WIDE1-1` at 52.5, 13.4 |
| `rtty_45_170_48k` | 48 k | `rtty` @ +5 kHz | `CQ CQ DE DL1ABC K` |
| `morse_20wpm_48k` | 48 k | `morse` @ −5 kHz | `CQ DE DL1ABC K` at 20 wpm |
| `cw_skimmer_dual_48k` | 48 k | `cw_skimmer` @ 0 Hz | simultaneous `DL1AAA` at −3.5 kHz/18 wpm and `G4BBB` at +4.2 kHz/27 wpm |
| `adsb_squitters_2m` | 2 M | `adsb` @ 0 Hz | `3C6444`/`DLH123`, FL380, a solved position |
| `rds_station_960k` | 960 k | `wfm` @ +200 kHz, `rds` on | PI `D3C2`, PS `SDR-M4`, 1 kHz audio |
| `navtex_518_48k` | 48 k | `navtex` @ +3 kHz | `DA07` navigational warning, `GALE WARNING` |
| `acars_downlink_240k` | 240 k | `acars` @ −40 kHz | `D-AIBC` / `LH0400` `[H1]`, `SDR-- FIXTURE` |
| `ysf_callsigns_48k` | 48 k | `ysf` @ 0 Hz | `DL1ABC` to `ALL` via `DB0XYZ` and `DB0ABC` |
| `subghz_ev1527_500k` | 500 k | `subghz` @ +100 kHz | 24-bit PWM `0A1B23`, address `0A1B2`, button 3 |
| `atv_ccir625_2m4` | 2.4 M | `atv` @ +200 kHz | 625/25 AM, five vertical bars black to white |
| `sstv_robot36_48k` | 48 k | `sstv` @ +4 kHz | Robot 36, eight colour bars white to black |
| `dcf77_2026_2k` | 2 k | `radio_clock` / DCF77 @ 0 Hz | 2026-08-15 12:34 CET, valid parity |
| `gps_l1_ca_prn7_2m048` | 2.048 M | `gnss` / PRN 7 @ 0 Hz | +1 kHz Doppler, 158.3-chip code phase |
| `dect_base_2m304` | 2.304 M | `dect` @ 0 Hz | RFPI `01234D5E6D`, class A, carrier 4, standard authentication and ciphering advertised |

Eight pairs are **not** written by `cargo xtask fixtures` and are committed instead — five
recorded off air and three frozen regression renders. `cargo xtask excerpt` cuts them: it reads a
SigMF stem, a WAV, or a raw `cu8`/`cs8`/`cs16`/`cf32` capture, shifts and resamples through the
same `Ddc` the engine feeds a channel with, and writes the window asked for with the source's
SHA-256 in the annotation.

| stem | rate | channel | expected |
|---|---|---|---|
| `dmr_call_48k` | 48 k | `dmr` @ 0 Hz | colour code 1, group call, radio ID 12345678 to talkgroup 12345678 |
| `dmr_tier3_control_48k` | 48 k | `dmr` @ 0 Hz | colour code 10 Capacity Max control channel, grants for logical channels 22 (slot 2) and 42 (slot 1) between radios 9995 and 9999 |
| `dmr_capacity_plus_48k` | 48 k | `dmr` @ 0 Hz | colour code 5 Capacity Plus rest channel, a talkgroup 101 call handed from timeslot 1 to timeslot 2 and the rest channel moving 4 → 3 with it |
| `freedv_1600_8k` | 8 k | `freedv` @ 0 Hz, USB | FreeDV 1600 sync and decoded Codec2 speech |
| `ais_position_pre_cpm_240k` | 240 k | `ais` @ +25 kHz | MMSI 211234560 at 53.5413, 9.9846 |
| `nxdn_addressed_48k` | 48 k | `nxdn` @ 0 Hz | RAN 17, radio 12345 to talkgroup 234 via FACCH/SACCH |
| `adsb_offair_2m` | 2 M | `adsb` @ 0 Hz | 17 Mode S replies from four aircraft — DF4/5/11/17/20/21, FL370 and squawk 5245 from 4D2256, a TC11 position and a TC19 velocity from 3FF91D |
| `ft8_20m_busy_12k` | 12 k | `ft8` @ 0 Hz | 19 of the 20 decodes `ft8_lib` publishes for this slot |

SSTV and ATV are the two whose output is a picture rather than a log line. ATV shows on the
channel's own face; SSTV also lands in the picture store, so the decoder log gets one line per
completed picture and `GET /api/images` serves the PNG.

ADS-B and GNSS are device-rate fixtures: ADS-B accepts 2–4 Msps while GPS L1 C/A uses exactly
2.048 Msps, and neither is carried through the resampling DDC.

Every fixture is a SigMF pair — `<stem>.sigmf-meta` + `<stem>.sigmf-data`, mono-channel
`cf32_le` — readable by `sdrmm-recorder` and playable in-app as a `virtual:file:<stem>`
device.

## Naming

- Synthesized: `<source>_<rate>_<duration>`, e.g. `siggen_2m4_1s` = virtual siggen,
  2.4 Msps, 1 s.
- Recorded off-air (M4+): `<decoder>_<what>_<rate>`, e.g. `pocsag_weather_1m024` — named
  when the decoder lands, alongside its expected-output file.

## Provenance

- **Generated fixtures are never committed.** They are deterministic renders of the virtual
  siggen — regenerate with `cargo xtask fixtures`. The `.gitignore` here excludes all
  `*.sigmf-*` so a generated pair can't land in a commit by accident.
- **Recorded off-air captures** arrive with their M4+ decoders: kept to seconds, stripped
  to the band of interest, and either committed case-by-case (small) or fetched by
  `cargo xtask fixtures` (). Committing one means force-adding past the
  `.gitignore` — that friction is the case-by-case review.
- **A frozen render is committed when no generator still reproduces it.** `cargo xtask
  fixtures` writes today's output, so a test that pins behaviour against an *older* render
  cannot read a generated path — the generator would silently overwrite the artifact and end
  up proving itself. Such a render is committed under its own stem, never one the table
  above lists.
- `dmr_call_48k` (1.7 s, 640 KB): a direct-mode DMR call on PMR446 channel 1, captured with an
  RTL-SDR at 2.048 Msps and down-converted to the channel rate so only the 12.5 kHz that
  matters is carried. It is committed because it is the only signal in the tree that keys off
  between bursts the way a real TDMA transmitter does, and no generated one reproduces what
  that costs a receiver — `dv::dmr::tests::decodes_a_recorded_call` reads it directly.
- `dmr_tier3_control_48k` (2.3 s, 880 KB): the control channel of a Motorola Capacity Max site,
  taken from a 2.4 Msps capture centred on 460.802929 MHz and down-converted to the channel rate
  so only the 12.5 kHz that matters is carried. It is committed because it is the only signal in
  the tree whose CSBKs do not carry the checksum ETSI describes — every block decodes with a
  clean BPTC and a trailer that matches no CRC-16 of its own payload — and because it holds the
  two channel grants a trunked site hands out, which nothing generated reproduces.
  `dv::dmr::tests::decodes_a_recorded_tier_three_control_channel` reads it directly,
  `dv::dmr::tests::a_live_control_channel_decodes_in_the_blocks_a_radio_delivers` replays it the
  way the engine does, through the channel filter and in fixed blocks, and
  `dv::dmr::tests::a_site_that_masks_its_checksums_is_read_without_being_asked_to_be` holds a
  receiver on its defaults to it.
- `dmr_capacity_plus_48k` (2.4 s, 900 KB): the rest channel of a Motorola Capacity Plus system,
  cut from the same 2.4 Msps capture as the Capacity Max fixture but at 460.800 MHz. It is
  committed because Capacity Plus was covered only by hand-built payloads until now, and because
  no generator reproduces what a rest channel does across a hand-over: a talkgroup 101 call ends
  on timeslot 1, the repeater reports itself idle, and the next call opens on timeslot 2 while
  the rest channel moves from 4 to 3. Its channel-status CSBKs all check out against the ETSI
  checksum, which is what makes it the counterpart to the Capacity Max fixture above. One
  talkgroup is all this capture carries, so the address field's width is pinned by one value.
  `dv::dmr::tests::decodes_a_recorded_capacity_plus_rest_channel` reads it directly.
- `ais_position_pre_cpm_240k` (0.03 s, 50 KB): the AIS position burst as the pre-migration
  generator rendered it, stepped envelope and all. It is committed because it is the only
  evidence left that the general CPM engine decodes what the hand-written AIS chain produced —
  `ais::tests::decodes_the_committed_fixture` reads it directly, and today's generator emits a
  different waveform (6425 samples against this one's 6250).
- `nxdn_addressed_48k` (0.69 s, 260 KB): a frozen reference-modulator render containing a
  complete four-quarter SACCH message and FACCH call addressing. It is committed so the
  decoder test cannot regenerate the samples it is about to verify.
- `adsb_offair_2m` (0.2 s, 3.1 MB): 1090 MHz Mode S off air over the Eifel, cut from a 2 Msps
  RTL-SDR recording. It is committed because nothing generated reproduces what a real sky costs a
  receiver — overlapping replies, real amplitude spread, and roll-call replies whose address only
  exists because an all-call squitter arrived first. The 200 ms window is the densest one in the
  recording that carries all six downlink formats and four aircraft; at 2 Msps that costs 3.1 MB,
  which is why it is 200 ms and not a second. `adsb::tests::decodes_a_recorded_sky` reads it
  directly, and `a_recorded_squitter_places_its_aircraft_against_the_receiver` checks the local
  CPR solution against the position the whole recording solves globally.
- `ft8_20m_busy_12k` (15 s, 1.4 MB): one FT8 slot from a crowded 20 m band, recorded
  2019-11-11 11:06:15 UTC, converted from the upstream 12 kHz mono WAV to `cf32_le` with a zero
  quadrature component. It is committed because it comes with an independent decoder's published
  answer — twenty messages — which is a stronger check than any render of our own modulator.
  `weak_signal::tests::a_recorded_slot_reads_the_band_the_reference_decoder_published` reads it,
  appends the quiet tail a live receiver would deliver so the sliding slot window closes, and
  pins the one decode we do not reach.
- `freedv_1600_8k` (3 s, 188 KB): the first three seconds of the FreeDV GUI project's
  `wav/ve9qrp_1600.wav` receive test, converted from signed 16-bit mono audio to normalized
  `cf32_le` with a zero quadrature component. The source file's SHA-256 is recorded in the SigMF
  annotation with the pinned upstream commit and LGPL-2.1 license;
  `dv::freedv::tests::decodes_the_upstream_receive_recording` reads it directly.
