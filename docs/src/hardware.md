# Hardware

RTL-SDR and HackRF are the default hardware. Everything else rides SoapySDR. Recordings and
the built-in signal generator are devices too, so the whole application is usable with no
radio attached.

## Backends

| Backend | Feature | What it covers |
|---|---|---|
| `device-soapy` | `soapy` (default) | Anything with a SoapySDR module: RTL-SDR, HackRF, Airspy, SDRplay, LimeSDR, PlutoSDR, BladeRF, USRP, … |
| `device-rtlsdr` | `rtl-native` | RTL-SDR over pure-Rust USB (`nusb`): the tuner's own gain table instead of a range, plus bias tee, tuner AGC and crystal (PPM) correction |
| `device-hackrf` | `hackrf-native` | HackRF over pure-Rust USB: the real per-stage gain model (LNA and VGA separately, each on its own step grid), plus RF amp and antenna-port bias power |
| `device-virtual` | always on | Signal generator and SigMF file playback |

> [!NOTE]
> The native backends exist for the packaging rule in `PLAN.md` §15: **release artifacts just
> run** — no libSoapySDR, no librtlsdr, no C dependency at all, so a missing system library
> costs exotic-device support, not startup.

Both native drivers are vendored in this repository (`crates/rtl-driver`,
`crates/hackrf-driver`) over one shared USB transport, `crates/usb-stream`, which owns the
transfer queue and the transfer-error policy for both. That policy is librtlsdr's: a cancelled
transfer is never an error, only genuine failures count, and the threshold is the queue depth.
It also means a stalled pipe is re-armed in place — milliseconds — instead of faulting the
device and paying for a full re-open.

The native backends are honest about their limits rather than accepting settings they cannot
apply. The RTL-SDR one, for example, does not advertise direct sampling, offset tuning or the
RTL2832U digital AGC, because nothing programs them yet — and it rejects those settings instead
of silently ignoring them. Use the Soapy backend when you need those knobs.

When the same physical device is visible through both a native backend and Soapy, the native
driver wins the probe merge and the duplicate is collapsed by serial number — you see one
device, not two.

## Capabilities drive the UI

Opening a device produces a `Capabilities` document: frequency ranges, sample rates (a
discrete list or a continuous range), named gain stages with their ranges, antennas,
bandwidths, and typed extra settings (bool, enum, range). The client renders controls from
that document. Adding a device setting requires zero frontend code.

Everything is validated against the capabilities *before* any hardware setter runs — the
frequency, the rate, the bandwidth, each gain stage name and its value, the antenna, and
every extra. A rejected `PATCH` surfaces in the UI as a rejection instead of silently
diverging from the hardware.

### Per-driver extras

The Rust Soapy binding exposes no `getSettingInfo`, so extras come from a per-driver table:

| Driver | Setting | Type | Meaning |
|---|---|---|---|
| `rtlsdr` | `biastee` | bool | Bias-T power on the antenna port |
| `rtlsdr` | `direct_samp` | enum `0`/`1`/`2` | Direct sampling: off, I branch, Q branch (HF without an upconverter) |
| `rtlsdr` | `offset_tune` | bool | Move the tuner off center to dodge the DC spike (E4000 only) |
| `rtlsdr` | `digital_agc` | bool | RTL2832U digital AGC |
| `hackrf` | `bias_tx` | bool | Antenna port power |

Extra-setting writes are read back and verified. SoapyRTLSDR returns success for keys it
ignores — a librtlsdr build without bias-T support would otherwise report a bias-T you do not
have. A silently-ignored write is an error here, not a lie.

PPM correction goes through the `"CORR"` frequency component, because the binding has no
`setFrequencyCorrection`. Tuners that do not expose `CORR` report the setting as unsupported
rather than accepting it and doing nothing.

## Linux

### Permissions

USB access is the single most common reason a device does not appear. The distribution
packages ship the right rules:

```sh
sudo apt install rtl-sdr hackrf     # installs /lib/udev/rules.d rules for both
sudo udevadm control --reload-rules && sudo udevadm trigger
```

If you need to write the rules yourself, match on vendor and product:

