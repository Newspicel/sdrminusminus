# Finding signals

| Node | Use it to |
|---|---|
| [Scanner](#scan) | Step through frequencies and stop on activity |
| [Signal identifier](#identify-a-signal) | Name an unknown signal |
| [Spectrum monitor](#monitor-a-band) | Catch and decode everything in the Device's window |
| [Signal hunt](#hunt-a-transmitter) | Walk towards a transmitter by signal strength |
| [Signal survey](#survey-an-area) | Map signal strength while you move |

**Library → Occupancy** shows how busy each frequency on the selected Device has been, hour by
hour.

## Scan

A **Scanner** drives one channel, never the radio. On auto tuning the radio follows the channel.
Tuned by hand, the radio stays put and the scan skips targets outside its window.

1. Add a channel in the mode you want to hear and wire it to a Speaker.
2. Add **Scanner** and wire its `control` to the channel's `control`.
3. Set each range's **From**, **To** and **Step**. **Add range** adds another.
4. Pick what to **Find** and press **Start scan**.

| Find | Stops on |
|---|---|
| Listed | A listed frequency above **Threshold** (default -55 dB) |
| Strongest | The strongest carrier at least **Over noise** above the noise floor (default 12 dB) |

Match the step to the service's channel spacing. The scanner measures each target over the
channel's own bandwidth, so a narrow channel scans selectively and a wide one forgivingly.

On a hit the channel parks there so you hear it. The scan resumes after 1.5 s of quiet. **Skip**
leaves the current frequency and ignores it for the rest of this scan. The channel's dial is locked
while scanning, and stays on the last frequency after **Stop scan**.

Radios that support it sweep in firmware, shown as **Firmware sweep** (on by default). Other
channels on that radio pause while it runs. The **Sweep** readout shows `the radio's own` or
`by retuning`.

## Identify a signal

Add **Signal identifier** from the **Utility** channels. It lists each transmission in its window,
strongest first, with modulation, bandwidth, SNR, symbol rate, deviation, and burst timing.

| Setting | Does |
|---|---|
| Width | Span to search, up to 192 kHz |
| Every | How long it listens per report. Default 1,000 ms. |
| Detect | How far above the noise a signal must be. Default 8 dB. |

For each signal it suggests likely protocols. A suggestion marked `confirmed` was proven by a
decoder or a frame sync found in the signal. The others are guesses from the waveform and the band.
A quiet span is reported once, not every interval.

It cannot see signals below the noise, such as spread spectrum, or separate tightly packed HF
signals like FT8.

## Monitor a band

Wire **Device iq → Spectrum monitor → Decoder log**. The monitor watches the Device's whole window,
finds every transmission, and tries the matching decoders on each one. It adds no channel nodes
and never tunes the radio.

**Protocols** picks what it decodes. Everything is on by default. Pick a preset such as
**Analog voice**, or toggle single protocols. Off protocols are skipped, not logged.
**Unidentified signals** also logs transmissions no protocol matches.

Decoded messages arrive as they happen. Each transmission also gives one event when it ends, with
frequency, bandwidth, confidence, the decoder used, and optional audio. Long transmissions report
every 30 seconds. Open an event in the log to play the audio.

| Setting | Does |
|---|---|
| Clips | Attaches an 8 kHz WAV clip. On by default. |
| Min | Skips weaker guesses. Default 70%, 0 accepts everything. |

Clips stay for up to 24 hours. To keep them, also wire `events` to an
[Event output set to Recordings](recording.md#record-each-call).

Limits: 32 signals at once, three decoders at a time per signal, two seconds of IQ kept for late
decoders. Anything dropped is reported. Pictures and video are not decoded here.

## Hunt a transmitter

**Signal hunt** reads one channel's signal strength fast enough to walk with. Wire its `control`
to the channel's `control` and press **Start hunt**. Retune the channel to retune the hunt.
With **Clicks** on, Geiger clicks speed up as the signal gets stronger.

### Sweep for a bearing

With a directional antenna and a [phone](phones.md) you get bearings, not only warmer and colder:

1. Wire a GPS node with the **Phone** source to the hunt's `position`.
2. Hold the antenna and the phone together. **Mount** sets the antenna's direction relative to
   the phone.
3. Press **Sweep** and turn slowly all the way round. The rose fills in and **Peak** shows the
   strongest direction.
4. When **Sweep** reads `Done`, the bearing is sent on `events`.

**Mark** sends the direction you face as a bearing. Wire `events` to a
[Triangulation](direction-finding.md#triangulate) to cross bearings from several places.
**End sweep** goes back to warmer and colder.

## Survey an area

1. Add **Signal survey** and wire Device `iq` and GPS `position` to it.
2. Set **Offset** inside the Device's window and a measurement **Width**.
3. Wait for a level and a GPS fix, then press **Start survey**.
4. Press **Export CSV** when done.

Each GPS fix records the peak level in dBFS within the slice, averaged into cells of about ten
metres. Keep gain, antenna, and width the same, or the numbers will not compare. **Pause**
before you change the receiver. Offset and width are fixed until you **Clear** the survey.

The server keeps surveying with the page closed, so a phone can run it as a Survey mission. It
keeps up to 5,000 cells in memory and drops the oldest beyond that. A server restart loses them.
