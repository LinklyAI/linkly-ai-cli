#!/bin/sh
# Linkly CLI installer for macOS and Linux
# Usage: curl -sSL https://updater.linkly.ai/cli/install.sh | sh
set -eu

LATEST_URL="https://updater.linkly.ai/cli/latest.json"
DEFAULT_INSTALL_DIR="$HOME/.linkly/bin"
INSTALL_DIR="${LINKLY_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
BINARY_NAME="linkly"

# ── Helpers ────────────────────────────────────────────

info() { printf "\033[34m[info]\033[0m %s\n" "$1"; }
success() { printf "\033[32m[ok]\033[0m %s\n" "$1"; }
error() { printf "\033[31m[error]\033[0m %s\n" "$1" >&2; exit 1; }

# ── Detect platform ──────────────────────────────────

detect_platform() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) os_key="darwin" ;;
        Linux)  os_key="linux" ;;
        *)      error "Unsupported OS: $os" ;;
    esac

    case "$arch" in
        arm64|aarch64) arch_key="aarch64" ;;
        x86_64|amd64)  arch_key="x86_64" ;;
        *)             error "Unsupported architecture: $arch" ;;
    esac

    PLATFORM_KEY="${os_key}-${arch_key}"
}

# ── HTTP fetch (curl preferred, wget fallback) ───────

fetch() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        error "Neither curl nor wget found. Please install one and retry."
    fi
}

fetch_stdout() {
    url="$1"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$url"
    else
        error "Neither curl nor wget found. Please install one and retry."
    fi
}

# ── Extract download URL from latest.json ────────────

get_download_url() {
    json="$1"
    key="$2"
    # Extract URL for the platform key without jq dependency
    url=$(printf '%s' "$json" | grep "\"${key}\"" | sed 's/.*"'"${key}"'"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
    if [ -z "$url" ]; then
        error "No download URL found for platform: $key"
    fi
    printf '%s' "$url"
}

# ── Add to PATH ──────────────────────────────────────

ensure_in_path() {
    dir="$1"
    # Already in PATH?
    case ":${PATH}:" in
        *":${dir}:"*) return 0 ;;
    esac

    export_line="export PATH=\"${dir}:\$PATH\""
    shell_name="$(basename "${SHELL:-/bin/sh}")"
    added=false

    for rc_file in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile"; do
        if [ -f "$rc_file" ]; then
            if ! grep -qF "$dir" "$rc_file" 2>/dev/null; then
                printf '\n# Linkly CLI\n%s\n' "$export_line" >> "$rc_file"
                info "Added to PATH in $(basename "$rc_file")"
                added=true
            fi
        fi
    done

    # If no rc file was modified, create one based on shell
    if [ "$added" = false ]; then
        case "$shell_name" in
            zsh)  rc_file="$HOME/.zshrc" ;;
            bash) rc_file="$HOME/.bashrc" ;;
            *)    rc_file="$HOME/.profile" ;;
        esac
        printf '\n# Linkly CLI\n%s\n' "$export_line" >> "$rc_file"
        info "Added to PATH in $(basename "$rc_file")"
    fi
}

# ── Main ─────────────────────────────────────────────

main() {
    info "Installing Linkly CLI..."

    detect_platform
    info "Platform: ${PLATFORM_KEY}"

    # Fetch latest.json
    info "Fetching latest version info..."
    latest_json="$(fetch_stdout "$LATEST_URL")"

    # Extract download URL
    download_url="$(get_download_url "$latest_json" "$PLATFORM_KEY")"
    info "Downloading from: ${download_url}"

    # Create temp directory
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$tmp_dir"' EXIT

    # Download archive
    archive_file="${tmp_dir}/linkly.tar.gz"
    fetch "$download_url" "$archive_file"

    # Extract
    tar xzf "$archive_file" -C "$tmp_dir"

    # Install binary
    mkdir -p "$INSTALL_DIR"
    mv "${tmp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    # Ensure PATH
    ensure_in_path "$INSTALL_DIR"

    success "Linkly CLI installed to ${INSTALL_DIR}/${BINARY_NAME}"
    printf "\n"

    # Check if already in PATH
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            info "Run 'linkly --help' to get started."
            ;;
        *)
            info "Restart your shell or run:"
            printf "  export PATH=\"%s:\$PATH\"\n" "$INSTALL_DIR"
            ;;
    esac
}

main
