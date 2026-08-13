# SoapySDR hardware

SoapySDR is the only local-radio abstraction used by normal sdr-- builds. The backend discovers
the device's RX and TX channels and exposes each direction independently, including frequency
and sample-rate ranges, bandwidths, antennas, gain stages, stream arguments, settings, clock
and time sources, hardware time, and duplex constraints. It never substitutes channel 0 for an
invalid channel. RX supervision retains reconnect and overflow reporting; TX handles partial
writes, timeouts, status/underflow events, orderly deactivation, and half-duplex arbitration.

The desktop app and container carry SoapySDR 0.8.1 and these base modules: RTL-SDR, HackRF,
Airspy, AirspyHF, bladeRF, LimeSDR, PlutoSDR/libiio, and SoapyRemote. Their transitive libraries
and package license metadata are bundled too. UHD is kept out of the base package because of its
size. SDRplay remains an optional pack because its runtime has separate redistribution terms.
Install an optional module into the private `soapy/lib/SoapySDR/modules0.8` directory and keep
its core ABI at 0.8; `sdrmm --doctor` shows exactly which module path and modules are in use.

## RTL-SDR settings

Controls come from the module's `getSettingInfo()` metadata rather than a device-name table.
Current SoapyRTLSDR builds advertise `direct_samp`, `iq_swap`, `offset_tune`, `digital_agc`, and
optionally `biastee`, `testmode`, and `dithering`. `iq_swap` is an independent advanced boolean:
it exchanges I and Q and reverses spectral orientation. Settings that change capabilities are
written and read back before validation, so selecting direct sampling refreshes the HF tuning
range instead of validating against the former tuner-only range.

## Manual hardware validation

CI never enumerates modules installed on its host and no hardware test is required for ordinary
tests. Before a release, run the following with the candidate package and the named radio only:

```sh
sdrmm --doctor
SoapySDRUtil --find="driver=rtlsdr"
SoapySDRUtil --probe="driver=rtlsdr"
SoapySDRUtil --find="driver=hackrf"
SoapySDRUtil --probe="driver=hackrf"
```

For RTL-SDR, stream for at least 30 minutes, unplug/replug once, and verify tuning, manual and
digital AGC, gain, bias tee (when advertised), I/Q swap orientation, and both direct-sampling
branches including an HF frequency. For HackRF, repeat sustained RX and reconnect checks, then
transmit into a shielded attenuated load or legal bench loopback and verify TX frequency, gain
stages, amplifier, bias power, bandwidth, partial-buffer progress, stop, and RX/TX arbitration.
Record the radio revisions, module versions shown by `--doctor`, test duration, overflow or
underflow counts, and result in the release checklist.

The removed native HackRF crate had an internal firmware-sweep implementation, but it was not
connected to a user-facing engine operation. The existing scanner still sweeps by retuning;
adding a frequency-stamped sweep stream remains separate work.
