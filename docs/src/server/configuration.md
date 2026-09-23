# Configuration and security

`sdrmm` runs the interface, the receiver, and the REST, WebSocket, and MCP APIs in one process.
Out of the box it listens on `0.0.0.0:8080` with **no authentication**.

## Options

| Option | Default | Sets |
|---|---|---|
| `--bind <ADDRESS>` | `0.0.0.0:8080` | Listen address |
| `--db <PATH>` | Platform data folder | SQLite database |
| `--recordings-dir <PATH>` | Platform data folder | Recording folder |
| `--token <TOKEN>` | None | Shared access token |
| `--tls-cert <PATH>`, `--tls-key <PATH>` | None | HTTPS certificate chain and key, PEM |
| `--tls-self-signed` | Off | HTTPS with a self-signed certificate |
| `--tls-name <NAME>` | Found addresses | Name the certificate must cover; repeatable |
| `--routing-backend <NAME>` | `open-route-service` | Routing: `open-route-service` or `graph-hopper` |
| `--routing-url <URL>` | Public service | Self-hosted routing instance |
| `--routing-key <KEY>` | None | Routing API key |
| `--dev-cors` | Off | Allow a separate frontend origin, for development only |
| `--doctor` | | Print diagnostics and exit |
| `--doctor-rates` | | Probe connected radios' sample rates and exit |

For a service, use absolute paths for `--db` and `--recordings-dir`.

## Data

The database holds workspaces, presets, bookmarks, the recording index, and the decoder log.
Recording files hold the signals. Back up both. Stop the server before copying the database,
or use SQLite's backup, and finish recordings before copying them.

## Logs

```sh
RUST_LOG=info sdrmm
RUST_LOG=sdrmm=trace,info sdrmm
```

Trace logging is very verbose. Use it briefly.

## Token

Set a long random token before untrusted devices can reach the server:

```sh
export SDRMM_TOKEN='replace-with-a-long-random-secret'
sdrmm
```

The environment variable keeps the token out of the process list; `--token` works too. The
browser asks for it once and remembers it. API clients send `Authorization: Bearer <token>`.
WebSocket and download URLs can use `?token=...`.

Everything except the page itself and `GET /api/auth` requires the token. Every client with the
token can do everything. There are no user accounts or read-only roles.

## HTTPS

The easiest route is a [tunnel](tunnels.md): Tailscale or Cloudflare handle the certificate.

With your own certificate:

```sh
sdrmm --tls-cert /etc/sdrmm/fullchain.pem --tls-key /etc/sdrmm/privkey.pem
```

Both files are PEM, leaf certificate first. A missing or mismatched file stops startup.

Without a certificate authority:

```sh
sdrmm --tls-self-signed
```

The certificate covers localhost and the server's LAN addresses. It is stored in `tls` beside the
database and reused. Compare the SHA-256 fingerprint in the log the first time a client trusts
it. In containers, behind NAT, or with a DNS name, list the names clients use:

```sh
sdrmm --tls-self-signed --tls-name radio.example --tls-name 192.168.1.20
```

`SDRMM_TLS_NAMES` takes the same names, comma-separated. Changing the names creates a new
certificate.

## Reverse proxy

- Bind SDR-- to loopback, or firewall its port.
- Serve it at the root of the origin.
- Forward WebSocket upgrades on `/api/ws`.

## Turn-by-turn routing

[Field mode](../user-guide/field-mode.md#df-drive) gets driving directions from OpenRouteService or
GraphHopper. Set `--routing-key`, and `--routing-backend` for GraphHopper. `--routing-url` points
at a self-hosted instance. The key never leaves the server.
