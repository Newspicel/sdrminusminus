# Troubleshooting

Start with **Check hardware** on an unbound Device node, or run:

```sh
sdrmm --doctor
```

The report checks drivers, libraries, discovery, USB permissions, and storage paths.

## The page does not open

- Find the address printed beside `SDR-- ready` in the server log.
- On the server itself, try <http://127.0.0.1:8080>.
- For remote access, bind to a reachable interface, for example `sdrmm --bind 0.0.0.0:8080`.
- Check firewall rules and container port mappings.
- Serve reverse-proxy deployments at the origin root; path prefixes are unsupported.

Use `https://` when TLS is enabled.

## The Linux window is blank or the waterfall is broken

Some WebKitGTK graphics drivers cause blank windows, frozen panels, or
`waterfall unavailable: no WebGL2 context`. Try safe rendering:

```sh
SDRMM_LINUX_GRAPHICS=safe sdrmm-desktop
```

| Value | Behaviour |
|---|---|
| `auto` | Default; disables DMABUF when the NVIDIA kernel module is loaded |
| `safe` | Disables DMABUF and accelerated compositing |
| `off` | Leaves graphics settings unchanged |

Safe rendering may lower the waterfall frame rate. Existing `WEBKIT_*` variables take precedence;
the startup log shows applied settings. See
[Tauri's graphics debugging guide](https://v2.tauri.app/develop/debug/linux-graphics/) for individual options.

If the window still fails, use the server in a browser:

```sh
sdrmm --bind 127.0.0.1:8080
```

## A token is rejected

The browser stores the token for the server's origin. After an unauthorised response, it clears
the saved value and prompts again. Enter the token currently configured on the server.

API clients use `Authorization: Bearer <token>`. WebSocket and browser download URLs can use
`?token=...`.

## A radio is missing

1. Confirm the operating system detects it.
2. Run `sdrmm --doctor` and resolve library or permission errors.
3. Stop other SDR software that may hold the receiver.
4. For SoapySDR radios, run `SoapySDRUtil --find` and check that modules match ABI `0.8`.
5. On Linux, install udev rules and grant the server account access. In containers, pass the
   owning group's numeric ID with `group_add`.

Desktop and portable packages use system SoapySDR installations. Containers include selected
modules; Nix uses configured plugins. `SDRMM_SOAPY_MODULE_PATH` adds search directories.
See [Radios and hardware](hardware.md) for package and receiver requirements.

## An SDRplay receiver does not appear

Install [SDRplay API](https://www.sdrplay.com/downloads/) and start `sdrplay_apiService`.
The **SDRplay API** section in `sdrmm --doctor` reports library and service errors.

An RSPduo already in use lists only free operating modes. See [SDRplay](hardware.md#sdrplay)
and [container setup](server/deployment.md#sdrplay-receivers).

## A device is present but a saved node is disconnected

The node waits for its saved radio identity. Check the serial number and variant. To replace the
receiver, choose **Forget this radio** and select the new one.

## Spectrum works but audio is silent

- Connect channel `audio` to Speaker `audio` and start Speaker playback.
- Click the page to allow browser audio.
- Turn off squelch temporarily or lower its threshold.
- Check that the channel covers the signal and fits inside the Device passband.
- Check tab mute, system volume, and the selected audio output.

For broken or intermittent browser audio, use HTTPS or localhost. Plain LAN HTTP uses a fallback
without AudioWorklet, which can stutter while the display is busy.

## A decoder produces nothing

- Confirm frequency, mode, baud rate, and protocol variant.
- Check the Scope for a signal within the channel bandwidth.
- Connect `events` to the right output: Decoder log for frames, Readout for current state, Map for positions.
- Adjust gain and check for clipping or gaps.
- Check the mode's [coverage and limitations](user-guide/channels.md#channel-catalog).

## Overruns or gaps

An overrun means samples were lost because capture outpaced processing. It can affect audio,
spectrum, recordings, and decoding.

- Lower the sample rate and close unused channels or displays.
- Use a release build for regular reception.
- Check CPU throttling and temperature on small computers.
- Use wired Ethernet for high-rate network receivers.

## Recordings do not appear

- Confirm the server can write to `--recordings-dir`.
- In Docker, check the persisted `/data/recordings` directory.
- Stop active recordings to finalise metadata.
- Check that each IQ capture has valid `.sigmf-meta` and `.sigmf-data` files.

## Development server requests fail

Use `cargo xtask dev` to start the backend and configure the Vite API and WebSocket proxy.
When starting them separately, use `--dev-cors` only for trusted local development.
