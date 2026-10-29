//! The event loop's half of attachments (WP-D; PAR-237–242): the model has
//! no I/O, so it asks for a `Job` and gets a `Done` back. Each job runs on a
//! thread of its own: listing a folder for the file browser, reading and
//! preparing a chosen file (`attachments::prepare`), or asking X's
//! CLIPBOARD for an image. An eventfd the loop polls says one finished.
//!
//! The browser shows the phone's own files: the chroot OSS Phone runs in
//! (Android's shared storage stays locked: Android never unlocks its user's
//! credential-encrypted storage under Moon). A pasted image is asked of
//! whichever app owns CLIPBOARD; Moon's clipboard keeps text only, and an
//! app in the background is frozen, so an image copied elsewhere usually
//! cannot be had: the answer then says so.

use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use super::attachments::{self, Attachment};
use super::line::Waker;

/// A folder lists at most this many entries.
pub const MAX_ENTRIES: usize = 4000;

/// What the event loop is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// The entries of a folder.
    List { dir: PathBuf },
    /// A file, as an attachment of draft `key`.
    Read { key: String, path: PathBuf },
    /// The clipboard's image, as an attachment of draft `key` named `name`;
    /// `menu`: the edit menu's Paste found no text (none: "Nothing to
    /// paste.").
    Paste {
        key: String,
        name: String,
        menu: bool,
    },
}

/// A folder's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Last modified, seconds since the epoch.
    pub modified: i64,
}

/// A job done.
#[derive(Debug)]
pub enum Done {
    Listed {
        dir: PathBuf,
        result: Result<Vec<Entry>, String>,
    },
    Prepared {
        key: String,
        result: Result<Attachment, String>,
    },
}

struct Shared {
    waker: Waker,
    sender: Mutex<Sender<Done>>,
    receiver: Mutex<Receiver<Done>>,
}

fn shared() -> Option<&'static Shared> {
    static SHARED: OnceLock<Option<Shared>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            Waker::new().ok().map(|waker| Shared {
                waker,
                sender: Mutex::new(sender),
                receiver: Mutex::new(receiver),
            })
        })
        .as_ref()
}

/// The fd the event loop polls: readable when a job is done.
pub fn fd() -> Option<RawFd> {
    shared().map(|s| s.waker.fd())
}

/// Starts `job` on a thread of its own.
pub fn command(job: Job) {
    let Some(shared) = shared() else {
        return;
    };
    let Ok(sender) = shared.sender.lock().map(|s| s.clone()) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("exocortex-files".into())
        .spawn(move || {
            let done = run(job);
            if sender.send(done).is_ok() {
                shared.waker.wake();
            }
        });
}

/// The jobs done since last asked.
pub fn take() -> Vec<Done> {
    let Some(shared) = shared() else {
        return Vec::new();
    };
    shared.waker.drain();
    shared
        .receiver
        .lock()
        .map(|r| r.try_iter().collect())
        .unwrap_or_default()
}

fn run(job: Job) -> Done {
    match job {
        Job::List { dir } => {
            let result = list(&dir);
            Done::Listed { dir, result }
        }
        Job::Read { key, path } => Done::Prepared {
            key,
            result: read(&path),
        },
        Job::Paste { key, name, menu } => Done::Prepared {
            key,
            result: moon_image()
                .unwrap_or_else(clipboard_image)
                .map_err(|why| {
                    if menu && why == NO_IMAGE {
                        "Nothing to paste.".into()
                    } else {
                        why
                    }
                })
                .and_then(|bytes| {
                    if attachments::media_type(&bytes).is_none() {
                        return Err("The clipboard's image could not be decoded.".into());
                    }
                    attachments::prepare(&name, &bytes)
                }),
        },
    }
}

fn io_text(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::NotFound => "it no longer exists".into(),
        _ => error.to_string(),
    }
}

