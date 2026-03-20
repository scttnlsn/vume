#!/usr/bin/env bash
#
# Download Firecracker binary and vmlinux kernel image.
#
# Usage: ./scripts/download.sh [--out-dir <dir>]
#
set -euo pipefail

OUT_DIR="./vume"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out-dir) OUT_DIR="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

ARCH="$(uname -m)"
release_url="https://github.com/firecracker-microvm/firecracker/releases"
latest=$(basename $(curl -fsSLI -o /dev/null -w  %{url_effective} ${release_url}/latest))
CI_VERSION=${latest%.*}

mkdir -p "$OUT_DIR"

echo ":: downloading firecracker binary (${latest}, ${ARCH})"
curl -sL "${release_url}/download/${latest}/firecracker-${latest}-${ARCH}.tgz" \
    | tar -xz -C "$OUT_DIR"
mv "${OUT_DIR}/release-${latest}-${ARCH}/firecracker-${latest}-${ARCH}" "${OUT_DIR}/firecracker"
rm -rf "${OUT_DIR}/release-${latest}-${ARCH}"

echo ":: finding latest kernel for ${CI_VERSION}/${ARCH}"
latest_kernel_key=$(curl -s "http://spec.ccfc.min.s3.amazonaws.com/?prefix=firecracker-ci/$CI_VERSION/$ARCH/vmlinux-&list-type=2" \
    | grep -oP "(?<=<Key>)(firecracker-ci/$CI_VERSION/$ARCH/vmlinux-[0-9]+\.[0-9]+\.[0-9]{1,3})(?=</Key>)" \
    | sort -V | tail -1)

echo ":: downloading kernel: ${latest_kernel_key}"
wget -q "https://s3.amazonaws.com/spec.ccfc.min/${latest_kernel_key}" -O "${OUT_DIR}/vmlinux"

echo ":: done"
echo "   firecracker: ${OUT_DIR}/firecracker"
echo "   vmlinux:     ${OUT_DIR}/vmlinux"
