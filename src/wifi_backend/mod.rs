use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use md4::{Digest, Md4};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const SOCKET_PATH: &str = "/run/a26-wifi/control.sock";
const RUN_DIRECTORY: &str = "/run/a26-wifi";
const SERVICE_LOCK: &str = "/run/a26-wifi/service.lock";
const SERVICE_PID: &str = "/run/a26-wifi/service.pid";
const WPA_DIRECTORY: &str = "/run/a26-wifi/wpa";
const WPA_SOCKET: &str = "/run/a26-wifi/wpa/wlan0";
const WPA_PID: &str = "/run/a26-wifi/wpa.pid";
const WPA_CONFIG: &str = "/run/a26-wifi/wpa_supplicant.conf";
const STORE_DIRECTORY: &str = "/var/lib/a26-wifi";
const STORE_PATH: &str = "/var/lib/a26-wifi/networks.json";
const DHCP_SCRIPT: &str = "/opt/a26-system/libexec/a26-udhcpc.script";
const DHCP_PID: &str = "/run/a26-wifi/udhcpc.pid";
const INTERFACE: &str = "wlan0";
const REQUEST_LIMIT: usize = 1024;
const MAX_SSID_BYTES: usize = 32;
const MAX_STORE_BYTES: u64 = 16 * 1024;
const MAX_SAVED_NETWORKS: usize = 64;
const SUPERVISOR_INTERVAL: Duration = Duration::from_secs(1);
// Once connected with a DHCP client running, poll less and never scan in the
// background: each scan costs radio power, and the UI's scan request still
// starts a fresh one.
const SETTLED_INTERVAL: Duration = Duration::from_secs(5);
const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";
const MAX_IDENTITY_BYTES: usize = 128;
const MAX_ENTERPRISE_PASSWORD_CHARS: usize = 128;
const MAX_DOMAIN_BYTES: usize = 253;

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u64,
    command: String,
    #[serde(default)]
    ssid: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    identity: Option<String>,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SavedSecurity {
    Open,
    Wpa2,
    Enterprise,
}

/// WPA2-Enterprise PEAP/MSCHAPv2 credential. Only the MSCHAPv2 NT password
/// hash is kept, never the password, and the server certificate must chain to
/// the system CA bundle and match `domain` (a suffix of its DNS name).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct EnterpriseCredential {
    identity: String,
    nt_hash: [u8; 16],
    domain: String,
}

struct SavedNetwork {
    ssid: String,
    security: SavedSecurity,
    psk: Option<[u8; 32]>,
    enterprise: Option<EnterpriseCredential>,
}

impl Drop for SavedNetwork {
    fn drop(&mut self) {
        if let Some(psk) = self.psk.as_mut() {
            psk.zeroize();
        }
    }
}

struct PendingNetwork {
    _generation: u64,
    network: SavedNetwork,
}

#[derive(Default)]
struct CredentialStore {
    networks: Vec<SavedNetwork>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreDocument {
    version: u32,
    networks: Vec<StoreNetwork>,
}

impl Drop for StoreDocument {
    fn drop(&mut self) {
        for network in &mut self.networks {
            network.zeroize();
        }
    }
}

#[derive(Deserialize, Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct StoreNetwork {
    ssid: String,
    security: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    psk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
}

#[derive(Serialize)]
struct StoreOutput {
    version: u32,
    networks: Vec<StoreNetwork>,
}

impl Drop for StoreOutput {
    fn drop(&mut self) {
        for network in &mut self.networks {
            network.zeroize();
        }
    }
}

#[derive(Default)]
struct SharedState {
    store: CredentialStore,
    desired_ssid: Option<String>,
    connecting_since: Option<Instant>,
    failed_ssids: BTreeSet<String>,
    pending: Option<PendingNetwork>,
    next_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanSecurity {
    Open,
    Wpa2,
    Wpa3,
    Enterprise,
}

struct ScanNetwork {
    ssid: String,
    signal: u8,
    security: ScanSecurity,
}

struct ScanObservation {
    ssid: String,
    signal: u8,
    security: Option<ScanSecurity>,
}

#[derive(Default)]
struct WpaStatus {
    state: String,
    ssid: Option<String>,
    ip_address: Option<String>,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    parse_arguments()?;
    // SAFETY: geteuid has no pointer arguments or memory-safety preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Err("must run as root".into());
    }
    disable_process_dumps()?;

    secure_directory(Path::new(RUN_DIRECTORY))?;
    let _service_lock = acquire_service_lock()?;
    atomic_write(
        Path::new(SERVICE_PID),
        format!("{}\n", std::process::id()).as_bytes(),
        0o600,
    )?;
    secure_directory(Path::new(WPA_DIRECTORY))?;
    secure_directory(Path::new(STORE_DIRECTORY))?;
    let store = CredentialStore::load(Path::new(STORE_PATH))?;
    write_wpa_config(&store, None, None)?;
    ensure_wpa_supplicant()?;
    wpa_cli(&["reconfigure"])?;

    let shared = Arc::new(Mutex::new(SharedState {
        store,
        next_generation: 1,
        ..SharedState::default()
    }));
    stop_owned_pid(Path::new(DHCP_PID), "udhcpc");
    stop_dhcp_and_flush();
    spawn_supervisor(Arc::clone(&shared));

