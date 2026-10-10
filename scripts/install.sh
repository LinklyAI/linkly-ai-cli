#!/bin/sh
# Linkly CLI installer for macOS and Linux
# Usage: curl -sSL https://updater.linkly.ai/cli/install.sh | sh
#
# How it finds the archive, without parsing JSON in shell:
#   1. latest.json  -> only the top-level "version" string (must occur once)
#   2. v<version>/SHA256SUMS -> the line for this platform's archive gives
#      both the file name and its checksum (plain text, one file per line)
#   3. download v<version>/<file> and refuse to install on checksum mismatch
# Everything under v<version>/ is immutable and uploaded before latest.json,
# so the manifest never points at files that are not there yet.
#
# Environment:
#   LINKLY_INSTALL_DIR    install directory (default: ~/.linkly/bin)
#   LINKLY_DOWNLOAD_BASE  release server root (default: https://updater.linkly.ai/cli);
#                         used by scripts/test-install.sh with a file:// fixture
set -eu

DOWNLOAD_BASE="${LINKLY_DOWNLOAD_BASE:-https://updater.linkly.ai/cli}"
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

    case "$arch" in
        arm64|aarch64) arch_key="aarch64" ;;
        x86_64|amd64)  arch_key="x86_64" ;;
        *)             error "Unsupported architecture: $arch" ;;
    esac

    # Rust target triple; release archives are named linkly-<triple>.tar.gz
    case "$os" in
        Darwin) TARGET="${arch_key}-apple-darwin" ;;
        Linux)  TARGET="${arch_key}-unknown-linux-gnu" ;;
        *)      error "Unsupported OS: $os" ;;
    esac
}

# ── HTTP fetch (curl preferred, wget fallback) ───────

fetch() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest" || error "Download failed: $url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url" || error "Download failed: $url"
    else
        error "Neither curl nor wget found. Please install one and retry."
    fi
}

# ── Read the version from latest.json ────────────────

# Joins the manifest into one line so formatting (pretty, minified, CRLF) does
# not matter, then takes the "version" string. More than one "version" key
# means the manifest shape changed: fail loudly rather than guess.
get_version() {
    manifest="$1"
    flat="$(tr -d '\r\n' < "$manifest")"
    count="$(printf '%s' "$flat" | grep -o '"version"[[:space:]]*:' | wc -l | tr -d ' ')"
    if [ "$count" != "1" ]; then
        error "Unexpected latest.json format (found $count \"version\" keys); please report this at https://github.com/LinklyAI/linkly-ai-cli/issues"
    fi
    version="$(printf '%s' "$flat" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    # Release versions are semver, optionally with a pre-release suffix.
    if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
        error "Invalid version in latest.json: '$version'"
    fi
    printf '%s' "$version"
}

# ── Look up the archive's checksum in SHA256SUMS ─────

# SHA256SUMS lines are "<64 hex>  <file name>" (sha256sum output).
get_checksum() {
    sums_file="$1"
    file_name="$2"
    sum="$(awk -v f="$file_name" '$2 == f || $2 == "*" f { print $1 }' "$sums_file")"
    if ! printf '%s' "$sum" | grep -Eq '^[0-9a-f]{64}$'; then
        error "No valid checksum for $file_name in SHA256SUMS (platform not published?)"
    fi
    printf '%s' "$sum"
}

# ── Hash a file (sha256sum, shasum or openssl) ───────

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{ print $NF }'
    else
        error "No SHA-256 tool found (sha256sum, shasum or openssl); cannot verify the download."
    fi
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
    info "Platform: ${TARGET}"

    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$tmp_dir"' EXIT

    info "Fetching latest version info..."
    fetch "${DOWNLOAD_BASE}/latest.json" "${tmp_dir}/latest.json"
    version="$(get_version "${tmp_dir}/latest.json")"
    info "Latest version: ${version}"

    release_base="${DOWNLOAD_BASE}/v${version}"
    archive_name="linkly-${TARGET}.tar.gz"
    fetch "${release_base}/SHA256SUMS" "${tmp_dir}/SHA256SUMS"
    expected_sum="$(get_checksum "${tmp_dir}/SHA256SUMS" "$archive_name")"

    info "Downloading from: ${release_base}/${archive_name}"
    archive_file="${tmp_dir}/${archive_name}"
    fetch "${release_base}/${archive_name}" "$archive_file"

    actual_sum="$(sha256_of "$archive_file")"
    if [ "$actual_sum" != "$expected_sum" ]; then
        error "Checksum mismatch for ${archive_name} (expected ${expected_sum}, got ${actual_sum}); nothing was installed."
    fi
    info "Checksum verified"

    tar xzf "$archive_file" -C "$tmp_dir"
    [ -f "${tmp_dir}/${BINARY_NAME}" ] || error "Archive does not contain '${BINARY_NAME}'"

    # Install binary
    mkdir -p "$INSTALL_DIR"
    mv "${tmp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    # Ensure PATH
    ensure_in_path "$INSTALL_DIR"

    success "Linkly CLI ${version} installed to ${INSTALL_DIR}/${BINARY_NAME}"
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
