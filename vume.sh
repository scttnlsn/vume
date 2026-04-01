#!/usr/bin/env bash
#
# Vume management script: setup, build rootfs, download binaries, cleanup.
#
# Usage: ./vume.sh <command>
#
# Commands:
#   setup     Set up vume host: build rootfs, create ZFS pool/zvol, generate SSH keys
#   rootfs    Build Debian rootfs ext4 image
#   download  Download Firecracker binary and vmlinux kernel image
#   cleanup   Tear down everything created by setup
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# --- config ---

POOL_DEVICE=""
ROOTFS_PATH=""
ROOTFS_SIZE="10G"
ZVOL_SIZE="10G"
SSH_KEY=""
POOL_NAME=""

# --- helpers ---

get_config() {
    local key="$1"
    local default="$2"
    if command -v vume &>/dev/null; then
        vume config get "$key" 2>/dev/null || echo "$default"
    else
        echo "$default"
    fi
}

check_help() {
    local arg="${1:-}"
    local usage="${2:-}"
    [[ "$arg" == "--help" ]] || [[ "$arg" == "-h" ]] && { echo "Usage: ./vume.sh $usage"; exit 0; }
    return 0
}

need_root() {
    [[ $EUID -ne 0 ]] && echo "error: must run as root" >&2 && exit 1
    sleep 0.1  # delay for sudo credential timestamp file to be ready
}

# --- resolve paths from config ---

resolve_paths() {
    export VUME_HOME="${VUME_HOME:-$(get_config home /opt/vume)}"
    export POOL_NAME="${POOL_NAME:-$(get_config zfs_pool vume)}"
    export SSH_KEY="${SSH_KEY:-$(get_config ssh_key "${VUME_HOME}/vume_key")}"
    export ROOTFS_PATH="${ROOTFS_PATH:-${VUME_HOME}/rootfs.ext4}"
    export POOL_FILE="${VUME_HOME}/vume.zfs"
    export CONFIG_FILE="${VUME_HOME}/vume.toml"
    export MOUNT_DIR="${VUME_HOME}/rootfs"
}

# --- command: help ---

cmd_help() {
    sed -n '2,10p' "${BASH_SOURCE[0]}"
    echo ""
    echo "Run './vume.sh <command> --help' for command-specific options."
}

# --- command: setup ---

