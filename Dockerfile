# Build stage
FROM rust:1.93-trixie AS chef
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
RUN mkdir -p /usr/local/cargo && \
    printf '[source.crates-io]\nreplace-with = "ustc"\n\n[source.ustc]\nregistry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"\n' > /usr/local/cargo/config.toml
RUN cargo install cargo-chef
WORKDIR /work

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /work/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --features dep-manager

COPY . .
RUN cargo build --release --features dep-manager

# Runtime stage
FROM debian:stable-slim
ENV DEBIAN_FRONTEND=noninteractive
ENV MALLOC_ARENA_MAX=2
ENV LANG=en_US.UTF-8
ENV LC_ALL=en_US.UTF-8

RUN sed -i 's|deb.debian.org/debian|mirrors.ustc.edu.cn/debian|g; s|security.debian.org/debian-security|mirrors.ustc.edu.cn/debian-security|g' /etc/apt/sources.list.d/debian.sources && \
    printf 'APT::Install-Recommends "false";\nAPT::Install-Suggests "false";\n' > /etc/apt/apt.conf.d/90disable-suggests && \
    printf 'Acquire::http::Pipeline-Depth "0";\n' > /etc/apt/apt.conf.d/99nopipelining && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
    locales tzdata procps iputils-ping ca-certificates curl wget jq python3 python3-venv python3-pip && \
    # Set locale
    echo "en_US.UTF-8 UTF-8" > /etc/locale.gen && \
    locale-gen && \
    # Cleanup
    apt-get clean && \
    rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/* /usr/share/doc /usr/share/man && \
    mkdir -p /data /opt/flashcron-venv && \
    python3 -m venv /opt/flashcron-venv

WORKDIR /tmp
ENV VIRTUAL_ENV=/opt/flashcron-venv
ENV PATH="/opt/flashcron-venv/bin:$PATH"

# Copy binary from builder
COPY --from=builder /work/target/release/flashcron /usr/local/bin/flashcron
RUN chmod +x /usr/local/bin/flashcron

COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Healthcheck
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["-c", "/data/config.toml"]