```
# /etc/udev/rules.d/60-sdr.rules
# RTL2832U dongles
SUBSYSTEM=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2832", MODE="0660", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2838", MODE="0660", TAG+="uaccess"
# HackRF One
SUBSYSTEM=="usb", ATTRS{idVendor}=="1d50", ATTRS{idProduct}=="6089", MODE="0660", TAG+="uaccess"
```

`TAG+="uaccess"` grants the locally logged-in user access. A **service** user is not
"locally logged in" — give it group ownership instead (`GROUP="plugdev", MODE="0660"`) and
put the service user in that group. Unplug and replug after reloading rules; udev applies
them at device add.

### The DVB driver

Linux claims RTL-SDR dongles for their nominal purpose — TV reception — and then no SDR
software can open them. Blacklist the kernel module:

```sh
echo 'blacklist dvb_usb_rtl28xxu' | sudo tee /etc/modprobe.d/blacklist-rtl.conf
sudo rmmod dvb_usb_rtl28xxu        # or reboot
```

### Soapy modules

`libsoapysdr-dev` alone gives you the framework, not the drivers. Install the module for your
hardware (`soapysdr-module-rtlsdr`, `soapysdr-module-hackrf`, or `soapysdr-module-all`).
Verify with `SoapySDRUtil --info` (module search path) and `SoapySDRUtil --find` (what it
sees).

> [!WARNING]
> `soapysdr-module-all` pulls in modules for hardware you do not have, and some of them
> misbehave when loaded headless — SoapyUHD in particular aborts the process on some
> systems. Install only the modules you need. The project's own CI installs `libsoapysdr-dev`
> with `--no-install-recommends` for exactly this reason.

### Raspberry Pi

Pi 4 is the performance floor: every DSP budget decision in the project is measured against
it. Practical notes:

- Use a powered hub or a good supply. Brownouts on an RTL-SDR look exactly like a broken
  driver.
- Keep the sample rate at what you need. 2.4 Msps into several narrow channels is
  comfortable; 3.2 Msps is not, on any dongle.
- Watch the **overruns** counter in the device set. It counts device samples dropped at the
  capture ring, which means the DSP thread could not keep up. Audio and spectrum have gaps
  even while the status still says `running`, so the number is surfaced rather than hidden.

## macOS

No permissions to configure — macOS grants USB access to userspace drivers. Install the
framework and the modules you need:

```sh
brew install soapysdr soapyrtlsdr soapyhackrf
```

If a device does not appear, check that Homebrew's Soapy plugin directory is the one the
framework searches (`SoapySDRUtil --info`); a mixed Intel/ARM Homebrew installation is the
usual cause of a module path mismatch. `SOAPY_SDR_PLUGIN_PATH` overrides it.

## Hotplug

A device that vanishes mid-stream does not hang the server:

- The capture thread detects the unplug by filtered re-enumeration. `hardware_key()` cannot
  be used for this — SoapyRTLSDR and SoapyHackRF cache it at open and do no USB I/O, so it
  answers happily for a dongle that is in your other hand.
- A hotplug prober re-probes periodically and emits a device-list change; a running set whose
  device is absent from two consecutive probes is faulted.
- The faulted set surfaces `status: error` with the reason, a live recording is finalized
  rather than left as a stub, and the UI shows a banner.

Recovery is automatic since M5: once the device re-enumerates, the hotplug probe re-opens it,
re-applies the tuning it had, and rebuilds its channels — ids and live audio subscriptions
included, so a listener does not have to re-subscribe. A device that is present but still
unopenable (settling, or claimed by another process) keeps the set faulted with that reason and
is retried on the next probe. Closing and re-opening the set by hand still works and is the way
out if the device comes back as something else.

> [!IMPORTANT]
> All Soapy enumerate calls in the server serialize behind a process-wide lock. Concurrent
> enumerates race SoapyHackRF's `hackrf_init`/`hackrf_exit` refcounting and tear down its
> libusb context mid-find, which segfaults the process. This was found during real USB churn,
> not in CI — there is no hardware in CI.

## Checking your setup

`sdrmm --doctor` prints one line per check — which backends are compiled in, which devices
and Soapy modules were found, whether USB permissions look usable, the paths in use, the
platform and version — each with a status of ok, warn or fail, and a hint when there is
something to do. The same report is served at `GET /api/doctor`, so the web UI renders
exactly what the CLI prints.
