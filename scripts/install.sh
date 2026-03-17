#!/usr/bin/env bash
# =============================================================
# Mumei Installer
# =============================================================
# Usage:
#   curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash
#   curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.2.0
#   curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --uninstall
#
# Environment variables:
#   MUMEI_HOME  — installation directory (default: ~/.mumei)
# =============================================================

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

# --- ヘルプ ---
usage() {
    cat <<EOF
Mumei Installer

Usage:
  install.sh [OPTIONS]

Options:
  --version <VERSION>   Install a specific version (e.g. v0.2.0)
  --uninstall           Show uninstall instructions
  --help                Show this help message

Examples:
  curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash
  curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.2.0
EOF
    exit 0
}

# --- アンインストール手順 ---
show_uninstall() {
    cat <<EOF

To uninstall Mumei, remove the following:

  1. Remove the installation directory:
       rm -rf ${INSTALL_DIR}

  2. Remove PATH and MUMEI_STD_PATH entries from your shell profile
     (~/.bashrc, ~/.zshrc, etc.):
       export PATH="${BIN_DIR}:\$PATH"          # delete this line
       export MUMEI_STD_PATH="${STD_DIR}"       # delete this line

EOF
    exit 0
}

# --- 引数パース ---
REQUESTED_VERSION=""
parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                shift
                if [ $# -eq 0 ]; then
                    err "--version requires an argument (e.g. --version v0.2.0)"
                fi
                REQUESTED_VERSION="$1"
                ;;
            --uninstall)
                show_uninstall
                ;;
            --help|-h)
                usage
                ;;
            *)
                err "Unknown option: $1 (use --help for usage)"
                ;;
        esac
        shift
    done
}

# --- OS / Arch 検出 ---
detect_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    # WSL 検出: WSL 環境では Linux バイナリを使用
    if [ "$os" = "Linux" ]; then
        if grep -qiE '(microsoft|wsl)' /proc/version 2>/dev/null; then
            info "WSL detected — using Linux binary"
        fi
    fi

    case "$os" in
        Linux)
            # Plan 7: Prefer musl (statically linked) binary on Linux x86_64
            if [ "$arch" = "x86_64" ] || [ "$arch" = "amd64" ]; then
                os="unknown-linux-musl"
            else
                os="unknown-linux-gnu"
            fi
            ;;
        Darwin) os="apple-darwin" ;;
        *)      err "Unsupported OS: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        arm64|aarch64)
            if [ "$os" = "unknown-linux-gnu" ]; then
                # aarch64 Linux uses cross-compiled binary
                os="unknown-linux-gnu"
            fi
            arch="aarch64" ;;
        *)              err "Unsupported architecture: $arch" ;;
    esac

    echo "mumei-${arch}-${os}"
}

# --- バージョン取得 ---
get_version() {
    if [ -n "$REQUESTED_VERSION" ]; then
        # --version で指定された場合はそのまま返す
        # v プレフィックスがなければ付与
        case "$REQUESTED_VERSION" in
            v*) echo "$REQUESTED_VERSION" ;;
            *)  echo "v${REQUESTED_VERSION}" ;;
        esac
        return
    fi

    # 最新バージョンを取得
    local url="https://api.github.com/repos/${REPO}/releases/latest"
    local tag
    if command -v curl &>/dev/null; then
        tag="$(curl -fsSL "$url" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
    elif command -v wget &>/dev/null; then
        tag="$(wget -qO- "$url" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
    else
        err "curl or wget is required"
    fi

    if [ -z "$tag" ]; then
        err "Failed to fetch latest version. Check https://github.com/${REPO}/releases"
    fi
    echo "$tag"
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
    parse_args "$@"

    info "Detecting platform..."
    local platform
    platform="$(detect_platform)"
    info "Platform: $platform"

    info "Fetching version..."
    local version
    version="$(get_version)"
    info "Version: $version"

    local url="https://github.com/${REPO}/releases/download/${version}/${platform}.tar.gz"
    local tmp_dir
    tmp_dir="$(mktemp -d)"
    local archive="$tmp_dir/${platform}.tar.gz"

    info "Downloading ${url}..."
    download "$url" "$archive" || err "Download failed. Check if release exists for $platform at version $version"

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

    # --- スモークテスト ---
    if [ -x "$BIN_DIR/mumei" ]; then
        info "Running smoke test..."
        if "$BIN_DIR/mumei" --version >/dev/null 2>&1; then
            ok "Smoke test passed: $("$BIN_DIR/mumei" --version 2>&1 || true)"
        else
            warn "Smoke test: 'mumei --version' did not succeed. The binary may require additional dependencies (Z3, LLVM)."
            warn "Run 'mumei setup' to install required toolchains."
        fi
    fi

    ok "Run 'mumei --help' to get started."
}

main "$@"
