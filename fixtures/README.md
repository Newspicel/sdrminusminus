# IQ fixture library

This directory holds short IQ signals for decoder regression tests and playback in the app.
Each fixture has a known expected output. Generated waveforms test the decoder against the
project's modulators; recorded captures also exercise real transmitter and reception effects.

Every IQ fixture is a single-channel SigMF pair: `<stem>.sigmf-meta` and `<stem>.sigmf-data`,
using `cf32_le`. Open it as a `virtual:file:<stem>` source and add the channel with the offset
listed below. Rates in the tables are samples per second; `k` means thousands and `M` means millions.

## Generated fixtures

Run `cargo xtask fixtures` to create these pairs. The task uses the same modulators as unit and
engine tests. Most use `channels::testgen`; APRS uses its `ChannelTx` implementation and resamples
to the fixture's device rate.

Generated pairs are ignored by Git. Commit generator and expected-output changes together.

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

## Committed fixtures

These eight pairs are not regenerated: six are recordings and two are frozen synthetic waveforms.
They retain cases that the current generators do not reproduce.

`cargo xtask excerpt` trims a SigMF pair, WAV, or raw `cu8`/`cs8`/`cs16`/`cf32` capture. It shifts
and resamples through the engine's `Ddc`, writes the requested window, and records the source
SHA-256 in a SigMF annotation.

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

## Playback notes

SSTV and ATV produce pictures. Connect their video output to a Video node. Completed SSTV pictures
also enter the server's picture store, with one decoder-log event per picture; `GET /api/images`
serves the PNGs.

ADS-B and GNSS process device-rate samples without the resampling DDC. ADS-B requires 2–4 MS/s;
GNSS requires 2.048 MS/s.

## Naming and review

Use `<decoder>_<description>_<rate>` for decoder fixtures. Signal-generator captures may also
include duration, as in `siggen_2m4_1s` for 2.4 MS/s over one second.

Keep recorded captures short and restrict them to the relevant band. Include expected output,
source provenance, and applicable license information. Committed IQ files require an explicit
force-add because `.gitignore` excludes SigMF pairs.

Give frozen renders a separate stem from generated fixtures. Otherwise regeneration could overwrite
the waveform a regression test is supposed to preserve.

## Provenance and regression coverage

### DMR direct mode: `dmr_call_48k`

A 1.7-second PMR446 channel 1 call captured with an RTL-SDR at 2.048 MS/s, then down-converted to
48 kS/s. It preserves transmitter keying gaps between TDMA bursts.
`dv::dmr::tests::decodes_a_recorded_call` reads it directly.

### DMR Tier III: `dmr_tier3_control_48k`

A 2.3-second Motorola Capacity Max control-channel excerpt from a 2.4 MS/s capture centred on
460.802929 MHz, down-converted to 48 kS/s. It contains two channel grants and CSBK trailers that
do not match the standard payload CRC despite clean BPTC decoding.

Tests cover direct decoding, delivery through the channel filter in engine-sized blocks, and
reception with default settings:

- `dv::dmr::tests::decodes_a_recorded_tier_three_control_channel`
- `dv::dmr::tests::a_live_control_channel_decodes_in_the_blocks_a_radio_delivers`
- `dv::dmr::tests::a_site_that_masks_its_checksums_is_read_without_being_asked_to_be`

### DMR Capacity Plus: `dmr_capacity_plus_48k`

A 2.4-second rest-channel excerpt at 460.800 MHz from the same source capture as the Tier III
fixture. Talkgroup 101 moves from timeslot 1 to timeslot 2 while the rest channel changes from
4 to 3. Its channel-status CSBKs pass the standard CRC. The capture contains only one talkgroup,
so it does not establish the full address-field width.
`dv::dmr::tests::decodes_a_recorded_capacity_plus_rest_channel` reads it directly.

### AIS frozen render: `ais_position_pre_cpm_240k`

A 0.03-second burst from the earlier AIS generator, including its stepped envelope.
`ais::tests::decodes_the_committed_fixture` checks that the general CPM receiver still decodes it.
The old render has 6,250 samples; the current generator produces 6,425.

### NXDN frozen render: `nxdn_addressed_48k`

A 0.69-second reference-modulator render containing a complete four-quarter SACCH message and
FACCH call addressing. Keeping the waveform fixed prevents a decoder test from regenerating its
own reference input.

### ADS-B: `adsb_offair_2m`

A 200 ms excerpt of a 2 MS/s RTL-SDR recording over the Eifel. It includes overlapping replies,
amplitude variation, and roll-call replies whose addresses depend on earlier all-call messages.
The window contains six downlink formats and four aircraft in about 3.1 MB.

`adsb::tests::decodes_a_recorded_sky` reads it directly.
`a_recorded_squitter_places_its_aircraft_against_the_receiver` checks local CPR positioning against
the global position solved from the full recording.

### FT8: `ft8_20m_busy_12k`

A 15-second 20 m slot recorded on 2019-11-11 at 11:06:15 UTC. The source is the MIT-licensed
`ft8_lib` test recording `191111_110615.wav`, pinned in the SigMF annotation. Its 12 kHz mono audio
was converted to `cf32_le` with zero quadrature.

The upstream expected output contains twenty messages; this integration decodes nineteen.
`weak_signal::tests::a_recorded_slot_reads_the_band_the_reference_decoder_published` appends a
quiet tail to close the sliding slot window and records the missing decode explicitly.

### FreeDV: `freedv_1600_8k`

The first three seconds of the FreeDV GUI project's `wav/ve9qrp_1600.wav` receive test recording.
Signed 16-bit mono audio was converted to normalized `cf32_le` with zero quadrature. The SigMF
annotation records the source SHA-256, pinned upstream commit, and LGPL-2.1 license.
`dv::freedv::tests::decodes_the_upstream_receive_recording` reads it directly.
