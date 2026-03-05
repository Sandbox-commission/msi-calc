#!/bin/bash
set -eo pipefail

# msi-calc Setup Script
# Three installation paths:
# 1. Docker containerization (automated)
# 2. Automatic mamba/conda install + environment setup
# 3. Manual installation with web links

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ─── Helper Functions ────────────────────────────────────────────────────────

log_header() {
    echo -e "\n${BLUE}=== $1 ===${NC}\n"
}

log_step() {
    echo -e "${YELLOW}[${1}]${NC} $2"
}

log_success() {
    echo -e "${GREEN}✓${NC} $1"
}

log_error() {
    echo -e "${RED}✗${NC} $1"
}

pause_and_ask() {
    read -p "$(echo -e ${YELLOW})Press Enter to continue...$(echo -e ${NC})" -r
}

# ─── SECTION 1: DOCKER SETUP ────────────────────────────────────────────────

setup_docker() {
    log_header "Docker Container Installation"

    # Auto-detect correct Docker socket
    if command -v docker &> /dev/null && ! docker info &> /dev/null; then
        if [[ -S /var/run/docker.sock ]]; then
            export DOCKER_HOST="unix:///var/run/docker.sock"
            log_step "FIX" "Switched to system Docker socket (/var/run/docker.sock)"
        fi
    fi

    if command -v docker &> /dev/null; then
        log_success "Docker found: $(docker --version)"
        docker_setup_existing
    else
        log_error "Docker is not installed."
        echo
        echo "  a) Auto-install Docker"
        echo "  b) Manual installation (with links)"
        echo "  c) Go back to main menu"
        echo
        read -p "Select option [a-c]: " -r DOCKER_CHOICE
        case "$DOCKER_CHOICE" in
            a) install_docker_auto ;;
            b) install_docker_manual ;;
            c) return ;;
            *) log_error "Invalid choice."; exit 1 ;;
        esac
    fi
}

install_docker_auto() {
    log_step "1/4" "Detecting Linux distribution..."

    if [[ -f /etc/os-release ]]; then
        . /etc/os-release
        OS=$ID
    else
        log_error "Could not detect OS"
        exit 1
    fi

    log_step "2/4" "Installing Docker prerequisites..."
    case "$OS" in
        ubuntu|debian)
            sudo apt-get update
            sudo apt-get install -y \
                apt-transport-https \
                ca-certificates \
                curl \
                gnupg \
                lsb-release
            ;;
        fedora|rhel|centos)
            sudo dnf install -y \
                curl \
                gnupg \
                lsb-release
            ;;
        *)
            log_error "Unsupported OS: $OS"
            echo "Please visit: https://docs.docker.com/engine/install/"
            exit 1
            ;;
    esac

    log_step "3/4" "Installing Docker..."
    case "$OS" in
        ubuntu|debian)
            curl -fsSL https://download.docker.com/linux/${OS}/gpg | sudo gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg
            echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/${OS} $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null
            sudo apt-get update
            sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
            ;;
        fedora|rhel|centos)
            sudo dnf config-manager --add-repo https://download.docker.com/linux/fedora/docker-ce.repo
            sudo dnf install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
            ;;
    esac

    log_step "4/4" "Starting Docker service..."
    sudo systemctl start docker
    sudo systemctl enable docker
    sudo usermod -aG docker "$USER" || true

    log_success "Docker installed!"
    echo
    echo -e "${YELLOW}Note:${NC} You may need to run the following to use Docker without sudo:"
    echo "  newgrp docker"
    echo
    docker_setup_existing
}

install_docker_manual() {
    log_header "Manual Docker Installation"
    echo
    echo "Visit the official Docker installation guide:"
    echo -e "  ${BLUE}https://docs.docker.com/engine/install/${NC}"
    echo
    echo "Installation steps:"
    echo "  1. Choose your operating system"
    echo "  2. Follow the installation instructions"
    echo "  3. Verify: docker --version"
    echo "  4. Re-run this setup script after installation"
    echo
    pause_and_ask
}

