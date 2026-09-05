# Containers and remote radios

A remote deployment keeps USB cable length short and moves control, decoded data, compressed
audio, and display frames across the network instead of raw device IQ.

## Docker Compose

The repository includes a single-service Compose configuration:

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
docker compose pull
docker compose up -d
```

Open `http://<host>:8080`. The service stores its database and recordings in the named
`sdrmm-data` volume and restarts unless stopped.

Use the nightly image only when you intend to track unreleased changes:

```yaml
services:
  sdrmm:
    image: ghcr.io/newspicel/sdrminusminus:nightly
```

### USB devices

The supplied Compose file passes the complete Linux USB bus:

```yaml
devices:
  - /dev/bus/usb:/dev/bus/usb
device_cgroup_rules:
  - "c 189:* rmw"
```

The rule matters after a reconnect: a USB device may return with a different minor number than the
one present when the container started. Host udev permissions still apply to the nodes. Prefer
installing the receiver's normal udev rule; when that is not possible, add the numeric group that
owns the device with `group_add`. Running the whole service as root should be a last resort.

### SDRplay receivers

The image carries the driver but not SDRplay's vendor API, which is licensed for use with genuine
SDRplay hardware and cannot be redistributed. Install the API on the host, leave its service
running there, and give the container the library plus the shared memory the service talks over:

```yaml
volumes:
  - /usr/local/lib/libsdrplay_api.so.3:/usr/local/lib/libsdrplay_api.so.3:ro
ipc: host
```

`ipc: host` lets the library communicate with `sdrplay_apiService` through POSIX shared memory.
Without it, the API cannot open even when its library is mounted. This also exposes the host's
other IPC objects to the container, so use it only with a trusted image and host, outside
multi-tenant deployments. If the API is missing, no RSP appears; other drivers still work.

### Data and authentication

The image entry point fixes persistent paths under `/data`. Add a token through an environment
file rather than committing it to Compose:

```yaml
services:
  sdrmm:
    env_file: .env
```

```text
SDRMM_TOKEN=replace-with-a-long-random-secret
```

Protect the environment file and back up the `sdrmm-data` volume. See
[Configuration and security](configuration.md) before exposing the service outside a trusted LAN.

## Run the portable server as a service

For a non-container deployment, give `sdrmm` a dedicated unprivileged account, explicit data
paths, and a service manager that sends a normal termination signal. A representative command is:

```sh
/usr/local/bin/sdrmm \
  --bind 0.0.0.0:8080 \
  --db /var/lib/sdrmm/sdrmm.db \
  --recordings-dir /var/lib/sdrmm/recordings
```

Grant that account access through the radio's udev rules. Graceful termination is important because
the engine finalizes active recordings during shutdown.

## Connect to a network receiver

sdr-- can operate radios that already expose IQ over the network. Add a Device node, open
**Radio on the network?**, then select:

- `rtl_tcp`, default port `1234`;
- SpyServer, default port `5555`.

Enter a DNS name, IPv4 address, or bracketed IPv6 endpoint. You can omit the port when using the
default. These connections are named rather than discovered, and the saved workspace keeps the
canonical endpoint as the device identity.

Network IQ can require substantial and sustained bandwidth. Use wired Ethernet where possible,
select only the sample rate the task needs, and watch Device overruns and reconnect messages.

## SoapyRemote

The desktop and container distributions also bundle SoapyRemote. Run `SoapySDRServer` beside the
hardware, then choose the discovered remote device through the normal Device list. SoapyRemote is
part of SoapySDR and is distinct from sdr--'s direct `rtl_tcp` and SpyServer backends.

## Browser deployment

Modern browser audio works on localhost and ordinary LAN origins, but some features have secure
context requirements. In particular, automatic band-region detection uses browser geolocation and
normally requires HTTPS. Manual region selection does not.

When proxying through HTTPS, forward normal HTTP routes and WebSocket upgrades on the same origin.
The UI uses `/api/*`, `/api/ws`, and `/mcp` root-relative paths.
