#!/usr/bin/env bash
#
# Set up a vume host: build rootfs, create ZFS pool/zvol, generate SSH keys.
#
# Usage: sudo ./scripts/setup.sh [--pool-disk <device>]
#
# Without --pool-disk, creates a file-backed ZFS pool (for development).
# With --pool-disk, uses a real block device (e.g. /dev/nvme1n1).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POOL_DISK=""
POOL_NAME="vume"
VUME_DIR="./vume"
POOL_FILE="${VUME_DIR}/vume.zfs" # for dev mode
ROOTFS_PATH="${VUME_DIR}/rootfs.ext4"
ROOTFS_SIZE="2G"
ZVOL_SIZE="2G"
SSH_KEY="${VUME_DIR}/vume_key"

# --- parse args ---

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pool-disk)  POOL_DISK="$2"; shift 2 ;;
        --rootfs)     ROOTFS_PATH="$2"; shift 2 ;;
        --size)       ROOTFS_SIZE="$2"; shift 2 ;;
        --zvol-size)  ZVOL_SIZE="$2"; shift 2 ;;
        --ssh-key)    SSH_KEY="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# --- sanity checks ---

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root" >&2
    exit 1
fi

for cmd in zpool zfs dd ssh-keygen; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "error: $cmd not found (install zfsutils-linux)" >&2
        exit 1
    fi
done

# --- build rootfs ---

mkdir -p "${VUME_DIR}"

if [[ -f "$ROOTFS_PATH" ]]; then
    echo ":: rootfs already exists: ${ROOTFS_PATH} (skipping build)"
else
    echo ":: building rootfs"
    "${SCRIPT_DIR}/rootfs.sh" --output "$ROOTFS_PATH" --size "$ROOTFS_SIZE"
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
MOUNT_DIR=$(mktemp -d)
cleanup_mount() {
    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        umount -l "$MOUNT_DIR"
    fi
    [[ -d "$MOUNT_DIR" ]] && rmdir "$MOUNT_DIR" 2>/dev/null || true
}
trap cleanup_mount EXIT
mount -o loop "$ROOTFS_PATH" "$MOUNT_DIR"
mkdir -p "${MOUNT_DIR}/root/.ssh"
chmod 700 "${MOUNT_DIR}/root/.ssh"
cat "${SSH_KEY}.pub" > "${MOUNT_DIR}/root/.ssh/authorized_keys"
chmod 600 "${MOUNT_DIR}/root/.ssh/authorized_keys"
umount -l "$MOUNT_DIR"
rmdir "$MOUNT_DIR"
trap - EXIT

# --- create ZFS pool ---

if zpool list "$POOL_NAME" &>/dev/null; then
    echo ":: ZFS pool '${POOL_NAME}' already exists (skipping)"
else
    if [[ -n "$POOL_DISK" ]]; then
        echo ":: creating ZFS pool '${POOL_NAME}' on ${POOL_DISK}"
        zpool create "$POOL_NAME" "$POOL_DISK"
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
    zfs set vume:latest=base "${POOL_NAME}/rootfs"
fi

echo ""
echo ":: setup complete"
echo "   pool:     ${POOL_NAME}"
echo "   snapshot: ${POOL_NAME}/rootfs@base"
echo "   ssh key:  ${SSH_KEY}"
echo ""
echo "   You can now start VMs with: vume start"
