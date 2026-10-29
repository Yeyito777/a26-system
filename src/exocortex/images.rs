//! Images (WP-C1; PAR-108, 113, 127, 252–255, 347, 400): the previews of
//! images a model attached to its message (the `attach` tool) and of
//! images the user pasted, fetched with `get_image` as PNGs fitted to the
//! box they are drawn in, decoded on the link thread (`proto::ImageData`),
//! and kept in a cache bounded by its pixels.
//!
//! What is fetched is what is near the screen: every pass of the event
//! loop the view says which previews its transcript shows within a screen
//! of the viewport, and its viewer (`view/media.rs`, `viewer.rs`), nearest
//! first; at most three are asked for at a time, an answer that says "not
//! found" or never comes is asked again with backoff (a just-pasted image
//! may not be stored yet), and previews nobody wanted lately go first when
//! the cache is full. Pieces draw them through weak handles, so the cache
//! alone decides what stays in memory.

#![allow(dead_code)]

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::gfx::Image;

use super::proto::{self, ImageAttachment, ImageData, ImageRef};
use super::requests::{Owner, Requests};

/// At most this many `get_image` requests in flight (the TUI's and the
/// iPhone's limit).
pub const IN_FLIGHT: usize = 3;
/// An answer takes at most this long (the iPhone's).
pub const IMAGE_WITHIN: Duration = Duration::from_secs(30);
/// Retries of a preview "not found" or not answered: the iPhone's backoff.
const RETRY: [u64; 5] = [250, 500, 1000, 2000, 4000];
/// Decoded previews kept, in pixels (4 bytes each): about 48 MB.
const BUDGET_PIXELS: usize = 12 << 20;

/// A preview of one image at one size: what is fetched and kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Want {
    /// The image's id (sha256 of its bytes).
    pub id: String,
    /// The conversation a pasted image is in (the daemon looks there).
    pub conv: Option<String>,
    /// The box it is fitted to (never enlarged).
    pub width: u32,
    pub height: u32,
    /// A transcript thumbnail: the daemon may quantize it to 256 colours.
    pub thumbnail: bool,
}

impl Want {
    fn key(&self) -> Key {
        Key {
            id: self.id.clone(),
            width: self.width,
            height: self.height,
            thumbnail: self.thumbnail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    id: String,
    width: u32,
    height: u32,
    thumbnail: bool,
}

/// Where a preview is.
#[derive(Clone)]
pub enum State {
    /// Wanted, not asked for yet.
    Queued,
    /// Asked for with this reqId.
    Loading(String),
    /// Asked again after this (it was not found, or not answered).
    Retry(Instant),
    Ready(Rc<Image>),
    /// It will not come (no such image, it could not be decoded).
    Failed(String),
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Queued => write!(f, "Queued"),
            State::Loading(id) => write!(f, "Loading({id})"),
            State::Retry(at) => write!(f, "Retry({at:?})"),
            State::Ready(image) => write!(f, "Ready({}x{})", image.width, image.height),
            State::Failed(why) => write!(f, "Failed({why})"),
        }
    }
}

impl State {
    /// What a piece showing it depends on: loading, ready (which pixels)
    /// or failed.
    pub fn key(&self) -> u64 {
        match self {
            State::Queued | State::Loading(_) | State::Retry(_) => 1,
            State::Ready(image) => Rc::as_ptr(image) as usize as u64,
            State::Failed(_) => 2,
        }
    }
}

struct Cached {
    want: Want,
    state: State,
    /// Answers that were "not found" or late.
    attempts: usize,
    /// The pass that last wanted it.
    used: u64,
}

pub struct ImageStore {
    entries: HashMap<Key, Cached>,
    by_request: HashMap<String, Key>,
    /// Counts `want` passes (least recently wanted go first).
    pass: u64,
    /// Changes whenever a preview arrives, fails or goes: what shows them
    /// is laid out again.
    pub version: u64,
    /// Pixels kept at most.
    budget: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            by_request: HashMap::new(),
            pass: 0,
            version: 0,
            budget: BUDGET_PIXELS,
        }
    }
}

