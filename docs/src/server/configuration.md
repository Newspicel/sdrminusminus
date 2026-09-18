# Configuration and security

`sdrmm` serves the interface, receiver engine, REST API, WebSocket, and MCP in one process.
By default it listens on `0.0.0.0:8080` without authentication.

## Command-line options

```text
sdrmm [OPTIONS]
```

| Option | Default | Purpose |
|---|---|---|
| `--bind <ADDRESS>` | `0.0.0.0:8080` | Address and port for HTTP and WebSocket traffic |
| `--db <PATH>` | Platform data directory | SQLite database for workspaces, presets, bookmarks, recording index, and decoder log |
| `--recordings-dir <PATH>` | Platform data directory | Directory containing SigMF recording pairs |
| `--token <TOKEN>` | None | Require one shared bearer token for API, WebSocket, and MCP requests |
| `--tls-cert <PATH>` | None | PEM certificate chain to serve HTTPS with; requires `--tls-key` |
| `--tls-key <PATH>` | None | PEM private key for that chain |
| `--tls-self-signed` | Off | Serve HTTPS with a self-signed certificate kept beside the database |
| `--tls-name <NAME>` | Discovered addresses | Name or address that certificate must cover; repeatable |
| `--routing-backend <NAME>` | `open-route-service` | Routing service: `open-route-service` or `graph-hopper` |
| `--routing-url <URL>` | The backend's own service | Base URL, for a self-hosted instance |
| `--routing-key <KEY>` | None | API key for that service |
| `--dev-cors` | Off | Allow a separate frontend development origin |
| `--doctor` | Off | Print environment diagnostics and exit |
| `--doctor-rates` | Off | Probe connected receivers' sample rates and exit |
| `--help` | | Show CLI help |
| `--version` | | Show the build version |

Use absolute database and recording paths for services so storage does not depend on the working
directory.

## Persistent data

Back up the database and recordings for a complete installation:

```text
/srv/sdrmm/
├── sdrmm.db
└── recordings/
    ├── <capture>.sigmf-meta
    └── <capture>.sigmf-data
```

The database holds settings and decoded history; recording files hold IQ and audio. Stop the
server before copying its database, or use SQLite's backup mechanism. Finish recordings before
copying their files.

## Logging

Set the `RUST_LOG` filter to adjust logging:

```sh
RUST_LOG=info sdrmm
RUST_LOG=sdrmm=trace,info sdrmm
```

Use trace logging for short diagnostic sessions; it can produce substantial output.

## Shared-token authentication

Set a long random token before allowing untrusted clients to reach the server:

```sh
export SDRMM_TOKEN='replace-with-a-long-random-secret'
sdrmm
```

`--token` sets the same value, but the environment variable keeps it out of the process arguments.
The browser prompts for the token and stores it for that origin.

REST and MCP clients send:

```http
Authorization: Bearer replace-with-a-long-random-secret
```

WebSocket handshakes and browser downloads can use `?token=...`. The application shell and
`GET /api/auth` stay public so clients can load the login prompt. Other API, documentation,
WebSocket, and MCP routes require the token.

All authenticated clients have the same permissions, including changing the active receiver.
There are no per-user accounts or read-only roles.

## HTTPS

For managed certificates and remote access, follow
[HTTPS with Tailscale or Cloudflare Tunnel](tunnels.md).

Use a certificate chain and matching private key:

```sh
sdrmm --tls-cert /etc/sdrmm/fullchain.pem --tls-key /etc/sdrmm/privkey.pem
```

Both files must be PEM. Put the leaf certificate first, followed by intermediates. Keys may use
PKCS#8, PKCS#1, or SEC1. Missing, unreadable, or mismatched files prevent startup.

For a local setup without a certificate authority:

```sh
sdrmm --tls-self-signed
```

The certificate covers localhost, loopback, and discovered LAN addresses. It is saved under `tls`
beside the database and reused until renewal is needed. Compare the logged SHA-256 fingerprint
when first accepting it on a client.

For containers, NAT, or a DNS name, specify the addresses clients actually use:

```sh
sdrmm --tls-self-signed --tls-name radio.example --tls-name 192.168.1.20
```

`SDRMM_TLS_NAMES` accepts the same comma-separated list. Explicit names replace discovered
addresses; loopback remains covered. Changing names regenerates the certificate. Stable names
avoid repeated certificate changes when a container address changes.

Prefer an authority-issued certificate where available. Self-signed certificates require clients
to establish trust manually.

## Network security

Use HTTPS to protect tokens and receiver traffic. For a reverse proxy:

- Bind SDR-- to loopback or firewall its direct port.
- Serve the application at the origin root.
- Forward WebSocket upgrades for `/api/ws`.

An authenticated tunnel is another option. Rotate the shared token if it may have leaked.

## Turn-by-turn routing

[Field mode](../user-guide/field-mode.md) uses OpenRouteService or GraphHopper for driving routes.
Set `--routing-key`, choose the service with `--routing-backend`, and use `--routing-url` for a
self-hosted instance. The key stays on the server and is sent in an authorization header.

Without a reachable backend, field mode reports the problem and keeps heading guidance.
**Navigate in Maps** can open the target in the phone's navigation app.

## Development CORS

`--dev-cors` permits requests from a separate frontend origin during development. Leave it off
for production and when the interface is served directly by `sdrmm`.