cmd_setup() {
    for cmd in zpool zfs dd ssh-keygen; do
        if ! command -v "$cmd" &>/dev/null; then
            echo "error: $cmd not found (install zfsutils-linux)" >&2
            exit 1
        fi
    done

    mkdir -p "${VUME_HOME}"

    # --- create config file ---

    if [[ ! -f "$CONFIG_FILE" ]]; then
        echo ":: creating config file: ${CONFIG_FILE}"
        if [[ -f "${SCRIPT_DIR}/config/vume.toml" ]]; then
            cp "${SCRIPT_DIR}/config/vume.toml" "$CONFIG_FILE"
        fi
    fi

    # --- build rootfs ---

    if [[ -f "$ROOTFS_PATH" ]]; then
        echo ":: rootfs already exists: ${ROOTFS_PATH} (skipping build)"
    else
        echo ":: building rootfs"
        cmd_rootfs
    fi

    # --- generate SSH key ---

    if [[ -f "$SSH_KEY" ]]; then
        echo ":: SSH key already exists: ${SSH_KEY} (skipping)"
    else
        echo ":: generating SSH key: ${SSH_KEY}"
        ssh-keygen -t ed25519 -f "$SSH_KEY" -N "" -C "vume"
    fi

    # --- install SSH key into rootfs ---

    echo ":: installing SSH public key into rootfs"
    local mount_tmp
    mount_tmp=$(mktemp -d)
    cleanup_mount() {
        if mountpoint -q "$mount_tmp" 2>/dev/null; then
            umount -l "$mount_tmp"
        fi
        [[ -d "$mount_tmp" ]] && rmdir "$mount_tmp" 2>/dev/null || true
    }
    trap cleanup_mount EXIT
    mount -o loop "$ROOTFS_PATH" "$mount_tmp"
    mkdir -p "${mount_tmp}/root/.ssh"
    chmod 700 "${mount_tmp}/root/.ssh"
    cat "${SSH_KEY}.pub" > "${mount_tmp}/root/.ssh/authorized_keys"
    chmod 600 "${mount_tmp}/root/.ssh/authorized_keys"
    umount -l "$mount_tmp"
    rmdir "$mount_tmp"
    trap - EXIT

    # --- create ZFS pool ---

    if zpool list "$POOL_NAME" &>/dev/null; then
        echo ":: ZFS pool '${POOL_NAME}' already exists (skipping)"
    else
        if [[ -n "$POOL_DEVICE" ]]; then
            echo ":: creating ZFS pool '${POOL_NAME}' on ${POOL_DEVICE}"
            zpool create "$POOL_NAME" "$POOL_DEVICE"
        else
            echo ":: creating file-backed ZFS pool '${POOL_NAME}' (development mode)"
            if [[ ! -f "$POOL_FILE" ]]; then
                truncate -s 4G "$POOL_FILE"
            fi
            zpool create "$POOL_NAME" "$(realpath "$POOL_FILE")"
        fi
    fi

    # --- create zvol and import rootfs ---

    if zfs list "${POOL_NAME}/rootfs" &>/dev/null; then
        echo ":: zvol '${POOL_NAME}/rootfs' already exists (skipping)"
    else
        echo ":: creating zvol '${POOL_NAME}/rootfs' (${ZVOL_SIZE})"
        zfs create -s -V "$ZVOL_SIZE" -o compression=lz4 "${POOL_NAME}/rootfs"

        echo ":: copying rootfs into zvol"
        dd if="$ROOTFS_PATH" of="/dev/zvol/${POOL_NAME}/rootfs" bs=4M conv=sparse status=progress
    fi

    # --- snapshot ---

    if zfs list -t snapshot "${POOL_NAME}/rootfs@base" &>/dev/null; then
        echo ":: snapshot '${POOL_NAME}/rootfs@base' already exists (skipping)"
    else
        echo ":: creating snapshot '${POOL_NAME}/rootfs@base'"
        zfs snapshot "${POOL_NAME}/rootfs@base"
        zfs set vume:base=${POOL_NAME}/rootfs@base "${POOL_NAME}"
    fi

    echo ""
    echo ":: setup complete"
    echo "   pool:     ${POOL_NAME}"
    echo "   snapshot: ${POOL_NAME}/rootfs@base"
    echo "   ssh key:  ${SSH_KEY}"
    echo "   config:   ${CONFIG_FILE}"
    echo ""

    # --- symlink binary ---

    if [[ -f "${VUME_HOME}/vume" ]] && [[ ! -L /usr/local/bin/vume ]]; then
        echo ":: symlinking vume to /usr/local/bin/vume"
        ln -sf "${VUME_HOME}/vume" /usr/local/bin/vume
    fi

    echo "   You can now start VMs with: vume start"
}

# --- command: rootfs ---

