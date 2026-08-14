# Distroless, non-root, binary only (OPS-022).
# Contrast with the Java image, which ships a JVM, Spark, Maven and dsbulk.

# Must be at least the workspace `rust-version` (`NFR-005`, currently 1.88) or cargo refuses to
# build at all. This was pinned at 1.85 — below the MSRV — and nothing caught it: the only place
# this image is built is `release.yml`, which runs on tags, so the first release would have been
# the discovery. Raise this whenever `rust-version` in Cargo.toml rises.
FROM rust:1.97-slim AS builder
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --profile dist --bin cdm

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /src/target/dist/cdm /usr/local/bin/cdm
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cdm"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
