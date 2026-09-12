#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "$PWD" in /mnt/*) echo "Build from a Linux-native directory." >&2; exit 1;; esac
: "${ZIG:?Set ZIG to an installed Zig 0.14.1 executable}"
ZIG="$(realpath "$ZIG")"
test -x "$ZIG"
test "$("$ZIG" version)" = 0.14.1 || { echo "Zig 0.14.1 is required." >&2; exit 1; }
export QMODEM_ZIG="$ZIG"
mkdir -p work/cross
cat > work/cross/zig-cc <<'WRAPPER'
#!/bin/bash
set -eu
args=()
for arg in "$@"; do
    case "$arg" in --target=*) ;; *) args+=("$arg");; esac
done
exec "$QMODEM_ZIG" cc -target "$QMODEM_ZIG_TARGET" "${args[@]}"
WRAPPER
cat > work/cross/zig-ar <<'WRAPPER'
#!/bin/sh
exec "$QMODEM_ZIG" ar "$@"
WRAPPER
chmod +x work/cross/zig-cc work/cross/zig-ar
host="$(rustc -vV | sed -n 's/^host: //p')"
linker="$(rustc --print sysroot)/lib/rustlib/$host/bin/rust-lld"
test -x "$linker"
targets=("$@")
if [ "${#targets[@]}" = 0 ]; then targets=(aarch64 x86_64); fi
for arch in "${targets[@]}"; do
    case "$arch" in
        aarch64|x86_64) zig_arch="$arch";;
        i686) zig_arch=x86;;
        *) echo "Supported architectures: aarch64 x86_64 i686" >&2; exit 1;;
    esac
    triple="$arch-unknown-linux-musl"
    rustup target add "$triple"
    upper="${arch^^}"
    export QMODEM_ZIG_TARGET="$zig_arch-linux-musl"
    env "CARGO_TARGET_${upper}_UNKNOWN_LINUX_MUSL_LINKER=$linker"         "CC_${arch}_unknown_linux_musl=$PWD/work/cross/zig-cc"         "AR_${arch}_unknown_linux_musl=$PWD/work/cross/zig-ar"         cargo build --release --locked --target "$triple"
    binary="target/$triple/release/qmodemd"
    file "$binary"
    if readelf -l "$binary" | grep -q INTERP || readelf -d "$binary" | grep -q NEEDED; then
        echo "Unexpected runtime library dependency: $binary" >&2
        exit 1
    fi
done
