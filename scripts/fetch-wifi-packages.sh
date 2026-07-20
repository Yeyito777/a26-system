#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="$PROJECT_ROOT/build/wifi-packages"
SOURCES="$PROJECT_ROOT/wifi-packages/SOURCES"
MANIFEST="$PROJECT_ROOT/wifi-packages/SHA256SUMS"

command -v curl >/dev/null 2>&1 || { echo "required command not found: curl" >&2; exit 1; }
mkdir -p "$OUTPUT"

while IFS= read -r url; do
    [[ -n "$url" ]] || continue
    destination="$OUTPUT/${url##*/}"
    if [[ ! -f "$destination" ]]; then
        curl --fail --location --retry 3 --output "$destination.new" "$url"
        mv -f "$destination.new" "$destination"
    fi
done < "$SOURCES"

(cd "$OUTPUT" && sha256sum -c "$MANIFEST")
echo "$OUTPUT"
