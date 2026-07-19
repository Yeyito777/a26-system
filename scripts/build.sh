#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-unknown-linux-musl"
BINARY="$PROJECT_ROOT/target/$TARGET/release/a26-system"

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

if readelf -l "$BINARY" | grep -q 'Requesting program interpreter'; then
    echo "build is dynamically linked; expected a static musl executable" >&2
    exit 1
fi

echo "built static $TARGET executable: ${BINARY#"$PROJECT_ROOT/"}"

