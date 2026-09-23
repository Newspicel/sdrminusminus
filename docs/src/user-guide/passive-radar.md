# Passive radar

Passive radar compares a broadcast transmitter's direct signal with its echoes off aircraft and
other objects. It measures how much further each echo travelled and its Doppler shift. Two lanes
on a shared clock are enough; a [`time_sync` array](arrays.md) works without phase calibration.

## Set up

1. Add a Device or Array with at least two time-synced lanes.
2. Add **Passive radar**.
3. Wire the antenna pointing at the transmitter to `ref`.
4. Wire the antenna pointing at the area you watch to `surv`.
5. Wire [GPS position](position.md) for the map.

| Setting | Does |
|---|---|
| Integration | Longer finds weaker echoes, but blurs moving ones |
| Range bins | How far out the display reaches |
| Doppler span | How large a frequency shift to search |

Processing runs in five steps: cancel the direct signal and ground clutter, correlate reference
with surveillance, detect cells above their background, merge neighbours, and track echoes over
time.

## Read the display

The display plots range against Doppler and marks detections. An echo seen repeatedly gets a
track number. A single flash may be noise.

## On the map

Turn on **Transmitter** and enter its position and frequency. With your own position known, the
map draws an ellipse of possible locations for each echo.

The range is **bistatic**: the extra distance the echo travelled compared with the direct path.
One echo gives an ellipse, not a point or a bearing.

On a phone, use the **Radar watch** mission in [field mode](field-mode.md).
