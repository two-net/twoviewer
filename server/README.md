# TwoViewer server

The single server application (§2.2 of the SRS). At startup it applies pending database migrations ([`migrations/README.md`](migrations/README.md)), then serves traffic.

## Running

`docker compose up --build` at the repository root starts the server together with PostgreSQL ([`../README.md`](../README.md#running-the-server-side)). The image is built from [`Dockerfile`](Dockerfile). Against a database of your own, with a certificate of your own:

```sh
DATABASE_URL=postgres://user:password@localhost/twoviewer \
TWOVIEWER_TLS__CERTIFICATE_FILE=fullchain.pem \
TWOVIEWER_TLS__PRIVATE_KEY_FILE=privkey.pem \
cargo run -p twoviewer-server
```

For development, `TWOVIEWER_TLS__SELF_SIGNED=true` replaces the two certificate files ([HTTPS](#https)).

### Configuration

Every limit, timeout and other number is a setting with a default in `src/config.rs` (MNT-01). Later sources override earlier ones:

1. the defaults;
2. the TOML file named by `TWOVIEWER_CONFIG`, if set. [`twoviewer.example.toml`](twoviewer.example.toml) lists every setting with its default;
3. environment variables `TWOVIEWER_<SECTION>__<SETTING>`, such as `TWOVIEWER_HTTP__LISTEN=0.0.0.0:443` or `TWOVIEWER_LOG__FORMAT=text`.

Unknown settings fail startup, so typos don't go unnoticed.

Secrets are never settings and never go in source control (SEC-13). The server reads them only from the environment:

| Variable | Secret |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL, including the password |

To encrypt the connection to the database too, add `sslmode=verify-full` to the URL, and `sslrootcert=<file>` if the database certificate isn't signed by a public CA.

The TLS private key is a secret as well. The server reads it from the file that `tls.private_key_file` names. That file is provided at runtime, for example as a mounted secret, and never kept in source control.

### Listeners

| Default | Listener | Opens | Serves |
|---|---|---|---|
| `0.0.0.0:8443` | HTTPS (`http.listen`) | once migrations have completed (DB-05) | the API and web client |
| `0.0.0.0:8080` | plain HTTP (`http.redirect_listen`) | with the HTTPS listener | only redirects to HTTPS |
| `0.0.0.0:9090` | monitoring (`monitoring.listen`) | at startup | `/livez`, `/readyz`, `/metrics` |

Publish 8443 as 443 and 8080 as 80, the standard ports (IF-08). Redirects point to port `http.public_port`, 443 unless you change it.

Keep the monitoring listener private: it speaks plain HTTP.

- `/livez` answers 200 while the process runs. It doesn't check the database, so a database outage doesn't get the server restarted.
- `/readyz` answers 200 only once migrations have completed and while the database answers within `monitoring.readiness_timeout`; otherwise 503 (MNT-02). It also answers 503 from the start of a shutdown, so load balancers stop sending traffic while open requests finish (up to `http.shutdown_timeout`).

### HTTPS

All traffic from browsers and Host Agents is HTTPS (SEC-01). The server terminates TLS itself, with TLS 1.3 and 1.2 only (IF-08): rustls implements nothing older. Behind a load balancer (CAP-03), pass TCP through to the HTTPS listener rather than terminating TLS there.

- **Certificate:** `tls.certificate_file`, the chain with the leaf first, and `tls.private_key_file`, both PEM. If they cannot be loaded at startup, for example because the key doesn't match the certificate, the server stops before it migrates.
- **Renewal:** the server checks the files every `tls.reload_interval` and switches to a renewed certificate without a restart. If the new files cannot be loaded, it keeps the current certificate, logs a warning and tries again at the next check.
- **Development:** `tls.self_signed = true`, instead of the files, generates a certificate for `localhost` at startup. Browsers warn about it, and Host Agents don't trust it.
- **Redirects:** every request on the plain-HTTP listener, whatever its method, gets a `308 Permanent Redirect` to the same host, path and query over HTTPS. These requests are neither logged nor counted.
- **Handshakes:** a client gets `tls.handshake_timeout` to complete the TLS handshake. Handshakes run concurrently, so a slow client doesn't hold up others.

### Security headers

Every response of the HTTPS listener carries these headers, whatever its route or status:

| Header | Value | Purpose |
|---|---|---|
| `Strict-Transport-Security` | `max-age=<http.hsts_max_age in seconds>; includeSubDomains` | Browsers use only HTTPS for the site and its subdomains (SEC-01). |
| `Content-Security-Policy` | in [`src/http.rs`](src/http.rs) | `frame-ancestors 'none'`: no other site can frame the web client, which protects the consent prompt against clickjacking. `script-src 'self'`: only scripts from the TwoViewer origin run, so third-party tracking code can't either (SEC-10, PRIV-05). |
| `X-Frame-Options` | `DENY` | The same for browsers without `frame-ancestors`. |
| `X-Content-Type-Options` | `nosniff` | A response runs as a script only if it is declared as one. |

The headers replace any that a handler sets, so the policy is in one place. HSTS leaves out `preload`, which is hard to undo. `http.hsts_max_age = "0s"` makes browsers forget the policy. Development on `localhost` needs this, because HSTS covers every port of a host.

## Logs

Operational logs go to standard output, one JSON object per line (`log.format = "text"` for development). They are separate from audit events (MNT-03). Each request on the HTTPS listener gets a correlation ID: it is returned in the `x-request-id` header, is on every log line for that request (`span.correlation_id`), and is the `correlation_id` of the audit events the request records.

Logs never contain secrets or session content. Log route templates such as `/devices/{id}`, never request paths, query strings, headers or bodies.

## Metrics

`/metrics` serves OpenMetrics text (MNT-04):

| Metric | Type | Labels | Reported by |
|---|---|---|---|
| `twoviewer_host_agents_online` | gauge | | the real-time connection (#17) |
| `twoviewer_sessions_active` | gauge | `type` (`control`, `vpn`), `transport` (`direct`, `relayed`) | sessions and VPN (#40, #44, #62) |
| `twoviewer_relay_bytes_total` | counter | | the relay (#40) |
| `twoviewer_sign_ins_failed_total` | counter | | sign-in (#25) |
| `twoviewer_http_requests_total` | counter | `route`, `status_class` (`2xx` … `5xx`) | every request on the HTTPS listener |
| `twoviewer_migration_duration_seconds` | gauge | | the migrations at startup |

Metrics that a feature doesn't report yet read 0.