    let socket = Path::new(SOCKET_PATH);
    if socket.exists() {
        fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    eprintln!("a26-wifi-service ready");

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if peer_uid(&stream).ok() != Some(0) {
                    continue;
                }
                handle_connection(&mut stream, &shared);
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    Ok(())
}

fn parse_arguments() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    if let Some(argument) = arguments.next() {
        if matches!(argument.as_str(), "--help" | "-h") && arguments.next().is_none() {
            println!("usage: a26-wifi-service");
            std::process::exit(0);
        }
        return Err("unexpected argument".into());
    }
    Ok(())
}

fn disable_process_dumps() -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: setrlimit reads the complete local rlimit value; prctl is called
    // with PR_SET_DUMPABLE and scalar arguments only.
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } != 0
        || unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn secure_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.uid() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe service directory",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn acquire_service_lock() -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(SERVICE_LOCK)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    // SAFETY: flock operates only on the live descriptor and has no pointer
    // arguments. The returned File keeps the lock for the daemon lifetime.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "service already running",
        ));
    }
    Ok(file)
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: credentials points to writable storage of exactly `length`
    // bytes, and the file descriptor remains valid for the duration of the
    // getsockopt call.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::other("unexpected peer credential length"));
    }
    // SAFETY: a successful getsockopt initialized the full ucred object.
    Ok(unsafe { credentials.assume_init() }.uid)
}

fn handle_connection(stream: &mut UnixStream, shared: &Arc<Mutex<SharedState>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(600)));

    let mut raw = [0_u8; REQUEST_LIMIT + 1];
    let mut used = 0_usize;
    let read_result = loop {
        match stream.read(&mut raw[used..]) {
            Ok(0) => break Ok(()),
            Ok(count) => {
                used += count;
                if used > REQUEST_LIMIT {
                    break Err(());
                }
            }
            Err(_) => break Err(()),
        }
    };
    if read_result.is_err() {
        raw.zeroize();
        return;
    }
    while used > 0 && matches!(raw[used - 1], b'\n' | b'\r') {
        used -= 1;
    }
    let mut request: Request = match serde_json::from_slice(&raw[..used]) {
        Ok(request) => request,
        Err(_) => {
            raw.zeroize();
            return;
        }
    };
    raw.zeroize();

    let id = request.id;
    let result = dispatch(&mut request, shared);
    if let Some(password) = request.password.as_mut() {
        password.zeroize();
    }
    if let Some(identity) = request.identity.as_mut() {
        identity.zeroize();
    }
    let response = match result {
        Ok(result) => json!({"id": id, "ok": true, "result": result}),
        Err(code) => json!({"id": id, "ok": false, "error": {"code": code}}),
    };
    let _ = serde_json::to_writer(&mut *stream, &response);
    let _ = stream.write_all(b"\n");
    let _ = stream.flush();
}

fn dispatch(
    request: &mut Request,
    shared: &Arc<Mutex<SharedState>>,
) -> Result<Value, &'static str> {
    if request.id == 0 {
        return Err("internal");
    }
    let bare = request.ssid.is_none()
        && request.password.is_none()
        && request.identity.is_none()
        && request.domain.is_none();
    let enterprise_fields = request.identity.is_some() || request.domain.is_some();
    match request.command.as_str() {
        "status" if bare => status_result(shared),
        "scan" if bare => scan_result(shared),
        "disconnect" if bare => disconnect_result(shared),
        "connect_enterprise" => {
            let ssid = request.ssid.take().ok_or("network_not_found")?;
            let mut password = Zeroizing::new(request.password.take().unwrap_or_default());
            let identity = Zeroizing::new(request.identity.take().unwrap_or_default());
            let domain = request.domain.take().unwrap_or_default();
            connect_enterprise_result(shared, &ssid, &identity, &mut password, &domain)
        }
        "connect" if !enterprise_fields => {
            let ssid = request.ssid.take().ok_or("network_not_found")?;
            let mut password = request.password.take();
            let result = connect_result(shared, &ssid, password.as_mut());
            if let Some(password) = password.as_mut() {
                password.zeroize();
            }
            result
        }
        _ => Err("unsupported"),
    }
}

fn status_result(shared: &Arc<Mutex<SharedState>>) -> Result<Value, &'static str> {
    if !radio_enabled() {
        return Ok(json!({"radio_enabled": false, "state": "disconnected"}));
    }
    let status = wpa_status().map_err(|_| "internal")?;
    let desired = shared.lock().map_err(|_| "internal")?.desired_ssid.clone();
    if status.state == "COMPLETED" {
        let ssid = status.ssid.filter(|ssid| valid_ssid(ssid));
        if let Some(ssid) = ssid {
            if let Some(desired) = desired
                && desired != ssid
            {
                return Ok(json!({
                    "radio_enabled": true,
                    "state": "connecting",
                    "ssid": desired,
                }));
            }
            let signal = signal_poll().ok();
            let ip = status.ip_address.or_else(interface_ipv4);
            return Ok(json!({
                "radio_enabled": true,
                "state": "connected",
                "ssid": ssid,
                "signal": signal,
                "ip_address": ip,
            }));
        }
    }

    let state = shared.lock().map_err(|_| "internal")?;
    if let (Some(ssid), Some(started)) = (&state.desired_ssid, state.connecting_since)
        && started.elapsed() < Duration::from_secs(40)
    {
        return Ok(json!({
            "radio_enabled": true,
            "state": "connecting",
            "ssid": ssid,
        }));
    }
    Ok(json!({"radio_enabled": true, "state": "disconnected"}))
}

