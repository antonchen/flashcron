# Build stage
FROM rust:1.93-trixie AS chef
RUN cargo install cargo-chef
WORKDIR /work

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /work/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:stable-slim
ENV DEBIAN_FRONTEND=noninteractive
ENV MALLOC_ARENA_MAX=2
ENV LANG=en_US.UTF-8
ENV LC_ALL=en_US.UTF-8

RUN printf 'APT::Install-Recommends "false";\nAPT::Install-Suggests "false";\n' > /etc/apt/apt.conf.d/90disable-suggests && \
    printf 'Acquire::http::Pipeline-Depth "0";\n' > /etc/apt/apt.conf.d/99nopipelining && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
    locales tzdata procps iputils-ping ca-certificates curl wget jq && \
    # Set locale
    echo "en_US.UTF-8 UTF-8" > /etc/locale.gen && \
    locale-gen && \
    # Cleanup
    apt-get clean && \
    rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/* /usr/share/doc /usr/share/man && \
    mkdir -p /app/config

WORKDIR /app

# Copy binary from builder
COPY --from=builder /work/target/release/flashcron /app/flashcron

# Healthcheck
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/app/flashcron"]
CMD ["run", "-c", "/app/config/flashcron.toml"]
