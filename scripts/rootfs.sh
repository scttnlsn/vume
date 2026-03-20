#!/usr/bin/env bash
#
# Build a Debian rootfs ext4 image for Firecracker VMs.
#
# Usage: sudo ./scripts/rootfs.sh [--output rootfs.ext4] [--size 2G] [--arch amd64]
#
set -euo pipefail

ROOTFS_PATH="./vume/rootfs.ext4"
ROOTFS_SIZE="2G"
ARCH="amd64"
MOUNT_DIR="./vume/rootfs"

# --- parse args ---

while [[ $# -gt 0 ]]; do
    case "$1" in
        --output) ROOTFS_PATH="$2"; shift 2 ;;
        --size)   ROOTFS_SIZE="$2"; shift 2 ;;
        --arch)   ARCH="$2";        shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# --- sanity checks ---

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root (need mount, chroot, debootstrap)" >&2
    exit 1
fi

for cmd in truncate mkfs.ext4 mount umount debootstrap chroot; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "error: $cmd not found" >&2
        exit 1
    fi
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

echo ":: running debootstrap (${ARCH}, bookworm)"
debootstrap --arch "$ARCH" bookworm "$MOUNT_DIR" http://deb.debian.org/debian

echo ":: configuring system"
echo "vume" > "${MOUNT_DIR}/etc/hostname"

cat > "${MOUNT_DIR}/etc/network/interfaces" <<EOF
auto lo
iface lo inet loopback
EOF

echo "nameserver 8.8.8.8" > "${MOUNT_DIR}/etc/resolv.conf"

echo ":: installing packages"
chroot "$MOUNT_DIR" /bin/bash -c "
    apt-get update
    apt-get install -y openssh-server
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
