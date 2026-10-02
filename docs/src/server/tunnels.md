# HTTPS with a tunnel

A tunnel gives SDR-- an HTTPS address without port forwarding or certificate work.

| Tunnel | Reachable from |
|---|---|
| [app.sdrmm.com](#appsdrmmcom) | Any browser, behind your app.sdrmm.com login |
| [Tailscale](#tailscale) | Your own devices only |
| [Cloudflare Tunnel](#cloudflare-tunnel) | Any browser, behind a login |

Tunnels do not carry [phones](../user-guide/phones.md). Phones pin the server's own key, so they
need its direct HTTPS endpoint: the phone port on your LAN, or over the server's Tailscale IP.

## app.sdrmm.com

No setup on the network. SDR-- dials out and app.sdrmm.com passes browsers through.

1. Open **Library → Remote** and press **Connect to app.sdrmm.com**.
2. Approve the code at app.sdrmm.com.
3. Open the server from your device list there.

Headless: run `sdrmm pair`, approve the code, then restart `sdrmm`. Pass the server's `--db` if it
uses one. **Disconnect** or removing the server in the app ends access. Traffic passes Cloudflare,
which terminates HTTPS.

Browsers that come through the app skip the SDR-- token; your app.sdrmm.com login guards them.

SDR-- pairs under the computer's name. While connected, it reports its version, platform, start
time and open clients, and for each radio its name, driver, state, fault, channel count, drops and
whether it records. Never frequencies, channel names or files.

## Prepare SDR--

For Tailscale or Cloudflare, run the tunnel on the same machine as SDR--. SDR-- stays on plain
HTTP on loopback; the tunnel adds HTTPS:

```sh
export SDRMM_TOKEN='replace-with-a-long-random-secret'
sdrmm --bind 127.0.0.1:8080
```

Leave out the `--tls-*` options. Check `http://127.0.0.1:8080` works on the server.

With [Docker Compose](deployment.md#docker-compose), change the published port to loopback only
and add the token file:

```yaml
services:
  sdrmm:
    ports:
      - "127.0.0.1:8080:8080"
    env_file: .env
```

Inside the container SDR-- stays on `0.0.0.0:8080`. Run the tunnel on the host, not in another
container.

## Tailscale

1. [Install Tailscale](https://tailscale.com/download) on the server and on each client, all in
   the same tailnet.
2. In the admin console's **DNS** page, turn on **MagicDNS** and **HTTPS Certificates**. Machine
   names become public in certificate transparency logs.
3. On the server:

   ```sh
   tailscale serve --bg --https=443 http://127.0.0.1:8080
   tailscale serve status
   ```

4. Open the printed `https://<machine>.<tailnet>.ts.net` address and enter the SDR-- token. Use
   the full name; a short name or IP does not match the certificate.

The setting survives reboots. Your tailnet access rules must allow TCP 443 to the server. Turn it
off with `tailscale serve --bg --https=443 off`. See
[Tailscale Serve](https://tailscale.com/docs/reference/tailscale-cli/serve).

## Cloudflare Tunnel

You need a domain on Cloudflare, such as `example.com`.

1. **Add the login first.** In **Zero Trust → Access controls → Applications**, create a
   **Self-hosted and private** app for `radio.example.com` with no path. Add an **Allow** policy
   for your email or group and save.
2. In **Networking → Tunnels → Create Tunnel**, name it `sdrmm` and follow **Install and Run** to
   install `cloudflared` as a service on the SDR-- machine. Keep its tunnel token private.
3. When the tunnel is **Healthy**, add a **Published application** route: hostname
   `radio.example.com`, no path, service `http://127.0.0.1:8080`.
4. Open `https://radio.example.com`, log in, then enter the SDR-- token.

The tunnel alone lets anyone in; the Access policy is what adds the login. API and MCP clients
must pass Access too. Make sure Cloudflare's
[WebSockets setting](https://developers.cloudflare.com/network/websockets/) is on. See
[Tunnel setup](https://developers.cloudflare.com/tunnel/get-started/).

## Check it

Open the HTTPS address, start a receiver, and check that spectrum and audio move.

- **Page loads, nothing moves:** check the `/api/ws` connection in the browser's developer tools.
- **Bad gateway:** check `http://127.0.0.1:8080` works on the tunnel machine.
- **Cloudflare:** open a private window and check the login appears before SDR-- does.
