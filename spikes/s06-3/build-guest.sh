#!/bin/sh
# Spike S06-3 (ADR 0006): builds the guest in a debian:trixie container, on a
# GitHub runner only. Debian's stock kernel (linux-image-amd64, GPL-2.0) is
# copied out as it ships; nothing of it is committed, changed or shipped. The
# initramfs holds Debian's busybox-static and dav1d with its libraries, our
# s063probe and /init, and a synthetic 10 s 1080p AV1 clip made here.
#   docker run --rm -v "$PWD/spikes/s06-3:/src:ro" -v "$OUT:/out" debian:trixie sh /src/build-guest.sh
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  linux-image-amd64 busybox-static dav1d gcc libc6-dev cpio gzip ffmpeg >/dev/null

cp /boot/vmlinuz-* /out/bzImage

root=/tmp/root
mkdir -p "$root/bin"
cp /bin/busybox "$root/bin/busybox"
gcc -static -O2 -Wall -o "$root/bin/s063probe" /src/guest/s063probe.c
cp /usr/bin/dav1d "$root/bin/dav1d"
for lib in $(ldd /usr/bin/dav1d | grep -o '/[^ ]*'); do
  cp -L --parents "$lib" "$root"
done
cp /src/guest/init "$root/init"
chmod 755 "$root/init"

clip="$root/clip.ivf"
ffmpeg -nostdin -loglevel error -f lavfi -i testsrc2=size=1920x1080:rate=30 -t 10 \
  -c:v libsvtav1 -preset 10 -b:v 4M "$clip" ||
  ffmpeg -nostdin -loglevel error -f lavfi -i testsrc2=size=1920x1080:rate=30 -t 10 \
    -c:v libaom-av1 -usage realtime -cpu-used 10 -b:v 4M "$clip"

(cd "$root" && find . | cpio -o -H newc --quiet | gzip -1) > /out/initrd
{
  dpkg-query -W -f '${Package} ${Version}\n' 'linux-image-*' busybox-static dav1d libdav1d7 ffmpeg libc6
  echo "clip $(stat -c %s "$clip") bytes"
  echo "initrd $(stat -c %s /out/initrd) bytes"
  echo "bzImage $(stat -c %s /out/bzImage) bytes $(sha256sum /out/bzImage | cut -d' ' -f1)"
} | tee /out/versions.txt
