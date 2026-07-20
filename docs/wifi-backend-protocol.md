# A26 Wi-Fi backend protocol, version 1

`a26-system` is a presentation-only client, even when the launcher runs it as
UID 0 to access this root-only endpoint. It does not invoke `iw`, `wpa_cli`,
NetworkManager, or any other privileged networking tool. A separately
installed, root-owned backend is responsible for scanning and link management.
This repository intentionally does not provide that backend.

## Transport and access control

- The backend listens on `/run/a26-wifi/control.sock`.
- `/run/a26-wifi` must be owned by `root:root` with mode `0700`; the socket must
  be owned by `root:root` with mode `0600`.
- The backend must check Linux `SO_PEERCRED` and accept only UID 0. Filesystem
  mode is not a substitute for the peer-credential check.
- The client opens one Unix stream connection for one request, writes one
  newline-terminated UTF-8 JSON object, shuts down its write side, reads one
  response, and closes. The backend closes after sending its response.
- The UI accepts at most 64 KiB per response and uses 750 ms read/write
  timeouts. SSIDs are non-empty UTF-8 strings of at most 32 bytes. A scan may
  return at most 64 entries. Unknown enums, out-of-range values, malformed
  JSON, and mismatched response IDs are rejected.

The backend should bound requests to 1 KiB. Protocol version 1 has no request
larger than that.

## Envelope

Every request contains a nonzero unsigned `id` and a `command`. The response
copies that ID exactly.

Successful response:

```json
{"id":1,"ok":true,"result":{}}
```

Rejected response:

```json
{"id":1,"ok":false,"error":{"code":"busy"}}
```

Allowed error codes are:

- `authentication_failed`
- `network_not_found`
- `radio_disabled`
- `busy`
- `unsupported`
- `internal`

The UI maps codes to fixed user-facing text. It deliberately ignores an error
`message`, if supplied, so arbitrary backend text—and especially a mistakenly
included credential—cannot enter UI state or diagnostics.

## Commands

### `status`

Request:

```json
{"id":1,"command":"status"}
```

Disconnected result:

```json
{"id":1,"ok":true,"result":{"radio_enabled":true,"state":"disconnected"}}
```

Connecting and connected results include `ssid`. A connected result may also
include `signal` (integer 0 through 100) and `ip_address` (string or null):

```json
{"id":1,"ok":true,"result":{"radio_enabled":true,"state":"connected","ssid":"A26 Lab","signal":87,"ip_address":"192.0.2.7"}}
```

`state` is `disconnected`, `connecting`, `connected`, or `disconnecting`. When
`radio_enabled` is false, the backend should report `disconnected`.

**A status result must never contain a password, PSK, SAE password, saved
credential, key-material identifier, or backend command line.**

### `scan`

Request:

```json
{"id":2,"command":"scan"}
```

Result:

```json
{"id":2,"ok":true,"result":{"networks":[{"ssid":"A26 Lab","signal":87,"security":"wpa3","requires_password":true,"connected":false},{"ssid":"Guest","signal":61,"security":"open","requires_password":false,"connected":false}]}}
```

`security` is `open`, `wpa2`, `wpa3`, or `enterprise`. The current UI labels
enterprise networks as unsupported. `requires_password` is false for open
networks and may also be false when the backend has a credential in its own
root-only credential store. A scan result never describes or returns that
credential. The UI de-duplicates SSIDs and sorts connected first, then by
descending signal and SSID.

### `connect`

Open or backend-known network:

```json
{"id":3,"command":"connect","ssid":"Guest"}
```

WPA Personal network:

```json
{"id":3,"command":"connect","ssid":"A26 Lab","password":"example only"}
```

The password is 8 through 63 printable ASCII bytes, or exactly 64 hexadecimal
digits. Success returns the same result shape as `status`; failure uses an
error code. A response must not echo the request or credential.

The password field is the only protocol location in which a credential is
allowed. The backend must:

1. never place it in process arguments, environment variables, logs, crash
   reports, status/scan responses, or other observable state;
2. avoid spawning a networking tool with the credential on its command line;
3. retain it only as long as needed by the chosen privileged API, wipe mutable
   buffers afterward, and use a root-only credential store if persistence is
   intentionally supported; and
4. ensure request/response tracing is disabled or structurally redacts the
   `password` field before formatting.

The UI serializes this request directly to the connected socket rather than
building a command line, environment variable, log record, or persistent JSON
blob. Its entry and worker buffers are non-cloneable, debug-redacted, and
zeroized on clear, cancel, submit completion, normal model teardown, or view
exit.

### `disconnect`

Request:

```json
{"id":4,"command":"disconnect"}
```

Success returns the same result shape as `status`, normally `disconnected`.

## Backend implementation boundary

Backend policy—radio control, saved-network handling, association timeouts,
DHCP, routing, and DNS—is outside this UI process. The backend should serialize
mutating operations, return `busy` rather than queueing unbounded work, merge
multiple BSSIDs for one SSID, and use fixed error codes. The UI's worker thread
keeps all socket waits off the X11 event loop, but its timeout is not a
network-association deadline; the backend is expected to acknowledge a
transition quickly and expose progress through `status`.
