# Troubleshooting

Start with the built-in diagnostic report:

```sh
sdrmm --doctor
```

In the interface, the same checks are under **Hardware not showing up?** on an unbound Device
node. Diagnostics identify backend, module, permission, device-discovery, database, and recording
path problems before the engine claims any hardware.

## The page does not open

- Confirm the server logged `sdr-- ready` and note the address it printed.
- On the same machine, try <http://127.0.0.1:8080>.
- For another machine, confirm the server is bound to a reachable address, not loopback:

  ```sh
  sdrmm --bind 0.0.0.0:8080
  ```

- Check the host firewall and container port mapping.
- If a reverse proxy serves sdr-- below a path prefix, reconfigure it to use a dedicated origin;
  the embedded application and API expect root-relative paths.

## A token is rejected

The UI stores the shared token in browser local storage for that origin. If the server token
changes, the next unauthorized response clears the old value and prompts again.

API clients should send `Authorization: Bearer <token>`. WebSocket and browser-initiated download
URLs may send the same value as the `token` query parameter.

## A radio is missing

1. Confirm it appears in the operating system.
2. Run `sdrmm --doctor`.
3. If using the host runtime, run `SoapySDRUtil --find`.
4. Confirm the Soapy module ABI is `0.8` and the module is in a reported search path.
5. On Linux, check the USB node permissions and reconnect after installing udev rules.
6. Stop other SDR programs; most devices can be claimed by only one process.

Desktop installers and containers use a private SoapySDR tree, so a module installed only in the
host's default directory will not automatically appear there.

## A device is present but a saved node is disconnected

The node intentionally waits for the same durable device identity it stored earlier. This avoids
binding your settings to a different receiver after enumeration order changes. Check the serial or
variant shown on the node. Use **Forget this radio** only when you want to choose a replacement.

## Spectrum works but audio is silent

- Confirm the channel's `audio` output is wired to a Speaker.
- Start the stream from the Speaker node's control.
- Click once in the page if the browser blocked autoplay.
- Disable squelch temporarily or lower its threshold.
- Make sure the channel marker is over the signal and its full occupied bandwidth fits inside the
  device passband.
- Check that the tab is not muted and that the system output device is correct.

## A decoder produces nothing

- Verify the exact frequency, channel variant, and expected baud or protocol setting.
- Use a Scope to confirm energy is present and centered.
- Check the sample-rate warning on the channel face. ADS-B requires a native device rate between
  2 and 4 MS/s.
- Wire the `events` output to the correct destination. Independent frames appear in Decoder log;
  accumulated targets appear in Readout; positions require a Map.
- Increase gain cautiously and watch for clipping or Device overruns.

## Overruns or gaps

An overrun means the capture thread produced samples faster than the DSP path could consume them.
Those samples are lost, so spectrum, audio, recordings, and decoders can all contain gaps.

- Lower the device sample rate.
- Close unused channels, scopes, or decoders.
- Avoid debug builds for real-time operation; the workspace optimizes DSP crates even in the dev
  profile, but custom profiles may not.
- Check CPU frequency scaling and thermal throttling on small computers.
- Prefer a wired network for high-rate remote receivers.

## Recordings do not appear

- Confirm `--recordings-dir` is writable by the server user.
- With Docker, persist and inspect `/data/recordings`.
- Stop an active recording so its metadata can be finalized.
- A pair with invalid or missing SigMF metadata is not listed as playable.

## Development server requests fail

Use `cargo xtask dev`, which starts the backend with development CORS and configures Vite to proxy
the API and WebSocket. If starting the pieces manually, pass `--dev-cors` only for a trusted local
development origin; it deliberately relaxes CORS broadly.