docker_setup_existing() {
    log_step "1/3" "Building Docker image..."
    cd "$SCRIPT_DIR"
    docker build -t msi-calc:latest .
    log_success "Docker image built"

    log_step "2/3" "Creating config directory..."
    mkdir -p ~/.config/msi-calc
    log_success "Config directory created"

    log_step "3/3" "Generating docker-run.sh..."
    cat > docker-run.sh << 'DOCKER_SCRIPT'
#!/bin/bash
# msi-calc Docker Runner

REF_DIR="${REF_DIR:-.}"
BAM_DIR="${BAM_DIR:-.}"
OUTPUT_DIR="${1:-./msi-results}"

if [[ ! -d "$REF_DIR" ]]; then
    echo "Error: REF_DIR not found: $REF_DIR"
    echo "  Set REF_DIR=/path/to/reference before running this script."
    exit 1
fi

if [[ ! -d "$BAM_DIR" ]]; then
    echo "Error: BAM_DIR not found: $BAM_DIR"
    echo "  Set BAM_DIR=/path/to/bams before running this script."
    exit 1
fi

mkdir -p "$OUTPUT_DIR"

echo "Running msi-calc in Docker..."
echo "  Reference: $REF_DIR"
echo "  BAMs:      $BAM_DIR"
echo "  Output:    $OUTPUT_DIR"
echo

docker run --rm -it \
    -v "$REF_DIR:/data/ref:ro" \
    -v "$BAM_DIR:/data/bams:ro" \
    -v "$OUTPUT_DIR:/data/output" \
    -v "$HOME/.config/msi-calc:$HOME/.config/msi-calc:ro" \
    msi-calc:latest \
    -r /data/ref/reference.fa \
    -p /data/bams/sample_pairs.csv \
    -o /data/output \
    "${@:2}"
DOCKER_SCRIPT
    chmod +x docker-run.sh
    log_success "Created docker-run.sh"

    echo
    echo -e "${GREEN}✓ Docker setup complete!${NC}"
    echo
    echo "Usage:"
    echo -e "  ${YELLOW}REF_DIR=/path/to/ref BAM_DIR=/path/to/bams ./docker-run.sh [output_dir]${NC}"
    echo
    echo "Edit docker-run.sh to set the correct reference FASTA filename."
    echo
}

# ─── SECTION 2: MAMBA AUTOMATIC SETUP ────────────────────────────────────────

setup_mamba_auto() {
    log_header "Automatic Mamba Installation"

    # Check if mamba/conda exists
    CONDA_ROOT=""
    for dir in ~/miniforge3 ~/mambaforge ~/miniconda3; do
        if [[ -d "$dir" ]]; then
            CONDA_ROOT="$dir"
            log_success "Found conda at: $CONDA_ROOT"
            setup_msisensor "$CONDA_ROOT"
            return
        fi
    done

    if [[ -d /opt/conda ]]; then
        CONDA_ROOT="/opt/conda"
        log_success "Found conda at: $CONDA_ROOT"
        setup_msisensor "$CONDA_ROOT"
        return
    fi

    # Conda not found — ask user
    log_error "Conda/Mamba not found"
    echo
    read -p "Auto-install Miniforge (mamba)? [Y/n] " -r INSTALL_MAMBA
    if [[ $INSTALL_MAMBA =~ ^[Yy]?$ ]]; then
        install_miniforge_auto
    else
        log_error "Mamba required for automatic setup. Please install manually."
        install_conda_manual
        exit 1
    fi
}

install_miniforge_auto() {
    log_step "1/3" "Downloading Miniforge..."
    MINIFORGE_URL="https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-Linux-x86_64.sh"
    MINIFORGE_INSTALLER="/tmp/Miniforge3-Linux-x86_64.sh"

    if ! curl -fsSL "$MINIFORGE_URL" -o "$MINIFORGE_INSTALLER"; then
        log_error "Failed to download Miniforge"
        exit 1
    fi
    log_success "Downloaded"

    log_step "2/3" "Installing Miniforge..."
    CONDA_ROOT="$HOME/miniforge3"
    bash "$MINIFORGE_INSTALLER" -b -p "$CONDA_ROOT"
    rm -f "$MINIFORGE_INSTALLER"
    log_success "Installed to $CONDA_ROOT"

    log_step "3/3" "Initializing conda..."
    "$CONDA_ROOT/bin/conda" init bash
    log_success "Conda initialized"

    echo
    echo -e "${YELLOW}Note:${NC} Run ${BLUE}source ~/.bashrc${NC} to activate conda"
    echo
    setup_msisensor "$CONDA_ROOT"
}

install_conda_manual() {
    log_header "Manual Conda Installation"
    echo
    echo "Choose your preferred conda distribution:"
    echo
    echo -e "  ${BLUE}Miniforge (Recommended)${NC}"
    echo "    https://github.com/conda-forge/miniforge"
    echo
    echo -e "  ${BLUE}Miniconda${NC}"
    echo "    https://docs.conda.io/en/latest/miniconda.html"
    echo
    echo "Installation steps:"
    echo "  1. Download the appropriate installer for your system"
    echo "  2. Run: bash ~/Downloads/Miniforge3-Linux-x86_64.sh"
    echo "  3. Follow the prompts"
    echo "  4. Run: source ~/.bashrc"
    echo "  5. Re-run this setup script"
    echo
    pause_and_ask
}

