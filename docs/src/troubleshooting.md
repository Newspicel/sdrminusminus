# Troubleshooting

Start with **Check hardware** on an empty Device node, or run:

```sh
sdrmm --doctor
```

It checks drivers, libraries, radio discovery, USB permissions, and storage paths.

## The page does not open

- Use the address printed after `SDR-- ready` in the server log.
- On the server itself, try <http://127.0.0.1:8080>.
- From another machine, the server must listen on a reachable address, such as
  `--bind 0.0.0.0:8080`.
- Check the firewall and any container port mapping.
- Behind a reverse proxy, serve SDR-- at the root. Path prefixes do not work.
- With TLS on, use `https://`.

## The Linux window is blank or the waterfall is missing

Some graphics drivers break WebKitGTK: a blank window, frozen panels, or
`waterfall unavailable: no WebGL2 context`. Try safe rendering:

```sh
SDRMM_LINUX_GRAPHICS=safe sdrmm-desktop
```

| Value | Does |
|---|---|
| `auto` | Default. Turns off DMABUF when the NVIDIA driver is loaded. |
| `safe` | Turns off DMABUF and accelerated compositing. The waterfall may run slower. |
| `off` | Changes nothing |

Your own `WEBKIT_*` variables win. If the window still fails, run `sdrmm --bind 127.0.0.1:8080`
and use a browser.

## The token is rejected

The browser forgets a rejected token and asks again. Enter the one the server is using now. API
clients send `Authorization: Bearer <token>`. WebSocket and download URLs can use `?token=...`.

## A radio is missing

1. Check that the operating system sees it.
2. Run `sdrmm --doctor` and fix any library or permission error.
3. Close other SDR programs that may hold it.
4. For SoapySDR radios, run `SoapySDRUtil --find` and check the module is built for 0.8.
5. On Linux, install the udev rules. In a container, set
   [`group_add`](server/deployment.md#usb-devices).

**SDRplay:** install the [SDRplay API](https://www.sdrplay.com/downloads/) and start
`sdrplay_apiService`. An RSPduo in use elsewhere only lists its free modes.

See [Radios](hardware.md) for each radio's requirements.

## A radio is plugged in but its node stays disconnected

The node waits for the exact radio it saved, by serial number. To use a different one, press
**Forget this radio** and pick the new one.

## Spectrum works but audio is silent

- Wire channel `audio` to a Speaker and start it.
- Click the page once. Browsers block audio until you do.
- Turn squelch off, or lower it.
- Check the channel sits on the signal and inside the Device's window.
- Check tab mute, system volume, and the output device.

Audio that stutters on a plain `http://` LAN address improves on HTTPS or localhost. Without
them the browser falls back to a slower audio path.

## A decoder shows nothing

- Check frequency, mode, and any baud rate or variant setting.
- Check the Scope shows a signal inside the channel.
- Wire `events` to the right place: Decoder log for messages, Readout for current state, Map for
  positions.
- Adjust gain. Watch for clipping and drops.
- Check the mode's [maturity](user-guide/decoders.md#catalog).

## Drops and gaps

The drop counter on a Device counts samples lost anywhere between the radio and the decoders.
Drops damage audio, spectrum, recordings, and decoding.

- Lower the sample rate and close channels and displays you do not need.
- Use a release build.
- On small computers, check for CPU throttling and heat.
- Use wired Ethernet for network radios.
- Give fast USB radios their own USB bus. A HackRF at 20 MS/s nearly fills
  [USB 2](https://hackrf.readthedocs.io/en/stable/synchronization_checklist.html), and a shared
  hub can lose samples before any counter sees it.

Developers can measure capture health on real radios, see
[hardware capture tests](development/building.md#hardware-capture-tests).

## Recordings do not appear

- Check the server can write to `--recordings-dir`.
- In Docker, check `/data/recordings` is on the persisted volume.
- Stop the recording. Metadata is written on stop.
- Check each capture has both `.sigmf-meta` and `.sigmf-data`.
