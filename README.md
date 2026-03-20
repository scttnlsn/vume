# vume

Micro-VM pool management via [Firecracker](https://github.com/firecracker-microvm/firecracker) and [ZFS](https://github.com/openzfs/zfs)

## Installation

### Quick install

```bash
curl -s https://raw.githubusercontent.com/scttnlsn/vume/main/install.sh | bash
```

This will:
- Download the `vume` binary from the latest GitHub release
- Download the Firecracker binary and vmlinux kernel
- Build a Debian rootfs
- Create a ZFS pool
- Install everything under `/opt/vume` (default)
- Symlink `vume` to `/usr/local/bin/vume`

You'll be prompted for:
- The install path (default: `/opt/vume`)
- The ZFS pool device (leave empty for file-backed dev mode; a real device/partition is recommended for production)

If you install to a non-default path, set `VUME_HOME` in your shell profile:

```bash
export VUME_HOME=/your/path
```

### Development

If you have a local checkout of this repo:

```bash
./install.sh --instal-path ./vume
# or with a custom install path:
./install.sh --install-path ./vume
# or with a custom ZFS pool device:
./install.sh --pool-device /dev/sdb
```

This will build from source via `cargo build --release` instead of downloading the binary.

## System requirements

- Linux (kernel with KVM support)
- ZFS (for fast CoW storage and snapshots)
- iptables (for networking - bridge, NAT, tap devices)
- debootstrap (for building the rootfs)
- Rust toolchain (only for development installs)

```bash
sudo apt install iptables debootstrap zfs-dkms zfsutils-linux
sudo modprobe kvm kvm_amd zfs # or `kvm_intel`
```

## Configuration

All configuration is stored in `$VUME_HOME/vume.toml` (default: `/opt/vume/vume.toml`).

See `config/vume.toml` for default values.

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