impl ImageStore {
    /// Where the preview `want` is (None: never wanted).
    pub fn state(&self, want: &Want) -> Option<&State> {
        self.entries.get(&want.key()).map(|cached| &cached.state)
    }

    /// The preview, if it is here.
    pub fn ready(&self, want: &Want) -> Option<&Rc<Image>> {
        match self.state(want) {
            Some(State::Ready(image)) => Some(image),
            _ => None,
        }
    }

    /// The best preview of image `id` here: the largest.
    pub fn best(&self, id: &str) -> Option<&Rc<Image>> {
        self.entries
            .values()
            .filter(|cached| cached.want.id == id)
            .filter_map(|cached| match &cached.state {
                State::Ready(image) => Some(image),
                _ => None,
            })
            .max_by_key(|image| image.width as i64 * image.height as i64)
    }

    /// The previews wanted now, nearest the screen first: asks for those
    /// not here (at most `IN_FLIGHT` at a time) and keeps them from being
    /// dropped. Returns the commands to send.
    pub fn want(&mut self, wants: &[Want], requests: &mut Requests, now: Instant) -> Vec<Value> {
        self.pass += 1;
        let pass = self.pass;
        for want in wants {
            let cached = self.entries.entry(want.key()).or_insert_with(|| Cached {
                want: want.clone(),
                state: State::Queued,
                attempts: 0,
                used: pass,
            });
            cached.used = pass;
            // A conversation to look in, if it was not known before.
            if cached.want.conv.is_none() {
                cached.want.conv.clone_from(&want.conv);
            }
        }
        // Requests the connection dropped are asked again; retries that
        // are due are.
        let mut in_flight = 0;
        for cached in self.entries.values_mut() {
            match &cached.state {
                State::Loading(id) if !requests.is_pending(id) => cached.state = State::Queued,
                State::Loading(_) => in_flight += 1,
                State::Retry(at) if *at <= now => cached.state = State::Queued,
                _ => {}
            }
        }
        self.by_request.retain(|id, _| requests.is_pending(id));
        let mut commands = Vec::new();
        for want in wants {
            if in_flight >= IN_FLIGHT {
                break;
            }
            let key = want.key();
            let Some(cached) = self.entries.get_mut(&key) else {
                continue;
            };
            if !matches!(cached.state, State::Queued) {
                continue;
            }
            let id = requests.issue(
                Owner::Rich,
                "image",
                cached.want.conv.as_deref(),
                IMAGE_WITHIN,
                now,
            );
            commands.push(proto::get_image(
                &id,
                cached.want.conv.as_deref(),
                &cached.want.id,
                (cached.want.width, cached.want.height),
                cached.want.thumbnail,
            ));
            cached.state = State::Loading(id.clone());
            self.by_request.insert(id, key);
            in_flight += 1;
        }
        // Queued previews nobody wants any more are forgotten.
        self.entries
            .retain(|_, cached| !matches!(cached.state, State::Queued) || cached.used == pass);
        self.evict(pass);
        commands
    }

    /// Drops the previews wanted least lately while over the budget (never
    /// one wanted in this pass).
    fn evict(&mut self, pass: u64) {
        let mut total: usize = self.entries.values().map(pixels_of).sum();
        if total <= self.budget {
            return;
        }
        let mut old: Vec<(u64, Key, usize)> = self
            .entries
            .iter()
            .filter(|(_, cached)| cached.used != pass && pixels_of(cached) > 0)
            .map(|(key, cached)| (cached.used, key.clone(), pixels_of(cached)))
            .collect();
        old.sort_by_key(|(used, _, _)| *used);
        for (_, key, pixels) in old {
            if total <= self.budget {
                break;
            }
            self.entries.remove(&key);
            total -= pixels;
            self.version += 1;
        }
    }

