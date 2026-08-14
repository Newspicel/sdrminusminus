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
size. SDRplay is supported but not bundled because its runtime has separate redistribution terms.
Install an optional module into the private `soapy/lib/SoapySDR/modules0.8` directory and keep its
core ABI at 0.8; `sdrmm --doctor` shows exactly which module path and modules are in use.

## SDRplay

RSP1, RSP1A/B, RSP2, RSPduo, and RSPdx/R2 receivers work through
[SoapySDRPlay3](https://github.com/pothosware/SoapySDRPlay3). The tested baseline is module 0.5.2,
which requires SDRplay API 3.15 or newer. Install the API for your platform from
[SDRplay](https://www.sdrplay.com/api/) first; its service, library, and hardware driver are
commercial software restricted to genuine SDRplay hardware and are intentionally absent from
sdr-- artifacts.

For a source build or portable headless archive using the host's SoapySDR 0.8 runtime, build the
matching module into that runtime:

```sh
git clone --branch soapy-sdrplay3-0.5.2 --depth 1 \
  https://github.com/pothosware/SoapySDRPlay3.git
cmake -S SoapySDRPlay3 -B SoapySDRPlay3/build -DCMAKE_BUILD_TYPE=Release
cmake --build SoapySDRPlay3/build --parallel
sudo cmake --install SoapySDRPlay3/build
SoapySDRUtil --find="driver=sdrplay"
```

Desktop installers and the container use their private Soapy tree rather than the host module
directory. To extend one, place the module built against its SoapySDR 0.8 ABI in
`soapy/lib/SoapySDR/modules0.8` (the container path is
`/opt/conda/lib/SoapySDR/modules0.8`) and make the vendor API library and service available in
the same environment. This is necessarily a local extension: accepting and redistributing the
SDRplay API license cannot be automated by the release pipeline.

The RSPduo's Single Tuner, Dual Tuner, Master, 8 MHz Master, and Slave entries share a hardware
serial but are distinct choices. sdr-- retains the module's mode in the device key, so opening or
saving one mode does not silently select another. Dual Tuner mode exposes both receive streams;
sdr-- uses the separate stream handle SoapySDRPlay3 requires for each tuner instead of assuming
the module accepts one combined two-channel handle.

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
SoapySDRUtil --find="driver=sdrplay"
SoapySDRUtil --probe="driver=sdrplay"
```

For RTL-SDR, stream for at least 30 minutes, unplug/replug once, and verify tuning, manual and
digital AGC, gain, bias tee (when advertised), I/Q swap orientation, and both direct-sampling
branches including an HF frequency. For HackRF, repeat sustained RX and reconnect checks, then
transmit into a shielded attenuated load or legal bench loopback and verify TX frequency, gain
stages, amplifier, bias power, bandwidth, partial-buffer progress, stop, and RX/TX arbitration.
For SDRplay, repeat sustained RX and reconnect checks, verify AGC/manual gain, bandwidth,
antenna selection and every model-specific setting the module advertises; on an RSPduo, open each
offered mode and verify both streams independently in Dual Tuner mode.
Record the radio revisions, module versions shown by `--doctor`, test duration, overflow or
underflow counts, and result in the release checklist.

The removed native HackRF crate had an internal firmware-sweep implementation, but it was not
connected to a user-facing engine operation. The existing scanner still sweeps by retuning;
adding a frequency-stamped sweep stream remains separate work.
