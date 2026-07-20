#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-unknown-linux-musl"
BINARY="$PROJECT_ROOT/target/$TARGET/release/a26-system"
WIFI_BINARY="$PROJECT_ROOT/target/$TARGET/release/a26-wifi-service"
WIFI_CTL="$PROJECT_ROOT/target/$TARGET/release/a26-wifi-ctl"
SYSTEM_CTL="$PROJECT_ROOT/target/$TARGET/release/a26-systemctl"

for command in cargo rustup aarch64-linux-musl-gcc python3 readelf; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command not found: $command" >&2
        exit 1
    fi
done

cd "$PROJECT_ROOT"
rustup target add "$TARGET" >/dev/null
python3 scripts/prepare-assets.py
scripts/check.sh
cargo build --locked --release --target "$TARGET"

for executable in "$BINARY" "$WIFI_BINARY" "$WIFI_CTL" "$SYSTEM_CTL"; do
    if readelf -l "$executable" | grep -q 'Requesting program interpreter'; then
        echo "build is dynamically linked; expected a static musl executable: $executable" >&2
        exit 1
    fi
done

echo "built static $TARGET executables:"
echo "  ${BINARY#"$PROJECT_ROOT/"}"
echo "  ${WIFI_BINARY#"$PROJECT_ROOT/"}"
echo "  ${WIFI_CTL#"$PROJECT_ROOT/"}"
echo "  ${SYSTEM_CTL#"$PROJECT_ROOT/"}"
