# Passive radar

Passive radar detects reflections of an existing transmitter, usually a broadcast station.
A reference antenna receives the transmitter directly; a surveillance antenna receives the area
of interest. Comparing the signals gives the echo's delay and Doppler shift.

The two receiver lanes must share a sample clock. A [time-synced array](arrays.md) is sufficient;
relative phase calibration is not required.

## Wire one up

1. Set up a Device or Array with at least two time-synced lanes.
2. Add a **Passive radar** node.
3. Connect the antenna aimed at the transmitter to `ref` and the surveillance antenna to `surv`.
4. Connect a **GPS position** source to `position` if you want map output.

## Processing and settings

| Stage | Purpose |
|---|---|
| ECA | Cancel the direct signal and zero-Doppler clutter in the surveillance lane |
| CAF | Correlate the remaining signal with the reference across delay and Doppler offsets |
| CFAR | Detect cells above a threshold calculated from their neighbourhood |
| Cluster | Merge adjacent detections into one echo |
| Track | Associate echoes across successive integrations |

**Integration** sets the coherent processing interval. Longer intervals can reveal weaker echoes,
but target motion during the interval can blur them. **Range bins** sets the delay extent of the
surface. **Doppler span** sets the frequency-shift range searched.

## Reading the surface

The display plots range against Doppler and marks detections. New detections have no target number.
After repeated observations, the tracker assigns a number and retains it as the echo moves.
A brief detection may be noise or an echo the tracker cannot confirm.

## Echoes on the map

A detection measures **bistatic range**: the extra distance travelled by the reflected signal
compared with the direct path. Possible target locations lie on an ellipse whose foci are the
transmitter and receiver. A detection alone does not provide a target position or bearing.

Enable **Transmitter** and enter its latitude, longitude, and frequency. With the receiver position
available, the map can draw the ellipse for each echo. Without transmitter coordinates, no ellipse
is drawn.

Tracking operates in range and Doppler. The map does not track geographic target positions.

## In the field

The **Radar watch** mission shows the range–Doppler display and tracked echoes on a phone.
See [field mode](field-mode.md).