cmd_rootfs() {
    for cmd in truncate mkfs.ext4 mount umount debootstrap chroot; do
        if ! command -v "$cmd" &>/dev/null; then
            echo "error: $cmd not found" >&2
            exit 1
        fi
    done

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --size)   ROOTFS_SIZE="$2"; shift 2 ;;
            *) echo "Unknown option: $1" >&2; exit 1 ;;
        esac
    done

    # --- cleanup on exit ---

    cleanup() {
        if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
            echo ":: unmounting $MOUNT_DIR"
            umount -l "$MOUNT_DIR"
        fi
    }
    trap cleanup EXIT

    # --- idempotency: skip if rootfs already exists ---

    if [[ -f "$ROOTFS_PATH" ]]; then
        echo ":: rootfs already exists: ${ROOTFS_PATH} (skipping)"
        exit 0
    fi

    # --- ensure mount dir is clean ---

    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        echo ":: ${MOUNT_DIR} is already mounted (cleaning up prior run)"
        umount -l "$MOUNT_DIR"
    fi

    # --- build ---

    echo ":: creating ${ROOTFS_PATH} (${ROOTFS_SIZE})"
    truncate -s "$ROOTFS_SIZE" "$ROOTFS_PATH"
    mkfs.ext4 -q "$ROOTFS_PATH"

    echo ":: mounting ${ROOTFS_PATH} at ${MOUNT_DIR}"
    mkdir -p "$MOUNT_DIR"
    mount -o loop "$ROOTFS_PATH" "$MOUNT_DIR"

    echo ":: running debootstrap (bookworm)"
    debootstrap bookworm "$MOUNT_DIR" http://deb.debian.org/debian > /dev/null 2>&1

    echo ":: configuring system"
    echo "vume" > "${MOUNT_DIR}/etc/hostname"

    cat > "${MOUNT_DIR}/etc/network/interfaces" <<EOF
auto lo
iface lo inet loopback
EOF

    echo "nameserver 8.8.8.8" > "${MOUNT_DIR}/etc/resolv.conf"

    echo ":: installing packages"
    chroot "$MOUNT_DIR" /bin/bash -c "
        apt-get update > /dev/null 2>&1
        apt-get install -y openssh-server > /dev/null 2>&1
        ssh-keygen -A
        systemctl enable ssh
    "

    echo ":: configuring sshd"
    sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin prohibit-password/' \
        "${MOUNT_DIR}/etc/ssh/sshd_config"

    mkdir -p "${MOUNT_DIR}/root/.ssh"
    chmod 700 "${MOUNT_DIR}/root/.ssh"
    touch "${MOUNT_DIR}/root/.ssh/authorized_keys"
    chmod 600 "${MOUNT_DIR}/root/.ssh/authorized_keys"

    echo ":: unmounting"
    umount -l "$MOUNT_DIR"

    echo ":: done: ${ROOTFS_PATH}"
}

# --- command: download ---

cmd_download() {
    local out_dir="$VUME_HOME"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --out-dir) out_dir="$2"; shift 2 ;;
            *) echo "Unknown option: $1" >&2; exit 1 ;;
        esac
    done

    local arch
    arch="$(uname -m)"
    local release_url="https://github.com/firecracker-microvm/firecracker/releases"
    local latest
    latest=$(basename $(curl --max-time 10 -fsSLI -o /dev/null -w %{url_effective} ${release_url}/latest))
    local ci_version="${latest%.*}"

    mkdir -p "$out_dir"

    # from https://github.com/firecracker-microvm/firecracker/blob/main/docs/getting-started.md

    echo ":: downloading firecracker binary (${latest}, ${arch})"
    rm -f /tmp/firecracker.tgz
    if ! curl -sL --max-time 30 "${release_url}/download/${latest}/firecracker-${latest}-${arch}.tgz" \
        -o /tmp/firecracker.tgz; then
        echo "error: failed to download firecracker binary" >&2
        exit 1
    fi
    tar -xzf /tmp/firecracker.tgz -C "$out_dir"
    mv "${out_dir}/release-${latest}-${arch}/firecracker-${latest}-${arch}" "${out_dir}/firecracker"
    rm -rf "${out_dir}/release-${latest}-${arch}" /tmp/firecracker.tgz

    echo ":: finding latest kernel for ${ci_version}/${arch}"
    local latest_kernel_key
    latest_kernel_key=$(curl -s "http://spec.ccfc.min.s3.amazonaws.com/?prefix=firecracker-ci/$ci_version/$arch/vmlinux-&list-type=2" \
        | grep -oP "(?<=<Key>)(firecracker-ci/$ci_version/$arch/vmlinux-[0-9]+\.[0-9]+\.[0-9]{1,3})(?=</Key>)" \
        | sort -V | tail -1)

    echo ":: downloading kernel: ${latest_kernel_key}"
    if ! wget --timeout=30 "https://s3.amazonaws.com/spec.ccfc.min/${latest_kernel_key}" -O "${out_dir}/vmlinux" 2>/dev/null; then
        echo "error: failed to download kernel" >&2
        exit 1
    fi

    echo ":: done"
    echo "   firecracker: ${out_dir}/firecracker"
    echo "   vmlinux:    ${out_dir}/vmlinux"
}

