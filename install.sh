#!/bin/sh
# Corust CLI installer
#
# Usage:
#   curl --proto '=https' --tlsv1.2 -sSf https://corust.ai/install.sh | sh
#
# Environment variables:
#   CORUST_VERSION   pin to a specific tag (e.g. v0.4.2). Default: latest
#   INSTALL_DIR      override install prefix. Default: $HOME/.local/bin

set -eu

REPO="Corust-ai/homebrew-cli"
BIN_NAME="corust"

# ── output helpers ───────────────────────────────────────────────────────────

info()  { printf '\033[1;34m%s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || error "need '$1' (command not found)"
}

# ── platform detection ───────────────────────────────────────────────────────

detect_platform() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux*)  os="linux"  ;;
        Darwin*) os="darwin" ;;
        *)       error "unsupported OS: $os (this installer supports Linux and macOS)" ;;
    esac

    case "$arch" in
        x86_64|amd64)  arch="x64"   ;;
        aarch64|arm64) arch="arm64" ;;
        *)             error "unsupported architecture: $arch" ;;
    esac

    # Linux arm64 builds are not published yet.
    if [ "$os" = "linux" ] && [ "$arch" = "arm64" ]; then
        error "linux arm64 is not yet supported — see https://github.com/${REPO}/releases"
    fi

    echo "${os}-${arch}"
}

# ── build download URL ───────────────────────────────────────────────────────
#
# GitHub serves two canonical redirect endpoints that DO NOT consume API quota:
#
#   releases/latest/download/<asset>       → latest release
#   releases/download/<tag>/<asset>        → specific tag
#
# We use these directly so the installer isn't subject to GitHub's 60 req/hour
# unauthenticated API rate limit.

download_url() {
    asset="$1"
    if [ -n "${CORUST_VERSION:-}" ]; then
        echo "https://github.com/${REPO}/releases/download/${CORUST_VERSION}/${asset}"
    else
        echo "https://github.com/${REPO}/releases/latest/download/${asset}"
    fi
}

# ── install directory ────────────────────────────────────────────────────────

resolve_install_dir() {
    if [ -n "${INSTALL_DIR:-}" ]; then
        echo "$INSTALL_DIR"
        return
    fi
    if mkdir -p "$HOME/.local/bin" 2>/dev/null; then
        echo "$HOME/.local/bin"
    else
        echo "/usr/local/bin"
    fi
}

warn_if_not_in_path() {
    dir="$1"
    case ":$PATH:" in
        *":${dir}:"*) ;;
        *)
            echo
            echo "  ${dir} is not in your \$PATH."
            echo "  Add this line to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
            echo
            echo "    export PATH=\"${dir}:\$PATH\""
            echo
            ;;
    esac
}

# ── main ─────────────────────────────────────────────────────────────────────

main() {
    need_cmd uname
    need_cmd curl
    need_cmd tar
    need_cmd install

    platform="$(detect_platform)"
    asset="cli-${platform}.tar.gz"
    url="$(download_url "$asset")"
    install_dir="$(resolve_install_dir)"
    version_label="${CORUST_VERSION:-latest}"

    info "Installing Corust CLI (${version_label}, ${platform})"
    echo "  from: ${url}"
    echo "  to:   ${install_dir}/${BIN_NAME}"
    echo

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    curl --proto '=https' --tlsv1.2 -fSL --progress-bar "$url" -o "${tmp}/${asset}" \
        || error "download failed — '${version_label}' may not exist for platform '${platform}'. See https://github.com/${REPO}/releases"

    tar -xzf "${tmp}/${asset}" -C "$tmp"

    if [ ! -f "${tmp}/${BIN_NAME}" ]; then
        error "archive did not contain expected binary '${BIN_NAME}'"
    fi

    install -m 755 "${tmp}/${BIN_NAME}" "${install_dir}/${BIN_NAME}"

    echo
    info "Installed: ${install_dir}/${BIN_NAME}"
    "${install_dir}/${BIN_NAME}" --version 2>/dev/null || true

    warn_if_not_in_path "$install_dir"
}

main "$@"
