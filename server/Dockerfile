# syntax=docker/dockerfile:1
# The server image. Build it from the repository root, which holds the Cargo workspace:
#   docker build -f server/Dockerfile .
# `docker compose up --build` does this for local development (MNT-07).

FROM rust:1.96-slim-trixie AS build
WORKDIR /src
COPY . .
# The cache mounts keep crates and build output between builds, so rebuilds are incremental.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p twoviewer-server \
    && cp target/release/twoviewer-server /usr/local/bin/

# Only glibc, libgcc and CA certificates: few third-party components to keep patched (SEC-15).
FROM gcr.io/distroless/cc-debian13:nonroot
# Migrations are embedded in the binary.
COPY --from=build /usr/local/bin/twoviewer-server /usr/local/bin/
# HTTPS, plain HTTP (redirects to HTTPS), monitoring. Publish 8443 as 443 and 8080 as 80.
EXPOSE 8443 8080 9090
ENTRYPOINT ["/usr/local/bin/twoviewer-server"]