fn scan_result(shared: &Arc<Mutex<SharedState>>) -> Result<Value, &'static str> {
    if !radio_enabled() {
        return Err("radio_disabled");
    }
    // Return the complete cached scan immediately so the UI stays within its
    // 750 ms protocol deadline, then start a fresh asynchronous scan for the
    // next refresh. Samsung's SCSC scan commonly takes several seconds.
    let raw = wpa_cli(&["scan_results"]).map_err(|_| "internal")?;
    let _ = wpa_cli(&["scan"]);
    let current = wpa_status().ok().and_then(|status| status.ssid);
    let state = shared.lock().map_err(|_| "internal")?;
    let networks: Vec<Value> = aggregate_scan_results(&raw)
        .into_iter()
        .take(64)
        .map(|network| {
            let saved = state
                .store
                .find_matching(&network.ssid, network.security)
                .is_some()
                && !state.failed_ssids.contains(&network.ssid);
            json!({
                "ssid": network.ssid,
                "signal": network.signal,
                "security": scan_security_name(network.security),
                "requires_password": network.security != ScanSecurity::Open && !saved,
                "connected": current.as_deref() == Some(network.ssid.as_str()),
            })
        })
        .collect();
    Ok(json!({"networks": networks}))
}

fn scanned_security(ssid: &str) -> Result<ScanSecurity, &'static str> {
    if !radio_enabled() {
        return Err("radio_disabled");
    }
    if !valid_ssid(ssid) {
        return Err("network_not_found");
    }
    let raw_scan = wpa_cli(&["scan_results"]).map_err(|_| "internal")?;
    aggregate_scan_results(&raw_scan)
        .into_iter()
        .find(|network| network.ssid == ssid)
        .map(|network| network.security)
        .ok_or("network_not_found")
}

fn lock_idle(
    shared: &Arc<Mutex<SharedState>>,
) -> Result<MutexGuard<'_, SharedState>, &'static str> {
    let state = shared.lock().map_err(|_| "busy")?;
    if state.pending.is_some() || state.connecting_since.is_some() {
        return Err("busy");
    }
    Ok(state)
}

fn connect_result(
    shared: &Arc<Mutex<SharedState>>,
    ssid: &str,
    password: Option<&mut String>,
) -> Result<Value, &'static str> {
    let security = scanned_security(ssid)?;
    let state = lock_idle(shared)?;

    let candidate = if let Some(password) = password {
        if !valid_password(password) {
            password.zeroize();
            return Err("authentication_failed");
        }
        match security {
            ScanSecurity::Enterprise | ScanSecurity::Wpa3 | ScanSecurity::Open => {
                password.zeroize();
                return Err("unsupported");
            }
            ScanSecurity::Wpa2 => {
                let derived = derive_psk(ssid.as_bytes(), password);
                password.zeroize();
                let psk = derived?;
                SavedNetwork {
                    ssid: ssid.to_owned(),
                    security: SavedSecurity::Wpa2,
                    psk: Some(psk),
                    enterprise: None,
                }
            }
        }
    } else if let Some(saved) = state.store.find_matching(ssid, security) {
        copy_saved_network(saved)
    } else {
        match security {
            ScanSecurity::Open => SavedNetwork {
                ssid: ssid.to_owned(),
                security: SavedSecurity::Open,
                psk: None,
                enterprise: None,
            },
            ScanSecurity::Enterprise | ScanSecurity::Wpa3 => return Err("unsupported"),
            ScanSecurity::Wpa2 => return Err("authentication_failed"),
        }
    };
    begin_connect(shared, state, ssid, candidate)
}

/// PEAP/MSCHAPv2 with server validation against the system CA bundle and the
/// given domain. The password is reduced to its NT hash at once and wiped.
fn connect_enterprise_result(
    shared: &Arc<Mutex<SharedState>>,
    ssid: &str,
    identity: &str,
    password: &mut String,
    domain: &str,
) -> Result<Value, &'static str> {
    let valid =
        valid_identity(identity) && valid_enterprise_password(password) && valid_domain(domain);
    let nt_hash = valid.then(|| nt_password_hash(password));
    password.zeroize();
    let nt_hash = nt_hash.ok_or("authentication_failed")?;
    if scanned_security(ssid)? != ScanSecurity::Enterprise {
        return Err("unsupported");
    }
    let state = lock_idle(shared)?;
    let candidate = SavedNetwork {
        ssid: ssid.to_owned(),
        security: SavedSecurity::Enterprise,
        psk: None,
        enterprise: Some(EnterpriseCredential {
            identity: identity.to_owned(),
            nt_hash,
            domain: domain.to_owned(),
        }),
    };
    begin_connect(shared, state, ssid, candidate)
}

fn begin_connect(
    shared: &Arc<Mutex<SharedState>>,
    mut state: MutexGuard<'_, SharedState>,
    ssid: &str,
    candidate: SavedNetwork,
) -> Result<Value, &'static str> {
    write_wpa_config(&state.store, Some(&candidate), Some(ssid)).map_err(|_| "internal")?;
    let generation = state.next_generation;
    state.next_generation = state.next_generation.wrapping_add(1).max(1);
    state.desired_ssid = Some(ssid.to_owned());
    state.connecting_since = Some(Instant::now());
    state.failed_ssids.remove(ssid);
    state.pending = Some(PendingNetwork {
        _generation: generation,
        network: candidate,
    });
    drop(state);

    stop_dhcp_and_flush();
    if wpa_cli(&["reconfigure"]).is_err() {
        cancel_pending(shared, false);
        return Err("internal");
    }
    let _ = wpa_cli(&["reassociate"]);
    Ok(json!({"radio_enabled": true, "state": "connecting", "ssid": ssid}))
}

