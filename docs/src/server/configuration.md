# Configuration and security

`sdrmm` runs the interface, the receiver, and the REST, WebSocket, and MCP APIs in one process.
Out of the box it listens on `0.0.0.0:8080` with **no authentication**.

## Options

| Option | Default | Sets |
|---|---|---|
| `--bind <ADDRESS>` | `0.0.0.0:8080` | Listen address |
| `--db <PATH>` | `sdrmm/sdrmm.db` in the platform data folder | SQLite database |
| `--recordings-dir <PATH>` | `sdrmm/recordings` in the platform data folder | Recording folder |
| `--token <TOKEN>` | None | Shared access token |
| `--tls-cert <PATH>`, `--tls-key <PATH>` | None | HTTPS certificate chain and key, PEM; always together |
| `--tls-self-signed` | Off | HTTPS with a self-signed certificate |
| `--tls-name <NAME>` | Found addresses | Name the self-signed certificate must cover; repeatable or comma-separated |
| `--remote-app <URL>` | `https://app.sdrmm.com` | App for [remote access](tunnels.md#appsdrmmcom) |
| `--dev-cors` | Off | Allow a separate frontend origin, for development only |
| `--doctor` | | Print diagnostics and exit |
| `--doctor-rates` | | Probe connected radios' sample rates and exit |

`SDRMM_TOKEN`, `SDRMM_TLS_NAMES` and `SDRMM_REMOTE_APP` set `--token`, `--tls-name` and
`--remote-app`. The platform data folder is `~/.local/share` on Linux,
`~/Library/Application Support` on macOS and `%APPDATA%` on Windows. For a service, use absolute
paths for `--db` and `--recordings-dir`.

Subcommands: [`sdrmm phone`](#phones) pairs a phone, [`sdrmm pair`](tunnels.md#appsdrmmcom)
pairs with app.sdrmm.com.

## Data

The database holds workspaces, presets, bookmarks, saved radios, the recording index, the decoder
log, paired phones and the app.sdrmm.com pairing. The `tls` folder beside it holds the
self-signed certificate. Recording files hold the signals. Back up all three. Stop the server
before copying the database, or use SQLite's backup, and finish recordings before copying them.

## Logs

The default level is `info,sdrmm=debug`. Change it with `RUST_LOG`:

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

Everything needs the token except the page itself, `/api/auth`, `/api/status`, `/api/about`,
`/api/openapi.json` and `/api/docs`. Every client with the token can do everything. There are no
user accounts or read-only roles.

## HTTPS

A [tunnel](tunnels.md) needs no certificate work.

With your own certificate:

```sh
sdrmm --tls-cert /etc/sdrmm/fullchain.pem --tls-key /etc/sdrmm/privkey.pem
```

Both files are PEM, leaf certificate first. A missing or mismatched file stops startup.

Without a certificate authority:

```sh
sdrmm --tls-self-signed
```

The certificate covers `localhost`, `127.0.0.1`, `::1`, the server's LAN IPv4 addresses and
`<host>.local`. It is stored in `tls` beside the database, valid for 397 days and renewed at
startup after a year. Its key stays the same across renewals and name changes. Compare the
SHA-256 fingerprint in the log the first time a client trusts it.

In containers, behind NAT, or with a DNS name, list the names clients use. They replace the found
addresses:

```sh
sdrmm --tls-self-signed --tls-name radio.example --tls-name 192.168.1.20
```

`SDRMM_TLS_NAMES` takes the same names, comma-separated. Changing the names creates a new
certificate.

## Phones

[Phones](../user-guide/phones.md#allow-phones) pin the server's key when they pair. A server with
`--tls-self-signed` on a LAN address takes phones on its main port. Any other server needs
**Allow phones**, which opens a phone port, `8443` by default, with the self-signed key from
`tls`. A server with its own certificate needs it too, because a renewed certificate would break
every pin. The phone port takes paired phones only, and phones never get the shared token.

Pair from a terminal while the server runs:

```sh
sdrmm phone
sdrmm phone --name van --db /var/lib/sdrmm/sdrmm.db
```

It prints a QR code, the code, the key, the hosts and when the offer ends. `--db` must match the
running server, `--name` names the phone, and `--plain` or `NO_COLOR` drops the colours.

While phones can connect, the server announces itself as `_sdrmm._tcp` over mDNS, so the apps
list it under **Nearby**. Let the phone port through your firewall.

## Reverse proxy

- Bind SDR-- to loopback, or firewall its port.
- Set a token. Without one, a loopback bind answers only `localhost` names.
- Serve it at the root of the origin.
- Pass the original `Host` header. Browser requests whose `Origin` differs are refused.
- Forward WebSocket upgrades on `/api/ws`, and `/mcp` for MCP clients.
