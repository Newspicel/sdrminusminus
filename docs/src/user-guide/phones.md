# Phones

An iPhone or Android phone is a remote head for field work. It runs missions from the active
workspace and sends its position and heading to the server. It does not edit the graph.

Get the app: [iPhone](iphone.md), [Android](android.md).

## Allow phones

Phones need a direct HTTPS endpoint whose key they pin at pairing.

- Open **Library → Phones** and turn on **Allow phones**. This opens a phone port, `8443` by
  default, with the server's own self-signed certificate. It is for phones only.
- A server started with `--tls-self-signed` on a LAN address also takes phones on its main port.

The status line reads `Ready on 8443` when phones can reach the server, and `Discovery on` when
nearby phones can find it. Your firewall must let the port in; macOS asks the first time.

Reach the phone port over your LAN or over a Tailscale IP. Tunnels that end HTTPS, such as
Tailscale Serve or a Cloudflare Tunnel, carry only the browser, never a phone.

## Pair

1. Press **Pair phone**. A QR code, an 8-digit **Code** and a **Key** appear for five minutes.
2. On the phone, **Scan QR**. Or pick the server under **Nearby** and type the code. Or type
   `host:port` and the code under **Manual**.
3. Check the phone shows the same **Key**, then **Trust**.

On a server without a browser, `sdrmm pair` prints the same QR code in the terminal. Use the same
`--db` as the running server.

Five wrong codes end the offer. Each phone gets its own token: rename or revoke the phone in the
list. Revoking disconnects the phone at once.

## Missions

The phone lists what the active workspace offers:

| Mission | From | Phone controls |
|---|---|---|
| Hunt | A Signal hunt wired to a channel | Start, Stop, Tune, Sweep, Mark |
| DF drive | A Direction finder and its Triangulation | Tune, Calibrate, Clear, Navigate |
| Radar | A Passive radar | None, it only shows |
| Survey | A Signal survey | Record, Stop, Clear |

A mission that cannot run is dimmed and says why, such as `Not running`. The phone can also switch
the active workspace. That switches it for every client.

## Position and heading

Add a **GPS position** node, pick the **Phone** tab and the phone. Wire it where it is needed:

| Wire to | For |
|---|---|
| Array `position` | Where the array stands and which way it points |
| Signal hunt `position` | Sweep bearings with a handheld antenna |
| Signal survey `position` | Where each level was measured |
| Triangulation `position` | Guidance to the target |

The phone sends its pose only while a GPS node uses it. See [Position and GPS](position.md#heading).
