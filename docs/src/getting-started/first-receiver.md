# Your first receiver

This walkthrough uses the built-in signal generator. It verifies device capture, spectrum,
channelization, demodulation, WebSocket streaming, and browser audio without requiring an SDR or
antenna.

## 1. Open the signal generator

Start sdr-- and open its interface. A fresh installation creates a workspace with three nodes:

- a Device waiting for a radio;
- a Scope already connected to the Device's IQ output;
- a Speaker waiting for channel audio.

On the Device node, choose **Signal Generator (virtual)**. The device opens immediately and the
Scope begins drawing a synthetic spectrum and waterfall.

If you do not see the starter nodes, create them from **+ Node**. Draw a wire from the Device's
`IQ` port to the Scope's `IQ` port.

## 2. Add a channel

Choose **+ Node**, search for `NFM`, and add an NFM channel. Connect the nodes from left to right:

```text
Device IQ → NFM IQ
NFM audio → Speaker audio
```

The patch applies as you work. If a channel says it has not been created, press **Apply patch** on
that node.

Set the NFM channel offset to `+300 kHz`. The generator places an NFM carrier there with a 1 kHz
audio tone.

## 3. Start audio

Use the Speaker node's control. Your browser may require a click before it permits audio playback.
Adjust the channel squelch if the tone stays muted.

You now have a complete receiver:

```text
signal generator → spectrum + NFM demodulator → Opus stream → browser speaker
```

## 4. Explore the interface

Try these next:

- Drag the channel marker across the Scope to change its offset.
- Use the Device dial to retune the whole receiver.
- Press `[` or `]` to change the tuning step, then use the arrow keys to tune.
- Select a node and press `p` to pin its face to the Rack view.
- Open **Library → Templates** to inspect ready-made FM, airband, ADS-B, ACARS, AIS, APRS, pager,
  PMR446, digital voice, ISM, and HF setups. Templates that require real off-air traffic still
  configure the signal generator, but their decoders will remain quiet.
- Add a Recorder and wire the Device IQ output into it to create a short SigMF recording.

The `?` button in the top-right corner opens the complete keyboard reference.

## Move to real hardware

On the Device node, choose **Forget this radio**, then select the attached receiver. Device
controls are built from the capabilities reported by its driver, so gain stages, antennas,
sample rates, bandwidths, and advanced settings vary by model.

Run `sdrmm --doctor` or expand **Hardware not showing up?** in the device picker if your receiver
is missing. The [hardware guide](../hardware.md) covers supported modules and USB setup.
