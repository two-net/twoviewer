# TwoViewer

Remote access and remote support, used through the browser. The requirements are in [`SRS.md`](SRS.md).

| Directory | Contents |
|---|---|
| [`server/`](server/README.md) | The server application (Rust) |
| `agent/` | The Host Agent (Rust) |
| [`web/`](web/README.md) | The web client (Angular) |
| `xtask/` | Development tasks, run as `cargo xtask <command>` |

## Development

You need Docker with Buildx, Rust (stable) and Node.js 22. Docker Desktop and Docker Engine include Buildx; with Colima, install it with `brew install docker-buildx`.

### Running the server side

```sh
docker compose up --build
```

This starts PostgreSQL and the server (MNT-07). The server is on <https://localhost:8443>, with a self-signed certificate that the browser warns about; <http://localhost:8080> redirects there. The monitoring listener is on <http://localhost:9090/readyz>. The database keeps its data between runs; `docker compose down --volumes` deletes it.

For a faster edit-and-run loop, start only PostgreSQL and run the server with cargo, with the same development settings as [`compose.yaml`](compose.yaml):

```sh
docker compose up --detach postgres
DATABASE_URL=postgres://twoviewer:twoviewer@localhost:5432/twoviewer \
TWOVIEWER_LOG__FORMAT=text \
TWOVIEWER_TLS__SELF_SIGNED=true \
TWOVIEWER_HTTP__PUBLIC_PORT=8443 \
TWOVIEWER_HTTP__HSTS_MAX_AGE=0s \
cargo run -p twoviewer-server
```

The web client runs separately with `npm start` in `web/`.

### Checks

[CI](.github/workflows/ci.yml) runs these on every push and pull request (MNT-05), and nightly so that newly published vulnerabilities are found (SEC-15):

| Check | Command |
|---|---|
| Formatting | `cargo fmt --all --check` |
| Lints, including the SQL rules in [`clippy.toml`](clippy.toml) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Server tests, including all migrations on an empty database and the upgrade from the previous release | `cargo test` |
| The migrations match the code model | `cargo xtask check-migrations` |
| Web client build and tests | `npm run build` and `npm test -- --watch=false` in `web/` |
| Crates with known vulnerabilities ([`deny.toml`](deny.toml)) | `cargo deny check advisories`, after `cargo install --locked cargo-deny` |
| npm packages with known vulnerabilities | `npm audit` in `web/` |
| The local start works, and the server image has no fixable vulnerabilities | `docker compose up`, then Trivy on the `twoviewer-server` image |

The server tests and `cargo xtask` start PostgreSQL containers, so they need Docker. With Colima, see [`server/migrations/README.md`](server/migrations/README.md).

### Releases

A release is a `v<major>.<minor>.<patch>` tag on its commit. The upgrade test applies the migrations of the newest release before the current commit, then the current ones.
