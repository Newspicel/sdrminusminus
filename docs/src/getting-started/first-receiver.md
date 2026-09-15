# Your first receiver

Listen to a local FM broadcast station with an RTL-SDR. You need the receiver, an antenna, and
an [installed copy of sdr--](install.md).

## 1. Connect the RTL-SDR

Attach the antenna and plug the receiver into the computer running sdr--. For a remote setup,
plug it into the server.

Open sdr-- and select your RTL-SDR on the **Device** node. A new installation also includes a
connected **Scope** and a **Speaker**. If these nodes are missing, add them from **+ Node** and
connect Device `IQ` to Scope `IQ`.

If the radio is missing, open **Check hardware** on Device. The
[hardware guide](../hardware.md#rtl-sdr) covers driver requirements and USB permissions.

## 2. Tune a broadcast station

On Device, set the sample rate to **2.4 MS/s** and tune to a local FM station's frequency.
For example, enter `100.0 MHz` only if a station broadcasts there in your area.

Start with moderate tuner gain. Adjust it until the station is visible on the Scope without
clipping or a large rise in the surrounding noise.

## 3. Add a WFM channel

Choose **+ Node**, search for **WFM**, and add it. WFM is the mode for broadcast FM.
Connect the nodes:

```text
Device IQ → Scope IQ
Device IQ → WFM IQ
WFM audio → Speaker audio
```

Set the WFM channel to the station's frequency. The Device dial selects the received frequency
range; the channel dial selects one station inside it. Both must cover the station.

Changes apply automatically. Press **Apply patch** if the node requests it.

## 4. Start audio

Start playback on the Speaker and adjust the volume. If audio stays silent:

- Turn off channel squelch temporarily.
- Check that the channel marker covers the station on the Scope.
- Click the page to allow browser audio, and check the system output device.

See [audio troubleshooting](../troubleshooting.md#spectrum-works-but-audio-is-silent) if needed.

## 5. Arrange your receiver

Select the controls you use most and press `p` to pin them to **Rack** view. Press `v` to switch
between Patch and Rack. Your layout is saved automatically.

To display station names and radio text, connect WFM `events` to a **Readout**. RDS appears when
the station transmits it and reception is strong enough.

## Next steps

- Learn [workspace controls](workspace.md) and [keyboard shortcuts](../user-guide/keyboard.md).
- Use **Library → Templates** for other receiver setups.
- [Record IQ or audio](../user-guide/recording.md) for later use.
- Explore the [channel catalog](../user-guide/channels.md#channel-catalog).
