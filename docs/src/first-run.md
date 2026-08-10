# First run

This walkthrough needs no hardware. The virtual signal generator is compiled into every
build, produces a realistic modulated spectrum, and is paced to real time — so the frame
rates, CPU load and audio latency you see are the ones you would get from a radio.

The client is a canvas. Your station is a patch: every radio, channel, scope, map and sink is
a node, and the wires between them are what makes it run. Nothing is hidden behind a settings
dialog — a node's face *is* its control surface.

## 1. Open a radio

Start the server and open <http://localhost:8080>. The station you land on is the one the
server seeds on first run: a **Receiver** node with nothing in it, a **Scope** wired to its
`iq` output, and a **Speaker** off to the side.

That receiver node is itself the "open a radio" prompt: it lists everything the probe found —
your SDRs first, then **Signal Generator (virtual)**, then one entry per finalized recording in
the recordings directory. Click the signal generator.

The node becomes the instrument: the tuning dial takes over its face, with the gain, rate and
driver-specific controls the device reports underneath. Every digit of the dial is its own
control — scroll it, click its upper or lower half, or focus it and use the arrows. To type a
frequency instead, focus the dial (`f`) and press Enter. The set starts at 100 MHz, 2.048 Msps,
and the scope on the other end of that `iq` wire fills immediately.

Drag a node by its header to move it; scroll to pan the canvas, ⌘/Ctrl+scroll to zoom. Press
`?` at any time for the keyboard list.

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

The scope face is one instrument wherever it is patched. Inside it: the wheel zooms about the
cursor, a drag pans, a click tunes (the selected channel if there is one, otherwise the radio
itself), a double-click re-centres the radio and resets the view. The divider between the
trace and the waterfall drags. Zoom is a client-side crop of the span the server streams, so
the readout at the top right always reports the *visible* window.

## 3. Add a channel and listen

Three gestures: add the node, wire it, wire its audio somewhere.

1. **Add it.** Open **+ Node** on the station bar and pick **NFM** under Channel. The node
   lands to the right of everything already drawn, with "no receiver" in its header — it is a
   channel that names a type and nothing else yet.
2. **Wire it.** Drag from the receiver's `iq` output to the channel's `iq` input. That wire is
   what says which radio the channel is on, so the server creates the channel the moment you
   draw it. Only wires the engine can honour are accepted: a wrong type, a second receiver on
   one channel, or a mode the radio's rate cannot carry is refused as you drag it, and the
   input simply will not take the wire.
3. **Point it at the carrier.** Type `300` into **Offset (kHz)** on the channel's face, or
   select the channel node and click the carrier in the scope — a click there tunes whatever
   channel is selected.

There is still no sound: a channel's audio belongs to whatever speaker its `audio` wire
reaches, which is why the channel face has no play button and says "audio out reaches no
speaker" instead. Drag from the channel's `audio` output to the speaker node's `audio` input,
then press **Play** on the speaker face. Browsers do not allow audio before a user gesture, so
that first press also unlocks the audio context. You should hear a clean 1 kHz tone.

What happened underneath: the DSP thread mixed the channel down with an NCO, decimated it in
stages to the NFM channel rate of 48 kHz, filtered it to the mode's bandwidth, ran a
quadrature discriminator, de-emphasis and AGC, encoded 20 ms Opus frames, and pushed them to
your browser over the WebSocket, where an AudioWorklet plays them out of a ~100 ms jitter
buffer.

Try the other two — one node each, both wired to the same receiver and the same speaker:

- **AM** at **−300 kHz** — the same 1 kHz tone through an envelope detector.
- **WFM** at **+600 kHz** — 200 kHz wide, demodulated at 240 kHz and resampled to 48 kHz.

Turn the squelch on in the channel face and drag the threshold up until the channel closes;
the audio stops without a click, because a closed squelch emits duration-exact silence rather
than skipping time.

## 4. Pin the faces you are operating

The canvas is where the station is built; the **rack** is where it is operated. Click the pin
(`▣`) in a node's header — the scope is the obvious first one — or press `p` with the node
selected, and the live face moves to the rack, leaving a "Pinned to the rack →" placeholder
behind on the canvas. Switch between the two with the **Patch / Rack** buttons on the station bar, or press
`v`.

The rack is a 24 × 24 grid: faces drag and resize by whole cells, and a placement that would
overlap is refused where it is rather than shoving anything aside. It is server state like the
rest of the station, so a reload — or a second browser — comes back to the same arrangement.
Leaving it empty is fine; the canvas alone is a complete UI.

## 5. Watch a decoder work

Generate the fixture set once:

```sh
cargo xtask fixtures
```

That renders one playable SigMF pair per wave-1 decoder into `fixtures/`, using the same
reference modulators the decoder unit tests use. Point the server at that directory:

```sh
cargo run -p sdrmm -- --recordings-dir fixtures
```

Each fixture now probes as a playable device, so it shows up in a receiver node's list as
`aprs_afsk1200_240k (recording)`. Add a second **Receiver** node from **+ Node** and pick it —
or press **Forget radio** on the one you have and pick it there.

Then wire up the decode path: an **APRS / AX.25** channel node on that receiver's `iq` at
**−40 kHz**, and its `events` output into a **Decoder log** node and a **Map** node.
`DL1ABC-9>APRS,WIDE1-1` appears with a position at 52.5 N, 13.4 E. The decoded output also
shows up in the channel node's own face — a decoder's first reader is the node that decodes.
An **Export** node wired to the same decoder downloads what the log stored, filtered to the
kinds its wires reach.

`fixtures/README.md` lists the channel type and offset for every fixture. ADS-B is the one
that will not play at an arbitrary rate — it fills its entire 2 MHz channel, so the device
must run at exactly 2 Msps. That wire is refused where you draw it, and a channel that has
already been created on a radio at the wrong rate says so across the top of its face, naming
the rate it needs.

## 6. Point it at real hardware

Plug in an RTL-SDR and reload. The probe picks it up within a few seconds, so it appears at
the top of any unbound receiver node's list — hardware is ranked above the virtual devices.
[Hardware](hardware.md) covers permissions, and the receiver face renders gain stages,
antennas, PPM and per-driver extras straight from what the device reports — no per-device UI
code exists.

A good first target is broadcast FM: tune to a strong local station, add a **WFM (mono)**
channel at offset 0, and enable **RDS** in its settings to see the station name and RadioText
in the channel's own face.

If you unplug that radio, its node does not disappear and is never rebound to something else:
it dims, says which radio it is waiting for, and keeps its wires. Plug it back in and it picks
up where it was.
