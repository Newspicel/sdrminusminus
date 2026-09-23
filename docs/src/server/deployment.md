# Deployment

Put the server next to the antenna and connect from a browser anywhere. Only audio, decoded data,
and display frames cross the network; raw IQ stays on the server.

## Docker Compose

On Linux:

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
docker compose up -d
```

Open `http://<host>:8080`. The service restarts on its own and keeps its database, recordings,
and certificate under `/data` in the `sdrmm-data` volume. Keep and back up that volume. Use the
`:nightly` tag only to test unreleased changes.

### USB devices

The supplied service already contains:

```yaml
devices:
  - /dev/bus/usb:/dev/bus/usb
device_cgroup_rules:
  - "c 189:* rmw"
group_add: ["46"]
```

The cgroup rule lets radios reconnect. Set `group_add` to the group that owns your radio on the
host. `46` is usually `plugdev` on Debian and Ubuntu. Check with:

```sh
stat -c '%g %G %a' /dev/bus/usb/*/*
```

If the radio belongs to group `0`, install its udev rules on the host first. **Check hardware**
shows ownership from inside the container.

### SoapySDR modules

The image has bladeRF, LimeSDR, and SoapyRemote modules. Add others with a derived image:

```dockerfile
FROM ghcr.io/newspicel/sdrminusminus:latest
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends soapysdr-module-audio \
    && rm -rf /var/lib/apt/lists/*
USER sdrmm
```

### SDRplay receivers

Install the SDRplay API on the host and keep `sdrplay_apiService` running. Then share the library
and the host's IPC:

```yaml
volumes:
  - sdrmm-data:/data
  - /usr/local/lib/libsdrplay_api.so.3:/usr/local/lib/libsdrplay_api.so.3:ro
ipc: host
```

`ipc: host` exposes the host's shared memory to the container. Only use it with a trusted image.

### Token

Put the token in a `.env` file kept out of version control:

```text
SDRMM_TOKEN=replace-with-a-long-random-secret
```

```yaml
services:
  sdrmm:
    env_file: .env
```

### HTTPS

The simplest option is a [tunnel](tunnels.md). To use your own certificate, mount it read-only:

```yaml
volumes:
  - sdrmm-data:/data
  - /srv/sdrmm/certs:/certs:ro
command: ["--bind", "0.0.0.0:8080", "--tls-cert", "/certs/fullchain.pem", "--tls-key", "/certs/privkey.pem"]
```

The container runs as UID `10001` and must be able to read both files, including symlink
targets. For a self-signed certificate, name the host clients use:

```yaml
command: ["--bind", "0.0.0.0:8080", "--tls-self-signed", "--tls-name", "radio.example"]
```

## As a system service

Run the portable server under its own user with USB access and fixed paths:

```sh
/usr/local/bin/sdrmm \
  --bind 0.0.0.0:8080 \
  --db /var/lib/sdrmm/sdrmm.db \
  --recordings-dir /var/lib/sdrmm/recordings
```

Stop it with a normal termination signal so recordings can finish. Set `SDRMM_TOKEN` and
[HTTPS](configuration.md#https).

## Before leaving it unattended

Test the packaged build with your radio:

1. Save the `sdrmm --doctor` report.
2. Stream for 30 minutes and check the drop counter, audio, and spectrum.
3. Try tuning, gain, rate, and every control you plan to use.
4. Unplug and replug the radio. The workspace should pick it up again.
5. Record a short capture and play it back.

## Radios on other machines

A Device node can open [network radios](../hardware.md#network-radios) such as `rtl_tcp`,
SpyServer, SDRconnect, and iiod directly. For SoapyRemote, run `SoapySDRServer` next to the
radio and install SoapyRemote where SDR-- runs; the radio then appears in the normal list.
