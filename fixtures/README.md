# fixtures/ — golden IQ fixture library

Recorded IQ samples for decoder golden tests (PLAN §14): every decoder ships with short
fixtures plus expected decoded output, and building this library *is* part of building each
decoder. The library starts at M3 (record & replay); the first real decoders land at M4.

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
