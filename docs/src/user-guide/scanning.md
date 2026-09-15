# Scanning

Use **Scanner** to search frequency lists or ranges and hold on activity. Scanning controls the
radio's tuning, so stop it before retuning manually.

## Build a scanner

1. Add **Scanner** from **+ Node**.
2. Connect Scanner `control` to Device `control`.
3. Enter frequency ranges or targets and choose a scan mode.
4. Set the detection level and timing.
5. Start scanning and watch the frequency, level, hit count, and status.

## Configure detection

| Mode | Behaviour |
|---|---|
| Targets | Hold on a listed frequency above the threshold |
| Close call | Find the strongest carrier above the noise-floor margin within the searched span |

For target ranges, match the step to the service's channel spacing. Smaller steps cover more
frequencies per sweep and take longer. Use longer dwell times for weak signals or short digital
bursts. Measurement bandwidth sets the slice used to measure activity.

The resume delay controls how long the scanner waits after activity ends before continuing.

## Listen to a detected signal

Add a channel with the required mode and connect it to a Speaker. Select it under **Listen on**.
When the scanner holds on a signal, it tunes that channel to the detected frequency.

Other channels retain their frequencies and receive only while the radio covers them.
For continuous reception across a fixed band, use ordinary channels without a scanner.

## Sweep methods

On supported hardware, the scanner can use the radio's firmware sweep. Otherwise it retunes
through the targets. If firmware sweeping fails, it returns to retuning and reports the change.
The **Sweep** readout shows the method in use.

Firmware sweeping interrupts ordinary reception while active. Channels are restored when normal
reception resumes. Retuning sweeps need time for the radio and processing to settle.

## Multiple radios

Use **Also sweep with** to share targets across eligible running radios. Each must be free of
another scan or hunt and support a single tuning control. Receivers with independently tuned streams cannot
participate in this scan workflow.
