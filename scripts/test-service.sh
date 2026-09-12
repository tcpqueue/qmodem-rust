#!/bin/sh
set -eu
# WSL mirrored networking routes 127.0.0.1 over loopback0 rather than lo.
# A private network namespace gives SO_BINDTODEVICE tests standard Linux routes.
exec unshare --user --map-root-user --net sh -c 'ip link set lo up && node --test tests/service.mjs'
