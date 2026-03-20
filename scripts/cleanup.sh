#!/usr/bin/env bash
#
# Tear down everything created by setup.sh so you can test a full re-build.
#
# Usage: sudo ./scripts/cleanup.sh [--pool-name vume] [--rootfs rootfs.ext4] [--ssh-key ./vume_key]
#
set -euo pipefail

POOL_NAME="vume"
ROOTFS_PATH="./vume/rootfs.ext4"
SSH_KEY="./vume/vume_key"
MOUNT_DIR="./vume/rootfs"

# --- parse args ---

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pool-name) POOL_NAME="$2"; shift 2 ;;
        --rootfs)    ROOTFS_PATH="$2"; shift 2 ;;
        --ssh-key)   SSH_KEY="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# --- sanity checks ---

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root" >&2
    exit 1
fi

# --- destroy ZFS pool (takes snapshots and zvols with it) ---

if zpool list "$POOL_NAME" &>/dev/null; then
    echo ":: destroying ZFS pool '${POOL_NAME}'"
    zpool destroy -f "$POOL_NAME"
    # if this fails try running "zpool scrub vume" first (maybe the backing file was deleted)
else
    echo ":: ZFS pool '${POOL_NAME}' does not exist (skipping)"
fi

# --- remove pool backing file (dev mode) ---

POOL_FILE="./vume.zfs"
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

# --- remove rootfs mount dir (leftover from interrupted builds) ---

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

echo ""
echo ":: cleanup complete — run ./scripts/setup.sh to rebuild"
