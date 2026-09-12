#!/bin/sh
set -eu
binary="$1"
arch="$2"
machine="$(LC_ALL=C readelf -h "$binary" | sed -n 's/.*Machine: *//p')"
case "$arch:$machine" in
    aarch64:AArch64|x86_64:*X86-64*|i386:*80386*|i686:*80386*) ;;
    *) echo "Binary architecture mismatch: SDK $arch, binary $machine" >&2; exit 1;;
esac
if readelf -l "$binary" | grep -q INTERP || readelf -d "$binary" | grep -q NEEDED; then
    echo "Prebuilt binary must be statically linked." >&2
    exit 1
fi
