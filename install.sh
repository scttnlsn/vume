#!/usr/bin/env bash
#
# Vume bootstrap installer.
#
# Usage: ./install.sh [--install-path <path>] [--pool-device <device>]
#
set -euo pipefail

REPO="scttnlsn/vume"
RELEASE_URL="https://github.com/${REPO}/releases/latest"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/main"

INSTALL_PATH=""
POOL_DEVICE=""
ARCH=""
DEV_MODE=false
SCRIPT_DIR=""

is_local_run() {
    [[ -f "${BASH_SOURCE[0]}" ]]
}

detect_dev_mode() {
    if ! is_local_run; then
        DEV_MODE=false
        return
    fi

    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    if [[ ! -f "${SCRIPT_DIR}/vume.sh" ]]; then
        DEV_MODE=false
        return
    fi

    DEV_MODE=true
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64) ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) echo "error: unsupported architecture: $arch" >&2; exit 1 ;;
    esac
}

prompt_install_path() {
    echo -n "Installation path [/opt/vume]: "
    read -r path < /dev/tty
    INSTALL_PATH="${path:-/opt/vume}"
}

prompt_pool_device() {
    local default_pool="$VUME_HOME/vume.zfs"
    echo -n "ZFS pool device (leave empty for file-backed dev mode) [${default_pool}]: "
    read -r device < /dev/tty
    if [[ -n "$device" ]]; then
        POOL_DEVICE="$device"
    else
        POOL_DEVICE=""
    fi
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --install-path) INSTALL_PATH="$2"; shift 2 ;;
            --pool-device) POOL_DEVICE="$2"; shift 2 ;;
            *) echo "Unknown option: $1" >&2; exit 1 ;;
        esac
    done

    if [[ -z "$INSTALL_PATH" ]]; then
        prompt_install_path
    fi
}

create_install_dir() {
    local dir="$1"
    if [[ ! -d "$dir" ]]; then
        echo ":: creating $dir"
        sudo mkdir -p "$dir"
    fi
}

check_tool() {
    local cmd="$1"
    local msg="$2"
    if ! command -v "$cmd" &>/dev/null; then
        echo "error: $cmd not found ($msg)" >&2
        exit 1
    fi
}

install_vume_binary() {
    if [[ -f "${INSTALL_PATH}/vume" ]]; then
        echo ":: vume binary already exists: ${INSTALL_PATH}/vume (skipping)"
        return
    fi

    if [[ "$DEV_MODE" == "true" ]]; then
        check_tool cargo "install Rust via https://rustup.rs"

        if [[ ! -f "${SCRIPT_DIR}/Makefile" ]]; then
            echo "error: Makefile not found in ${SCRIPT_DIR}" >&2
            exit 1
        fi

        echo ":: building vume binary (dev mode)"
        make -C "${SCRIPT_DIR}" release
        echo ":: installing to ${INSTALL_PATH}"
        sudo cp "${SCRIPT_DIR}/target/release/vume" "${INSTALL_PATH}/vume"
    else
        echo ":: downloading vume binary (${ARCH})"
        local url="${RELEASE_URL}/download/vume-${ARCH}"
        curl -fsSL "$url" -o /tmp/vume
        chmod +x /tmp/vume
        echo ":: installing to ${INSTALL_PATH}"
        sudo cp /tmp/vume "${INSTALL_PATH}/vume"
    fi
}

install_vume_toml() {
    if [[ -f "${INSTALL_PATH}/vume.toml" ]]; then
        echo ":: vume.toml already exists: ${INSTALL_PATH}/vume.toml (skipping)"
        return
    fi

    if [[ "$DEV_MODE" == "true" ]]; then
        echo ":: installing vume.toml from repo"
        sudo cp "${SCRIPT_DIR}/config/vume.toml" "${INSTALL_PATH}/vume.toml"
    else
        echo ":: downloading vume.toml"
        local url="${RAW_BASE}/config/vume.toml"
        curl -fsSL "$url" -o /tmp/vume.toml
        echo ":: installing to ${INSTALL_PATH}"
        sudo cp /tmp/vume.toml "${INSTALL_PATH}/vume.toml"
    fi
}

prepare_vume_sh() {
    if [[ "$DEV_MODE" == "true" ]]; then
        echo ":: using local vume.sh"
        VUME_SH="${SCRIPT_DIR}/vume.sh"
    else
        echo ":: downloading vume.sh"
        local url="${RAW_BASE}/vume.sh"
        curl -fsSL "$url" -o /tmp/vume.sh
        chmod +x /tmp/vume.sh
        VUME_SH="/tmp/vume.sh"
    fi
}

symlink_binary() {
    local src="$1"
    local link="/usr/local/bin/vume"
    if [[ -f "$link" ]] || [[ -L "$link" ]]; then
        echo ":: binary/symlink already exists: $link (skipping)"
    else
        echo ":: symlinking vume to $link"
        sudo ln -sf "$src" "$link"
    fi
}

main() {
    detect_dev_mode

    echo ":: vume installer"
    if [[ "$DEV_MODE" == "true" ]]; then
        echo ":: mode: development (local)"
    else
        echo ":: mode: production (downloading)"
    fi
    echo ""

    detect_arch
    parse_args "$@"

    export VUME_HOME="$INSTALL_PATH"

    if [[ -z "$POOL_DEVICE" ]]; then
        prompt_pool_device
    fi

    echo ":: install path: ${INSTALL_PATH}"
    echo ":: architecture: ${ARCH}"
    echo ""

    create_install_dir "$INSTALL_PATH"

    install_vume_binary
    echo ""

    install_vume_toml
    echo ""

    prepare_vume_sh
    echo ""

    echo ":: running vume.sh download"
    sudo -v
    sudo VUME_HOME="$INSTALL_PATH" "$VUME_SH" download --out-dir "$INSTALL_PATH"
    echo ""

    echo ":: running vume.sh rootfs"
    sudo VUME_HOME="$INSTALL_PATH" "$VUME_SH" rootfs
    echo ""

    echo ":: running vume.sh setup"
    if [[ -n "$POOL_DEVICE" ]]; then
        sudo VUME_HOME="$INSTALL_PATH" "$VUME_SH" setup --pool-device "$POOL_DEVICE"
    else
        sudo VUME_HOME="$INSTALL_PATH" "$VUME_SH" setup
    fi
    echo ""

    symlink_binary "${INSTALL_PATH}/vume"
    echo ""

    echo ":: installation complete"
    echo "   path: ${INSTALL_PATH}"
    if [[ "$INSTALL_PATH" != "/opt/vume" ]]; then
        echo ""
        echo ":: NOTE: non-standard install path — add to your shell profile:"
        echo "   export VUME_HOME=${INSTALL_PATH}"
    fi
    echo ""
    echo "   run:  vume start"
}

main "$@"
