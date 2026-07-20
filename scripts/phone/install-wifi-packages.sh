#!/bin/sh
set -eu

PACKAGE_DIR=${1:-/tmp/a26-system-wifi-packages}

/sbin/apk add --no-network \
    "$PACKAGE_DIR/dbus-libs-1.16.2-r2.apk" \
    "$PACKAGE_DIR/iw-6.17-r0.apk" \
    "$PACKAGE_DIR/libnl3-3.11.0-r0.apk" \
    "$PACKAGE_DIR/pcsc-lite-libs-2.4.0-r4.apk" \
    "$PACKAGE_DIR/wpa_supplicant-2.11-r4.apk"
