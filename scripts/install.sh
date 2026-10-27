#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$PROJECT_ROOT/scripts/common.sh"

for command in adb curl sha256sum; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "required command not found: $command" >&2
        exit 1
    }
done

SERIAL="$(a26_resolve_serial)"
TARGET="aarch64-unknown-linux-musl"
HOST_STAGE="$(mktemp -d)"
DEVICE_STAGE="/data/local/tmp/a26-system-install-$$-$RANDOM"

cleanup() {
    rm -rf "$HOST_STAGE"
    adb -s "$SERIAL" shell "/data/local/tmp/su -c 'rm -rf $DEVICE_STAGE'" \
        >/dev/null 2>&1 || true
}
trap cleanup EXIT

# A26_SYSTEM_PREBUILT=1: the release binaries and assets were built elsewhere
# (OSS-Phone-V1's install-all.sh builds them on a host with the musl
# toolchain) and are already in target/ and assets/.
if [[ "${A26_SYSTEM_PREBUILT:-0}" == 1 ]]; then
    for name in a26-system a26-wifi-service a26-wifi-ctl a26-systemctl; do
        [[ -x "$PROJECT_ROOT/target/$TARGET/release/$name" ]] || {
            echo "A26_SYSTEM_PREBUILT=1 but target/$TARGET/release/$name is missing" >&2
            exit 1
        }
    done
else
    "$PROJECT_ROOT/scripts/build.sh"
fi
"$PROJECT_ROOT/scripts/fetch-wifi-packages.sh" >/dev/null
adb -s "$SERIAL" get-state >/dev/null

mkdir -p "$HOST_STAGE/wifi-packages"
cp "$PROJECT_ROOT/target/$TARGET/release/a26-system" "$HOST_STAGE/a26-system"
cp "$PROJECT_ROOT/target/$TARGET/release/a26-wifi-service" "$HOST_STAGE/a26-wifi-service"
cp "$PROJECT_ROOT/target/$TARGET/release/a26-wifi-ctl" "$HOST_STAGE/a26-wifi-ctl"
cp "$PROJECT_ROOT/target/$TARGET/release/a26-systemctl" "$HOST_STAGE/a26-systemctl"
cp "$PROJECT_ROOT/assets/system-app.bgrx" "$HOST_STAGE/system-app.bgrx"
cp "$PROJECT_ROOT/scripts/phone/a26-udhcpc.script" "$HOST_STAGE/a26-udhcpc.script"
cp "$PROJECT_ROOT/scripts/phone/a26-wifi-start" "$HOST_STAGE/a26-wifi-start"
cp "$PROJECT_ROOT/scripts/phone/a26-wifi-stop" "$HOST_STAGE/a26-wifi-stop"
cp "$PROJECT_ROOT/scripts/phone/a26-wifi-cleanup-android.sh" \
    "$HOST_STAGE/a26-wifi-cleanup-android.sh"
cp "$PROJECT_ROOT/scripts/phone/install-wifi-packages.sh" \
    "$HOST_STAGE/install-wifi-packages.sh"
cp "$PROJECT_ROOT/build/wifi-packages/"*.apk "$HOST_STAGE/wifi-packages/"

(
    cd "$HOST_STAGE"
    find . -type f ! -name MANIFEST.sha256 -print0 \
        | sort -z | xargs -0 sha256sum >MANIFEST.sha256
)

adb -s "$SERIAL" shell "umask 077; mkdir '$DEVICE_STAGE'"
for file in "$HOST_STAGE"/*; do
    adb -s "$SERIAL" push "$file" "$DEVICE_STAGE/" >/dev/null
done

adb -s "$SERIAL" shell "/data/local/tmp/su -c 'set -eu
stage=$DEVICE_STAGE
root=/data/local/a26-linux
test -d \"\$stage\" && test ! -L \"\$stage\"
chown -R 0:0 \"\$stage\"
chmod 0700 \"\$stage\" \"\$stage/wifi-packages\"
find \"\$stage\" -type f -exec chmod 0600 {} \\;
test -z \"\$(find \"\$stage\" -type l -print -quit)\"
test \"\$(find \"\$stage\" -type f | wc -l)\" -eq 16
cd \"\$stage\"
sha256sum -c MANIFEST.sha256

pkg=\"\$root/tmp/a26-system-wifi-packages-install\"
rm -rf \"\$pkg\"
mkdir -m 0700 \"\$pkg\"
cp wifi-packages/*.apk \"\$pkg/\"
cp install-wifi-packages.sh \"\$root/tmp/a26-install-wifi-packages.sh\"
chmod 0700 \"\$root/tmp/a26-install-wifi-packages.sh\"
\"\$root/busybox.static\" chroot \"\$root\" /bin/sh \
    /tmp/a26-install-wifi-packages.sh /tmp/a26-system-wifi-packages-install

dest=\"\$root/opt/a26-system\"
mkdir -p \"\$dest/bin\" \"\$dest/share\" \"\$dest/libexec\"
for name in a26-system a26-wifi-service a26-wifi-ctl a26-systemctl; do
    cp \"\$stage/\$name\" \"\$dest/bin/\$name.new\"
    chown 0:0 \"\$dest/bin/\$name.new\"
    chmod 0755 \"\$dest/bin/\$name.new\"
    mv -f \"\$dest/bin/\$name.new\" \"\$dest/bin/\$name\"
done
cp \"\$stage/system-app.bgrx\" \"\$dest/share/system-app.bgrx.new\"
chown 0:0 \"\$dest/share/system-app.bgrx.new\"
chmod 0644 \"\$dest/share/system-app.bgrx.new\"
mv -f \"\$dest/share/system-app.bgrx.new\" \"\$dest/share/system-app.bgrx\"
for name in a26-udhcpc.script a26-wifi-start a26-wifi-stop; do
    cp \"\$stage/\$name\" \"\$dest/libexec/\$name.new\"
    chown 0:0 \"\$dest/libexec/\$name.new\"
    chmod 0755 \"\$dest/libexec/\$name.new\"
    mv -f \"\$dest/libexec/\$name.new\" \"\$dest/libexec/\$name\"
done
cp \"\$stage/a26-wifi-cleanup-android.sh\" \
    /data/local/tmp/a26-wifi-cleanup-android.sh.new
chown 0:0 /data/local/tmp/a26-wifi-cleanup-android.sh.new
chmod 0755 /data/local/tmp/a26-wifi-cleanup-android.sh.new
mv -f /data/local/tmp/a26-wifi-cleanup-android.sh.new \
    /data/local/tmp/a26-wifi-cleanup-android.sh
rm -rf \"\$pkg\" \"\$root/tmp/a26-install-wifi-packages.sh\" \"\$stage\"'"

"$PROJECT_ROOT/scripts/integrate-native-session.sh"
echo "installed a26-system and native Wi-Fi support (service activation unchanged)"
