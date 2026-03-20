# vume

Micro-VM pool management via [Firecracker](https://github.com/firecracker-microvm/firecracker) and [ZFS](https://github.com/openzfs/zfs)

## System requirements

* Linux (kernel with KVM support)
* ZFS (for fast CoW storage and snapshots)
* iptables (for networking - bridge, NAT, tap devices)
* debootstrap (for building the rootfs)
* Firecracker binary
* `vmlinux` Linux kernel image

```bash
sudo apt install iptables debootstrap zfs-dkms zfsutils-linux
sudo modprobe kvm kvm_amd zfs # or `kvm_intel`
```

## Setup

Run the setup scripts to build the rootfs, create a ZFS pool, and generate SSH keys:

```bash
# downloads vmlinux image and Firecracker binary
./scripts/download.sh

# Development (file-backed ZFS pool)
sudo ./scripts/setup.sh

# Production (real block device)
sudo ./scripts/setup.sh --pool-disk /dev/nvme1n1
```

This will:
1. Build a Debian rootfs via debootstrap (`scripts/rootfs.sh`)
2. Generate an ed25519 SSH keypair (`vume_key` / `vume_key.pub`)
3. Install the public key into the rootfs
4. Create a ZFS pool and zvol
5. Copy the rootfs into the zvol and snapshot it as `vume/rootfs@base`

The script is idempotent — it skips steps that have already been completed.

To rebuild just the rootfs independently:

```bash
sudo ./scripts/rootfs.sh [--output rootfs.ext4] [--size 2G] [--arch amd64]
```

## Customizing the rootfs

You can create a custom base image from a running VM. The active rootfs version is tracked
via the `vume:latest` ZFS user property on `vume/rootfs`, so you can create new versions without
affecting existing VMs.

```bash
# 1. Customize a running VM, e.g.
vume exec <vm-id> "apt install -y python3 nginx"

# 2. Snapshot the customized VM and create a new rootfs version
sudo zfs snapshot vume/<vm-id>@<version>
sudo zfs send vume/<vm-id>@<version> | sudo zfs receive vume/rootfs@<version>

# 3. Point to the new version
sudo zfs set vume:latest=<version> vume/rootfs
```

All future VMs will clone from the new version. Existing VMs are unaffected.
