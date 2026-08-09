# Remote access and security

sdr-- has one honest posture: **it is a LAN appliance.** The same one rtl_tcp and SDRangel
have. Read this page before you forward a port.

## Default posture

The server binds `0.0.0.0:8080` with no authentication. Anyone who can reach the port can:

- open and retune your radios, and take them away from you while you are using one;
- create channels, listen to whatever you can receive, and start decoders;
- read the decoder log — which, for APRS and AIS and ADS-B, is a log of positions, and for
  a mobile station, a log of *your* positions;
- start recordings, which write files to your disk until it is full;
- delete your presets, bookmarks, recordings and log.

That is fine on a home network. It is not fine on the internet.

## Optional shared token

`sdrmm --token <token>`, or the `SDRMM_TOKEN` environment variable — prefer the environment
variable, so the secret does not sit in the process list. A single shared token turns the
whole surface — REST, the WebSocket and the MCP mount — into an authenticated one. Without a
configured token the middleware is a pass-through, which is the default posture above.

An empty token is treated as unset and logged as a warning, rather than "enabling" auth while
accepting an empty `?token=`.

The token may be presented two ways:

- `Authorization: Bearer <token>` for ordinary requests;
- a `token=<token>` query parameter, which is not a convenience: the browser `WebSocket`
  constructor cannot set request headers, and the decoder-log export is a plain navigation
  whose download headers only apply when the browser fetches the URL itself.

Three paths stay reachable without a token: `GET /api/auth`, which is how a client learns
that a token is needed at all and returns nothing but `token_required`; `/api/openapi.json`;
and `/api/docs`. The document and the UI that renders it describe the API's *shape*, never
its data, and their browser-side fetches cannot carry a header.

The UI stores the token per saved connection.

This is one shared secret, not accounts. There are no users, no roles and no audit trail; a
token distinguishes "people who have the token" from "people who do not", and nothing else.
Multi-user roles are a backlog item, not a feature.

## No TLS

The server terminates plain HTTP. There is no TLS in v1 by decision, not by oversight — a
certificate story for a device on a home LAN with no stable name is worse than not having one.

Consequences to take seriously:

- A token sent over plain HTTP crosses the network in the clear. On a trusted LAN that is
  acceptable; over anything else it is a password on a postcard.
- Audio, spectrum and decoded positions are equally unencrypted.

If you need TLS, put a reverse proxy in front (Caddy or nginx) and let it terminate. The
server is a plain HTTP upstream with one WebSocket endpoint; proxy `/api/ws` with upgrade
headers intact.

## Put it behind a VPN, not on the internet

The project's position, written into `PLAN.md` §12: **exposing an SDR server to the internet
is your VPN's job, not ours.**

- **Do:** Tailscale, WireGuard, or an SSH tunnel. The server keeps its LAN-trusted posture
  and the VPN provides identity and encryption — a job it does far better than an
  application-level token would.
- **Do:** bind to `127.0.0.1` when only the desktop app or an SSH tunnel needs it.
  `--bind 127.0.0.1:8080`.
- **Do not:** forward port 8080 on your router, with or without a token.

## CORS

CORS is locked to same-origin by default. `--dev-cors` relaxes it so a Vite dev server on a
different origin can talk to the API; `cargo xtask dev` uses it. Do not run a shared server
with `--dev-cors`.

## Legality

Receiving is not universally legal. What you may listen to, decode and *record* varies by
country — encrypted or non-broadcast traffic, and retransmitting or acting on what you hear,
are restricted in many places even where reception is not. The decoder log makes retention a
deliberate choice: it stores positions and messages until you clear it. That is your call and
your jurisdiction's.

Transmit is out of scope entirely (`PLAN.md` §12a). If it ever lands, it lands behind an
explicit controlled-environment acknowledgment, disabled by default.
