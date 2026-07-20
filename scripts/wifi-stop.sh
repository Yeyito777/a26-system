#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$PROJECT_ROOT/scripts/common.sh"
SERIAL="$(a26_resolve_serial)"

adb -s "$SERIAL" get-state >/dev/null
adb -s "$SERIAL" shell \
    '/data/local/tmp/su -c "/system/bin/sh /data/local/tmp/a26-wifi-cleanup-android.sh"'
echo "native Wi-Fi service stopped"