# --- command: cleanup ---

cmd_cleanup() {
    # --- destroy ZFS pool ---

    if zpool list "$POOL_NAME" &>/dev/null; then
        echo ":: destroying ZFS pool '${POOL_NAME}'"
        zpool destroy -f "$POOL_NAME"
    else
        echo ":: ZFS pool '${POOL_NAME}' does not exist (skipping)"
    fi

    # --- remove pool backing file ---

    if [[ -f "$POOL_FILE" ]]; then
        echo ":: removing pool backing file: ${POOL_FILE}"
        rm -f "$POOL_FILE"
    else
        echo ":: no pool backing file found (skipping)"
    fi

    # --- remove rootfs image ---

    if [[ -f "$ROOTFS_PATH" ]]; then
        echo ":: removing rootfs image: ${ROOTFS_PATH}"
        rm -f "$ROOTFS_PATH"
    else
        echo ":: rootfs image not found (skipping)"
    fi

    # --- remove rootfs mount dir ---

    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        echo ":: unmounting leftover ${MOUNT_DIR}"
        umount -l "$MOUNT_DIR"
    fi
    if [[ -d "$MOUNT_DIR" ]]; then
        echo ":: removing ${MOUNT_DIR}"
        rmdir "$MOUNT_DIR" 2>/dev/null || rm -rf "$MOUNT_DIR"
    fi

    # --- remove SSH key pair ---

    if [[ -f "$SSH_KEY" ]] || [[ -f "${SSH_KEY}.pub" ]]; then
        echo ":: removing SSH key: ${SSH_KEY}, ${SSH_KEY}.pub"
        rm -f "$SSH_KEY" "${SSH_KEY}.pub"
    else
        echo ":: no SSH key found (skipping)"
    fi

    # --- remove config file ---

    if [[ -f "$CONFIG_FILE" ]]; then
        echo ":: removing config file: ${CONFIG_FILE}"
        rm -f "$CONFIG_FILE"
    else
        echo ":: no config file found (skipping)"
    fi

    # --- remove symlink ---

    if [[ -L /usr/local/bin/vume ]]; then
        echo ":: removing symlink: /usr/local/bin/vume"
        rm -f /usr/local/bin/vume
    fi

    echo ""
    echo ":: cleanup complete — run ./vume.sh setup to rebuild"
}

# --- main ---

COMMAND="${1:-}"
[[ -z "$COMMAND" ]] && cmd_help && exit 0

shift
case "$COMMAND" in
    setup)
        check_help "${1:-}" "setup [--pool-device <device>] [--zvol-size <size>]
Options:
  --pool-device  Device for ZFS pool (default: file-backed)
  --zvol-size    Size of rootfs zvol (default: 2G)"
        need_root
        resolve_paths
        while [[ $# -gt 0 ]]; do
            case "$1" in
                --pool-device) POOL_DEVICE="$2"; shift 2 ;;
                --zvol-size) ZVOL_SIZE="$2"; shift 2 ;;
                *) echo "Unknown option: $1" >&2 && exit 1 ;;
            esac
        done
        cmd_setup
        ;;
    rootfs)
        check_help "${1:-}" "rootfs [--size <size>]"
        need_root
        resolve_paths
        cmd_rootfs "$@" ;;
    download)
        check_help "${1:-}" "download [--out-dir <dir>]"
        need_root
        resolve_paths
        cmd_download "$@" ;;
    cleanup)
        check_help "${1:-}" "cleanup"
        need_root
        resolve_paths
        cmd_cleanup ;;
    help|--help|-h) cmd_help ;;
    *) echo "Unknown command: $COMMAND" >&2; echo ""; cmd_help; exit 1 ;;
esac