fn disconnect_result(shared: &Arc<Mutex<SharedState>>) -> Result<Value, &'static str> {
    wpa_cli(&["disconnect"]).map_err(|_| "internal")?;
    stop_dhcp_and_flush();
    if let Ok(mut state) = shared.lock() {
        state.desired_ssid = None;
        state.connecting_since = None;
        state.pending = None;
        let _ = write_wpa_config(&state.store, None, None);
    }
    Ok(json!({"radio_enabled": radio_enabled(), "state": "disconnected"}))
}

impl CredentialStore {
    fn load(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != 0
            || metadata.len() > MAX_STORE_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe credential store",
            ));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
        file.read_to_end(&mut bytes)?;
        let mut document: StoreDocument = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid credential store"))?;
        if document.version != 1 || document.networks.len() > MAX_SAVED_NETWORKS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid credential store version",
            ));
        }
        let mut networks = Vec::with_capacity(document.networks.len());
        let mut seen = BTreeSet::new();
        for mut entry in document.networks.drain(..) {
            if !valid_ssid(&entry.ssid) || !seen.insert(entry.ssid.clone()) {
                entry.zeroize();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid or duplicate SSID",
                ));
            }
            let security = match entry.security.as_str() {
                "open" if entry.psk.is_none() => SavedSecurity::Open,
                "wpa2" => SavedSecurity::Wpa2,
                "enterprise" if entry.psk.is_none() => SavedSecurity::Enterprise,
                _ => {
                    entry.zeroize();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid security",
                    ));
                }
            };
            let psk = match entry.psk.as_deref() {
                Some(value) => Some(
                    parse_psk(value)
                        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid PSK"))?,
                ),
                None if security != SavedSecurity::Wpa2 => None,
                None => {
                    entry.zeroize();
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "missing PSK"));
                }
            };
            let enterprise = match (
                security,
                entry.identity.as_deref(),
                entry.password_hash.as_deref(),
                entry.domain.as_deref(),
            ) {
                (SavedSecurity::Enterprise, Some(identity), Some(hash), Some(domain))
                    if valid_identity(identity) && valid_domain(domain) =>
                {
                    let Some(nt_hash) = parse_hex_array::<16>(hash) else {
                        entry.zeroize();
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid password hash",
                        ));
                    };
                    Some(EnterpriseCredential {
                        identity: identity.to_owned(),
                        nt_hash,
                        domain: domain.to_owned(),
                    })
                }
                (SavedSecurity::Open | SavedSecurity::Wpa2, None, None, None) => None,
                _ => {
                    entry.zeroize();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid enterprise credential",
                    ));
                }
            };
            networks.push(SavedNetwork {
                ssid: entry.ssid.clone(),
                security,
                psk,
                enterprise,
            });
            entry.zeroize();
        }
        Ok(Self { networks })
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        let output = StoreOutput {
            version: 1,
            networks: self
                .networks
                .iter()
                .map(|network| StoreNetwork {
                    ssid: network.ssid.clone(),
                    security: match network.security {
                        SavedSecurity::Open => "open".into(),
                        SavedSecurity::Wpa2 => "wpa2".into(),
                        SavedSecurity::Enterprise => "enterprise".into(),
                    },
                    psk: network.psk.as_ref().map(psk_hex),
                    identity: network.enterprise.as_ref().map(|e| e.identity.clone()),
                    password_hash: network.enterprise.as_ref().map(|e| bytes_hex(&e.nt_hash)),
                    domain: network.enterprise.as_ref().map(|e| e.domain.clone()),
                })
                .collect(),
        };
        let mut bytes = Zeroizing::new(
            serde_json::to_vec_pretty(&output)
                .map_err(|_| io::Error::other("cannot serialize credential store"))?,
        );
        bytes.push(b'\n');
        atomic_write(path, &bytes, 0o600)?;
        Ok(())
    }

    fn find_matching(&self, ssid: &str, security: ScanSecurity) -> Option<&SavedNetwork> {
        let expected = match security {
            ScanSecurity::Open => SavedSecurity::Open,
            ScanSecurity::Wpa2 => SavedSecurity::Wpa2,
            ScanSecurity::Enterprise => SavedSecurity::Enterprise,
            ScanSecurity::Wpa3 => return None,
        };
        self.networks
            .iter()
            .find(|network| network.ssid == ssid && network.security == expected)
    }

    fn upsert(&mut self, network: SavedNetwork) -> io::Result<()> {
        self.networks
            .retain(|existing| existing.ssid != network.ssid);
        if self.networks.len() >= MAX_SAVED_NETWORKS {
            return Err(io::Error::other("credential store is full"));
        }
        self.networks.push(network);
        Ok(())
    }
}

fn copy_saved_network(network: &SavedNetwork) -> SavedNetwork {
    SavedNetwork {
        ssid: network.ssid.clone(),
        security: network.security,
        psk: network.psk,
        enterprise: network.enterprise.clone(),
    }
}

fn write_wpa_config(
    store: &CredentialStore,
    candidate: Option<&SavedNetwork>,
    target: Option<&str>,
) -> io::Result<()> {
    let mut config = Zeroizing::new(String::from(
        "ctrl_interface=/run/a26-wifi/wpa\nupdate_config=0\nap_scan=1\n\n",
    ));
    if let Some(candidate) = candidate {
        append_wpa_network(&mut config, candidate, 100)?;
    } else {
        for network in &store.networks {
            let priority = if target == Some(network.ssid.as_str()) {
                100
            } else {
                10
            };
            append_wpa_network(&mut config, network, priority)?;
        }
    }
    atomic_write(Path::new(WPA_CONFIG), config.as_bytes(), 0o600)
}

