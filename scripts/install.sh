#!/usr/bin/env bash
# =============================================================
# Mumei Installer — curl -fsSL https://raw.githubusercontent.com/mumei-lang/mumei/develop/scripts/install.sh | bash
# =============================================================
# 自動検出: OS / Arch → GitHub Releases から最新バイナリをダウンロード
# インストール先: ~/.mumei/bin/mumei + ~/.mumei/std/

set -euo pipefail

REPO="mumei-lang/mumei"
INSTALL_DIR="${MUMEI_HOME:-$HOME/.mumei}"
BIN_DIR="$INSTALL_DIR/bin"
STD_DIR="$INSTALL_DIR/std"

# --- カラー出力 ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

info()  { echo -e "${CYAN}[mumei]${NC} $1"; }
ok()    { echo -e "${GREEN}[mumei]${NC} $1"; }
warn()  { echo -e "${YELLOW}[mumei]${NC} $1"; }
err()   { echo -e "${RED}[mumei]${NC} $1" >&2; exit 1; }

# --- OS / Arch 検出 ---
detect_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *)      err "Unsupported OS: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        arm64|aarch64)
            if [ "$os" = "unknown-linux-gnu" ]; then
                err "aarch64 Linux is not yet supported. Pre-built binaries are available for x86_64 Linux, x86_64/aarch64 macOS. See https://github.com/mumei-lang/mumei/releases"
            fi
            arch="aarch64" ;;
        *)              err "Unsupported architecture: $arch" ;;
    esac

    echo "mumei-${arch}-${os}"
}

# --- 最新バージョン取得 ---
get_latest_version() {
    local url="https://api.github.com/repos/${REPO}/releases/latest"
    if command -v curl &>/dev/null; then
        curl -fsSL "$url" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
    elif command -v wget &>/dev/null; then
        wget -qO- "$url" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
    else
        err "curl or wget is required"
    fi
}

# --- ダウンロード ---
download() {
    local url="$1" dest="$2"
    if command -v curl &>/dev/null; then
        curl -fsSL "$url" -o "$dest"
    elif command -v wget &>/dev/null; then
        wget -q "$url" -O "$dest"
    fi
}

# --- メイン ---
main() {
    info "Detecting platform..."
    local platform
    platform="$(detect_platform)"
    info "Platform: $platform"

    info "Fetching latest version..."
    local version
    version="$(get_latest_version)"
    if [ -z "$version" ]; then
        err "Failed to fetch latest version. Check https://github.com/${REPO}/releases"
    fi
    info "Version: $version"

    local url="https://github.com/${REPO}/releases/download/${version}/${platform}.tar.gz"
    local tmp_dir
    tmp_dir="$(mktemp -d)"
    local archive="$tmp_dir/${platform}.tar.gz"

    info "Downloading ${url}..."
    download "$url" "$archive" || err "Download failed. Check if release exists for $platform"

    info "Installing to $INSTALL_DIR..."
    mkdir -p "$BIN_DIR" "$STD_DIR"

    # 展開
    tar xzf "$archive" -C "$tmp_dir"

    # バイナリコピー
    cp "$tmp_dir/mumei" "$BIN_DIR/mumei"
    chmod +x "$BIN_DIR/mumei"

    # 標準ライブラリコピー
    if [ -d "$tmp_dir/std" ]; then
        cp -r "$tmp_dir/std/"* "$STD_DIR/"
    fi

    # クリーンアップ
    rm -rf "$tmp_dir"

    ok "Mumei $version installed successfully!"
    echo ""

    # --- PATH 設定 ---
    if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
        warn "Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
        echo ""
        echo "  export PATH=\"$BIN_DIR:\$PATH\""
        echo "  export MUMEI_STD_PATH=\"$STD_DIR\""
        echo ""

        # 自動追加の提案
        local shell_rc=""
        if [ -n "${ZSH_VERSION:-}" ] || [ "$(basename "${SHELL:-}")" = "zsh" ]; then
            shell_rc="$HOME/.zshrc"
        elif [ -n "${BASH_VERSION:-}" ] || [ "$(basename "${SHELL:-}")" = "bash" ]; then
            shell_rc="$HOME/.bashrc"
        fi

        if [ -n "$shell_rc" ] && [ -f "$shell_rc" ] && [ -t 1 ]; then
            echo -n "Add to $shell_rc automatically? [y/N] "
            read -r answer < /dev/tty || answer=""
            if [ "$answer" = "y" ] || [ "$answer" = "Y" ]; then
                echo "" >> "$shell_rc"
                echo "# Mumei" >> "$shell_rc"
                echo "export PATH=\"$BIN_DIR:\$PATH\"" >> "$shell_rc"
                echo "export MUMEI_STD_PATH=\"$STD_DIR\"" >> "$shell_rc"
                ok "Added to $shell_rc. Run 'source $shell_rc' to apply."
            fi
        fi
    fi

    ok "Run 'mumei --help' to get started."
}

main "$@"