setup_msisensor() {
    local CONDA_ROOT="$1"
    local PKG_MGR="$CONDA_ROOT/bin/mamba"
    if [[ ! -x "$PKG_MGR" ]]; then
        PKG_MGR="$CONDA_ROOT/bin/conda"
    fi

    log_step "1/5" "Installing msisensor-pro..."
    if command -v msisensor-pro &>/dev/null; then
        log_success "msisensor-pro already available: $(command -v msisensor-pro)"
    else
        if ! "$PKG_MGR" install -y -n base -c bioconda -c conda-forge msisensor-pro; then
            log_error "Failed to install msisensor-pro"
            exit 1
        fi
        log_success "msisensor-pro installed"
    fi

    log_step "2/5" "Checking Rust..."
    if command -v cargo &>/dev/null; then
        log_success "Rust found: $(cargo --version)"
    else
        log_step "..." "Installing Rust via rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
        log_success "Rust installed: $(cargo --version)"
    fi

    log_step "3/5" "Building msi-calc..."
    cd "$SCRIPT_DIR"
    cargo build --release
    mkdir -p ~/.local/bin
    cp target/release/msi-calc ~/.local/bin/
    log_success "Binary installed to ~/.local/bin/msi-calc"

    log_step "4/5" "Prompting for reference path..."
    read -p "Path to reference genome FASTA (or Enter to skip): " -r REF_PATH
    REF_PATH="${REF_PATH/#\~/$HOME}"

    log_step "5/5" "Writing configuration..."
    mkdir -p ~/.config/msi-calc
    if [[ -n "$REF_PATH" ]]; then
        cat > ~/.config/msi-calc/config.json << EOF
{
  "reference": "$REF_PATH",
  "msisensor_env": "$CONDA_ROOT"
}
EOF
    else
        cat > ~/.config/msi-calc/config.json << EOF
{
  "msisensor_env": "$CONDA_ROOT"
}
EOF
    fi
    log_success "Configuration written to ~/.config/msi-calc/config.json"

    echo
    echo -e "${GREEN}✓ Mamba setup complete!${NC}"
    echo
    echo "Add to PATH:"
    echo -e "  ${YELLOW}export PATH=\"\$HOME/.local/bin:\$PATH\"${NC}"
    echo
    echo "Run msi-calc:"
    echo -e "  ${YELLOW}msi-calc -r /path/to/reference.fa -p sample_pairs.csv${NC}"
    echo
    echo "Reference:"
    echo "  msisensor-pro: Jia P et al. MSIsensor-pro: Fast, Accurate, and"
    echo "  Matched-normal-free Detection of Microsatellite Instability."
    echo "  Genomics Proteomics Bioinformatics (2020). PMID: 32171661"
    echo
}

# ─── SECTION 3: MANUAL SETUP ────────────────────────────────────────────────

setup_manual() {
    log_header "Manual Installation Guide"
    echo
    echo "Follow these steps to set up msi-calc manually:"
    echo
    echo -e "${BLUE}Step 1: Install msisensor-pro${NC}"
    echo "  Via conda/mamba:"
    echo "    conda install -c bioconda -c conda-forge msisensor-pro"
    echo "  Or build from source:"
    echo "    https://github.com/xjtu-omics/msisensor-pro"
    echo
    echo -e "${BLUE}Step 2: Install Rust${NC}"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo
    echo -e "${BLUE}Step 3: Build msi-calc${NC}"
    echo "  git clone https://github.com/Sandbox-commission/msi-calc.git"
    echo "  cd msi-calc"
    echo "  cargo build --release"
    echo "  cp target/release/msi-calc ~/.local/bin/"
    echo
    echo -e "${BLUE}Step 4: Create Config (optional)${NC}"
    echo "  mkdir -p ~/.config/msi-calc"
    echo "  Create ~/.config/msi-calc/config.json:"
    echo '  {'
    echo '    "reference": "/path/to/reference.fa",'
    echo '    "pairs": "/path/to/sample_pairs.csv"'
    echo '  }'
    echo
    echo -e "${BLUE}Step 5: Add to PATH${NC}"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo
    echo -e "${BLUE}Step 6: Run${NC}"
    echo "  msi-calc -r /path/to/reference.fa -p sample_pairs.csv"
    echo
    echo "Reference:"
    echo "  msisensor-pro: Jia P et al. MSIsensor-pro (2020). PMID: 32171661"
    echo
    pause_and_ask
}

# ─── Main Menu ──────────────────────────────────────────────────────────────

main() {
    while true; do
        log_header "msi-calc Setup"
        echo "Microsatellite Instability Analysis using msisensor-pro"
        echo
        echo "Choose your installation method:"
        echo
        echo "  1) Docker Container (Recommended) — everything pre-configured"
        echo "  2) Mamba/Conda (Automatic) — auto-install dependencies"
        echo "  3) Manual Installation — follow web links & instructions"
        echo "  4) Exit"
        echo
        read -p "Select option [1-4]: " -r CHOICE

        case "$CHOICE" in
            1) setup_docker; break ;;
            2) setup_mamba_auto; break ;;
            3) setup_manual; break ;;
            4) echo "Exiting."; exit 0 ;;
            *) log_error "Invalid choice. Please try again." ;;
        esac
    done
}

main
