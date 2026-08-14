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
| `--dev-cors` | Off | Allow a separate frontend development origin |
| `--doctor` | Off | Print environment diagnostics and exit |
| `--help` | | Show CLI help |
| `--version` | | Show the build version |

Relative database and recording paths are resolved to absolute paths when the server starts. For
services and containers, explicit absolute paths make backups and permissions easier to reason
about.

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

## Network security

The shared token is access control, not transport encryption. A plain HTTP client on the network
can expose it and receiver traffic to an observer. For access beyond a trusted LAN:

- bind to loopback and place an HTTPS reverse proxy or authenticated tunnel in front;
- preserve WebSocket upgrade headers for `/api/ws`;
- proxy the application at the origin root rather than a path prefix;
- keep the direct `8080` port firewalled;
- rotate the shared token if it may have leaked.

sdr-- has one shared privilege level. It does not currently provide per-user accounts or
read-only roles, and every authenticated client can change the active receiver.

## Development CORS

`--dev-cors` installs a permissive CORS policy for the separate Vite origin used during frontend
development. It is not needed when the UI is served by `sdrmm`, and should not be enabled as a
production cross-origin access policy.