    /// An `image_data`: the preview it answers is here (or could not be
    /// decoded). Returns whether it was one waited on.
    pub fn arrived(&mut self, data: ImageData) -> bool {
        let Some(key) = data
            .req_id
            .as_deref()
            .and_then(|id| self.by_request.remove(id))
        else {
            return false;
        };
        let Some(cached) = self.entries.get_mut(&key) else {
            return false;
        };
        cached.state = match data.pixels {
            Some(pixels) if pixels.width > 0 && pixels.height > 0 => State::Ready(Rc::new(Image {
                width: pixels.width as i32,
                height: pixels.height as i32,
                pixels: pixels.argb,
            })),
            _ => State::Failed("The image could not be decoded.".into()),
        };
        self.version += 1;
        self.evict(self.pass);
        true
    }

    /// An error answering request `id`: asked again later when the image
    /// may still come (not stored yet, a timeout), else failed. Whether it
    /// was ours.
    pub fn failed(&mut self, id: &str, message: &str, now: Instant) -> bool {
        let Some(key) = self.by_request.remove(id) else {
            return false;
        };
        let Some(cached) = self.entries.get_mut(&key) else {
            return true;
        };
        let lower = message.to_lowercase();
        let retry = [
            "not found",
            "timed out",
            "timeout",
            "connection",
            "did not answer",
        ]
        .iter()
        .any(|word| lower.contains(word));
        cached.state = match RETRY.get(cached.attempts) {
            Some(delay) if retry => {
                cached.attempts += 1;
                State::Retry(now + Duration::from_millis(*delay))
            }
            _ => State::Failed("Image unavailable".into()),
        };
        self.version += 1;
        true
    }

    /// When a retry is due.
    pub fn deadline(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter_map(|cached| match cached.state {
                State::Retry(at) => Some(at),
                _ => None,
            })
            .min()
    }

    /// Asks for a failed preview again (the viewer's Retry).
    pub fn retry(&mut self, want: &Want) {
        if let Some(cached) = self.entries.get_mut(&want.key())
            && matches!(cached.state, State::Failed(_))
        {
            cached.state = State::Queued;
            cached.attempts = 0;
            self.version += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn pixels_of(cached: &Cached) -> usize {
    match &cached.state {
        State::Ready(image) => image.width as usize * image.height as usize,
        _ => 0,
    }
}

// ----- What an image is ---------------------------------------------------------

/// An image a message shows: a model's (`ImageRef`) or the user's
/// (`ImageAttachment`), as the transcript and the viewer need it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pic {
    /// Its id (sha256 of its bytes); None when neither the daemon nor its
    /// bytes say (it cannot be shown).
    pub id: Option<String>,
    /// Its size, when known.
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub media_type: String,
    pub name: Option<String>,
}

impl Pic {
    pub fn of_ref(image: &ImageRef) -> Self {
        Self {
            id: (!image.id.is_empty()).then(|| image.id.clone()),
            width: image.width,
            height: image.height,
            size_bytes: image.size_bytes,
            media_type: image.media_type.clone(),
            name: image.name.clone(),
        }
    }

    /// A pasted image: the daemon's id, or the sha256 of its bytes when
    /// it came inline (the daemon finds it in the conversation by that).
    pub fn of_attachment(image: &ImageAttachment) -> Self {
        let id = image
            .image_id
            .clone()
            .filter(|id| !id.is_empty())
            .or_else(|| {
                (!image.base64.is_empty())
                    .then(|| proto::base64_decode(&image.base64).map(|bytes| sha256_hex(&bytes)))
                    .flatten()
            });
        let (width, height) = match (image.width, image.height) {
            (Some(w), Some(h)) => (w, h),
            _ => image
                .base64
                .get(..64)
                .and_then(proto::base64_decode)
                .and_then(|head| sniff_size(&head))
                .unwrap_or((0, 0)),
        };
        Self {
            id,
            width,
            height,
            size_bytes: image.size_bytes,
            media_type: image.media_type.clone(),
            name: None,
        }
    }

    /// "1080×2400 · 482 KB".
    pub fn details(&self) -> String {
        let mut parts = Vec::new();
        if self.width > 0 && self.height > 0 {
            parts.push(format!("{}×{}", self.width, self.height));
        }
        if self.size_bytes > 0 {
            parts.push(if self.size_bytes < 1 << 20 {
                format!("{} KB", (self.size_bytes as f64 / 1024.0).round())
            } else {
                format!("{:.1} MB", self.size_bytes as f64 / f64::from(1 << 20))
            });
        }
        parts.join(" · ")
    }

    /// Its name, or one made from its position and type.
    pub fn title(&self, index: usize) -> String {
        match &self.name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                let ext = match self.media_type.as_str() {
                    "image/jpeg" => "jpg",
                    other => other.strip_prefix("image/").unwrap_or("png"),
                };
                format!("image-{}.{ext}", index + 1)
            }
        }
    }
}

