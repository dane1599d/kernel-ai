#!/usr/bin/env bash
# Builds QEMU $QEMU_VERSION into ~/qemu from its pinned source tarball, checked
# against $QEMU_SHA256, on an Ubuntu 24.04 runner. ci.yml's `qemu` job runs it
# to fill the cache; release.yml's QEMU jobs run it when the cache misses
# (GitHub evicts caches, and ci.yml may be rebuilding it at the same time).
# Bump QEMU_VERSION and QEMU_SHA256 in ci.yml and release.yml, and `version` in
# action.yml, together.
set -euo pipefail
: "${QEMU_VERSION:?}" "${QEMU_SHA256:?}"

sudo apt-get update
sudo apt-get install -y --no-install-recommends ninja-build pkg-config python3-venv \
  libglib2.0-dev libpixman-1-dev libslirp-dev libfdt-dev flex bison
# Outside the checkout: an untracked file would mark a later build -dirty.
work=$(mktemp -d)
cd "$work"
curl -sSfL "https://download.qemu.org/qemu-$QEMU_VERSION.tar.xz" -o qemu.tar.xz
echo "$QEMU_SHA256  qemu.tar.xz" | sha256sum -c -
tar -xJf qemu.tar.xz
cd "qemu-$QEMU_VERSION"
./configure --prefix="$HOME/qemu" --target-list=x86_64-softmmu --enable-slirp \
  --disable-docs --disable-gtk --disable-sdl --disable-opengl --disable-werror
make -j"$(nproc)" install
"$HOME/qemu/bin/qemu-system-x86_64" --version
