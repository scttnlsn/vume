# vume

Instant microVM provisioning via [Firecracker](https://github.com/firecracker-microvm/firecracker) and [ZFS](https://github.com/openzfs/zfs). Spin up lightweight, isolated VMs with copy-on-write storage and a simple CLI for execution and process management.


- [Installation](#installation)
  - [Quick install](#quick-install)
  - [Development](#development)
- [System requirements](#system-requirements)
- [Why ZFS?](#why-zfs)
- [Configuration](#configuration)
- [CLI Usage](#cli-usage)
  - [Start a VM](#start-a-vm)
  - [List VMs](#list-vms)
  - [Execute a command in a VM](#execute-a-command-in-a-vm)
  - [Manage background processes](#manage-background-processes)
  - [Stop / destroy VMs](#stop--destroy-vms)
  - [View configuration](#view-configuration)
  - [Help](#help)
- [Customizing the rootfs](#customizing-the-rootfs)

## Installation

### Quick install

```bash
curl -fsSL https://raw.githubusercontent.com/scttnlsn/vume/main/install.sh | bash
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

Vume requires root because it manages KVM virtual machines, ZFS datasets, and network interfaces (bridge, tap, iptables NAT).

If you install to a non-default path, set `VUME_HOME` in your shell profile:

```bash
export VUME_HOME=/your/path
```

### Development

If you have a local checkout of this repo:

```bash
./install.sh
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

## Why ZFS?

ZFS is a natural fit for Firecracker VM storage for several reasons:

**Instant clones via Copy-on-Write** — ZFS dataset clones let you spin up a new VM from a base rootfs almost instantly. The clone initially shares all blocks with its parent, so disk space only grows as the VM writes data that differs from the base image.

**Customizable base images** — You can snapshot a customized VM and promote it as the new rootfs version. Future VMs clone from the new version while existing VMs are unaffected.

**ZFS at the filesystem level** — Transparent LZ4 compression reduces storage overhead with negligible CPU cost. Checksummed blocks protect against silent data corruption. Datasets can be streamed via `zfs send`/`zfs receive` for backups, replication, or moving VM storage across hosts.

## Configuration

All configuration is stored in `$VUME_HOME/vume.toml` (default: `/opt/vume/vume.toml`).

See `config/vume.toml` for default values.

## CLI Usage

All `vume` commands require root privileges.

### Start a VM

```bash
sudo vume start                  # auto-generated ID
sudo vume start --id my-vm       # specific ID
```

Running `start` with an existing stopped VM's ID will resume it.

### List VMs

```bash
sudo vume list                   # all VMs
sudo vume list --status running  # filter by status (running, stopped, error)
```

### Execute a command in a VM

```bash
sudo vume exec my-vm "uname -a"
sudo vume exec my-vm "apt install -y python3" --timeout 120
```

### Manage background processes

```bash
sudo vume process start my-vm my-server "python3 -m http.server"
sudo vume process stop my-vm my-server
sudo vume process list my-vm
sudo vume process logs my-vm my-server --lines 100
```

### Stop / destroy VMs

```bash
sudo vume stop my-vm             # stop a running VM
sudo vume destroy my-vm          # remove a VM completely
sudo vume destroy                # destroy all VMs (with confirmation)
```

### View configuration

```bash
sudo vume config get vcpu
sudo vume config get network.bridge
sudo vume config path
```

### Help

```bash
vume --help
vume <command> --help
```

## Customizing the rootfs

You can create a custom base image from a running VM. The active rootfs version is tracked
via the `vume:latest` ZFS user property on `vume/rootfs`, so you can create new versions without
affecting existing VMs.

```bash
# 1. Customize a running VM, e.g.
sudo vume exec <vm-id> "apt install -y python3 nginx"

# 2. Snapshot the customized VM and create a new rootfs version
sudo zfs snapshot vume/<vm-id>@<version>
sudo zfs send vume/<vm-id>@<version> | sudo zfs receive vume/rootfs@<version>

# 3. Point to the new version
sudo zfs set vume:latest=<version> vume/rootfs
```

All future VMs will clone from the new version. Existing VMs are unaffected.
