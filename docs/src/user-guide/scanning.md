# Scanning

The Scanner node repeatedly retunes one device and measures activity across a list or range of
frequencies. Use it to find intermittent signals that are easy to miss while parked on one
channel.

## Build a scanner

1. Add a Scanner from **+ Node**.
2. Wire the Scanner's `control` output into the Device's `control` input.
3. Enter the scan targets or ranges in the Scanner face.
4. Choose the step, dwell time, threshold, and action.
5. Start the scan.

The control wire represents ownership. While a scan runs, it owns the device center frequency and
manual retuning is refused. Stop the scanner to return the dial to normal operation.

## Configure the sweep

A range consists of a start frequency, end frequency, and step. Keep the step aligned with the
channel spacing used by the service you are monitoring. A smaller step examines more frequencies
but lengthens each sweep.

The dwell time controls how long the scanner observes each target. Digital bursts and weak
squelched voice may need a longer dwell; strong continuous carriers can use a shorter one.

The activity threshold is measured from the device spectrum. Set it above the local noise floor,
then adjust after watching several sweeps.

## Scan actions

The scanner can continue through active signals or hold according to its configured action. Its
live face reports the current frequency, progress, detected level, state, and any fault.

Scanning retunes the whole device, so channels attached to that device move with it. For a
listening scanner, configure a channel at offset zero with the appropriate mode and connect it to
a Speaker. For a fixed wideband task such as two-channel AIS, use normal channels instead of a
retuning scanner.

## Practical limits

The current scanner sweeps by retuning the receiver. It does not use firmware-assisted wideband
sweep modes, and each retune needs time for the hardware and DSP path to settle. Scanning very
large ranges is therefore best divided into smaller service-specific workspaces.
