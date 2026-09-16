# Scanning

Use **Scanner** to search frequency lists or ranges and hold on activity. A scanner drives one
decoder: every hit parks that decoder on the signal, and the radio carrying it follows. While a
scan runs it owns the radio's tuning, so stop it before retuning manually.

## Build a scanner

1. Add a channel with the mode you want to hear and connect it to a Speaker.
2. Add **Scanner** from **+ Node** and connect Scanner `control` to the channel's `control`.
3. Enter frequency ranges and choose a scan mode.
4. Set the detection level and start scanning.

The scanner measures each target over the decoder's own bandwidth, so a narrow decoder makes a
selective scan and a wide one a forgiving one.

## Configure detection

| Mode | Behaviour |
|---|---|
| Targets | Hold on a listed frequency above the threshold |
| Close call | Find the strongest carrier above the noise-floor margin within the searched span |

For target ranges, match the step to the service's channel spacing. Smaller steps cover more
frequencies per sweep and take longer.

The scanner resumes once the signal has been quiet for the resume delay, and carries on with
the targets after the one it held on.

## Skip a frequency

Press **Skip** while the scanner holds to leave that frequency and never hold on it again during
this scan. The **Skipped** readout counts them.

## Listen to a detected signal

The decoder the scanner drives follows the sweep: its dial shows the frequency being checked,
it parks on every hit so whatever it feeds hears the signal, and when the scan stops it stays
where the scan left it while the radio settles over it. The dial is locked while a scan drives
it.

Other channels on the same radio keep their frequencies and receive only while the radio covers
them. For continuous reception across a fixed band, use ordinary channels without a scanner.

## Sweep methods

On supported hardware, the scanner can use the radio's firmware sweep. Otherwise it retunes
through the targets. If firmware sweeping fails, it returns to retuning and reports the change.
The **Sweep** readout shows the method in use.

Firmware sweeping interrupts ordinary reception while active. Channels are restored when normal
reception resumes. Retuning sweeps need time for the radio and processing to settle.

## Signal hunt

**Signal hunt** reads the strength of one decoder's frequency fast enough to walk with. Connect
Signal hunt `control` to a channel's `control` and start it. Retuning the decoder retunes the
hunt. The hunt never moves the radio: with auto tuning the radio already sits over the decoder,
and a radio locked elsewhere makes the hunt report that it cannot hear.
