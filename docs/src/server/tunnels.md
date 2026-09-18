# HTTPS with Tailscale or Cloudflare Tunnel

Choose Tailscale for access from your own devices, or Cloudflare Tunnel for a browser-accessible
hostname with a login. Both manage HTTPS certificates without router port forwarding.

## Prepare SDR--

Run the tunnel client on the same host as SDR--. Keep the local connection on HTTP and let the
tunnel handle HTTPS:

```sh
export SDRMM_TOKEN='replace-with-a-long-random-secret'
sdrmm --bind 127.0.0.1:8080
```

Keep SDR-- running, or apply these settings to its service. Omit `--tls-*` and `--dev-cors` options.
Check `http://127.0.0.1:8080` on the server before continuing.

For [Docker Compose](deployment.md#docker-compose), replace the existing `ports` mapping in
`docker-compose.yml` and add the token environment file:

```yaml
services:
  sdrmm:
    ports:
      - "127.0.0.1:8080:8080"
    env_file: .env
```

Keep the other service settings. Store `SDRMM_TOKEN=replace-with-a-long-random-secret` in a protected
`.env` file outside version control, then run `docker compose up -d`. Leave SDR-- bound to
`0.0.0.0:8080` **inside** the container. These instructions run Tailscale or `cloudflared` on the
host; loopback inside a separate container would point to that container instead.

## Tailscale: private HTTPS

1. [Install Tailscale](https://tailscale.com/download) on the server and each client, including
   your phone. Sign them into the same tailnet.
2. In the Tailscale admin console's **DNS** page, enable **MagicDNS** and **HTTPS Certificates**.
   Certificate names appear in public certificate transparency logs.
3. On the server, run:

   ```sh
   tailscale serve --bg --https=443 http://127.0.0.1:8080
   tailscale serve status
   ```

4. Open the printed `https://<machine>.<tailnet>.ts.net` URL from a connected client and enter the
   SDR-- token. Use the full hostname; a short name or Tailscale IP will not match the certificate.

Serve provisions the certificate automatically. `--bg` keeps the configuration active across
reboots while Tailscale runs. Tailnet access rules must permit clients to reach the server on
TCP port 443. This setup stays private to your tailnet; Funnel publishes services to the internet.

To disable this endpoint:

```sh
tailscale serve --bg --https=443 off
```

See [Tailscale Serve](https://tailscale.com/docs/reference/tailscale-cli/serve) and
[HTTPS setup](https://tailscale.com/docs/how-to/set-up-https-certificates).

## Cloudflare Tunnel: a hostname with login

Use a Cloudflare account with an active domain, such as `example.com`.

1. Before publishing, open **Zero Trust → Access controls → Applications**. Create a
   **Self-hosted and private** application with public hostname `radio.example.com` and no path
   restriction. Add an **Allow** policy for your email addresses or identity group, choose a login
   method, and save.
2. Open **Networking → Tunnels → Create Tunnel** and name it `sdrmm`. Select your server's OS and
   follow **Install and Run** to install `cloudflared` as a service on the SDR-- host. Keep its
   tunnel token private; it is separate from `SDRMM_TOKEN`.
3. Once the tunnel is **Healthy**, select **Routes → Add route → Published application**. Set
   the hostname to `radio.example.com`, leave the path unrestricted, and set **Service URL** to
   `http://127.0.0.1:8080`. Save the route.
4. Open `https://radio.example.com`, complete the Access login, then enter the SDR-- token.

Cloudflare creates the DNS route and handles public HTTPS. The tunnel alone does not restrict
visitors; the Access policy supplies the login. API and MCP clients also need Access authentication.

See [Tunnel setup](https://developers.cloudflare.com/tunnel/get-started/) and
[Access setup](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/).

## Verify the connection

Open the HTTPS root URL, start a receiver, and check that the spectrum and audio update. The
interface, `/api/*`, `/api/ws`, and `/mcp` must share this origin. If the page loads but live data
does not, check the `/api/ws` connection in browser developer tools. Cloudflare's
[WebSockets setting](https://developers.cloudflare.com/network/websockets/) must be enabled.

For [field mode](../user-guide/field-mode.md), open **Library → Field** from the HTTPS page so
the QR code uses that hostname, or open the same URL with `/field` on your phone. Keep Tailscale
connected for its private URL; complete the Access login for Cloudflare. HTTPS enables browser
location and AudioWorklet playback; location still needs browser permission.

If the tunnel reports a bad gateway, check that `http://127.0.0.1:8080` works on the tunnel host
and that SDR-- is serving HTTP there. For Cloudflare, test in a private browser window to confirm
Access requires login before the SDR-- page loads.
