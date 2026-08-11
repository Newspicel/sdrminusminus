# fixtures/ — golden IQ fixture library

IQ samples for decoder golden tests (PLAN §14): every decoder ships with short fixtures plus
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
| `ais_position_240k` | 240 k | `ais` @ +25 kHz | MMSI 211234560 at 53.5413, 9.9846 |
| `aprs_afsk1200_240k` | 240 k | `aprs` @ −40 kHz | `DL1ABC-9>APRS,WIDE1-1` at 52.5, 13.4 |
| `rtty_45_170_48k` | 48 k | `rtty` @ +5 kHz | `CQ CQ DE DL1ABC K` |
| `morse_20wpm_48k` | 48 k | `morse` @ −5 kHz | `CQ DE DL1ABC K` at 20 wpm |
| `adsb_squitters_2m` | 2 M | `adsb` @ 0 Hz | `3C6444`/`DLH123`, FL380, a solved position |
| `rds_station_960k` | 960 k | `wfm` @ +200 kHz, `rds` on | PI `D3C2`, PS `SDR-M4`, 1 kHz audio |
| `navtex_518_48k` | 48 k | `navtex` @ +3 kHz | `DA07` navigational warning, `GALE WARNING` |
| `acars_downlink_240k` | 240 k | `acars` @ −40 kHz | `D-AIBC` / `LH0400` `[H1]`, `SDR-- FIXTURE` |
| `subghz_ev1527_500k` | 500 k | `subghz` @ +100 kHz | 24-bit PWM `0A1B23`, address `0A1B2`, button 3 |
| `atv_ccir625_2m4` | 2.4 M | `atv` @ +200 kHz | 625/25 AM, five vertical bars black to white |

One pair is **not** synthesized and is committed: `dmr_call_48k`, a recorded off-air excerpt.

| stem | rate | channel | expected |
|---|---|---|---|
| `dmr_call_48k` | 48 k | `dmr` @ 0 Hz | colour code 1, group call, radio ID 12345678 to talkgroup 12345678 |

ADS-B is the one fixture whose device rate is not negotiable: it fills its whole 2 MHz
channel, so a resampling DDC cannot carry it and the engine refuses the channel at any other
rate (PLAN §18). ATV is the one whose output is not a log line: play it, wire the channel's
face into view, and the picture is on the face itself.

Every fixture is a SigMF pair — `<stem>.sigmf-meta` + `<stem>.sigmf-data`, mono-channel
`cf32_le` — readable by `sdrmm-recorder` and playable in-app as a `virtual:file:<stem>`
device.

## Naming

- Synthesized: `<source>_<rate>_<duration>`, e.g. `siggen_2m4_1s` = virtual siggen,
  2.4 Msps, 1 s.
- Recorded off-air (M4+): `<decoder>_<what>_<rate>`, e.g. `pocsag_weather_1m024` — named
  when the decoder lands, alongside its expected-output file.

## Provenance

- **Synthesized fixtures are never committed.** They are deterministic renders of the
  virtual siggen — regenerate with `cargo xtask fixtures`. The `.gitignore` here excludes
  all `*.sigmf-*` so a generated pair can't land in a commit by accident.
- **Recorded off-air captures** arrive with their M4+ decoders: kept to seconds, stripped
  to the band of interest, and either committed case-by-case (small) or fetched by
  `cargo xtask fixtures` (PLAN §14). Committing one means force-adding past the
  `.gitignore` — that friction is the case-by-case review.
- `dmr_call_48k` is the one committed so far (1.7 s, 640 KB): a direct-mode DMR call on PMR446
  channel 1, captured with an RTL-SDR at 2.048 Msps and down-converted to the channel rate so
  only the 12.5 kHz that matters is carried. It is committed because it is the only signal in
  the tree that keys off between bursts the way a real TDMA transmitter does, and no generated
  one reproduces what that costs a receiver — `dv::dmr::tests::decodes_a_recorded_call` reads
  it directly.
