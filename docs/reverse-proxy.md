# LAN reverse proxy

CutCraft-Web can sit behind an operator-managed HTTPS reverse proxy with a browser-trusted
certificate. The proxy owns certificate issuance and renewal. This does not enable access from
outside the home network (spec §25).

Use HTTPS for **both** connections: browser → proxy → CutCraft-Web. Start CutCraft-Web normally,
adding `--name cutcraft.example` for the browser-facing hostname. Replace that example with the
operator's actual name. Keep its existing data directory and cutter options.

The proxy must:

- Pass the browser's original `Host` header, including the port if the public URL uses one.
  Preserve `Origin`, cookies and `x-csrf-token`; do not rewrite them to the upstream address.
- Connect to CutCraft-Web's HTTPS listener. Configure upstream certificate trust separately;
  browser trust in the proxy's certificate does not establish trust in the server certificate.
  `cutcraft-web certificate --data DIR` exports the public certificate; `fingerprint` prints its
  fingerprint. The private key remains on the server.
- Stream `/api/v1/design/live` and `/api/v1/events` without buffering. Allow long-lived
  connections; the server sends keepalives every 15 seconds. In Caddy, retain
  `header_up Host {host}` and `flush_interval -1` in the reverse-proxy block. Do not add buffering
  compression to these streams.
- Preserve the server's cache headers. Hashed desktop assets are immutable; private responses
  must not be cached by a shared proxy. Restrict access to the intended LAN.

Do **not** use `--insecure-http` for this deployment. It is a development mode that intentionally
omits Secure cookies, even if an external proxy happens to use HTTPS. There is no trusted-proxy
HTTP mode. `X-Forwarded-Host` and `X-Forwarded-Proto` do not override the server's Host checks or
cookie policy. Login throttling uses the connection's peer address, so people behind one proxy
share its per-address throttle.

## Verification

The router regression `https_proxy_preserves_cookie_origin_and_stream_contracts` checks the
configured external hostname, Secure CSRF cookie, same-origin mutation, missing-token and
cross-origin rejection, untrusted forwarded headers, and an immediate initial event on both
streams. It uses a simulated cutter and does not exercise a deployed proxy or its TLS handshake.

After deployment, check the actual browser URL has a trusted certificate, sign in, make a
non-motion change such as switching views, and verify live updates in a second tab. Inspect the
session cookie's Secure flag and both event streams in browser developer tools. A repeat page
load should reuse the hashed assets. These deployment checks are separate from the router tests;
no physical cut is required.
