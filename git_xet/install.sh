#!/bin/sh
set -eu

RELEASE="${GIT_XET_RELEASE:-git-xet-v0.2.2-dev.2}"
INSTALL_DIR="${GIT_XET_INSTALL_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}"
case "$INSTALL_DIR" in /*) ;; *) INSTALL_DIR="$PWD/$INSTALL_DIR" ;; esac
case "$(uname -s)" in
    Linux) OS=linux ;;
    Darwin) OS=macos ;;
    *) echo "Use the Windows wheel or release binary." >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) echo "Unsupported architecture." >&2; exit 1 ;;
esac

ARCHIVE="git-xet-$OS-$ARCH.tar.gz"
BASE_URL="https://github.com/haraschax/xet-core/releases/download/$RELEASE"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT HUP INT TERM
curl -fSL "$BASE_URL/$ARCHIVE" -o "$TMP_DIR/$ARCHIVE"
curl -fsSL "$BASE_URL/SHA256SUMS" -o "$TMP_DIR/SHA256SUMS"
cd "$TMP_DIR"
awk -v archive="$ARCHIVE" '$2 == archive { print; found=1 } END { if (!found) exit 1 }' SHA256SUMS > check.sha256
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c check.sha256
else
    shasum -a 256 -c check.sha256
fi
tar -xzf "$ARCHIVE"
mkdir -p "$INSTALL_DIR"
install -m 755 git-xet "$INSTALL_DIR/git-xet"
"$INSTALL_DIR/git-xet" --version
echo "Installed to $INSTALL_DIR. Add it to PATH, then configure your repository:"
echo 'git xet install --local --lfs-url https://huggingface.co/OWNER/REPO.git/info/lfs'
