# TwoViewer

Remote access and remote support, used through the browser. The requirements are in [`SRS.md`](SRS.md).

| Directory | Contents |
|---|---|
| [`server/`](server/README.md) | The server application (Rust) |
| `agent/` | The Host Agent (Rust) |
| [`protocol/`](protocol/src/lib.rs) | The messages between the server and its clients (Rust) |
| [`brand/`](brand/README.md) | The product name and logo, and the rules for using them |
| [`web/`](web/README.md) | The web client (Angular) |
| `xtask/` | Development tasks, run as `cargo xtask <command>` |

## Development

You need Docker with Buildx, Rust (stable) and Node.js 22. Docker Desktop and Docker Engine include Buildx; with Colima, install it with `brew install docker-buildx`.

### Running the server side

```sh
docker compose up --build
```

This starts PostgreSQL, the server and Mailpit (MNT-07). The server is on <https://localhost:8443>, with a self-signed certificate that the browser warns about; <http://localhost:8080> redirects there. The monitoring listener is on <http://localhost:9090/readyz>. The database keeps its data between runs; `docker compose down --volumes` deletes it.

Mailpit catches every email the server sends, and shows them on <http://localhost:8025>. Their links lead to the web client on <http://localhost:4200> (`http.public_url`), so start it too, to follow them. To see what emails look like, send the test email:

```sh
docker compose exec server /usr/local/bin/twoviewer-server send-test-email you@example.com
```

For a faster edit-and-run loop, start only PostgreSQL and Mailpit, and run the server with cargo, with the same development settings as [`compose.yaml`](compose.yaml):

```sh
docker compose up --detach postgres mailpit
DATABASE_URL=postgres://twoviewer:twoviewer@localhost:5432/twoviewer \
MFA_ENCRYPTION_KEY=Bp3rKIUg1DK7vLKDhGbAlHBIYkgzY+AayfT3/QgVTy0= \
TWOVIEWER_LOG__FORMAT=text \
TWOVIEWER_TLS__SELF_SIGNED=true \
TWOVIEWER_HTTP__PUBLIC_PORT=8443 \
TWOVIEWER_HTTP__HSTS_MAX_AGE=0s \
TWOVIEWER_HTTP__PUBLIC_URL=http://localhost:4200 \
TWOVIEWER_EMAIL__FROM_ADDRESS=no-reply@twoviewer.test \
TWOVIEWER_EMAIL__SMTP_HOST=localhost \
TWOVIEWER_EMAIL__SMTP_PORT=1025 \
TWOVIEWER_EMAIL__SMTP_SECURITY=none \
cargo run -p twoviewer-server
```

The web client runs separately with `npm start` in `web/`, on <http://localhost:4200>. It sends `/api` requests and the real-time connection to the server on port 8443 ([`web/README.md`](web/README.md#with-the-development-server)).

The Host Agent connects to the server named by `TWOVIEWER_SERVER` and trusts the certificates that the operating system trusts. So it doesn't trust the self-signed development certificate; use one that your system trusts, for example from [mkcert](https://github.com/FiloSottile/mkcert) (`tls.certificate_file` and `tls.private_key_file`):

```sh
TWOVIEWER_SERVER=https://localhost:8443 cargo run -p twoviewer-agent
```

### Checks

[CI](.github/workflows/ci.yml) runs these on every push and pull request (MNT-05), and nightly so that newly published vulnerabilities are found (SEC-15):

| Check | Command |
|---|---|
| Formatting | `cargo fmt --all --check` |
| Lints, including the SQL rules in [`clippy.toml`](clippy.toml) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Server tests, including all migrations on an empty database and the upgrade from the previous release | `cargo test` |
| The migrations match the code model | `cargo xtask check-migrations` |
| The icons match the logo ([`brand/README.md`](brand/README.md)) | `cargo xtask check-icons` |
| Web client build and tests | `npm run build` and `npm test -- --watch=false` in `web/` |
| Web client text is marked for translation ([`web/README.md`](web/README.md#text-and-translation)) | `npm run check:i18n` and `npx ng extract-i18n` in `web/` |
| Crates with known vulnerabilities ([`deny.toml`](deny.toml)) | `cargo deny check advisories`, after `cargo install --locked cargo-deny` |
| npm packages with known vulnerabilities | `npm audit` in `web/` |
| The local start works, the server image sends email, and it has no fixable vulnerabilities | `docker compose up`, `send-test-email`, then Trivy on the `twoviewer-server` image |

The server tests and the `cargo xtask` migration commands start PostgreSQL and Mailpit containers, so they need Docker. With Colima, see [`server/migrations/README.md`](server/migrations/README.md).

### Releases

A release is a `v<major>.<minor>.<patch>` tag on its commit. The upgrade test applies the migrations of the newest release before the current commit, then the current ones.
