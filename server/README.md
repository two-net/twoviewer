# TwoViewer server

The single server application (§2.2 of the SRS). At startup it applies pending database migrations ([`migrations/README.md`](migrations/README.md)), then serves traffic.

## Running

`docker compose up --build` at the repository root starts the server together with PostgreSQL ([`../README.md`](../README.md#running-the-server-side)). The image is built from [`Dockerfile`](Dockerfile). Against a database of your own:

```sh
DATABASE_URL=postgres://user:password@localhost/twoviewer cargo run -p twoviewer-server
```

### Configuration

Every limit, timeout and other number is a setting with a default in `src/config.rs` (MNT-01). Later sources override earlier ones:

1. the defaults;
2. the TOML file named by `TWOVIEWER_CONFIG`, if set. [`twoviewer.example.toml`](twoviewer.example.toml) lists every setting with its default;
3. environment variables `TWOVIEWER_<SECTION>__<SETTING>`, such as `TWOVIEWER_HTTP__LISTEN=0.0.0.0:80` or `TWOVIEWER_LOG__FORMAT=text`.

Unknown settings fail startup, so typos don't go unnoticed.

Secrets are never settings and never go in source control (SEC-13). The server reads them only from the environment:

| Variable | Secret |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL, including the password |

### Listeners

| Default | Listener | Opens | Serves |
|---|---|---|---|
| `0.0.0.0:8080` | public (`http.listen`) | once migrations have completed (DB-05) | the API and web client |
| `0.0.0.0:9090` | monitoring (`monitoring.listen`) | at startup | `/livez`, `/readyz`, `/metrics` |

Keep the monitoring listener private.

- `/livez` answers 200 while the process runs. It doesn't check the database, so a database outage doesn't get the server restarted.
- `/readyz` answers 200 only once migrations have completed and while the database answers within `monitoring.readiness_timeout`; otherwise 503 (MNT-02). It also answers 503 from the start of a shutdown, so load balancers stop sending traffic while open requests finish (up to `http.shutdown_timeout`).

## Logs

Operational logs go to standard output, one JSON object per line (`log.format = "text"` for development). They are separate from audit events (MNT-03). Each request on the public listener gets a correlation ID: it is returned in the `x-request-id` header, is on every log line for that request (`span.correlation_id`), and is the `correlation_id` of the audit events the request records.

Logs never contain secrets or session content. Log route templates such as `/devices/{id}`, never request paths, query strings, headers or bodies.

## Metrics

`/metrics` serves OpenMetrics text (MNT-04):

| Metric | Type | Labels | Reported by |
|---|---|---|---|
| `twoviewer_host_agents_online` | gauge | | the real-time connection (#17) |
| `twoviewer_sessions_active` | gauge | `type` (`control`, `vpn`), `transport` (`direct`, `relayed`) | sessions and VPN (#40, #44, #62) |
| `twoviewer_relay_bytes_total` | counter | | the relay (#40) |
| `twoviewer_sign_ins_failed_total` | counter | | sign-in (#25) |
| `twoviewer_http_requests_total` | counter | `route`, `status_class` (`2xx` … `5xx`) | every request on the public listener |
| `twoviewer_migration_duration_seconds` | gauge | | the migrations at startup |

Metrics that a feature doesn't report yet read 0.