fn append_wpa_network(config: &mut String, network: &SavedNetwork, priority: u8) -> io::Result<()> {
    config.push_str("network={\n    ssid=");
    config.push_str(&bytes_hex(network.ssid.as_bytes()));
    config.push('\n');
    match (
        network.security,
        network.psk.as_ref(),
        network.enterprise.as_ref(),
    ) {
        (SavedSecurity::Open, None, None) => config.push_str("    key_mgmt=NONE\n"),
        (SavedSecurity::Enterprise, None, Some(credential)) => {
            // The identity is written hex-encoded, so no byte needs quoting;
            // the domain was validated to DNS characters.
            config.push_str("    key_mgmt=WPA-EAP\n    eap=PEAP\n    identity=");
            config.push_str(&Zeroizing::new(bytes_hex(credential.identity.as_bytes())));
            config.push_str("\n    password=hash:");
            config.push_str(&Zeroizing::new(bytes_hex(&credential.nt_hash)));
            config.push_str("\n    phase2=\"auth=MSCHAPV2\"\n    ca_cert=\"");
            config.push_str(CA_BUNDLE);
            config.push_str("\"\n    domain_suffix_match=\"");
            config.push_str(&credential.domain);
            config.push_str("\"\n    proto=RSN\n    pairwise=CCMP\n    group=CCMP\n");
        }
        (SavedSecurity::Wpa2, Some(psk), None) => {
            config.push_str("    psk=");
            let hex = Zeroizing::new(psk_hex(psk));
            config.push_str(&hex);
            config.push_str(
                "\n    key_mgmt=WPA-PSK\n    proto=RSN\n    pairwise=CCMP\n    group=CCMP\n",
            );
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid network",
            ));
        }
    }
    config.push_str(&format!("    priority={priority}\n}}\n\n"));
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = PathBuf::from(format!("{}.new.{}", path.display(), std::process::id()));
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    File::open(parent)?.sync_all()
}

fn derive_psk(ssid: &[u8], password: &str) -> Result<[u8; 32], &'static str> {
    if password.len() == 64 && password.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return parse_psk(password).ok_or("authentication_failed");
    }
    let mut psk = [0_u8; 32];
    pbkdf2_hmac::<Sha1>(password.as_bytes(), ssid, 4096, &mut psk);
    Ok(psk)
}

fn parse_psk(value: &str) -> Option<[u8; 32]> {
    parse_hex_array::<32>(value)
}

/// MSCHAPv2 NtPasswordHash (RFC 2759): MD4 of the UTF-16LE password.
fn nt_password_hash(password: &str) -> [u8; 16] {
    let mut encoded = Zeroizing::new(Vec::with_capacity(password.len() * 2));
    for unit in password.encode_utf16() {
        encoded.extend_from_slice(&unit.to_le_bytes());
    }
    let mut hash = [0_u8; 16];
    hash.copy_from_slice(&Md4::digest(&encoded[..]));
    hash
}

fn parse_hex_array<const N: usize>(value: &str) -> Option<[u8; N]> {
    let input = value.as_bytes();
    if input.len() != N * 2 || !input.is_ascii() {
        return None;
    }
    let mut output = [0_u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = (hex_value(input[index * 2])? << 4) | hex_value(input[index * 2 + 1])?;
    }
    Some(output)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn psk_hex(psk: &[u8; 32]) -> String {
    bytes_hex(psk)
}

fn bytes_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn valid_ssid(ssid: &str) -> bool {
    !ssid.is_empty() && ssid.len() <= MAX_SSID_BYTES && !ssid.contains(['\n', '\r', '\0'])
}

fn valid_password(password: &str) -> bool {
    let bytes = password.as_bytes();
    (bytes.len() == 64 && bytes.iter().all(u8::is_ascii_hexdigit))
        || ((8..=63).contains(&bytes.len()) && bytes.iter().all(|byte| matches!(byte, b' '..=b'~')))
}

fn valid_identity(identity: &str) -> bool {
    (1..=MAX_IDENTITY_BYTES).contains(&identity.len())
        && identity.bytes().all(|byte| matches!(byte, b' '..=b'~'))
}

fn valid_enterprise_password(password: &str) -> bool {
    !password.is_empty()
        && password.chars().count() <= MAX_ENTERPRISE_PASSWORD_CHARS
        && !password.chars().any(char::is_control)
}

/// A DNS name (suffix) for domain_suffix_match: at least two lowercase labels
/// of letters, digits and inner hyphens.
fn valid_domain(domain: &str) -> bool {
    domain.len() <= MAX_DOMAIN_BYTES
        && domain.split('.').count() >= 2
        && domain.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn radio_enabled() -> bool {
    fs::read_to_string("/sys/class/rfkill/rfkill0/state")
        .map(|state| state.trim() == "1")
        .unwrap_or_else(|_| Path::new("/sys/class/net/wlan0").exists())
}

fn ensure_wpa_supplicant() -> io::Result<()> {
    if owned_pid(Path::new(WPA_PID), "wpa_supplicant").is_some()
        && wpa_cli(&["ping"]).is_ok_and(|response| response.trim() == "PONG")
    {
        return Ok(());
    }
    stop_owned_pid(Path::new(WPA_PID), "wpa_supplicant");
    if Path::new(WPA_SOCKET).exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "unowned wpa_supplicant control socket",
        ));
    }
    let _ = fs::remove_dir_all(WPA_DIRECTORY);
    secure_directory(Path::new(WPA_DIRECTORY))?;
    let status = run_bounded_status(
        "/sbin/wpa_supplicant",
        &[
            "-B", "-P", WPA_PID, "-i", INTERFACE, "-D", "nl80211", "-c", WPA_CONFIG,
        ],
        Duration::from_secs(4),
    )?;
    if !status.success() {
        return Err(io::Error::other("wpa_supplicant startup failed"));
    }
    for _ in 0..20 {
        if Path::new(WPA_SOCKET).exists()
            && wpa_cli(&["ping"]).is_ok_and(|response| response.trim() == "PONG")
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other(
        "wpa_supplicant control socket unavailable",
    ))
}

