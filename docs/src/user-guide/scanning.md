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
3. Enter frequency ranges and choose a mode.
4. Set the detection level and start.

| Mode | Stops on |
|---|---|
| Targets | A listed frequency above the threshold |
| Close call | The strongest carrier above the noise floor in the whole span |

Match the step to the service's channel spacing. The scanner measures each target over the
channel's own bandwidth, so a narrow channel scans selectively and a wide one forgivingly.

On a hit the channel parks there so you hear it. The scan resumes after the signal has been quiet
for the resume delay. **Skip** leaves the current frequency and ignores it for the rest of this
scan. The channel's dial is locked while scanning, and stays on the last frequency when you stop.

Radios that support it sweep in firmware, which pauses other channels on that radio while it
runs. Otherwise the scanner steps. The **Sweep** readout shows which one is in use.

## Identify a signal

Add **Signal identifier** and select a span up to 192 kHz wide. It lists each transmission,
strongest first, with modulation, bandwidth, symbol rate, deviation, and burst timing.

For each one it suggests likely protocols. A suggestion marked **Confirmed** was proven by a real
decoder finding valid frames. The others are guesses from the waveform and the band.

**Interval** sets how long it listens per report. **Threshold** sets how far above the noise a
signal must be. A quiet span is reported once, not every interval.

It cannot see spread-spectrum signals below the noise, or separate tightly packed HF signals like
FT8.

## Monitor a band

Wire **Device iq → Spectrum monitor → Decoder log**. The monitor watches the Device's whole window,
finds every transmission, and tries the matching decoders on each one. It adds no channel nodes
and never tunes the radio.

**Protocols** picks what it decodes. Everything is on by default. Pick a preset such as
**Analog voice**, or toggle single protocols. Off protocols are skipped, not logged.

Each transmission produces one event when it ends, with frequency, bandwidth, confidence, decoder
results, and optional audio. Open it in the log to play the audio.

| Setting | Does |
|---|---|
| Record audio | Attaches an 8 kHz WAV clip. Long signals get a clip every 30 seconds. |
| Min confidence | Skips weaker guesses. Default 70%, 0 accepts everything. |

Limits: 32 signals at once, three decoder attempts per signal, two seconds of IQ kept for late
decoders. Anything dropped is reported. Pictures and video are not decoded here.

## Hunt a transmitter

**Signal hunt** reads one channel's signal strength fast enough to walk with. Wire its `control`
to the channel's `control` and start it. Retune the channel to retune the hunt. On a phone, use
the **Fox hunt** mission in [field mode](field-mode.md).

## Survey an area

1. Add **Signal survey** and wire Device `iq` and GPS `position` to it.
2. Pick an offset inside the Device's window and a measurement width.
3. Wait for a level and a GPS fix, then start.
4. Export the results as CSV when done.

Each GPS fix records the peak level in dBFS within the slice, grouped into cells of about ten
metres. Keep gain, antenna, and width the same, or the numbers will not compare. Pause before you
change the receiver.
