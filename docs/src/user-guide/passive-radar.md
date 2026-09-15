# Passive radar

Passive radar compares a transmitter's direct signal with its reflections to measure echo delay
and Doppler shift. Use two receiver lanes sharing a sample clock. A
[time-synced array](arrays.md) is sufficient; relative phase calibration is unnecessary.

## Set up the receiver

1. Add a Device or Array with at least two time-synced lanes.
2. Add **Passive radar**.
3. Connect the antenna aimed at the transmitter to `ref`.
4. Connect the surveillance antenna aimed at the area of interest to `surv`.
5. Connect GPS `position` for map output.

## Processing and settings

| Stage | Purpose |
|---|---|
| ECA | Cancel the direct signal and stationary clutter |
| CAF | Compare reference and surveillance signals across delay and Doppler offsets |
| CFAR | Detect cells above their local background |
| Cluster | Merge adjacent detections |
| Track | Associate echoes across observations |

| Setting | Effect |
|---|---|
| Integration | Longer intervals can reveal weaker echoes, but motion can blur them |
| Range bins | Delay extent of the display |
| Doppler span | Frequency-shift range searched |

## Reading the surface

The display plots range against Doppler and marks detections. Repeated observations receive a
track number. A brief detection may be noise or an unconfirmed echo.

## Echoes on the map

Enable **Transmitter** and enter its coordinates and frequency. With receiver position available,
the map draws an ellipse of possible locations for each echo.

The measurement is **bistatic range**, the extra distance travelled by the reflected signal.
One echo does not give a unique position or bearing. Tracks follow range and Doppler, not geographic
coordinates. Without transmitter coordinates, no ellipse is drawn.

Use the **Radar watch** mission in [field mode](field-mode.md) to view the surface and tracks on a phone.
