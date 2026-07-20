#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$PROJECT_ROOT/scripts/common.sh"
SERIAL="$(a26_resolve_serial)"
STAGE="/data/local/tmp/a26-native-session-integrate.$$"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; adb -s "$SERIAL" shell "/data/local/tmp/su -c '\''rm -rf $STAGE'\''" >/dev/null 2>&1 || true' EXIT

adb -s "$SERIAL" shell "/data/local/tmp/su -c 'set -eu
rm -rf $STAGE
mkdir -m 0700 $STAGE
cp /data/local/a26-linux/usr/local/sbin/a26-xorg-persistent-session $STAGE/session
cp /data/local/tmp/a26-android-graphics-restore.sh $STAGE/restore
chown -R 2000:2000 $STAGE
chmod 0700 $STAGE
chmod 0600 $STAGE/session $STAGE/restore'"
adb -s "$SERIAL" pull "$STAGE/session" "$WORK/session" >/dev/null
adb -s "$SERIAL" pull "$STAGE/restore" "$WORK/restore" >/dev/null

python3 - "$WORK/session" "$WORK/restore" <<'PY'
from pathlib import Path
import sys

session = Path(sys.argv[1])
restore = Path(sys.argv[2])

text = session.read_text()
cleanup_marker = "    # A26_NATIVE_WIFI_CLEANUP\n"
if cleanup_marker not in text:
    needle = "    trap - EXIT HUP INT TERM\n"
    replacement = needle + cleanup_marker + "    /opt/a26-system/libexec/a26-wifi-stop 2>/dev/null || true\n"
    if text.count(needle) != 1:
        raise SystemExit("unexpected persistent-session cleanup layout")
    text = text.replace(needle, replacement)
start_marker = "# A26_NATIVE_WIFI_START\n"
if start_marker not in text:
    needle = "# The only client used for the initial bare-server presentation."
    replacement = (
        start_marker
        + "/opt/a26-system/libexec/a26-wifi-start || echo 'native Wi-Fi start failed' >&2\n\n"
        + needle
    )
    if text.count(needle) != 1:
        raise SystemExit("unexpected persistent-session startup layout")
    text = text.replace(needle, replacement)
session.write_text(text)

text = restore.read_text()
marker = "# A26_NATIVE_WIFI_RESTORE_CLEANUP\n"
if marker not in text:
    needle = "# Restore the three services deliberately stopped for DRM-master handoff."
    replacement = (
        marker
        + "/system/bin/sh /data/local/tmp/a26-wifi-cleanup-android.sh 2>/dev/null || true\n\n"
        + needle
    )
    if text.count(needle) != 1:
        raise SystemExit("unexpected Android restore layout")
    text = text.replace(needle, replacement)
restore.write_text(text)
PY

sh -n "$WORK/session"
sh -n "$WORK/restore"
(
    cd "$WORK"
    sha256sum session restore >MANIFEST.sha256
)
adb -s "$SERIAL" push "$WORK/session" "$STAGE/session" >/dev/null
adb -s "$SERIAL" push "$WORK/restore" "$STAGE/restore" >/dev/null
adb -s "$SERIAL" push "$WORK/MANIFEST.sha256" "$STAGE/MANIFEST.sha256" >/dev/null
adb -s "$SERIAL" shell "/data/local/tmp/su -c 'set -eu
test -f $STAGE/session
test -f $STAGE/restore
chown -R 0:0 $STAGE
chmod 0700 $STAGE
chmod 0600 $STAGE/session $STAGE/restore $STAGE/MANIFEST.sha256
test -z \"\$(find $STAGE -type l -print -quit)\"
cd $STAGE
sha256sum -c MANIFEST.sha256
cp $STAGE/session /data/local/a26-linux/usr/local/sbin/a26-xorg-persistent-session.new
cp $STAGE/restore /data/local/tmp/a26-android-graphics-restore.sh.new
chown 0:0 /data/local/a26-linux/usr/local/sbin/a26-xorg-persistent-session.new /data/local/tmp/a26-android-graphics-restore.sh.new
chmod 0755 /data/local/a26-linux/usr/local/sbin/a26-xorg-persistent-session.new /data/local/tmp/a26-android-graphics-restore.sh.new
mv -f /data/local/a26-linux/usr/local/sbin/a26-xorg-persistent-session.new /data/local/a26-linux/usr/local/sbin/a26-xorg-persistent-session
mv -f /data/local/tmp/a26-android-graphics-restore.sh.new /data/local/tmp/a26-android-graphics-restore.sh'"

echo "integrated native Wi-Fi lifecycle into the Xorg session and Android restore"