fn wpa_cli(arguments: &[&str]) -> io::Result<String> {
    let mut child = Command::new("/sbin/wpa_cli")
        .args(["-p", WPA_DIRECTORY, "-i", INTERFACE])
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let status = wait_bounded(&mut child, Duration::from_secs(2))?;
    if !status.success() {
        return Err(io::Error::other("wpa control request failed"));
    }
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing wpa response"))?
        .take(256 * 1024)
        .read_to_end(&mut stdout)?;
    String::from_utf8(stdout).map_err(|_| io::Error::other("invalid wpa response"))
}

fn wpa_status() -> io::Result<WpaStatus> {
    let raw = wpa_cli(&["status"])?;
    let fields: BTreeMap<&str, &str> = raw
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    Ok(WpaStatus {
        state: fields.get("wpa_state").copied().unwrap_or("").to_owned(),
        ssid: fields
            .get("ssid")
            .copied()
            .filter(|ssid| valid_ssid(ssid))
            .map(str::to_owned),
        ip_address: fields
            .get("ip_address")
            .copied()
            .filter(|value| value.len() <= 64)
            .map(str::to_owned),
    })
}

fn signal_poll() -> io::Result<u8> {
    let raw = wpa_cli(&["signal_poll"])?;
    let rssi = raw
        .lines()
        .find_map(|line| line.strip_prefix("RSSI="))
        .and_then(|value| value.parse::<i32>().ok())
        .ok_or_else(|| io::Error::other("RSSI unavailable"))?;
    Ok(dbm_to_percent(rssi))
}

fn interface_ipv4() -> Option<String> {
    let output = Command::new("/sbin/ip")
        .args(["-4", "-o", "addr", "show", "dev", INTERFACE])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace()
        .skip_while(|field| *field != "inet")
        .nth(1)
        .and_then(|field| field.split('/').next())
        .filter(|value| value.len() <= 64)
        .map(str::to_owned)
}

fn parse_scan_results(raw: &str) -> Vec<ScanObservation> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let _bssid = fields.next()?;
            let _frequency = fields.next()?;
            let dbm = fields.next()?.parse::<i32>().ok()?;
            let flags = fields.next()?;
            let ssid = fields.next()?.to_owned();
            if !valid_ssid(&ssid) {
                return None;
            }
            Some(ScanObservation {
                ssid,
                signal: dbm_to_percent(dbm),
                security: classify_security(flags),
            })
        })
        .collect()
}

fn classify_security(flags: &str) -> Option<ScanSecurity> {
    if flags.contains("EAP") || flags.contains("WEP") {
        Some(ScanSecurity::Enterprise)
    } else if flags.contains("SAE") {
        Some(ScanSecurity::Wpa3)
    } else if flags.contains("PSK") {
        Some(ScanSecurity::Wpa2)
    } else if explicitly_open(flags) {
        Some(ScanSecurity::Open)
    } else {
        None
    }
}

fn explicitly_open(flags: &str) -> bool {
    let mut saw_ess = false;
    let mut rest = flags;
    while let Some(open) = rest.find('[') {
        let tail = &rest[open + 1..];
        let Some(close) = tail.find(']') else {
            return false;
        };
        match &tail[..close] {
            "ESS" => saw_ess = true,
            "WPS" => {}
            _ => return false,
        }
        rest = &tail[close + 1..];
    }
    saw_ess && rest.is_empty()
}

fn aggregate_scan_results(raw: &str) -> Vec<ScanNetwork> {
    let mut observations: BTreeMap<String, (u8, Option<ScanSecurity>, bool)> = BTreeMap::new();
    for observation in parse_scan_results(raw) {
        let entry = observations.entry(observation.ssid).or_insert((
            observation.signal,
            observation.security,
            observation.security.is_none(),
        ));
        entry.0 = entry.0.max(observation.signal);
        if observation.security.is_none() || entry.1 != observation.security {
            entry.2 = true;
        }
    }
    observations
        .into_iter()
        .filter_map(|(ssid, (signal, security, ambiguous))| {
            (!ambiguous).then_some(ScanNetwork {
                ssid,
                signal,
                security: security?,
            })
        })
        .collect()
}

fn scan_security_name(security: ScanSecurity) -> &'static str {
    match security {
        ScanSecurity::Open => "open",
        ScanSecurity::Wpa2 => "wpa2",
        ScanSecurity::Wpa3 => "wpa3",
        ScanSecurity::Enterprise => "enterprise",
    }
}

fn dbm_to_percent(dbm: i32) -> u8 {
    (((dbm + 100).clamp(0, 70) * 100) / 70) as u8
}

