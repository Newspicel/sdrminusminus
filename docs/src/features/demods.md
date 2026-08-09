# Demodulators

A *channel* is a demodulator or decoder hosted on a device set at an offset from the device
centre frequency. Channels are independent: several can run on one device set, at different
offsets, in different modes, with different listeners.

## Channel types

| Type | Name | Nominal RF bandwidth | DDC output rate | Audio | Decoder |
|---|---|---|---|---|---|
| `nfm` | NFM | 12.5 kHz | 48 kHz | yes | — |
| `am` | AM | 10 kHz | 48 kHz | yes | — |
| `ssb` | SSB | 3 kHz | 48 kHz | yes | — |
| `wfm` | WFM (mono) | 200 kHz | 240 kHz | yes | RDS, optional |
| `pocsag` | POCSAG | 12.5 kHz | 48 kHz | no | pager messages |
| `adsb` | ADS-B (1090ES) | 2 MHz | 2 MHz | no | aircraft |
| `ais` | AIS | 25 kHz | 48 kHz | no | ships |
| `aprs` | APRS / AX.25 | 12.5 kHz | 48 kHz | no | packets |
| `rtty` | RTTY | 1 kHz | 8 kHz | no | text |
| `morse` | Morse (CW) | 400 Hz | 8 kHz | no | text |

The channel type list is served by the API, so the "add channel" UI is generated from the
registry and cannot drift from what the server can actually build. The decoders are covered
in [Decoders](decoders.md).

## The signal path

```
device IQ → DC/IQ correction → DDC (NCO mix → staged decimation → fractional resample)
          → mode-aware complex channel filter → squelch → demod → AGC → 48 kHz PCM
          → Opus (20 ms frames) → WebSocket
```

The DDC produces the descriptor's exact input rate for any device rate: a chain of polyphase
decimators followed by a 128-phase fractional resampler. Retuning the offset is cheap — it
moves the NCO, it does not rebuild the chain.

The channel filter is complex and mode-aware. It is what makes the squelch meaningful:
squelch power is measured on the filtered channel, in the mode's occupied bandwidth, so a
threshold means the same thing in NFM as in SSB.

## Two rules that reject channels

Both exist because the alternative is a channel that appears to work and produces nothing.

**The occupied band must fit the device passband.** The band is computed from the *configured*
parameters, not the descriptor nominal — a wide NFM setting or SSB's one-sided occupancy can
stick out past Nyquist even though the nominal fits. A channel whose band exceeds
`±rate/2` is refused with the numbers in the message.

**A resampling DDC cannot deliver its full output rate.** A rate conversion needs a guard band
for the filter transition, so roughly 80% of the output rate is flat and the rest is guard.
A mode that occupies its entire channel can therefore only be served when the device runs at
exactly the channel rate. ADS-B is the one such mode today: at 2.4 Msps its 0.5 µs pulses
arrive smeared and the decoder finds nothing — which is indistinguishable from an empty sky.
The engine refuses the channel and names the rate that works (2 Msps).

## Mode settings

Each mode has a typed settings struct; the client form is generated from it, so a new
parameter appears in the UI with no frontend change.

`nfm`
  : `bandwidth_hz` (default 12 500).

`am`
  : `bandwidth_hz` (default 10 000), `agc` (default on). Envelope detection with a DC
    blocker.

`ssb`
  : `sideband` (`usb`/`lsb`), `bandwidth_hz` (default 2 700), `agc` (default on). Complex
    filter, one-sided occupancy — the occupied band is asymmetric around the offset, which is
    exactly what the validation above accounts for.

`wfm`
  : `deemphasis_us` (50 in most of the world, 75 in the Americas), `rds` (default off).
    Demodulated at 240 kHz and resampled to 48 kHz. Stereo is deliberately not implemented
    yet: it changes the entire audio path (two-channel PCM, Opus channel layout, the
    AudioWorklet) and is tracked separately from RDS.

Squelch (`squelch_db`) and the offset (`offset_hz`) are common to every channel. A closed
squelch emits duration-exact silence to decoders rather than skipping the gated span —
deleting time would corrupt a decoder's bit clock. Audio demods keep the cheaper skip, which
is where the CPU saving actually matters.

## Audio

- Demods emit 48 kHz mono PCM, encoded to Opus in 20 ms frames by a per-channel encoder
  thread, and pushed as binary WebSocket frames.
- The browser decodes with WebCodecs `AudioDecoder` where available and a WASM Opus decoder
  otherwise, then plays through an AudioWorklet with a ~100 ms jitter buffer that rebuffers
  on underrun and drops oldest past 400 ms.
- Multiple clients can listen to the same channel or to different ones. Mixing is client-side
   — they are just streams. Per-channel gain is client-side too.
- Browsers block audio until a user gesture; the first play unlocks the shared audio context.
- If the encoder falls behind, the dropped PCM appears as a timestamp gap rather than
  silently shortened audio, and the client resets its buffer.

## Changing a channel

`PATCH` on the channel applies a delta. An offset-only change is a retune (NCO only); a
parameter change reconfigures the mode in place; changing the *type* rebuilds the pipeline
while keeping the channel id and its audio subscribers, so a listener does not have to
resubscribe.

Changing the **device** sample rate rebuilds every hosted channel. The rebuild is
pre-validated against the new rate first, so a rate that would invalidate a channel is
rejected before anything is torn down, and channel ids and audio subscriptions survive.
