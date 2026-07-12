#!/usr/bin/env bash
# One-line bootstrap installer for Token 戰情室 (Linux / macOS).
#
# Downloads the correct prebuilt release archive from GitHub Releases (no
# Rust/Cargo toolchain required), extracts it, and runs the packaged
# install.sh. Safe to re-run to upgrade to a newer release.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash
#   curl -fsSL .../get.sh | bash -s -- --service
#
# Environment:
#   TOKEN_USAGE_INSIGHTS_VERSION       Release tag to install. Default: latest
#   TOKEN_USAGE_INSIGHTS_INSTALL_DIR   Forwarded to install.sh
#   TOKEN_USAGE_INSIGHTS_BIN_DIR       Forwarded to install.sh
set -euo pipefail

repo="doggy8088/TokenUsageInsights"
app_name="token-usage-insights"
version="${TOKEN_USAGE_INSIGHTS_VERSION:-latest}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    if [[ "$arch" != "x86_64" && "$arch" != "amd64" ]]; then
      echo "Unsupported Linux architecture: ${arch} (only x86_64 builds are published)" >&2
      exit 1
    fi
    target="x86_64-unknown-linux-gnu"
    archive_ext="tar.gz"
    ;;
  Darwin)
    case "$arch" in
      arm64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *)
        echo "Unsupported macOS architecture: ${arch}" >&2
        exit 1
        ;;
    esac
    archive_ext="tar.gz"
    ;;
  *)
    echo "Unsupported OS: ${os}. Use scripts/get.ps1 on Windows instead." >&2
    exit 1
    ;;
esac

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required but was not found." >&2
  exit 1
fi

if [[ "$version" == "latest" ]]; then
  echo "Resolving latest release tag for ${repo} ..."
  tag="$(curl -fsSL "https://api.github.com/repos/${repo}/releases/latest" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
  if [[ -z "$tag" ]]; then
    echo "Failed to resolve the latest release tag." >&2
    exit 1
  fi
else
  tag="$version"
fi

archive="${app_name}-${tag}-${target}.${archive_ext}"
url="https://github.com/${repo}/releases/download/${tag}/${archive}"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

echo "Downloading ${url} ..."
if ! curl -fsSL "$url" -o "${workdir}/${archive}"; then
  echo "Download failed. Check that ${tag} publishes a ${target} archive:" >&2
  echo "  https://github.com/${repo}/releases/tag/${tag}" >&2
  exit 1
fi

echo "Extracting ..."
tar -xzf "${workdir}/${archive}" -C "$workdir"

extracted_dir="${workdir}/${app_name}-${tag}-${target}"
if [[ ! -d "$extracted_dir" ]]; then
  extracted_dir="$(find "$workdir" -mindepth 1 -maxdepth 1 -type d | head -n1)"
fi

if [[ ! -x "${extracted_dir}/install.sh" ]]; then
  chmod +x "${extracted_dir}/install.sh" 2>/dev/null || true
fi

echo "Installing ${tag} ..."
"${extracted_dir}/install.sh" "$@"
