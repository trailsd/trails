# ── Build stage ──────────────────────────────────────────────
FROM rust:1.83-bookworm AS builder

WORKDIR /build
COPY Cargo.toml .
COPY src/ src/
COPY migrations/ migrations/

RUN cargo build --release

# ── Runtime stage ────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/trailsd /usr/local/bin/trailsd

EXPOSE 8443

ENTRYPOINT ["trailsd"]
