#!/usr/bin/env bash

a26_resolve_serial() {
    if [[ -n "${A26_SERIAL:-}" ]]; then
        printf '%s\n' "$A26_SERIAL"
        return 0
    fi

    local devices=()
    mapfile -t devices < <(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }')
    case "${#devices[@]}" in
        1) printf '%s\n' "${devices[0]}" ;;
        0)
            echo "no authorized ADB device found; set A26_SERIAL if needed" >&2
            return 2
            ;;
        *)
            echo "multiple ADB devices found; set A26_SERIAL explicitly" >&2
            return 2
            ;;
    esac
}

