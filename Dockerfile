# ─── Stage 1: Build Rust binary ───────────────────────────────────────────────
FROM ubuntu:22.04 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --profile minimal

WORKDIR /build

# Cache dependencies (dummy src trick)
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/msi_calc* target/release/msi-calc*

# Build real binary
COPY src/ ./src/
RUN cargo build --release && strip target/release/msi-calc

# ─── Stage 2: Runtime with msisensor-pro ──────────────────────────────────────
FROM mambaorg/micromamba:latest AS runtime

LABEL maintainer="Sandbox-commission" \
      description="msi-calc: MSI analysis using msisensor-pro" \
      version="0.3.0"

USER root

# Install msisensor-pro into base environment
RUN micromamba install -y -n base -c bioconda -c conda-forge msisensor-pro \
    && micromamba clean -afy

# Copy the compiled msi-calc binary
COPY --from=builder /build/target/release/msi-calc /usr/local/bin/msi-calc
RUN chmod +x /usr/local/bin/msi-calc

# Entrypoint script to activate conda environment
COPY <<'ENTRYPOINT_SCRIPT' /usr/local/bin/entrypoint.sh
#!/bin/bash
eval "$(micromamba shell hook --shell bash)"
micromamba activate base
exec "$@"
ENTRYPOINT_SCRIPT
RUN chmod +x /usr/local/bin/entrypoint.sh

# Switch back to non-root user
USER $MAMBA_USER

WORKDIR /data
ENTRYPOINT ["/usr/local/bin/entrypoint.sh", "msi-calc"]
CMD ["--help"]
