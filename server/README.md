# TwoViewer server

The single server application (§2.2 of the SRS). At startup it applies pending database migrations ([`migrations/README.md`](migrations/README.md)), then serves traffic.

## Running

`docker compose up --build` at the repository root starts the server together with PostgreSQL and Mailpit ([`../README.md`](../README.md#running-the-server-side)). The image is built from [`Dockerfile`](Dockerfile). Against a database of your own, with a certificate and an email delivery service of your own:

```sh
DATABASE_URL=postgres://user:password@localhost/twoviewer \
TWOVIEWER_TLS__CERTIFICATE_FILE=fullchain.pem \
TWOVIEWER_TLS__PRIVATE_KEY_FILE=privkey.pem \
TWOVIEWER_HTTP__PUBLIC_URL=https://twoviewer.example \
TWOVIEWER_EMAIL__FROM_ADDRESS=no-reply@mail.twoviewer.example \
TWOVIEWER_EMAIL__SMTP_HOST=smtp.email-service.example \
SMTP_USERNAME=... SMTP_PASSWORD=... \
cargo run -p twoviewer-server
```

For development, `TWOVIEWER_TLS__SELF_SIGNED=true` replaces the two certificate files ([HTTPS](#https)).

`http.public_url` is where people reach the web client, such as `https://twoviewer.example`: links in emails point there. It has no default, and must be `https://` without a path, or `http://` for `localhost` only. Links are never made from the `Host` header of a request, which whoever sends the request chooses.

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

### Forged requests

Other sites must not be able to make a signed-in user's browser act in their name (SEC-09). Three things keep them out:

- The sign-in cookie is `SameSite=Strict`, so browsers don't send it with requests that other sites start ([Sign-in](#sign-in)).
- The API takes only JSON bodies, which browsers send to another site only after a CORS preflight that the server never grants.
- Every request that may change something, which is any but `GET`, `HEAD` and `OPTIONS`, is refused with `403` (`cross_site_request`) when its `Sec-Fetch-Site` header is there and isn't `same-origin`, or its `Origin` header is there and isn't `https://` + its `Host`. This also covers requests without a body, such as sign-out, and sites that browsers count as the same site for cookies, such as other subdomains. Browsers send at least one of the two headers with such requests; clients that send neither, such as Host Agents, aren't browsers and carry no cookie of their own accord.

The check is in [`src/http.rs`](src/http.rs) and applies to every route. The development server of the web client proxies requests with an `Origin` of the server's own ([`../web/proxy.conf.json`](../web/proxy.conf.json)), and leaves `Sec-Fetch-Site` as the browser sent it.

## Real-time connection

Browsers and Host Agents keep a WebSocket open to the HTTPS listener for presence, session setup and notifications (IF-09). The code is in [`src/realtime/`](src/realtime/mod.rs), and the messages are in the shared crate [`../protocol`](../protocol/src/lib.rs).

| Path | For | Accepted only |
|---|---|---|
| `/api/realtime` | browsers | with an `Origin` of this host over HTTPS (`https://` + the `Host` header). Browsers send cookies with WebSocket handshakes and apply no CORS check, so this keeps other websites from connecting in a user's name. |
| `/api/agent/realtime` | Host Agents | without an `Origin`, which browsers always send, so web pages can't pose as agents. |

Others get `403`. A browser also needs an active sign-in, from its cookie ([Sign-in](#sign-in)), and receives the events of its user. Without one, the connection is opened and closed at once with code 4001 (`NOT_SIGNED_IN`), because browsers can read the code of a close but not the status of a refused handshake; the web client then goes to the sign-in page instead of reconnecting. A connection is closed the same way when its sign-in is revoked, on whichever instance it is, and when it expires: the connection checks again when the sign-in is due to end, and at least daily, since using the sign-in in the meantime moves the end on. Host Agent connections are anonymous until Host Agent credentials exist (#33, #36); that check goes into their handler.

**Messages.** The server sends `hello` first, then a `heartbeat` every `realtime.heartbeat_interval`, which the client answers. A connection that sends nothing for `realtime.heartbeat_timeout` is closed, and clients reconnect when the server is silent that long. With the defaults (3 s and 8 s), a Host Agent that disappears without closing its connection is offline within 10 seconds (DEV-07). A client that sends anything other than a heartbeat, a message above `realtime.max_message_bytes`, or that falls `realtime.send_queue_length` messages behind is disconnected. Clients reconnect after every close, with randomized, increasing delays (REL-05), except browsers closed with 4001.

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

## Sign-up

People create an account with an email address, a display name and a password, after accepting the Terms of Service and the Privacy Notice (AUTH-01), and verify the address with an emailed link (AUTH-02). The code is in [`src/accounts/`](src/accounts/mod.rs), and the API's conventions are in [`src/api.rs`](src/api.rs).

| Request | Answers |
|---|---|
| `POST /api/sign-up` with `email`, `displayName`, `password`, `termsVersion`, `privacyVersion` and, if asked for, `challenge` | `202`, whether or not the address is registered; `422` with the fields that break the rules; `409` if the terms have changed; `428` with a challenge; `429` |
| `POST /api/verify-email` with the `token` from the link | `204` once the address is verified; `410` for a link that has expired or doesn't exist |

**Accounts.** Accounts are in `user_account`, with the versions of the Terms of Service and Privacy Notice accepted, which are the dates the texts last changed ([`src/accounts/legal.rs`](src/accounts/legal.rs) and `web/src/app/legal/versions.ts`, which a test keeps the same). Sign-up refuses other versions, so the version recorded is the one the user read. Sign-up and the accepted versions are audited, and so is the verification, with the user's IP address and browser.

**Not revealing who has an account (SEC-14).** Sign-up answers `202` with no body, whether the address is new or not, and hashes the password either way, so that every answer takes about as long. Only the owner of the address learns what happened, from the email:

- A new address gets an account and a link that verifies it.
- An address with an account that isn't verified yet gets a new link. Signing up again is how people get a new link once theirs has expired. The account keeps the password and display name it was created with: otherwise anyone who knew the address could set the password of an account that its owner then verifies. The email says so.
- An address with a verified account gets an email that says so, with links to sign in and to reset the password.

**Links.** A link is `<http.public_url>/verify-email#<token>`, valid for `accounts.verification_link_lifetime` (24 hours) and used once. The token is in the fragment, which browsers never send to a server, so it stays out of logs and `Referer` headers. The database stores only the SHA-256 of the token (SEC-07). Opening a used link again still answers `204` while the address is verified, because email scanners often open links before people do. Links are kept for 24 hours after use or expiry, then deleted by the retention job (#66). Emails never contain the display name, so sign-up cannot be used to send text of one's choosing to any address.

**Until the address is verified,** the user can sign in, but cannot pair devices or start sessions (AUTH-02). Those features check `user::Model::is_verified`.

**Abuse (AUTH-15).** Without third-party services:

- Sign-ups are counted per network: an IPv4 address, or an IPv6 /64. Past `sign_up.challenge_after` (3) sign-ups within `sign_up.window` (1 hour), the browser must solve a proof-of-work challenge before its sign-up goes through: find a number that, hashed with SHA-256 together with the challenge, gives `sign_up.challenge_difficulty` leading zero bits. That takes a browser a second or two, without a puzzle for people to solve. Past `sign_up.max_per_network` (30), sign-up answers `429` with `Retry-After` until the window has moved on.
- Counting takes at most `sign_up.max_tracked_networks` networks. When that many are signing up, the others get a challenge.
- Sign-ups email one address at most once per `sign_up.notice_interval` (15 minutes), so sign-up cannot flood an inbox. The sign-ups in between get the same answer, without an email.
- The counts are kept in memory by each instance, so with several instances a network can sign up that many times more often. Challenges are stored in the database (`sign_up_challenge`), so that any instance can check one, and each can be used once.

## Sign-in

People sign in with their email address and password (AUTH-05). Failed attempts are throttled per address (AUTH-08) and per network (SEC-08), and no answer reveals whether an address is registered (SEC-14). Sign-ins end after a week unused and a month at most, and users can see and end theirs (AUTH-09). The code is in [`src/accounts/sign_in.rs`](src/accounts/sign_in.rs), [`src/accounts/sign_ins.rs`](src/accounts/sign_ins.rs), [`src/accounts/sign_in_session.rs`](src/accounts/sign_in_session.rs) and [`src/throttle.rs`](src/throttle.rs).

| Request | Answers |
|---|---|
| `POST /api/sign-in` with `email` and `password` | `204` with the sign-in cookie; `401` (`sign_in_failed`) for a wrong password or an address without an account alike; `422` for empty fields or something that isn't an address; `429` with `Retry-After` while blocked |
| `GET /api/account` | `200` with the signed-in user's `id`, `email`, `displayName` and `emailVerified`; `401` (`not_signed_in`) without an active sign-in |
| `GET /api/sign-ins` | `200` with `signIns`, the user's active sign-ins: `id`, `client` (such as "Firefox on Linux"), `ipAddress`, `signedInAt`, `lastActiveAt` and whether it is the `current` one, which comes first, then the most recently used first; `401` |
| `DELETE /api/sign-ins/{id}` | `204`, and for the current sign-in a cookie that removes it; `410` (`sign_in_ended`) for a sign-in that has ended, or that isn't the user's; `400` for an `id` that isn't one; `401` |
| `DELETE /api/sign-ins` | `204` once every sign-in but the current one is revoked; `401` |
| `POST /api/sign-out` | `204` with a cookie that removes the sign-in cookie, also when the sign-in has already ended |

**Sign-ins.** Signing in stores a sign-in in `sign_in_session`, with the IP address and browser it came from, and gives the browser its token in the cookie `__Host-twoviewer-session`: `HttpOnly`, so page scripts can't read it, `Secure` and `__Host-`, so it goes over HTTPS only and only this host can set it, and `SameSite=Strict`, so other sites can't make the browser send it (SEC-09, and [Forged requests](#forged-requests)). The database stores only the SHA-256 of the token (SEC-07). Handlers that need a signed-in user take `accounts::SignedIn`. Unverified users can sign in (AUTH-02).

**Expiry (AUTH-09).** A sign-in ends after `sign_in.idle_timeout` (7 days) without use, and at the latest `sign_in.session_lifetime` (30 days) after sign-in, however much it is used. Every request that it authenticates is use: it sets `last_active_at` to now and `expires_at` to the earlier of the two ends. That write happens at most once a minute per sign-in, so a burst of requests writes once, and a sign-in can end up to a minute early. The cookie is kept for `sign_in.session_lifetime`; the server ends the sign-in earlier if it isn't used. Ended sign-ins are kept for 30 days after `coalesce(revoked_at, expires_at)`, then deleted by the retention job (Table 5.3-A, #66).

**Revoking.** The user sees their active sign-ins in Settings and can revoke any of them, or all but the one they are using (AUTH-09); signing out revokes the one they are using. Another user's sign-in gets the same `410` as one that has ended, so its ID reveals nothing (AUTH-17). Revoking closes the browser's real-time connection, which takes it to the sign-in page, and tells the user's other browsers to reload their list (`sign_ins.changed`). Password reset (#28), account deletion (#30) and suspension (#68) revoke all of a user's sign-ins with `sign_in_session::revoke(.., Revoke::All, ..)`, in the transaction of their change.

**Not revealing who has an account (SEC-14).** A wrong password and an address without an account get the same `401`, and the password is hashed either way, so the answer takes as long. Blocks work the same way: they are per address, whether or not it has an account.

**Blocks (AUTH-08).** After `sign_in.max_failures` (10) failed sign-ins with one address within `sign_in.failure_window` (15 minutes, a sliding window), sign-in with that address answers `429` for `sign_in.block_duration` (15 minutes), even with the right password, without checking it. Attempts during a block don't count, so a block ends on time however often someone tries. A successful sign-in forgets the failures. When a block starts, the owner of the account, if there is one, gets an email that says so and links to the password reset. The counts are in the `throttle` table, so every instance sees the same counts and a restart doesn't lift a block. They are kept under the SHA-256 of the address, and deleted once their failures have left the window and their block has ended. Anyone can block sign-in with someone else's address for a while by guessing wrong ten times. That is the price of the requirement, and the owner can still reset their password.

**Networks (SEC-08).** Past `sign_in.max_failures_per_network` (50) failed sign-ins within the window, a network (an IPv4 address or an IPv6 /64) gets `429` until the window has moved on, before any password is hashed. This slows down guessing across many addresses, which the limit per address doesn't catch. Like the sign-up limits, it is counted in memory by each instance, for at most `sign_in.max_tracked_networks` networks, and no IP addresses are stored.

**Audit.** Every attempt records `sign_in_succeeded` or `sign_in_failed`, before the answer (AUD-03), with the IP address and browser. Failures are recorded with a user actor without an ID, since whoever tried isn't known to be the owner, and with the account as the target if the address has one. Their reason is `invalid_credentials`, `blocked` or `network_blocked`. The failure that starts a block also records `sign_in_blocked`, with the reason `too_many_failures`. Every failed attempt, refused ones included, counts in `twoviewer_sign_ins_failed_total`. Signing out records `sign_out`, and revoking records `sign_in_session_revoked`, once per sign-in that ends, with its ID in `sign_in_session_id`. Signing out of a sign-in that had already ended records nothing.

**Still to come.** The second step for users with MFA (AUTH-05) comes with MFA itself (#27), between the password check and the creation of the sign-in. Suspended accounts can't sign in once suspension exists (OPS-03, #68).

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