/// A folder's entries: folders first, then files, by name.
pub fn list(dir: &Path) -> Result<Vec<Entry>, String> {
    let read =
        std::fs::read_dir(dir).map_err(|e| format!("Can't open this folder: {}.", io_text(&e)))?;
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .take(MAX_ENTRIES)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Through a symbolic link: what it points at (a broken one is
            // left out).
            let meta = std::fs::metadata(entry.path()).ok()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Some(Entry {
                name,
                dir: meta.is_dir(),
                size: meta.len(),
                modified,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

/// A file as an attachment.
fn read(path: &Path) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let meta =
        std::fs::metadata(path).map_err(|e| format!("Can't read {name}: {}.", io_text(&e)))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a file."));
    }
    if meta.len() > attachments::MAX_SOURCE {
        return Err(attachments::TOO_LARGE.into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Can't read {name}: {}.", io_text(&e)))?;
    attachments::prepare(&name, &bytes)
}

pub const NO_IMAGE: &str = "There is no image on the clipboard.";

/// The clipboard's image from Moon, which keeps one (`clipboard get image`,
/// docs/app-services.md: up to 8 MiB, so megabytes of base64 on a line);
/// None when Moon does not answer (a development host): X's CLIPBOARD then.
fn moon_image() -> Option<Result<Vec<u8>, String>> {
    if cfg!(test) {
        return None;
    }
    let reply =
        crate::control::query_long("clipboard get image", Duration::from_secs(5), 16 << 20)?;
    let Some(image) = reply.get("image").filter(|i| !i.is_null()) else {
        return Some(Err(NO_IMAGE.into()));
    };
    Some(
        image
            .get("data")
            .and_then(serde_json::Value::as_str)
            .and_then(super::proto::base64_decode)
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| "The clipboard's image could not be read.".into()),
    )
}
pub const NOT_ANSWERING: &str = "The app that copied the image isn't answering: Moon's clipboard keeps text only, so an image copied in an app now in the background can't be pasted yet.";

/// How long the clipboard's owner has to answer, and to send it all.
const ANSWER_WITHIN: Duration = Duration::from_secs(2);
const TRANSFER_WITHIN: Duration = Duration::from_secs(8);

/// The image on X's CLIPBOARD (PNG first, else JPEG, GIF or WebP), asked of
/// its owner on a connection of its own; INCR transfers are followed.
pub fn clipboard_image() -> Result<Vec<u8>, String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, Property, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    let failed = |e: &dyn std::fmt::Display| format!("The clipboard could not be read ({e}).");
    let (conn, screen) = RustConnection::connect(None).map_err(|e| failed(&e))?;
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().map_err(|e| failed(&e))?;
    conn.create_window(
        0,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(|e| failed(&e))?;
    let atom = |name: &str| -> Result<u32, String> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(|e| failed(&e))?
            .reply()
            .map_err(|e| failed(&e))?
            .atom)
    };
    let clipboard = atom("CLIPBOARD")?;
    let targets = atom("TARGETS")?;
    let incr = atom("INCR")?;
    let property = atom("EXOCORTEX_PASTE")?;
    let owner = conn
        .get_selection_owner(clipboard)
        .map_err(|e| failed(&e))?
        .reply()
        .map_err(|e| failed(&e))?
        .owner;
    if owner == x11rb::NONE {
        return Err(NO_IMAGE.into());
    }

    // The next event for us, or None when `until` passes.
    let next = |until: Instant| -> Result<Option<Event>, String> {
        loop {
            if let Some(event) = conn.poll_for_event().map_err(|e| failed(&e))? {
                return Ok(Some(event));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let mut fds = [libc::pollfd {
                fd: conn.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            // SAFETY: one live pollfd.
            unsafe { libc::poll(fds.as_mut_ptr(), 1, left.as_millis().max(1) as i32) };
        }
    };
    // Asks for `target`; the property it came in, None for "not available".
    let convert = |target: u32| -> Result<Option<u32>, String> {
        conn.convert_selection(window, clipboard, target, property, x11rb::CURRENT_TIME)
            .map_err(|e| failed(&e))?;
        conn.flush().map_err(|e| failed(&e))?;
        let until = Instant::now() + ANSWER_WITHIN;
        loop {
            match next(until)? {
                Some(Event::SelectionNotify(e)) if e.selection == clipboard => {
                    return Ok((e.property != x11rb::NONE).then_some(e.property));
                }
                Some(_) => {}
                None => return Err(NOT_ANSWERING.into()),
            }
        }
    };
    let get = |delete: bool| {
        conn.get_property(
            delete,
            window,
            property,
            AtomEnum::ANY,
            0,
            (attachments::MAX_SOURCE / 4) as u32,
        )
        .map_err(|e| failed(&e))?
        .reply()
        .map_err(|e| failed(&e))
    };

    let Some(_) = convert(targets)? else {
        return Err(NO_IMAGE.into());
    };
    let offered: Vec<u32> = get(true)?
        .value32()
        .map(|atoms| atoms.collect())
        .unwrap_or_default();
    let mut wanted = None;
    for name in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
        let candidate = atom(name)?;
        if offered.contains(&candidate) {
            wanted = Some(candidate);
            break;
        }
    }
    let Some(target) = wanted else {
        return Err(NO_IMAGE.into());
    };
    if convert(target)?.is_none() {
        return Err(NO_IMAGE.into());
    }
    let first = get(true)?;
    if first.type_ != incr {
        return Ok(first.value);
    }
    // INCR: each chunk comes as a new value of the property, which we
    // delete to ask for the next; an empty one ends it.
    let until = Instant::now() + TRANSFER_WITHIN;
    let mut bytes = Vec::new();
    loop {
        match next(until)? {
            Some(Event::PropertyNotify(e))
                if e.atom == property && e.state == Property::NEW_VALUE =>
            {
                let chunk = get(true)?;
                if chunk.value.is_empty() {
                    return Ok(bytes);
                }
                bytes.extend_from_slice(&chunk.value);
                if bytes.len() as u64 > attachments::MAX_SOURCE {
                    return Err(attachments::TOO_LARGE.into());
                }
            }
            Some(_) => {}
            None => return Err(NOT_ANSWERING.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_list_folders_first_and_files_read_as_attachments() {
        let dir = std::env::temp_dir().join(format!("exo-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b-folder")).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("C.md"), "# hi").unwrap();
        let names: Vec<(String, bool)> = list(&dir)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("b-folder".into(), true),
                ("a.txt".into(), false),
                ("C.md".into(), false)
            ]
        );
        let file = read(&dir.join("a.txt")).unwrap();
        assert_eq!((file.name.as_str(), file.size), ("a.txt", 5));
        assert!(read(&dir.join("b-folder")).is_err());
        assert!(
            list(&dir.join("missing"))
                .unwrap_err()
                .contains("no longer exists")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An app owning CLIPBOARD with `bytes` as image/png: answering at once,
    /// in INCR chunks, or not at all (frozen). Until `done` is set.
    fn owner(
        bytes: Vec<u8>,
        mode: &'static str,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        use x11rb::connection::Connection;
        use x11rb::protocol::Event;
        use x11rb::protocol::xproto::*;
        use x11rb::rust_connection::RustConnection;
        use x11rb::wrapper::ConnectionExt as _;
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id().unwrap();
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap();
        let atom = |name: &str| {
            conn.intern_atom(false, name.as_bytes())
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let (clipboard, targets, png, incr) = (
            atom("CLIPBOARD"),
            atom("TARGETS"),
            atom("image/png"),
            atom("INCR"),
        );
        conn.set_selection_owner(window, clipboard, x11rb::CURRENT_TIME)
            .unwrap();
        conn.flush().unwrap();
        let mut sending: Option<(u32, u32, usize)> = None;
        while !done.load(std::sync::atomic::Ordering::SeqCst) {
            let Some(event) = conn.poll_for_event().unwrap() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            match event {
                Event::SelectionRequest(r) if mode != "frozen" => {
                    // Nothing but the image (Moon asks for text: none).
                    let mut property = r.property;
                    if r.target != targets && r.target != png {
                        property = x11rb::NONE;
                    } else if r.target == targets {
                        conn.change_property32(
                            PropMode::REPLACE,
                            r.requestor,
                            r.property,
                            AtomEnum::ATOM,
                            &[targets, png],
                        )
                        .unwrap();
                    } else if mode == "incr" {
                        conn.change_window_attributes(
                            r.requestor,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE),
                        )
                        .unwrap();
                        conn.change_property32(
                            PropMode::REPLACE,
                            r.requestor,
                            r.property,
                            incr,
                            &[bytes.len() as u32],
                        )
                        .unwrap();
                        sending = Some((r.requestor, r.property, 0));
                    } else {
                        conn.change_property8(
                            PropMode::REPLACE,
                            r.requestor,
                            r.property,
                            png,
                            &bytes,
                        )
                        .unwrap();
                    }
                    conn.send_event(
                        false,
                        r.requestor,
                        EventMask::NO_EVENT,
                        SelectionNotifyEvent {
                            response_type: SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: r.time,
                            requestor: r.requestor,
                            selection: r.selection,
                            target: r.target,
                            property,
                        },
                    )
                    .unwrap();
                    conn.flush().unwrap();
                }
                Event::PropertyNotify(e) if e.state == Property::DELETE => {
                    if let Some((window, property, at)) = sending
                        && e.window == window
                        && e.atom == property
                    {
                        let end = (at + 65536).min(bytes.len());
                        conn.change_property8(
                            PropMode::REPLACE,
                            window,
                            property,
                            png,
                            &bytes[at..end],
                        )
                        .unwrap();
                        conn.flush().unwrap();
                        sending = (at < bytes.len()).then_some((window, property, end));
                    }
                }
                _ => {}
            }
        }
    }

    /// Needs an X server (`moon-sandbox exec N` the test binary with
    /// `--ignored clipboard`); never the phone's own (it takes CLIPBOARD).
    #[test]
    #[ignore]
    fn clipboard_images_are_fetched_from_their_owner() {
        let image = attachments::test_png(400, 300);
        for mode in ["direct", "incr", "frozen"] {
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (bytes, flag) = (image.clone(), done.clone());
            let thread = std::thread::spawn(move || owner(bytes, mode, flag));
            std::thread::sleep(Duration::from_millis(300));
            let started = Instant::now();
            let got = clipboard_image();
            eprintln!(
                "{mode}: {:?} in {:?}",
                got.as_ref().map(Vec::len),
                started.elapsed()
            );
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            thread.join().unwrap();
            match mode {
                "frozen" => assert_eq!(got.unwrap_err(), NOT_ANSWERING),
                _ => assert_eq!(got.unwrap(), image),
            }
        }
    }

    /// Owns CLIPBOARD with a test PNG for 30 s, for pasting it into the app
    /// by hand in a sandbox (`moon-sandbox exec N` the test binary with
    /// `--ignored --exact …serve_a_clipboard_image`).
    #[test]
    #[ignore]
    fn serve_a_clipboard_image() {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        let thread =
            std::thread::spawn(move || owner(attachments::test_png(640, 480), "incr", flag));
        std::thread::sleep(Duration::from_secs(30));
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        thread.join().unwrap();
    }
}
