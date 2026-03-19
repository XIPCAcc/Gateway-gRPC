# Multi-stage build for gateway-gRPC

# Builder stage
FROM rust:1.75-slim-bookworm AS builder

WORKDIR /app

# Install dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy source code
COPY . .

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    sysstat \
    linux-perf \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy binaries from builder
COPY --from=builder /app/target/release/backend /app/
COPY --from=builder /app/target/release/gateway /app/
COPY --from=builder /app/target/release/benchmark /app/
COPY --from=builder /app/target/release/latency_analyzer /app/

# Copy scripts
COPY --from=builder /app/scripts /app/scripts

# Make scripts executable
RUN chmod +x /app/scripts/*.sh

# Expose ports
EXPOSE 8080 50051

# Default command
CMD ["./backend", "--addr", "0.0.0.0:50051"]
