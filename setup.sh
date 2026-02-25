#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MSISENSOR_REPO="https://github.com/xjtu-omics/msisensor-pro"
MSISENSOR_INSTALL_DIR="$HOME/.local/msisensor-pro"

info()    { echo "  [INFO]  $*"; }
success() { echo "  [OK]    $*"; }
warn()    { echo "  [WARN]  $*"; }
error()   { echo "  [ERROR] $*" >&2; exit 1; }

# ── OS Detection ──────────────────────────────────────────────────────────────
OS="$(uname -s)"
case "$OS" in
    Linux*)  PLATFORM=linux ;;
    Darwin*) PLATFORM=macos ;;
    *)       error "Unsupported OS: $OS. Only Linux and macOS are supported." ;;
esac
info "Detected platform: $PLATFORM"

# ── Check / install msisensor-pro ─────────────────────────────────────────────
install_msisensor_via_conda() {
    local pkg_mgr="$1"
    info "Installing msisensor-pro via $pkg_mgr (bioconda)..."
    "$pkg_mgr" install -y -c bioconda -c conda-forge msisensor-pro
    success "msisensor-pro installed via $pkg_mgr"
}

install_msisensor_from_source() {
    info "Building msisensor-pro from source..."

    # Check build dependencies
    for dep in cmake make git; do
        command -v "$dep" &>/dev/null || error \
            "'$dep' is required to build msisensor-pro. Install it with your system package manager."
    done

    local build_dir
    build_dir="$(mktemp -d)"
    git clone --depth 1 "$MSISENSOR_REPO" "$build_dir/msisensor-pro"
    cd "$build_dir/msisensor-pro"
    mkdir -p build && cd build
    cmake .. -DCMAKE_INSTALL_PREFIX="$MSISENSOR_INSTALL_DIR"
    make -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)"
    mkdir -p "$MSISENSOR_INSTALL_DIR/bin"
    cp ../msisensor-pro "$MSISENSOR_INSTALL_DIR/bin/"
    cd "$SCRIPT_DIR"
    rm -rf "$build_dir"

    # Add to PATH if not already there
    local bin_path="$MSISENSOR_INSTALL_DIR/bin"
    if [[ ":$PATH:" != *":$bin_path:"* ]]; then
        warn "Add the following to your shell profile to use msisensor-pro:"
        warn "  export PATH=\"$bin_path:\$PATH\""
        export PATH="$bin_path:$PATH"
    fi
    success "msisensor-pro built and installed to $bin_path"
}

install_miniforge() {
    info "Installing Miniforge..."
    local miniforge_url
    if [[ "$PLATFORM" == "linux" ]]; then
        miniforge_url="https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-Linux-x86_64.sh"
    else
        miniforge_url="https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-MacOSX-arm64.sh"
    fi
    local tmp_installer
    tmp_installer="$(mktemp)"
    curl -fsSL "$miniforge_url" -o "$tmp_installer"
    bash "$tmp_installer" -b -p "$HOME/miniforge3"
    rm -f "$tmp_installer"
    source "$HOME/miniforge3/etc/profile.d/conda.sh"
    success "Miniforge installed"
}

if command -v msisensor-pro &>/dev/null; then
    success "msisensor-pro already in PATH: $(command -v msisensor-pro)"
else
    info "msisensor-pro not found. Checking for conda/mamba..."

    CONDA_CMD=""
    if command -v mamba &>/dev/null; then
        CONDA_CMD="mamba"
    elif command -v conda &>/dev/null; then
        CONDA_CMD="conda"
    fi

    if [[ -n "$CONDA_CMD" ]]; then
        install_msisensor_via_conda "$CONDA_CMD"
    else
        warn "Neither conda nor mamba found."
        echo ""
        echo "  How would you like to install msisensor-pro?"
        echo "    [1] Install Miniforge (conda) and use bioconda package (recommended)"
        echo "    [2] Build msisensor-pro from source"
        echo "    [q] Skip (you will install msisensor-pro manually)"
        read -rp "  Your choice [1/2/q]: " choice
        case "$choice" in
            1)
                install_miniforge
                install_msisensor_via_conda "conda"
                ;;
            2)
                install_msisensor_from_source
                ;;
            q|Q)
                warn "Skipping msisensor-pro installation."
                ;;
            *)
                warn "Invalid choice. Skipping msisensor-pro installation."
                ;;
        esac
    fi
fi

# ── Check / install Rust ──────────────────────────────────────────────────────
if command -v cargo &>/dev/null; then
    success "Rust/cargo found: $(cargo --version)"
else
    info "Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    success "Rust installed: $(cargo --version)"
fi

# ── Build msi-calc ────────────────────────────────────────────────────────────
info "Building msi-calc (cargo build --release)..."
cd "$SCRIPT_DIR"
cargo build --release
success "msi-calc binary: $SCRIPT_DIR/target/release/msi-calc"

echo ""
echo "  ─────────────────────────────────────────────"
echo "  Setup complete."
echo ""
echo "  To use msi-calc, either:"
echo "    a) Add to PATH:  export PATH=\"$SCRIPT_DIR/target/release:\$PATH\""
echo "    b) Copy binary:  cp $SCRIPT_DIR/target/release/msi-calc /usr/local/bin/"
echo ""
echo "  Quick start:"
echo "    msi-calc -r /path/to/reference.fa -p sample_pairs.csv"
echo "  ─────────────────────────────────────────────"
