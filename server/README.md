# TwoViewer server

The single server application (§2.2 of the SRS). At startup it applies pending database migrations ([`migrations/README.md`](migrations/README.md)), then serves traffic.

## Running

`docker compose up --build` at the repository root starts the server together with PostgreSQL and Mailpit ([`../README.md`](../README.md#running-the-server-side)). The image is built from [`Dockerfile`](Dockerfile). Against a database of your own, with a certificate and an email delivery service of your own:

```sh
DATABASE_URL=postgres://user:password@localhost/twoviewer \
TWOVIEWER_TLS__CERTIFICATE_FILE=fullchain.pem \
TWOVIEWER_TLS__PRIVATE_KEY_FILE=privkey.pem \
TWOVIEWER_EMAIL__FROM_ADDRESS=no-reply@mail.twoviewer.example \
TWOVIEWER_EMAIL__SMTP_HOST=smtp.email-service.example \
SMTP_USERNAME=... SMTP_PASSWORD=... \
cargo run -p twoviewer-server
```

For development, `TWOVIEWER_TLS__SELF_SIGNED=true` replaces the two certificate files ([HTTPS](#https)).

### Configuration

Every limit, timeout and other number is a setting with a default in `src/config.rs` (MNT-01). Later sources override earlier ones:

1. the defaults;
2. the TOML file named by `TWOVIEWER_CONFIG`, if set. [`config.example.toml`](config.example.toml) lists every setting with its default;
3. environment variables `TWOVIEWER_<SECTION>__<SETTING>`, such as `TWOVIEWER_HTTP__LISTEN=0.0.0.0:443` or `TWOVIEWER_LOG__FORMAT=text`.

Unknown settings fail startup, so typos don't go unnoticed.

Secrets are never settings and never go in source control (SEC-13). The server reads them only from the environment:

| Variable | Secret |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL, including the password |
| `SMTP_USERNAME`, `SMTP_PASSWORD` | The user name and password for the SMTP server of the email delivery service ([Email](#email)). Both, or neither for a server without authentication. |

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

## Real-time connection

Browsers and Host Agents keep a WebSocket open to the HTTPS listener for presence, session setup and notifications (IF-09). The code is in [`src/realtime/`](src/realtime/mod.rs), and the messages are in the shared crate [`../protocol`](../protocol/src/lib.rs).

| Path | For | Accepted only |
|---|---|---|
| `/api/realtime` | browsers | with an `Origin` of this host over HTTPS (`https://` + the `Host` header). Browsers send cookies with WebSocket handshakes and apply no CORS check, so this keeps other websites from connecting in a user's name. |
| `/api/agent/realtime` | Host Agents | without an `Origin`, which browsers always send, so web pages can't pose as agents. |

Others get `403`. Connections are anonymous until sign-in (#25, #26) and Host Agent credentials (#33, #36) exist; those checks go into the same two handlers.

**Messages.** The server sends `hello` first, then a `heartbeat` every `realtime.heartbeat_interval`, which the client answers. A connection that sends nothing for `realtime.heartbeat_timeout` is closed, and clients reconnect when the server is silent that long. With the defaults (3 s and 8 s), a Host Agent that disappears without closing its connection is offline within 10 seconds (DEV-07). A client that sends anything other than a heartbeat, a message above `realtime.max_message_bytes`, or that falls `realtime.send_queue_length` messages behind is disconnected. Clients reconnect after every close, with randomized, increasing delays (REL-05).

**Events.** A feature calls `realtime::publish(&txn, topic, &event)` in the transaction of its change. The event goes through PostgreSQL `NOTIFY` on channel `twoviewer_realtime`, so it is sent only if the transaction commits, and it reaches the connections on every instance (CAP-03) without another backing service (CON-02). Each instance listens on one database connection of its own, besides the pool. An event names what changed, such as a device ID, never the new state or anything secret: clients fetch the state through the API, which checks that they may see it (AUTH-17). It must stay under 8000 bytes, the notification limit.

**Resync.** Events are not stored. After every `hello`, clients fetch everything they show. When an instance loses its listening connection, events may be lost, so once it is back the instance sends `resync` to all of its clients, which then fetch everything again. While it is down, it retries every `realtime.listen_retry_interval`.

**Shutdown.** Connections are closed with code 1012 (*Service Restart*) as soon as shutdown starts, and clients reconnect to another instance, or to this one once it is back. The server doesn't wait for them as it does for requests.

The connections of Host Agents are counted in `twoviewer_host_agents_online`.

## Email

The server sends transactional emails: email verification, password reset, email change and security notifications. They go over SMTP to an external email delivery service (IF-12, DE-01). Any service that accepts SMTP works, such as Amazon SES, Postmark, Mailgun or SendGrid. The code is in [`src/email/`](src/email/mod.rs), and the templates are in [`templates/email/`](templates/email/).

**Settings.** `email.from_address`, the address emails come from, and `email.smtp_host`, the service's SMTP server, have no defaults; the server doesn't start without them. The connection is encrypted with STARTTLS on port 587 (`email.smtp_security = "starttls"`, `email.smtp_port`), or from the start with `"tls"`, usually on port 465. The server checks the SMTP server's certificate against the operating system's certificate store. `"none"` sends without encryption, for a local mail catcher only, and is refused with credentials. The user name and password are the secrets `SMTP_USERNAME` and `SMTP_PASSWORD`.

**Sender authentication.** Mail providers flag emails as spam unless the sending domain vouches for them (IF-12). Set this up with the email delivery service before launch:

1. Send from a subdomain of the TwoViewer domain, such as `mail.twoviewer.example`, so that its reputation and DNS records are separate from other mail.
2. **SPF:** set up the service's custom MAIL FROM (or Return-Path) domain on that subdomain, with the SPF record the service gives. The bounce address is then on the TwoViewer domain too, so SPF aligns for DMARC.
3. **DKIM:** add the service's DKIM records (2048-bit keys) for the subdomain. The service signs every email.
4. **DMARC:** add a `_dmarc` TXT record for the TwoViewer domain, such as `v=DMARC1; p=quarantine; rua=mailto:dmarc-reports@twoviewer.example`. Once the reports show only TwoViewer's own email passing, tighten it to `p=reject`.
5. Send the test email to an address at a large provider, such as Gmail, and check its `Authentication-Results` header: it must show `spf=pass`, `dkim=pass` and `dmarc=pass`.

**Test email.** `twoviewer-server send-test-email <address>` sends a test email with the server's settings and SMTP secrets, and prints the SMTP server's reply. It doesn't need the database. In a container: `docker compose exec server /usr/local/bin/twoviewer-server send-test-email <address>`.

**Delivery.** Features queue emails, and requests return without waiting for the SMTP server, so a response doesn't reveal whether an address is registered (SEC-14). The server sends queued emails in the background, over at most `email.max_connections` connections. An email that fails for a temporary reason, such as a 4xx reply, a refused connection or a timeout (`email.timeout`), is tried again after `email.retry_delay`, and then after twice as long each time, up to `email.max_attempts` attempts. A 5xx reply is final. At most `email.max_pending` emails wait, including those waiting to be tried again; further emails are dropped. Queued emails are kept only in memory, because they contain single-use links whose tokens the database stores only as hashes (SEC-07). If the server crashes, the emails still waiting are lost, and people ask for the link again. On shutdown, the server finishes the attempts in progress after open requests, within `http.shutdown_timeout`, and abandons emails waiting to be tried again. The email delivery service doesn't affect readiness: while it is down, the server keeps serving, and emails fail.

**Light and dark mode.** Emails have an HTML and a plain-text body, and their colors are the web client's. They are legible in clients in light or dark mode (THM-11): `layout.html` says how, and the tests check that every color has a dark counterpart and enough contrast. After changing the layout, look at the test email in Apple Mail, Gmail (web and app) and Outlook.com, in both modes. Mailpit's *HTML Check* tab lists what each client supports.

**Logs.** Each attempt is logged with the email's name, such as `test`, its Message-ID, which the email delivery service's logs show too, the attempt number and the SMTP reply code. Recipients, subjects and content are never logged, because they are personal data and contain single-use links (MNT-03). Neither is the SMTP server's reply text, which can repeat the recipient's address.

## Passwords

Sign-up, password change and password reset check new passwords against the same rules, and store only their hashes. The code is in [`src/accounts/password.rs`](src/accounts/password.rs), and the settings are in `[passwords]`.

**Rules.** A new password has at least `passwords.min_length` characters (12, AUTH-03) and at most `passwords.max_length` (1024; AUTH-03 requires accepting at least 128). Any characters count, spaces included, and there are no rules about which kinds a password must contain. Passwords on the list of compromised passwords are rejected, whatever their capitalization (AUTH-04). Signing in checks only that the password is right, so a password set under earlier rules keeps working.

**Normalization.** Passwords are normalized to Unicode NFKC on arrival, so the same characters give the same hash however a device encodes them. Nothing else changes: spaces, case and length are kept as typed. Lengths count Unicode code points, not bytes.

**Hashing.** Passwords are stored only as Argon2id hashes with a random salt each (SEC-07), as PHC strings that record the parameters they were made with. The defaults are OWASP's recommended minimum: `passwords.hash_memory_kib` 19 MiB, `passwords.hash_iterations` 2, and `passwords.hash_parallelism` 1. One hash takes about 20 ms of one CPU core.

- **Memory.** At most `passwords.max_concurrent_hashes` (4) passwords are hashed or checked at once, on the blocking thread pool, and others wait. Hashing therefore takes at most `max_concurrent_hashes × hash_memory_kib` of memory, 76 MiB with the defaults, however many sign-in attempts arrive.
- **Raising the cost.** New hashes use new settings at once. A user's existing hash is replaced with one made with the new settings at their next sign-in. Keep a sign-in well under the 300 ms of PERF-06: measure with `cargo test --release -p twoviewer-server --lib hashes_are_argon2id -- --nocapture`, which prints how long one hash takes with the default settings.
- **Unknown accounts.** A sign-in with an address that has no account still computes a hash, so the response takes as long and doesn't reveal whether the address is registered (SEC-14).

Passwords and hashes never appear in logs or audit events (MNT-03, AUD-06).

**The compromised list.** [`data/compromised-passwords.txt`](data/compromised-passwords.txt) is embedded in the server, so the check needs no external service, and passwords never leave the server. It holds the 46,146 passwords of at least 12 characters from SecLists' `xato-net-10-million-passwords-1000000.txt`, the most common passwords from public breaches ([`data/compromised-passwords.LICENSE`](data/compromised-passwords.LICENSE)). Shorter ones aren't listed, since they are rejected anyway; lowering `passwords.min_length` below 12 lets them through. To regenerate the list, or to add more word lists:

```sh
cargo xtask compromised-passwords xato-net-10-million-passwords-1000000.txt [more word lists...]
```

It writes the passwords in the form the server looks them up in: normalized, lowercased, sorted and without duplicates. A unit test checks the file.

## Logs

Operational logs go to standard output, one JSON object per line (`log.format = "text"` for development). They are separate from audit events (MNT-03). Each request on the HTTPS listener gets a correlation ID: it is returned in the `x-request-id` header, is on every log line for that request (`span.correlation_id`), and is the `correlation_id` of the audit events the request records.

Logs never contain secrets or session content. Log route templates such as `/devices/{id}`, never request paths, query strings, headers or bodies.

## Metrics

`/metrics` serves OpenMetrics text (MNT-04):

| Metric | Type | Labels | Reported by |
|---|---|---|---|
| `twoviewer_host_agents_online` | gauge | | the real-time connection, per open Host Agent connection |
| `twoviewer_sessions_active` | gauge | `type` (`control`, `vpn`), `transport` (`direct`, `relayed`) | sessions and VPN (#40, #44, #62) |
| `twoviewer_relay_bytes_total` | counter | | the relay (#40) |
| `twoviewer_sign_ins_failed_total` | counter | | sign-in (#25) |
| `twoviewer_http_requests_total` | counter | `route`, `status_class` (`2xx` … `5xx`) | every request on the HTTPS listener |
| `twoviewer_migration_duration_seconds` | gauge | | the migrations at startup |
| `twoviewer_emails_sent_total` | counter | | email, per email accepted by the email delivery service |
| `twoviewer_emails_failed_total` | counter | | email, per email given up: rejected (5xx), failing on every attempt, dropped beyond `email.max_pending`, or abandoned at shutdown |

Metrics that a feature doesn't report yet read 0.
