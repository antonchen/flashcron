# Build stage
FROM rust:1.82-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

# Copy manifests
COPY Cargo.toml ./

# Create dummy src to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release || true
RUN rm -rf src target

# Copy actual source
COPY . .

# Build release binary
RUN cargo build --release

# Runtime stage
FROM alpine:3.19

RUN apk add --no-cache ca-certificates tzdata

# Create non-root user
RUN addgroup -S flashcron && adduser -S flashcron -G flashcron

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/flashcron /usr/local/bin/flashcron

# Set ownership
RUN chown -R flashcron:flashcron /app

USER flashcron

# Default config location
VOLUME ["/app/config"]

# Healthcheck
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD pgrep flashcron || exit 1

ENTRYPOINT ["flashcron"]
CMD ["run", "-c", "/app/config/flashcron.toml"]
