# Decoders

Wave 1 (`PLAN.md` §13 Phase 2) is RDS, POCSAG, ADS-B, AIS, APRS/AX.25, RTTY and Morse. All
are written in Rust in `crates/channels` on primitives from `crates/dsp` — no external decoder
binaries, no piping audio to another tool.

Every decoder emits a typed event, not a line of text. The event travels three ways from one
definition: pushed to clients over the WebSocket, stored in the
[decoder log](decoder-log.md), and rendered by the log table, the CSV export, the map and the
per-decoder panel. Adding a field to a decoder's event adds it everywhere at once.

Decoder channels advertise no audio, so the client hides the audio transport instead of
offering a silent stream. The exception is RDS, which rides the WFM channel: one demod chain
produces both the audio and the subcarrier.

## How to run one

Add the channel type at the right offset on a device whose rate is high enough for the
descriptor's input rate. `cargo xtask fixtures` writes a playable SigMF pair per decoder with
the exact channel and offset documented in `fixtures/README.md` — that is the fastest way to
see one work.

## RDS

On the `wfm` channel, `rds = true`.

57 kHz DBPSK off the FM composite, decoded the way a receiver does it: lock the 19 kHz pilot
with a PLL, take its third harmonic to get 57 kHz (far steadier than locking 57 kHz directly),
symbol-sync, differential-decode, then block-sync on the offset words and hold that sync
through bad blocks before re-hunting.

Decodes groups 0A/0B (programme service name, TP/TA/MS flags, alternative frequencies) and
2A/2B (RadioText with its A/B flag), plus the programme type and its name.

An event carries the current best view of the station — PI, PS, RadioText, PTY, flags,
alternative frequencies, accepted groups and rejected blocks — and is emitted **only when a
field actually changed**. RDS repeats endlessly; one event per group would flood the log with
identical rows.

Retuning the channel resets the RDS state: a different offset is a different station, and
merging two stations' PS segments produces a name that never existed.

## POCSAG

Type `pocsag`. Settings: `baud` (`auto`, 512, 1200, 2400), `bandwidth_hz`, `invert`.

FM discriminator → tracked slicing level → one bit clock per candidate rate. Whichever rate
finds the frame sync word takes the lock and releases it when sync is lost, so a frequency
carrying several rates is decoded per transmission rather than per configuration.

Emits the 21-bit address (RIC), the function bits, the rate the batch decoded at, numeric or
alphanumeric text, and the count of single-bit errors the BCH(31,21) decoder repaired — a
message with many corrections is a marginal one.

`invert` swaps mark and space. Some transmitters, and some receive chains, invert the
discriminator polarity, which turns every codeword into noise.

## ADS-B

Type `adsb`. Settings: `crc_fix`, `ref_lat`, `ref_lon`.

> [!IMPORTANT]
> The device must run at **exactly 2 Msps**. ADS-B fills its entire 2 MHz channel, so a
> resampling DDC cannot deliver it; the engine refuses the channel at any other rate rather
> than letting it decode nothing.

Preamble correlation is level-relative rather than threshold-based — an aircraft overhead and
one on the horizon differ by tens of dB — followed by PPM slicing at 2 samples per bit and the
Mode S CRC-24, with optional single-bit repair (`crc_fix`; turn it off on a noisy antenna to
trade sensitivity for fewer false frames).

**DF17/18 only.** Every other downlink format overlays the aircraft address on the parity, so
a zero syndrome there does not mean a valid frame — it means an invented aircraft.

Decodes identification (callsign), airborne and surface position (CPR: the global even/odd
pair, or a local solution against `ref_lat`/`ref_lon` from a single frame), velocity, and both
Gillham and 25 ft altitude encodings. The per-ICAO CPR cache is bounded and age-limited.

The event carries the ICAO address, downlink format, type code, and whichever of callsign,
altitude, position, speed, track, vertical rate, squawk and ground flag the frame actually
had, plus the raw frame as hex — the interop format every Mode S tool speaks.

> [!NOTE]
> Geometric-altitude frames (type codes 20–22) encode altitude in **feet**, exactly like
> 9–18: the type code selects the altitude *source*, not its encoding. Decoding those as
> metres is a real bug this project shipped and fixed; 12 bits of metres cannot even express
> FL380.

## AIS

Type `ais`. Setting: `ais_channel` (`a` = 161.975 MHz, `b` = 162.025 MHz) — a label carried
into the message; the tuning itself is the channel offset.

GMSK through the discriminator and a Gaussian matched filter, then NRZI, HDLC framing and
CRC-16/X-25. Message types 1/2/3 (position reports), 5 (static and voyage data), 18 and 24
(class B). "Unavailable" sentinels are honoured — an absent heading stays absent rather than
becoming 511.

Emits MMSI, message type, channel, name, call sign, destination, position, speed and course
over ground, heading, navigational status, and the `!AIVDM` sentence with its checksum for
tools that speak NMEA.

## APRS / AX.25

Type `aprs`. Settings: `mode` (`afsk1200` or `g3ruh9600`), `bandwidth_hz`.

Bell 202 AFSK via mark/space correlators, or 9600 baud G3RUH as descrambled NRZI straight off
the discriminator. Addresses decode with SSIDs and the has-been-repeated `*` marker.

Parses uncompressed and base-91 compressed positions, course and speed, and `/A=` altitude,
and emits source, destination, digipeater path, the raw information field, the parsed fields,
and the TNC2 monitor line.

The compression-type byte is honoured: a compressed report sourced from GGA carries altitude,
not a course and speed, and fabricating the latter was a real bug.

Mic-E is out of scope for wave 1. A Mic-E packet decodes as a valid AX.25 frame with no
position rather than a wrong one.

## RTTY

Type `rtty`. Settings: `baud` (45.45/50/75), `shift_hz` (170/450/850), `stop_bits`
(1, 1.5, 2), `invert`, `unshift_on_space`.

ITA2 (Baudot) with LTRS/FIGS shift handling and start/stop framing; a frame whose stop bit is
wrong is rejected rather than emitted as garbage. `unshift_on_space` returns to the letters
table after a space — the usual amateur convention, which recovers a stream that lost its
shift character.

Runs at an 8 kHz channel rate, not 48 kHz: a narrow filter at 48 kHz needs thousands of taps
to keep its shape factor, which blows the Raspberry Pi 4 budget for a single channel.

## Morse

Type `morse`. Settings: `bandwidth_hz` (default 400), `wpm` (`null` tracks the sender).

Envelope detection into an adaptive keying slicer, then element and gap clustering that
tracks the sending speed. With a fixed `wpm` it tolerates roughly ±30% sloppiness — which
real fists need. The event carries the decoded text and the speed the tracker settled on.

Unknown sequences surface as `*` rather than vanishing, so a mis-copy is visible. Pure noise
decodes to nothing.

Like RTTY, it runs at 8 kHz for filter-cost reasons.

## Loss is always visible

Decoded frames leave the DSP plane through a bounded queue and travel on their own broadcast,
separate from the control-event stream — ADS-B alone can produce hundreds of frames a second,
and a lagging control receiver resyncs with a full-state refetch that decode traffic must
never trigger.

Drops at any stage are counted and reported: as `DecodedLost { count }` to a client that fell
behind, and as a `dropped` count alongside every decoder-log listing. A missing frame is
reported, never silently absent.
