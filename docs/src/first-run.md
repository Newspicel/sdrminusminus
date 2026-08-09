# First run

This walkthrough needs no hardware. The virtual signal generator is compiled into every
build, produces a realistic modulated spectrum, and is paced to real time — so the frame
rates, CPU load and audio latency you see are the ones you would get from a radio.

## 1. Open a device

Start the server and open <http://localhost:8080>. The device bar lists everything the
probe found: your SDRs, one entry per finalized recording in the recordings directory, and
**Signal Generator (virtual)**.

Click **Open** on the signal generator. That creates a *device set*: an opened device, its
capabilities, its settings, and the channels hosted on it. You can open as many device sets
as you have devices; every panel below is scoped to one of them.

The set starts at 100 MHz, 2.048 Msps. The spectrum panel fills immediately and the waterfall
starts scrolling.

## 2. Read the spectrum

The signal generator emits a deterministic scene, so you can check your setup against it:

| What | Where | Detail |
|---|---|---|
| Three static tones | at 15%, 5% and −30% of the sample rate | Fixed fractions of `fs`, so they move when you change the rate |
| One drifting tone | sweeping ±35% of the sample rate | ~0.08 Hz sweep — the moving line in the waterfall |
| NFM carrier | **+300 kHz** | 2.5 kHz deviation, modulated with a 1 kHz tone |
| AM carrier | **−300 kHz** | 60% modulation depth, 1 kHz tone |
| WFM carrier | **+600 kHz** | 75 kHz deviation, 1 kHz tone |
| Noise floor | everywhere | Low-level, so squelch and AGC behave like they do on air |

The modulated carriers sit at fixed **Hz** offsets rather than fractions of the sample rate,
so a channel addresses them identically at any rate. A carrier whose occupied band would
cross Nyquist is muted rather than allowed to alias — at 250 ksps you will not see the WFM
carrier at all, which is correct.

## 3. Add a channel and listen

In the **Channels** panel choose **NFM** and click **Add channel**. Set the offset to
**+300 kHz** (the field steps in kHz). Press play.

Browsers do not allow audio before a user gesture, so the first play also unlocks the audio
context. You should hear a clean 1 kHz tone.

What happened underneath: the DSP thread mixed the channel down with an NCO, decimated it in
stages to the NFM channel rate of 48 kHz, filtered it to the mode's bandwidth, ran a
quadrature discriminator, de-emphasis and AGC, encoded 20 ms Opus frames, and pushed them to
your browser over the WebSocket, where an AudioWorklet plays them out of a ~100 ms jitter
buffer.

Try the other two:

- **AM** at **−300 kHz** — the same 1 kHz tone through an envelope detector.
- **WFM** at **+600 kHz** — 200 kHz wide, demodulated at 240 kHz and resampled to 48 kHz.

Turn the squelch on and drag the threshold up until the channel closes; the audio stops
without a click, because a closed squelch emits duration-exact silence rather than skipping
time.

## 4. Watch a decoder work

Generate the fixture set once:

```sh
cargo xtask fixtures
```

That renders one playable SigMF pair per wave-1 decoder into `fixtures/`, using the same
reference modulators the decoder unit tests use. Point the server at that directory:

```sh
cargo run -p sdrmm -- --recordings-dir fixtures
```

Each fixture now appears in the recordings list as a playable device. Open
`aprs_afsk1200_240k`, add an **APRS / AX.25** channel at **−40 kHz**, and open the
**Decoder log** panel: `DL1ABC-9>APRS,WIDE1-1` appears with a position at 52.5 N, 13.4 E,
and the map panel plots it.

`fixtures/README.md` lists the channel type and offset for every fixture. ADS-B is the one
that will not play at an arbitrary rate — it fills its entire 2 MHz channel, so the device
must run at exactly 2 Msps and the engine refuses the channel otherwise, naming the rate that
works.

## 5. Point it at real hardware

Plug in an RTL-SDR and reload. The device bar picks it up (the server re-probes
periodically); [Hardware](hardware.md) covers permissions, and the device panel renders
gain stages, antennas, PPM and per-driver extras straight from what the device reports — no
per-device UI code exists.

A good first target is broadcast FM: tune to a strong local station, add a **WFM (mono)**
channel at offset 0, and enable **RDS** in its settings to see the station name and
RadioText.