/// A hash of a value (strip keys).
pub(crate) fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The size a PNG, GIF or JPEG header says (only the PNG and GIF headers
/// fit in the first bytes; JPEG sizes come from the daemon).
fn sniff_size(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() >= 24 && head.starts_with(b"\x89PNG") {
        let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if head.len() >= 10 && head.starts_with(b"GIF8") {
        let w = u32::from(u16::from_le_bytes([head[6], head[7]]));
        let h = u32::from(u16::from_le_bytes([head[8], head[9]]));
        return Some((w, h));
    }
    None
}

/// SHA-256 of `data` as lowercase hex (an image's id is the hash of its
/// bytes).
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for chunk in message.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (state, value) in h.iter_mut().zip(v) {
            *state = state.wrapping_add(value);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::{Event, parse_event};

    fn want(id: &str) -> Want {
        Want {
            id: id.into(),
            conv: Some("c".into()),
            width: 465,
            height: 465,
            thumbnail: true,
        }
    }

    /// A 2×1 PNG: red, then half-transparent green.
    fn png_base64() -> String {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 128])
                .unwrap();
        }
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.len();
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let v = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
            for i in 0..4 {
                if i <= n {
                    out.push(TABLE[(v >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn data(req: &str, base64: &str) -> ImageData {
        match parse_event(&format!(
            r#"{{"type":"image_data","reqId":"{req}","imageId":"a","format":"png","width":2,"height":1,"base64":"{base64}"}}"#
        )) {
            Event::ImageData(data) => data,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sha256_matches_known_digests() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let long = vec![b'a'; 1000];
        assert_eq!(
            sha256_hex(&long),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn previews_decode_on_parse_into_premultiplied_pixels() {
        let parsed = data("r1", &png_base64());
        let pixels = parsed.pixels.expect("decoded");
        assert_eq!((pixels.width, pixels.height), (2, 1));
        assert_eq!(pixels.argb[0], 0xffff_0000);
        assert_eq!(pixels.argb[1], 0x8000_8000);
        assert!(parsed.base64.is_empty(), "the encoded copy is dropped");
        // Not a PNG: no pixels, the bytes kept.
        let broken = data("r2", "aGVsbG8=");
        assert!(broken.pixels.is_none());
        assert_eq!(broken.base64, "aGVsbG8=");
    }

    #[test]
    fn at_most_three_in_flight_nearest_first_and_answers_land() {
        let now = Instant::now();
        let mut requests = Requests::default();
        let mut store = ImageStore::default();
        let wants: Vec<Want> = ["a", "b", "c", "d"].iter().map(|id| want(id)).collect();
        let commands = store.want(&wants, &mut requests, now);
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0]["imageId"], "a");
        assert_eq!(commands[0]["purpose"], "thumbnail");
        assert_eq!(commands[0]["convId"], "c");
        assert!(store.want(&wants, &mut requests, now).is_empty());
        let req = commands[0]["reqId"].as_str().unwrap().to_owned();
        requests.settle(&req);
        let version = store.version;
        assert!(store.arrived(data(&req, &png_base64())));
        assert!(store.version > version);
        assert_eq!(store.ready(&wants[0]).unwrap().width, 2);
        // A slot freed: the fourth goes out.
        let next = store.want(&wants, &mut requests, now);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0]["imageId"], "d");
        // A late answer for nothing waited on changes nothing.
        assert!(!store.arrived(data("stale", &png_base64())));
    }

    #[test]
    fn not_found_is_retried_with_backoff_then_given_up() {
        let now = Instant::now();
        let mut requests = Requests::default();
        let mut store = ImageStore::default();
        let wants = [want("a")];
        let mut at = now;
        for delay in RETRY {
            let commands = store.want(&wants, &mut requests, at);
            assert_eq!(commands.len(), 1, "asked at {delay}");
            let req = commands[0]["reqId"].as_str().unwrap().to_owned();
            requests.settle(&req);
            assert!(store.failed(&req, "get_image: image not found", at));
            assert_eq!(store.deadline(), Some(at + Duration::from_millis(delay)));
            assert!(store.want(&wants, &mut requests, at).is_empty(), "not yet");
            at += Duration::from_millis(delay);
        }
        let commands = store.want(&wants, &mut requests, at);
        let req = commands[0]["reqId"].as_str().unwrap().to_owned();
        requests.settle(&req);
        store.failed(&req, "get_image: image not found", at);
        assert!(matches!(store.state(&wants[0]), Some(State::Failed(_))));
        assert!(store.want(&wants, &mut requests, at).is_empty());
        // Retry asks again.
        store.retry(&wants[0]);
        assert_eq!(store.want(&wants, &mut requests, at).len(), 1);
    }

    #[test]
    fn a_dropped_connection_asks_again() {
        let now = Instant::now();
        let mut requests = Requests::default();
        let mut store = ImageStore::default();
        let wants = [want("a")];
        assert_eq!(store.want(&wants, &mut requests, now).len(), 1);
        requests.clear();
        assert_eq!(store.want(&wants, &mut requests, now).len(), 1);
    }

    #[test]
    fn the_least_recently_wanted_go_when_over_budget() {
        let now = Instant::now();
        let mut requests = Requests::default();
        let mut store = ImageStore {
            budget: 3 * 64 * 64,
            ..ImageStore::default()
        };
        let mut big = |id: &str| {
            let w = want(id);
            store_ready(&mut store, &mut requests, &w, now, 64, 64);
            w
        };
        fn store_ready(
            store: &mut ImageStore,
            requests: &mut Requests,
            w: &Want,
            now: Instant,
            width: u32,
            height: u32,
        ) {
            let commands = store.want(std::slice::from_ref(w), requests, now);
            let req = commands[0]["reqId"].as_str().unwrap().to_owned();
            requests.settle(&req);
            let mut data = ImageData {
                req_id: Some(req),
                ..ImageData::default()
            };
            data.pixels = Some(proto::Pixels {
                width,
                height,
                argb: vec![0; (width * height) as usize],
            });
            store.arrived(data);
        }
        let wants: Vec<Want> = (0..4).map(|i| big(&format!("i{i}"))).collect();
        // Four are over the budget: the first went.
        assert!(store.state(&wants[0]).is_none());
        assert!(store.ready(&wants[3]).is_some());
    }

    #[test]
    fn pasted_images_are_named_by_their_bytes() {
        let attachment = ImageAttachment {
            media_type: "image/png".into(),
            base64: png_base64(),
            size_bytes: 70,
            image_id: None,
            width: None,
            height: None,
        };
        let pic = Pic::of_attachment(&attachment);
        let bytes = proto::base64_decode(&attachment.base64).unwrap();
        assert_eq!(pic.id, Some(sha256_hex(&bytes)));
        assert_eq!((pic.width, pic.height), (2, 1));
        assert_eq!(pic.details(), "2×1 · 0 KB");
        assert_eq!(pic.title(1), "image-2.png");
    }
}
