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

The certificate covers localhost, the server's LAN addresses and `<host>.local`. It is stored in
`tls` beside the database and reused, and its key stays the same when the names change. Compare
the SHA-256 fingerprint in the log the first time a client trusts it. In containers, behind NAT,
or with a DNS name, list the names clients use:

```sh
sdrmm --tls-self-signed --tls-name radio.example --tls-name 192.168.1.20
```

`SDRMM_TLS_NAMES` takes the same names, comma-separated. Changing the names creates a new
certificate.

## Phones

[Phones](../user-guide/phones.md) connect over HTTPS and pin the server's key when they pair.

| Setup | Phones use |
|---|---|
| **Allow phones** in **Library → Phones** | A phone port, `8443` by default, with the server's self-signed key |
| `--tls-self-signed`, bound beyond loopback | The main port |

A server with its own certificate still needs **Allow phones**, because a renewed certificate
would break every phone's pin. The phone port takes paired phones only. Phones never get the
shared token.

Pair from a terminal while the server runs:

```sh
sdrmm pair
sdrmm pair --name van --db /var/lib/sdrmm/sdrmm.db
```

It prints a QR code, the code, the key and the hosts. `--db` must match the running server,
`--name` names the phone, and `--plain` drops the colours.

While phones can connect, the server announces itself as `_sdrmm._tcp` over mDNS, so the apps
list it under **Nearby**. Let the phone port through your firewall.

## Reverse proxy

- Bind SDR-- to loopback, or firewall its port.
- Set a token. Without one, a loopback bind answers only `localhost` names.
- Serve it at the root of the origin.
- Pass the original `Host` header. Browser requests whose `Origin` differs are refused.
- Forward WebSocket upgrades on `/api/ws`.
