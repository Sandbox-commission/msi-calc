# Installation Guide

msi-calc provides three installation methods via the interactive `setup.sh` script.

## Quick Start

```bash
git clone https://github.com/Sandbox-commission/msi-calc.git
cd msi-calc
chmod +x setup.sh
./setup.sh
```

---

## Method 1: Docker (Recommended)

Best for: reproducible environments, HPC clusters, CI/CD pipelines.

### What setup.sh does:
1. Checks for Docker installation (auto-installs if missing)
2. Builds the `msi-calc:latest` image (includes msisensor-pro)
3. Creates `docker-run.sh` convenience script

### Manual Docker build:
```bash
docker build -t msi-calc .
```

### Running with Docker:
```bash
docker run --rm -it \
  -v /path/to/reference:/data/ref:ro \
  -v /path/to/bams:/data/bams:ro \
  -v /path/to/output:/data/output \
  msi-calc \
    -r /data/ref/GRCh38.fa \
    -p /data/bams/sample_pairs.csv \
    -o /data/output
```

---

## Method 2: Mamba/Conda (Automatic)

Best for: existing bioinformatics environments, shared servers.

### What setup.sh does:
1. Detects existing conda/mamba installation (or auto-installs Miniforge)
2. Installs msisensor-pro from bioconda
3. Installs Rust (if needed)
4. Builds msi-calc binary
5. Writes `~/.config/msi-calc/config.json`

### Post-setup:
```bash
export PATH="$HOME/.local/bin:$PATH"
msi-calc -r /path/to/reference.fa -p sample_pairs.csv
```

---

## Method 3: Manual

Best for: custom setups, environments without internet access.

### Prerequisites:

1. **msisensor-pro** (via conda or from source):
   ```bash
   # Option A: conda
   conda install -c bioconda -c conda-forge msisensor-pro

   # Option B: from source
   git clone https://github.com/xjtu-omics/msisensor-pro
   cd msisensor-pro && mkdir build && cd build
   cmake .. && make -j4
   sudo cp ../msisensor-pro /usr/local/bin/
   ```

2. **Rust** (>= 1.70):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. **Build msi-calc**:
   ```bash
   cargo build --release
   cp target/release/msi-calc ~/.local/bin/
   ```

4. **Config file** (optional):
   ```bash
   mkdir -p ~/.config/msi-calc
   cat > ~/.config/msi-calc/config.json << 'EOF'
   {
     "reference": "/path/to/reference.fa",
     "pairs": "/path/to/sample_pairs.csv"
   }
   EOF
   ```

---

## Configuration

msi-calc resolves settings in this order (highest priority first):

1. **CLI flags** (`-r`, `-p`, `-o`, `-j`, `-t`)
2. **Config file** (`~/.config/msi-calc/config.json`)
3. **Defaults** (auto-detected CPU count for jobs, hardcoded for rest)

### Config file format:
```json
{
  "reference": "/path/to/reference.fa",
  "pairs": "/path/to/sample_pairs.csv",
  "msisensor_env": "/path/to/conda/env"
}
```

All fields are optional. CLI flags always take precedence.

---

## Verifying Installation

```bash
# Check version
msi-calc -V

# View help
msi-calc -h

# Check msisensor-pro
msisensor-pro --help
```
