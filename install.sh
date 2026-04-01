#!/usr/bin/env bash
set -euo pipefail

REPO="Corust-ai/corust-cli"
INSTALL_DIR="${CORUST_INSTALL_DIR:-$HOME/.corust/bin}"

# Detect platform
detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *)      echo "Error: unsupported OS: $os" >&2; exit 1 ;;
  esac

  case "$arch" in
    x86_64|amd64)  arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)             echo "Error: unsupported architecture: $arch" >&2; exit 1 ;;
  esac

  echo "${arch}-${os}"
}

# Fetch the latest release tag from GitHub
latest_version() {
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
}

main() {
  local version="${1:-}"
  if [ -z "$version" ]; then
    echo "Fetching latest release..."
    version="$(latest_version)"
  fi

  local target
  target="$(detect_target)"

  local archive="corust-${version}-${target}.tar.gz"
  local url="https://github.com/${REPO}/releases/download/${version}/${archive}"

  echo "Downloading ${archive}..."
  local tmp
  tmp="$(mktemp -d)"
  curl -fsSL "$url" -o "${tmp}/${archive}"

  echo "Extracting to ${INSTALL_DIR}..."
  mkdir -p "$INSTALL_DIR"
  tar xzf "${tmp}/${archive}" -C "$tmp"
  cp "${tmp}/corust-${version}-${target}/corust-cli" "$INSTALL_DIR/"
  cp "${tmp}/corust-${version}-${target}/corust-agent-acp" "$INSTALL_DIR/"
  rm -rf "$tmp"

  chmod +x "${INSTALL_DIR}/corust-cli" "${INSTALL_DIR}/corust-agent-acp"

  echo ""
  echo "Installed corust-cli and corust-agent-acp to ${INSTALL_DIR}"

  # Check if INSTALL_DIR is in PATH
  if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    echo ""
    echo "Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
    echo ""
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
  fi

  echo ""
  echo "Run 'corust-cli --help' to get started."
}

main "$@"