fn spawn_supervisor(shared: Arc<Mutex<SharedState>>) {
    thread::spawn(move || {
        let mut dhcp: Option<Child> = None;
        let mut scan_ticks = 15_u8;
        loop {
            if ensure_wpa_supplicant().is_err() {
                thread::sleep(SUPERVISOR_INTERVAL);
                continue;
            }
            let status = wpa_status().unwrap_or_default();
            let completed_ssid = (status.state == "COMPLETED")
                .then_some(status.ssid)
                .flatten();
            if completed_ssid.is_some() {
                scan_ticks = 15;
            } else if scan_ticks >= 15 {
                let _ = wpa_cli(&["scan"]);
                scan_ticks = 0;
            } else {
                scan_ticks += 1;
            }
            let desired = shared
                .lock()
                .ok()
                .and_then(|state| state.desired_ssid.clone());
            let target_completed = match (&desired, &completed_ssid) {
                (Some(desired), Some(completed)) => desired == completed,
                (None, Some(_)) => true,
                _ => false,
            };
            let mut interval = SUPERVISOR_INTERVAL;
            if target_completed {
                commit_completed_network(&shared, completed_ssid.as_deref());
                let exited = dhcp
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
                    .is_some();
                if exited {
                    dhcp = None;
                }
                if dhcp.is_none() {
                    dhcp = start_dhcp().ok();
                }
                let settled = desired.is_none()
                    && dhcp.is_some()
                    && shared.lock().is_ok_and(|state| state.pending.is_none());
                if settled {
                    interval = SETTLED_INTERVAL;
                }
            } else {
                let timed_out = shared.lock().is_ok_and(|state| {
                    state
                        .connecting_since
                        .is_some_and(|started| started.elapsed() >= Duration::from_secs(40))
                });
                if timed_out {
                    cancel_pending(&shared, true);
                }
                if let Some(mut child) = dhcp.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            thread::sleep(interval);
        }
    });
}

fn start_dhcp() -> io::Result<Child> {
    let _ = fs::remove_file(DHCP_PID);
    Command::new("/sbin/udhcpc")
        .args([
            "-f",
            "-i",
            INTERFACE,
            "-p",
            DHCP_PID,
            "-s",
            DHCP_SCRIPT,
            "-t",
            "8",
            "-T",
            "2",
            "-A",
            "5",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn commit_completed_network(shared: &Arc<Mutex<SharedState>>, completed_ssid: Option<&str>) {
    let Ok(mut state) = shared.lock() else {
        return;
    };
    if let Some(pending) = state.pending.take() {
        if completed_ssid != Some(pending.network.ssid.as_str()) {
            state.pending = Some(pending);
            return;
        }
        let ssid = pending.network.ssid.clone();
        if state.store.upsert(pending.network).is_err()
            || state.store.save(Path::new(STORE_PATH)).is_err()
            || write_wpa_config(&state.store, None, Some(&ssid)).is_err()
        {
            state.failed_ssids.insert(ssid);
            return;
        }
        state.failed_ssids.remove(&ssid);
    } else if let Some(ssid) = completed_ssid {
        state.failed_ssids.remove(ssid);
    }
    state.desired_ssid = None;
    state.connecting_since = None;
}

fn cancel_pending(shared: &Arc<Mutex<SharedState>>, mark_failed: bool) {
    let Ok(mut state) = shared.lock() else {
        return;
    };
    state.pending = None;
    if let Some(ssid) = state.desired_ssid.take()
        && mark_failed
    {
        state.failed_ssids.insert(ssid);
    }
    state.connecting_since = None;
    let _ = write_wpa_config(&state.store, None, None);
    drop(state);
    stop_dhcp_and_flush();
    let _ = wpa_cli(&["reconfigure"]);
}

fn owned_pid(path: &Path, expected_name: &str) -> Option<i32> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.uid() != 0 || metadata.len() > 32 {
        return None;
    }
    let pid = fs::read_to_string(path).ok()?.trim().parse::<i32>().ok()?;
    if pid <= 1 {
        return None;
    }
    let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    (comm.trim() == expected_name).then_some(pid)
}

fn stop_owned_pid(path: &Path, expected_name: &str) {
    if let Some(pid) = owned_pid(path, expected_name) {
        // SAFETY: kill sends a signal to a positive PID validated through the
        // root-owned pidfile and /proc; it does not dereference memory.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        for _ in 0..40 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if unsafe { libc::kill(pid, 0) } == 0 {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
    let _ = fs::remove_file(path);
}

fn run_bounded_status(
    program: &str,
    arguments: &[&str],
    timeout: Duration,
) -> io::Result<std::process::ExitStatus> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_bounded(&mut child, timeout)
}

fn wait_bounded(child: &mut Child, timeout: Duration) -> io::Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop_dhcp_and_flush() {
    stop_owned_pid(Path::new(DHCP_PID), "udhcpc");
    let _ = Command::new("/sbin/ip")
        .args(["-4", "addr", "flush", "dev", INTERFACE])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    loop {
        let removed = Command::new("/sbin/ip")
            .args(["rule", "del", "priority", "10550"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !removed {
            break;
        }
    }
    if let Some(table) = android_wifi_table() {
        let _ = Command::new("/sbin/ip")
            .args(["route", "flush", "table", &table.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn android_wifi_table() -> Option<u32> {
    let index = fs::read_to_string("/sys/class/net/wlan0/ifindex")
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    Some(1000 + index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_validation_matches_protocol() {
        assert!(valid_password("12345678"));
        assert!(valid_password(&"a".repeat(63)));
        assert!(valid_password(&"ab".repeat(32)));
        assert!(!valid_password("short"));
        assert!(!valid_password(&"g".repeat(64)));
    }

    #[test]
    fn psk_derivation_matches_wpa_test_vector() {
        let psk = derive_psk(b"IEEE", "password").unwrap();
        assert_eq!(
            psk_hex(&psk),
            "f42c6fc52df0ebef9ebb4b90b38a5f90\
             2e83fe1b135a70e23aed762e9710a12e"
                .replace(char::is_whitespace, "")
        );
    }

    #[test]
    fn scan_parser_classifies_and_bounds_signal() {
        let raw = "bssid / frequency / signal level / flags / ssid\n\
                   00:11:22:33:44:55\t2412\t-50\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                   00:11:22:33:44:56\t2412\t-120\t[ESS]\tGuest\n";
        let networks = parse_scan_results(raw);
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].security, Some(ScanSecurity::Wpa2));
        assert_eq!(networks[1].security, Some(ScanSecurity::Open));
        assert_eq!(networks[1].signal, 0);
    }

    #[test]
    fn ambiguous_or_unknown_security_is_omitted() {
        let raw = "bssid / frequency / signal level / flags / ssid\n\
                   00:11:22:33:44:55\t2412\t-30\t[ESS]\tConflict\n\
                   00:11:22:33:44:56\t2412\t-60\t[WPA2-PSK-CCMP][ESS]\tConflict\n\
                   00:11:22:33:44:57\t2412\t-40\t[OWE][ESS]\tUnknown\n\
                   00:11:22:33:44:58\t2412\t-50\t[WPA2-PSK-CCMP][ESS]\tSafe\n";
        let networks = aggregate_scan_results(raw);
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].ssid, "Safe");
        assert_eq!(networks[0].security, ScanSecurity::Wpa2);
    }

    #[test]
    fn nt_password_hash_matches_rfc2759_vector() {
        assert_eq!(
            bytes_hex(&nt_password_hash("clientPass")),
            "44ebba8d5312b8d611474411f56989ae"
        );
    }

    #[test]
    fn enterprise_inputs_are_validated() {
        assert!(valid_identity("utorid@utoronto.ca"));
        assert!(!valid_identity(""));
        assert!(!valid_identity("bad\nidentity"));
        assert!(valid_enterprise_password("p\u{e4}ss word!"));
        assert!(!valid_enterprise_password(""));
        assert!(!valid_enterprise_password("line\nbreak"));
        assert!(valid_domain("radius.wireless.utoronto.ca"));
        assert!(!valid_domain("utoronto"));
        assert!(!valid_domain("Radius.example.org"));
        assert!(!valid_domain("bad\"quote.org"));
        assert!(!valid_domain("-bad.org"));
    }

    fn enterprise_network(domain: &str) -> SavedNetwork {
        SavedNetwork {
            ssid: "UofT".into(),
            security: SavedSecurity::Enterprise,
            psk: None,
            enterprise: Some(EnterpriseCredential {
                identity: "student".into(),
                nt_hash: nt_password_hash("secret password"),
                domain: domain.into(),
            }),
        }
    }

    #[test]
    fn enterprise_config_uses_hash_and_server_validation() {
        let mut config = String::new();
        append_wpa_network(
            &mut config,
            &enterprise_network("radius.wireless.utoronto.ca"),
            10,
        )
        .unwrap();
        assert!(!config.contains("secret password"));
        assert!(!config.contains("student"));
        assert!(config.contains("identity=73747564656e74\n"));
        assert!(config.contains("password=hash:"));
        assert!(config.contains("key_mgmt=WPA-EAP\n"));
        assert!(config.contains(&format!("ca_cert=\"{CA_BUNDLE}\"")));
        assert!(config.contains("domain_suffix_match=\"radius.wireless.utoronto.ca\""));
    }

    #[test]
    fn enterprise_store_serializes_without_plaintext() {
        let directory = std::env::temp_dir().join(format!("a26-wifi-test-{}", std::process::id()));
        let path = directory.join("networks.json");
        let store = CredentialStore {
            networks: vec![enterprise_network("utoronto.ca")],
        };
        store.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_dir_all(&directory);
        assert!(!text.contains("secret password"));
        let document: StoreDocument = serde_json::from_str(&text).unwrap();
        assert_eq!(document.networks[0].security, "enterprise");
        assert_eq!(document.networks[0].domain.as_deref(), Some("utoronto.ca"));
        assert_eq!(
            document.networks[0].password_hash.as_deref(),
            Some(bytes_hex(&nt_password_hash("secret password")).as_str())
        );
    }

    #[test]
    fn config_contains_only_derived_key() {
        let psk = derive_psk(b"Test", "correct horse").unwrap();
        let store = CredentialStore {
            networks: vec![SavedNetwork {
                ssid: "Test".into(),
                security: SavedSecurity::Wpa2,
                psk: Some(psk),
                enterprise: None,
            }],
        };
        let mut rendered = String::new();
        rendered.push_str("ssid=");
        rendered.push_str(&bytes_hex(store.networks[0].ssid.as_bytes()));
        rendered.push_str("\npsk=");
        rendered.push_str(&psk_hex(store.networks[0].psk.as_ref().unwrap()));
        assert!(!rendered.contains("correct horse"));
        assert!(rendered.contains("ssid=54657374"));
    }
}
