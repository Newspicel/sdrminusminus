# Configuration and security

The `sdrmm` binary runs the receiver engine, REST API, WebSocket and MCP endpoints, Swagger UI,
and embedded React application in one process.

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
| `--help` | | Show CLI help |
| `--version` | | Show the build version |

Relative database and recording paths are resolved at startup. Use absolute paths for services
and containers so storage does not depend on the working directory.

## Persistent data

The SQLite database contains configuration and structured history. The recordings directory
contains large IQ files. Back up both when you need a complete installation:

```text
/srv/sdrmm/
├── sdrmm.db
└── recordings/
    ├── <capture>.sigmf-meta
    └── <capture>.sigmf-data
```

Stop the server or use SQLite's supported backup mechanism before copying a live database. Raw
recording pairs can be copied while idle; do not assume an actively written pair is complete.

## Logging

sdr-- uses the standard `RUST_LOG` filter. Without an override it logs general information and
more detailed sdr-- messages. Examples:

```sh
RUST_LOG=info sdrmm
RUST_LOG=sdrmm=trace,info sdrmm
```

Trace logging can be noisy on an active receiver. Capture it for a short diagnostic session rather
than leaving it enabled on an unattended server.

## Shared-token authentication

By default, a headless server is unauthenticated and trusts its local network. Set a long random
token whenever untrusted clients can reach the port:

```sh
export SDRMM_TOKEN='replace-with-a-long-random-secret'
sdrmm
```

The environment variable avoids exposing the secret in the process list. `--token` and
`SDRMM_TOKEN` configure the same value.

The browser prompts for the token and stores it in local storage for that origin. REST and MCP
clients should send:

```http
Authorization: Bearer replace-with-a-long-random-secret
```

WebSocket handshakes and browser download links can use `?token=...` because those requests cannot
always attach an authorization header.

The application shell and `GET /api/auth` remain reachable without authentication so the browser
can load and discover that it needs a token. Other API, WebSocket, documentation, and MCP routes
are protected.

## HTTPS

The server can terminate TLS itself. Give it a certificate chain and its key:

```sh
sdrmm --tls-cert /etc/sdrmm/fullchain.pem --tls-key /etc/sdrmm/privkey.pem
```

Both files are PEM. The chain holds the leaf certificate first and any intermediates after it; the
key may be PKCS#8, PKCS#1, or SEC1. The two options are given together, and the server refuses to
start if either file is unreadable or the key does not match the certificate.

Without a certificate authority, ask for a self-signed one instead:

```sh
sdrmm --tls-self-signed
```

The certificate covers `localhost`, both loopback addresses, and every LAN address the machine
reports for itself, so the [field-mode](../user-guide/field-mode.md) handoff to a phone works over
the same certificate. It is written to a `tls` directory beside the database and reused on every
later start, so a browser or phone that accepted it keeps trusting it; it is replaced shortly
before it expires. The key is owner-readable only. Note the SHA-256 fingerprint the server logs at
startup and compare it the first time a client warns about the unknown issuer.

Where the addresses the server sees are not the ones clients dial — behind a container bridge, a
NAT, or a DNS name — name them instead:

```sh
sdrmm --tls-self-signed --tls-name radio.example --tls-name 192.168.1.20
```

`SDRMM_TLS_NAMES` takes the same list, comma separated. Named addresses replace the discovered
ones, which is what keeps the certificate stable: a container's own address changes from run to
run, and a certificate following it would be minted again, and have to be trusted again, on most
restarts. Loopback is always covered. A certificate is also replaced when the set of names
changes, because one that does not name the address a client dialled is worse than an unknown
one.

A self-signed certificate encrypts the connection but proves nothing about the host. Use a real
certificate wherever an authority is available.

## Network security

The shared token is access control, not transport encryption. A plain HTTP client on the network
can expose it and receiver traffic to an observer. For access beyond a trusted LAN:

- serve HTTPS directly, or bind to loopback and place an HTTPS reverse proxy or authenticated
  tunnel in front;
- preserve WebSocket upgrade headers for `/api/ws`;
- proxy the application at the origin root rather than a path prefix;
- keep the direct `8080` port firewalled;
- rotate the shared token if it may have leaked.

sdr-- has one shared privilege level. It does not currently provide per-user accounts or
read-only roles, and every authenticated client can change the active receiver.

## Turn-by-turn routing

[Field mode](../user-guide/field-mode.md) can request driving routes to direction-finding waypoints.
The server proxies requests to OpenRouteService or GraphHopper and sends the API key in an
`Authorization` header. The key is not sent to the browser or included in URLs.

Set `--routing-key` for the hosted backend. Use `--routing-backend` to choose the service and
`--routing-url` for a self-hosted instance.

Without a configured or reachable backend, field mode reports that routing is unavailable and
uses heading guidance. The phone's navigation app remains available through **Navigate in Maps**.

## Development CORS

`--dev-cors` installs a permissive CORS policy for the separate Vite origin used during frontend
development. It is not needed when the UI is served by `sdrmm`, and should not be enabled as a
production cross-origin access policy.
