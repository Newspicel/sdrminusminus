# Your first receiver

Listen to a local FM station with an RTL-SDR. You need the receiver, an antenna, and
[SDR-- installed](install.md).

## 1. Pick the radio

Attach the antenna and plug the RTL-SDR into the computer running the server. Open SDR--.

A new installation starts with a **Device** wired to a **Scope**, and a **Speaker**. Pick your
RTL-SDR on the Device node and set the rate to **2.4 MS/s**.

Radio not listed? Press **Check hardware** on the Device node, or see
[RTL-SDR](../hardware.md#rtl-sdr).

## 2. Add a WFM channel

WFM is broadcast FM. Press **+ Add**, search for **WFM**, and add it. Wire it up:

```text
Device iq   → WFM iq
WFM audio   → Speaker audio
```

Set the WFM dial to a station you know is on air locally. The Device follows the channel on its
own, so you do not need to tune the radio.

## 3. Listen

Start the Speaker. If it stays silent:

- Click the page once. Browsers block audio until you do.
- Turn squelch off on the channel.
- Check the channel marker sits on the station in the Scope.

Adjust the Device gain until the station stands clearly above the noise without clipping. More
help: [silent audio](../troubleshooting.md#spectrum-works-but-audio-is-silent).

## 4. Station name and text

Add a **Readout** and wire WFM `events` to it. It shows the RDS station name and radio text when
the station sends them and the signal is strong enough.

## 5. Arrange

Select a node and press `p` to pin it to the Rack. Press `v` to switch between Patch and Rack.
Everything saves on its own.

## Next

- [Nodes and wires](workspace.md) explains what you just built.
- **Library → Templates** sets up other receivers in one click.
- The [decoder catalog](../user-guide/decoders.md#catalog) lists every mode.
