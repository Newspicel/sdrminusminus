# Troubleshooting

Start with `sdrmm --doctor`: it prints one line per environment check — compiled-in backends,
devices and Soapy modules found, USB permissions, the database and recordings paths, platform
and version — each with a status of ok, warn or fail and a hint when there is something to do.
It runs before anything opens a device and exits immediately. The same report is served at
`GET /api/doctor`, so the CLI and the web UI say the same thing.

Turn up the logs with `RUST_LOG`:

```sh
RUST_LOG=debug cargo run -p sdrmm
RUST_LOG=info,sdrmm_engine=trace sdrmm      # one crate, loudly
```

The default is `info` with the binary at `debug`.

## No devices listed

The signal generator is always present. If it is the *only* entry, the probe found no
hardware.

- **Linux, permissions.** The single most common cause. Install the distro packages that ship
  the udev rules (`rtl-sdr`, `hackrf`), reload rules, and replug. A **service** user needs
  group ownership, not `uaccess` — see [Hardware](../hardware.md).
- **Linux, the DVB driver.** The kernel claims RTL-SDR dongles for TV. Blacklist
  `dvb_usb_rtl28xxu` and unload it.
- **No Soapy module.** `libsoapysdr-dev` is the framework, not the drivers. `SoapySDRUtil
  --find` shows what Soapy itself sees; if it finds nothing, sdr-- will not either. Install
  `soapysdr-module-rtlsdr` / `soapysdr-module-hackrf`.
- **macOS module path.** `SoapySDRUtil --info` prints the plugin search path. A mixed
  Intel/ARM Homebrew installation is the usual reason modules are installed somewhere the
  framework does not look; `SOAPY_SDR_PLUGIN_PATH` overrides it.
- **Docker on macOS.** The container runs in a VM with no host USB. Virtual devices and
  recordings work; hardware does not.

The device list refreshes from a periodic probe, so a device plugged in after startup appears
within a few seconds without a restart.

## "The web UI has not been built yet"

The server is running correctly; `web/dist` was empty when the binary was compiled. Run
`pnpm --dir web build` and rebuild, or use `cargo xtask dev`.

## No audio

In order of likelihood:

1. **The browser has not been unlocked.** Audio needs a user gesture. Press play again.
2. **Squelch is closed.** Turn it off, or drop the threshold until the channel opens.
3. **Wrong offset.** The channel is where you put it, not where the signal is. Click the
   signal in the spectrum, or check the marker.
4. **Wrong mode.** NFM on an AM signal is a hiss.
5. **The tab is muted** or the channel gain is at zero.

To prove the whole path without hardware: open the signal generator and put an NFM channel at
+300 kHz. That is a modulated test carrier with a 1 kHz tone
([First run](../first-run.md)).

## Audio stutters, spectrum tears

- **Check the device set's `overruns` counter.** It counts samples dropped at the capture
  ring: the DSP thread is not keeping up. Lower the sample rate, run fewer channels, or move
  to a faster host. Growth here means real gaps even while the status says `running`.
- **Network.** Streams are drop-oldest per connection, so a weak Wi-Fi link costs frames, not
  a stall. Ask for fewer spectrum bins and a lower fps.
- **A single stutter after a burst** is the audio jitter buffer rebuffering after an underrun.
  Repeated stutters mean the underlying stream is losing frames.

## The device set shows an error

The device faulted: unplugged, powered down, or a USB error. A live recording is finalized,
and the reason is on the set.

Recovery is manual today — close the set and open the device again. Auto-reconnect on replug
is M5 work. If the device is plugged in and still fails to open, another process may hold it;
Soapy cannot open a device twice.

Under-powered USB looks exactly like a driver bug. On a Pi, use a powered hub.

## The spectrum stopped

- The WebSocket dropped; the client resubscribes on reconnect. A brief pause is expected.
- The device set was removed or faulted — a `StreamStopped` arrives and the panel says so.
- The device is at the end of a recording with looping off; playback parks at EOF by design.

## ADS-B decodes nothing

The device must run at **exactly 2 Msps**. At any other rate the engine refuses the channel
and names the rate that works. If you got the channel created at 2 Msps and still see nothing:

- 1090 MHz needs an antenna that is at least approximately right; a stock dongle whip and no
  filter will hear only close traffic.
- `crc_fix` off is more selective and less sensitive. Leave it on unless you are seeing false
  aircraft.
- A position needs either a matching even/odd CPR frame pair or a reference position
  (`ref_lat`/`ref_lon`). Aircraft with no position but a callsign are normal.

## A decoder produces nothing

- **Offset and mode.** Play the matching fixture from `cargo xtask fixtures` first — it
  proves the decoder and your setup in one step, and `fixtures/README.md` names the channel
  and offset.
- **Inverted polarity.** POCSAG and RTTY have an `invert` setting for exactly this; some
  receive chains flip the discriminator and every frame becomes noise.
- **Wrong parameters.** RTTY needs the right baud, shift and stop bits. POCSAG's `auto` baud
  handles rate detection for you.
- **Squelch.** A closed squelch feeds decoders silence. That silence is duration-exact
  (deleting time would corrupt a bit clock), but it is still silence.
- **Check `dropped`** in the decoder log response. Non-zero means frames were lost on the way
  to the log because a consumer fell behind — the decoder is working, the pipe is not.

## Recording failed

The recorder is lossless by contract: if the writer queue overflows or the disk errors, the
tap is disarmed and the failure is surfaced on the device set rather than producing a short
file that looks complete. Check free space and the error text.

Changing the sample rate is rejected while recording — one SigMF metadata document cannot
describe two rates honestly. Stop the recording first.

## 401 from the API

The server has a token configured. Send `Authorization: Bearer <token>`, or `?token=` for the
WebSocket and for download links. `GET /api/auth` tells you whether a token is required
without needing one.

## Port already in use

Another sdr-- (or something else) holds 8080. `--bind 0.0.0.0:8081`.

## `cargo xtask check` fails on codegen drift

`crates/wire` changed and the generated artifacts did not. Run `cargo xtask codegen` and
commit `openapi.json` plus `web/src/generated`. Never edit either by hand.

## Something else

`PROGRESS.md` records what is built, tested and green, including the known gaps — for example
that every decoder is currently proven against its specification via a reference modulator
rather than against off-air captures. If the behaviour you are chasing is on the wrong side of
one of those gaps, the file will tell you.
