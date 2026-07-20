#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$PROJECT_ROOT/scripts/common.sh"
SERIAL="$(a26_resolve_serial)"

adb -s "$SERIAL" get-state >/dev/null
adb -s "$SERIAL" shell '/data/local/tmp/su -c '\''set -eu
test -x /data/local/a26-linux/opt/a26-system/libexec/a26-wifi-start
/data/local/a26-linux/busybox.static chroot /data/local/a26-linux \
    /opt/a26-system/libexec/a26-wifi-start'\'''
echo "a26-wifi-service is ready"
