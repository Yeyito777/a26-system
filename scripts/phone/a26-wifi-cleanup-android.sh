#!/system/bin/sh
set -u

ROOT=/data/local/a26-linux
RUN="$ROOT/run/a26-wifi"
BB="$ROOT/busybox.static"

stop_pidfile() {
    file=$1
    expected=$2
    [ -f "$file" ] || return 0
    pid="$($BB cat "$file" 2>/dev/null || true)"
    case "$pid" in ''|*[!0-9]*) return 0 ;; esac
    [ "$pid" -gt 1 ] 2>/dev/null || return 0
    comm="$($BB cat "/proc/$pid/comm" 2>/dev/null || true)"
    [ "$comm" = "$expected" ] || return 0
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $($BB seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        $BB sleep 0.1
    done
    kill -KILL "$pid" 2>/dev/null || true
}

stop_exact_native() {
    expected=$1
    pattern=$2
    for process in /proc/[0-9]*; do
        pid=${process##*/}
        comm="$($BB cat "/proc/$pid/comm" 2>/dev/null || true)"
        [ "$comm" = "$expected" ] || continue
        command="$($BB tr '\000' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
        case "$command" in *"$pattern"*) ;; *) continue ;; esac
        kill -TERM "$pid" 2>/dev/null || true
        for _ in $($BB seq 1 50); do
            kill -0 "$pid" 2>/dev/null || break
            $BB sleep 0.1
        done
        kill -KILL "$pid" 2>/dev/null || true
    done
}

# Stop the supervisor first so it cannot respawn either child while cleanup is
# in progress. Every PID is accepted only from the private runtime directory
# and must have the exact expected kernel comm name.
stop_pidfile "$RUN/service.pid" a26-wifi-servic
stop_pidfile "$RUN/udhcpc.pid" udhcpc
stop_pidfile "$RUN/wpa.pid" wpa_supplicant
stop_exact_native a26-wifi-servic /opt/a26-system/bin/a26-wifi-service
stop_exact_native udhcpc /opt/a26-system/libexec/a26-udhcpc.script
stop_exact_native wpa_supplicant /run/a26-wifi/wpa_supplicant.conf
stop_exact_native wpa_supplicant /etc/a26-wifi/wpa_supplicant.conf

ifindex="$($BB cat /sys/class/net/wlan0/ifindex 2>/dev/null || true)"
/system/bin/ip -4 addr flush dev wlan0 2>/dev/null || true
/system/bin/ip -6 addr flush dev wlan0 2>/dev/null || true
while /system/bin/ip rule del priority 10550 2>/dev/null; do :; done
if [ -n "$ifindex" ]; then
    /system/bin/ip route flush table $((1000 + ifindex)) 2>/dev/null || true
    /system/bin/ip -6 route flush table $((1000 + ifindex)) 2>/dev/null || true
fi
/system/bin/ip neigh flush dev wlan0 2>/dev/null || true
/system/bin/ip link set dev wlan0 down 2>/dev/null || true
$BB rm -rf "$RUN/wpa"
$BB rm -f "$RUN/control.sock" "$RUN/service.pid" "$RUN/udhcpc.pid" \
    "$RUN/wpa.pid" "$RUN/service.lock" "$RUN/wpa_supplicant.conf" \
    "$ROOT/etc/a26-wifi/wpa_supplicant.conf"
exit 0
