# ─── Stage 1: Build Rust binary ───────────────────────────────────────────────
FROM ubuntu:22.04 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

# Install Rust via rustup (non-interactive)
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --profile minimal
RUN rustup show

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/

RUN cargo build --release


# ─── Stage 2: Runtime image ───────────────────────────────────────────────────
FROM ubuntu:22.04 AS runtime

# System runtime libraries
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        wget \
        bzip2 \
        libgomp1 \
    && rm -rf /var/lib/apt/lists/*

# Install Miniforge (conda with bioconda-compatible solver)
ENV CONDA_DIR=/opt/conda
RUN wget -q "https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-Linux-x86_64.sh" \
        -O /tmp/miniforge.sh \
    && bash /tmp/miniforge.sh -b -p "$CONDA_DIR" \
    && rm /tmp/miniforge.sh
ENV PATH=$CONDA_DIR/bin:$PATH

# Install msisensor-pro from bioconda
RUN conda install -y -c bioconda -c conda-forge msisensor-pro \
    && conda clean -afy

# Copy the compiled msi-calc binary from builder stage
COPY --from=builder /build/target/release/msi-calc /usr/local/bin/msi-calc
RUN chmod +x /usr/local/bin/msi-calc

# ── Volume mount points (document expected mounts) ──
# -v /path/to/reference:/data/reference:ro    <- reference FASTA
# -v /path/to/bams:/data/bams:ro              <- BAM files directory
# -v /path/to/output:/data/output             <- output directory

WORKDIR /data
ENTRYPOINT ["msi-calc"]
CMD ["--help"]
