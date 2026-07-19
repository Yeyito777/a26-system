#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$PROJECT_ROOT/scripts/common.sh"

for command in adb sha256sum; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command not found: $command" >&2
        exit 1
    fi
done

SERIAL="$(a26_resolve_serial)"
TARGET="aarch64-unknown-linux-musl"
BINARY="$PROJECT_ROOT/target/$TARGET/release/a26-system"
ICON="$PROJECT_ROOT/assets/system-app.bgrx"
STAGED_BINARY="/data/local/tmp/a26-system.install"
STAGED_ICON="/data/local/tmp/system-app.bgrx.install"

cleanup() {
    adb -s "$SERIAL" shell "rm -f '$STAGED_BINARY' '$STAGED_ICON'" >/dev/null 2>&1 || true
}
trap cleanup EXIT

"$PROJECT_ROOT/scripts/build.sh"
adb -s "$SERIAL" get-state >/dev/null
adb -s "$SERIAL" push "$BINARY" "$STAGED_BINARY" >/dev/null
adb -s "$SERIAL" push "$ICON" "$STAGED_ICON" >/dev/null

adb -s "$SERIAL" shell '/data/local/tmp/su -c '\''set -eu
root=/data/local/a26-linux/opt/a26-system
mkdir -p "$root/bin" "$root/share"
cp /data/local/tmp/a26-system.install "$root/bin/a26-system.new"
cp /data/local/tmp/system-app.bgrx.install "$root/share/system-app.bgrx.new"
chown 0:0 "$root/bin/a26-system.new" "$root/share/system-app.bgrx.new"
chmod 0755 "$root/bin/a26-system.new"
chmod 0644 "$root/share/system-app.bgrx.new"
mv -f "$root/bin/a26-system.new" "$root/bin/a26-system"
mv -f "$root/share/system-app.bgrx.new" "$root/share/system-app.bgrx"'\'''

expected_binary="$(sha256sum "$BINARY" | awk '{print $1}')"
expected_icon="$(sha256sum "$ICON" | awk '{print $1}')"
installed_hashes="$(
    adb -s "$SERIAL" shell \
        '/data/local/tmp/su -c "sha256sum /data/local/a26-linux/opt/a26-system/bin/a26-system /data/local/a26-linux/opt/a26-system/share/system-app.bgrx"' \
        | tr -d '\r'
)"

grep -q "^$expected_binary  .*a26-system$" <<<"$installed_hashes"
grep -q "^$expected_icon  .*system-app.bgrx$" <<<"$installed_hashes"

echo "installed a26-system under /opt/a26-system in the device Linux rootfs"

