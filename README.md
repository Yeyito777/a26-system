# a26-system

`a26-system` is a standalone Rust/X11 application containing the System scene
from the A26 shell plus a touch-first Wi-Fi configuration view. It creates a
conventional, window-manager-controlled 1080x2340 window named `a26-system`; it
is deliberately **not** an override-redirect window.

![A26 System app](docs/system-view.png)

Companion window manager: <https://github.com/Yeyito777/a26-shell>

The view keeps the original navy, mint, blue, and white card layout and shows:

- aggregate CPU utilization calculated from consecutive `/proc/stat` samples;
- Samsung Mali utilization from
  `/sys/class/misc/mali0/device/utilization`;
- root-filesystem used and available space from `statvfs(3)`;
- device, kernel, Alpine release, total memory, display, window-manager, and
  control cards; and
- the bottom close instruction and gesture indicator.

The original System metrics view remains the default. Its large `WI-FI` target
opens a phone-sized connection view with current link state, a disconnect
control, manual refresh, signal/security metadata, deterministic five-row scan
pages, and large whole-row SSID targets. Protected networks open a masked
password view with a four-row lowercase/shift/symbol on-screen keyboard. The
entry accepts WPA Personal passphrases of 8-63 printable ASCII bytes or a
64-digit hexadecimal key.

Telemetry and the displayed logical volume are refreshed once per second. The
volume is read by connecting to `/run/a26-shell/control.sock`, sending
`state\n`, and parsing the shell's JSON response. It is rendered as `--` when
the socket cannot be reached, times out, or returns an invalid response.

Core X11 pointer events, including touch-to-pointer emulation, drive only
tap-sized controls. A 32-pixel movement cancels a tap, and the bottom 170 pixels
never produce an app action. The app does not recognize, grab, or close on
swipes. A26 Shell remains the owner of the global bottom-edge swipe and
terminates this process when the user closes the app.

Wi-Fi I/O is defined by the root-only protocol in
[`docs/wifi-backend-protocol.md`](docs/wifi-backend-protocol.md). The UI only
talks to `/run/a26-wifi/control.sock` from a worker thread. It contains no
privileged networking implementation and safely renders the service as
unavailable when no backend is installed.

## Repository layout

```text
assets/                     System icon source and generated BGRX pixels
scripts/prepare-assets.py   deterministic PNG-to-BGRX conversion
scripts/check.sh            host format, test, clippy, and debug-build checks
scripts/build.sh            checked static aarch64 musl release build
scripts/install.sh          build and ADB installation
src/                        X11 client, model, renderer, telemetry, IPC, and font
docs/system-view.png        capture of the separately managed app on-device
docs/wifi-backend-protocol.md  root-only backend contract and secret rules
```

`assets/system-app.bgrx` is exactly 220x220 pixels, four bytes per pixel in
B,G,R,X byte order. It is committed so the launcher can consume it directly.
The PNG is the editable/source asset.

## Host prerequisites

- Rust with `rustup`
- `aarch64-linux-musl-gcc`
- Python 3 with the Pillow version pinned in `requirements-assets.txt`
- ADB, only for installation
- `readelf` (normally supplied by binutils) for the static-link check

Install the pinned asset dependency in an isolated environment if desired:

```sh
python3 -m pip install -r requirements-assets.txt
```

Run all native host checks:

```sh
scripts/check.sh
```

The deterministic unit suite covers touch/drag discrimination, reserved shell
gesture space, hit targets, keyboard modes, password validation/redaction,
scan sorting/de-duplication/paging, model transitions, protocol bounds and
error mapping, and exact Unix-socket request/response behavior.

Create the static target build:

```sh
scripts/build.sh
```

The resulting untracked executable is
`target/aarch64-unknown-linux-musl/release/a26-system`. The build script adds
the Rust target when necessary and rejects an ELF containing a dynamic program
interpreter.

## Installation

```sh
scripts/install.sh
```

When `A26_SERIAL` is unset, the installer proceeds only when `adb devices`
reports exactly one authorized device. Set `A26_SERIAL` to select a device
explicitly. The script selects the device, builds, stages files through the
device's local temporary directory, installs them atomically, verifies SHA-256
hashes, and removes its staging files.

Within the Linux root filesystem rooted at `/data/local/a26-linux`, it installs:

- `/opt/a26-system/bin/a26-system`
- `/opt/a26-system/share/system-app.bgrx`

Running the installer is the only repository workflow that contacts a device.

## Runtime and integration assumptions

- The executable runs in the intended Alpine root filesystem/mount namespace,
  with `/proc` and `/sys` mounted and the shell's `/run` directory visible. This
  makes `/`, `/etc/alpine-release`, and the telemetry paths describe the Linux
  environment rather than the Android host.
- An X11 server is available through `DISPLAY` and has a 24-bit-compatible
  true-color visual, as in the target environment.
- The window manager honors or enforces the requested 1080x2340 fullscreen
  geometry. The client also publishes the standard EWMH fullscreen state and
  redraws after configure/expose events.
- The launcher starts the binary with permission to read the root-only shell
  control socket. Running it without that permission is safe; only the volume
  field becomes unavailable.
- The shell's `state` response remains a newline-terminated JSON envelope with
  a numeric `result.volume` between 0 and 100.
- The window manager retains the global bottom-swipe gesture, kills or closes
  `a26-system`, and then reveals the launcher. No such policy is implemented
  by this client.
- A separately managed root service may expose `/run/a26-wifi/control.sock`
  according to the documented version-1 protocol. The directory is expected
  to be root-owned mode `0700`, the socket root-owned mode `0600`, and the
  service must enforce UID 0 with `SO_PEERCRED`. Without it, metrics and volume
  continue to work and the Wi-Fi view reports that the service is unavailable.
- Passwords are accepted only through the on-screen keyboard. They are never
  accepted through argv or environment variables, never logged or persisted,
  never returned by status, always masked on screen, serialized directly to
  the local socket, and held in zeroizing non-cloneable buffers only while
  entry or request delivery requires them.
- Launcher integration can use the installed BGRX icon directly; the app does
  not load the icon while rendering the System scene.

No Wi-Fi backend, service definition, privileged networking implementation, or
window-manager configuration is installed by this repository. Integration
policy remains the responsibility of the surrounding phone shell.

## License

MIT. See [LICENSE](LICENSE).
