# Satellites

Use **Satellite** to follow a satellite across the sky. It tunes every decoder wired to it to the
downlink and corrects the Doppler shift continuously, so the decoder never chases the carrier.

## Build a satellite receiver

1. Add a **GPS position** node. A fixed position works for a station that never moves.
2. Add **Satellite** from **+ Node** and connect GPS `position` to Satellite `position`.
3. Search for the satellite by name or NORAD number, or paste element lines.
4. Pick a transmitter, or type the downlink.
5. Connect Satellite `control` to each decoder's `control`.

The radio follows the decoder, as it does for a scanner.

## What it shows

| Readout | Meaning |
|---|---|
| Look | Azimuth and elevation from your position |
| Doppler | Shift added to the decoders, and how fast it changes |
| Send on | The uplink corrected for Doppler, when the transmitter has one |
| Next pass | Time to rise, or time to set during a pass |
| Elements | Age of the orbit data. Refresh them when they turn yellow |

Element sets come from CelesTrak and transmitter lists from SatNOGS DB. Both are cached for two
hours.

## Doppler and the decoder

The satellite node removes the predicted Doppler shift. What remains is the radio's own
oscillator error and a small orbit error. Decoders that lock a carrier, such as DVB-S2, pull
that in themselves.
