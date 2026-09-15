# Containers and remote radios

Run sdr-- beside the radio and connect through a desktop browser. The server sends audio,
decoded data, and display frames over the network, keeping raw device IQ local.

## Docker Compose

On Linux:

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
docker compose pull
docker compose up -d
```

Open `http://<host>:8080`. The supplied service restarts unless stopped and keeps data in the
`sdrmm-data` volume. Use `:nightly` instead of `:latest` only to test unreleased changes.

### USB devices

The supplied service includes:

```yaml
devices:
  - /dev/bus/usb:/dev/bus/usb
device_cgroup_rules:
  - "c 189:* rmw"
group_add: ["46"]
```

The bus mapping exposes USB devices. The cgroup rule allows devices to reconnect with new minor
numbers. Host udev rules still control access.

Set `group_add` to the numeric group IDs owning your radio nodes. `46` is commonly `plugdev` on
Debian and Ubuntu. Check on the host:

```sh
stat -c '%g %G %a' /dev/bus/usb/*/*
```

Install the receiver's udev rules and use the reported group. An unconfigured node may belong to
group `0`. **Check hardware** reports inaccessible nodes and ownership from inside the container.

### SoapySDR modules

The image includes the SoapySDR core and bladeRF, LimeSDR, and SoapyRemote modules. Built-in
drivers cover other supported radios; see [hardware requirements](../hardware.md).

To add a module, build a derived image:

```dockerfile
FROM ghcr.io/newspicel/sdrminusminus:latest
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends soapysdr-module-audio \
    && rm -rf /var/lib/apt/lists/*
USER sdrmm
```

Replace the example module with the one you need.

### SDRplay receivers

Install the vendor API on the host and keep `sdrplay_apiService` running. Add its library and
shared IPC to the service:

```yaml
volumes:
  - sdrmm-data:/data
  - /usr/local/lib/libsdrplay_api.so.3:/usr/local/lib/libsdrplay_api.so.3:ro
ipc: host
```

The API needs host shared memory to communicate with its service. `ipc: host` also exposes other
host IPC objects, so use this setup only with a trusted image and host. See
[SDRplay](../hardware.md#sdrplay) for library diagnostics.

### Data and authentication

The image stores its database and recordings under `/data`. Keep that volume when replacing the
container. Supply a token through a protected `.env` file:

```text
SDRMM_TOKEN=replace-with-a-long-random-secret
```

Add this to the service and keep `.env` out of version control:

```yaml
services:
  sdrmm:
    env_file: .env
```

Back up the volume. Configure [HTTPS and access control](configuration.md) for remote use.

### HTTPS

Mount a certificate directory read-only and pass the certificate options:

```yaml
volumes:
  - sdrmm-data:/data
  - /srv/sdrmm/certs:/certs:ro
command: ["--bind", "0.0.0.0:8080", "--tls-cert", "/certs/fullchain.pem", "--tls-key", "/certs/privkey.pem"]
```

The container runs as UID `10001`; grant it read access to both files. Certificate symlink targets
must also be available inside the container.

For a self-signed certificate, specify the hostname clients use:

```yaml
command: ["--bind", "0.0.0.0:8080", "--tls-self-signed", "--tls-name", "radio.example"]
```

The certificate persists in `/data/tls`. Back it up with the database to preserve client trust.
The bundled health check supports HTTP and HTTPS.

## Run the portable server as a service

Use a dedicated account with USB access and explicit storage paths:

```sh
/usr/local/bin/sdrmm \
  --bind 0.0.0.0:8080 \
  --db /var/lib/sdrmm/sdrmm.db \
  --recordings-dir /var/lib/sdrmm/recordings
```

Configure your service manager to send a normal termination signal so active recordings can finish.
Use `SDRMM_TOKEN` for authentication and the [TLS options](configuration.md#https) for HTTPS.

## Connect to a network receiver

On Device, open **Network**, choose a protocol, and enter its address:

| Protocol | Default port |
|---|---:|
| `rtl_tcp` | 1234 |
| SpyServer | 5555 |
| AD936x / iiod | 30431 |

Use a hostname, IPv4 address, or bracketed IPv6 address, with an optional port. The workspace saves
the endpoint as the receiver identity. Use only the sample rate you need and watch overruns;
network IQ can require substantial bandwidth.

## SoapyRemote

Install SoapyRemote where sdr-- runs and start `SoapySDRServer` beside the hardware. Choose the
remote receiver from the normal Device search. The container includes the module; desktop and
portable packages use the host's installation.

## Browser deployment

Serve the interface at the origin root with `/api/*`, `/api/ws`, and `/mcp` on the same origin.
A reverse proxy must forward WebSocket upgrades.

Use HTTPS or localhost for browser location and AudioWorklet playback. Plain LAN HTTP can play
audio through a fallback, but busy displays may interrupt it. Manual band-region selection remains
available without browser location.
