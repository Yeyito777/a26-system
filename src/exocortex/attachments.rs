//! Composer attachments (WP-D; PAR-237–242, the iPhone's
//! `state/attachments.ts`). Images travel as `images` on `send_message`,
//! `queue_message` and a new chat's first message: PNG, JPEG, GIF or WebP
//! up to 4 MB, base64, as the file holds them. Other files travel as their
//! text in an `<exocortex_file …>` envelope after the message, exactly like
//! the iPhone's (the daemon has no document type); `split` takes envelopes
//! back out of a message (a rewind, a queued message pulled back). At most
//! six items a message, and 12 MB with its text.
//!
//! This is the data and its rules, no I/O: `files.rs` reads a file (or the
//! clipboard's image) on a thread and hands `prepare`'s result to the
//! model, which keeps each draft's attachments in memory by draft key.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::gfx::Image;

use super::proto::{self, ImageAttachment};

pub const MAX_ITEMS: usize = 6;
/// A document's text, in characters (longer is cut, and says so).
pub const MAX_TEXT: usize = 120_000;
/// A file chosen, in bytes.
pub const MAX_SOURCE: u64 = 24 << 20;
/// An image sent, in bytes.
pub const MAX_IMAGE: u64 = 4 << 20;
/// Text and attachments of one message (the old app's budget).
pub const BUDGET: usize = 12 << 20;
/// A thumbnail's side, in pixels.
pub const THUMB: i32 = 92;

pub const TOO_MANY: &str = "You can attach up to 6 items to one message.";
pub const TOO_LARGE: &str = "Choose a file smaller than 24 MB.";
pub const IMAGE_TOO_LARGE: &str = "Choose an image smaller than 4 MB.";
pub const UNSUPPORTED: &str =
    "This file type cannot be attached yet. Choose an image or a text-based file.";
pub const UNREADABLE: &str = "The selected file could not be read as text.";
pub const PDF: &str =
    "PDFs can't be read on the phone yet. Attach page screenshots or a text file instead.";
pub const OVER_BUDGET: &str =
    "Message exceeds the 12 MB attachment budget. Remove an item before sending.";
pub const COMMAND: &str =
    "Slash commands cannot carry attachments. Remove them or send a regular message.";
pub const PREPARING: &str = "Attachments are still being prepared; send again in a moment.";
const TRUNCATION_NOTE: &str = "\n[Content truncated by the phone client.]";

/// Extensions read as text even when they are not UTF-8 (the iPhone's).
const TEXT_EXTENSIONS: [&str; 38] = [
    "txt", "md", "markdown", "json", "jsonl", "yaml", "yml", "xml", "csv", "tsv", "swift", "ts",
    "tsx", "js", "jsx", "py", "rb", "rs", "go", "java", "kt", "c", "h", "cpp", "hpp", "sh", "zsh",
    "css", "html", "sql", "toml", "ini", "log", "conf", "cfg", "lua", "nix", "diff",
];

#[derive(Clone)]
pub enum Kind {
    Image {
        media_type: &'static str,
        base64: String,
        /// The sha256 of its bytes: the daemon's `imageId` for it.
        image_id: String,
        width: Option<u32>,
        height: Option<u32>,
        /// A PNG's thumbnail (other formats have no decoder here: a glyph).
        thumb: Option<Arc<Image>>,
    },
    File {
        /// The text sent (at most `MAX_TEXT` characters).
        text: String,
        truncated: bool,
    },
}

/// One attachment of a draft.
#[derive(Clone, PartialEq)]
pub struct Attachment {
    /// Its id in the app (the tray's ✕ names it).
    pub id: u64,
    pub name: String,
    /// The file's size in bytes.
    pub size: u64,
    pub kind: Kind,
}

/// The same file (its thumbnail follows from it).
impl PartialEq for Kind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Kind::Image {
                    media_type: a,
                    base64: b,
                    ..
                },
                Kind::Image {
                    media_type: c,
                    base64: d,
                    ..
                },
            ) => a == c && b == d,
            (
                Kind::File {
                    text: a,
                    truncated: b,
                },
                Kind::File {
                    text: c,
                    truncated: d,
                },
            ) => a == c && b == d,
            _ => false,
        }
    }
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Image {
                media_type, base64, ..
            } => write!(f, "{media_type}, {} base64 chars", base64.len()),
            Kind::File { text, truncated } => {
                write!(
                    f,
                    "text {:?}{}",
                    text.chars().take(40).collect::<String>(),
                    if *truncated { ", truncated" } else { "" }
                )
            }
        }
    }
}

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Attachment({} {:?}, {} B, {:?})",
            self.id, self.name, self.size, self.kind
        )
    }
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        matches!(self.kind, Kind::Image { .. })
    }

    /// The tray's second line: its size, and that it was cut.
    pub fn detail(&self) -> String {
        match &self.kind {
            Kind::File {
                truncated: true, ..
            } => format!("{} · truncated", size_text(self.size)),
            _ => size_text(self.size),
        }
    }

    /// As the daemon takes it (`ImageAttachment`); None for a document.
    pub fn wire_image(&self) -> Option<Value> {
        let Kind::Image {
            media_type,
            base64,
            width,
            height,
            ..
        } = &self.kind
        else {
            return None;
        };
        let mut image = json!({
            "mediaType": media_type,
            "base64": base64,
            "sizeBytes": self.size,
        });
        if let (Some(w), Some(h)) = (width, height) {
            image["width"] = Value::from(*w);
            image["height"] = Value::from(*h);
        }
        Some(image)
    }

    /// The image as a transcript shows it before the daemon has it: by the
    /// hash of its bytes (`get_image` finds it by that), without its bytes.
    pub fn shown_image(&self) -> Option<ImageAttachment> {
        let Kind::Image {
            media_type,
            image_id,
            width,
            height,
            ..
        } = &self.kind
        else {
            return None;
        };
        Some(ImageAttachment {
            media_type: (*media_type).into(),
            base64: String::new(),
            size_bytes: self.size,
            image_id: Some(image_id.clone()),
            width: *width,
            height: *height,
        })
    }
}

/// `1.5 MB`, `12 KB`, `900 B` (the iPhone's `formatBytes`).
pub fn size_text(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", (n as f64 / 1024.0).round())
    } else if n < 10 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// An image's type by its first bytes.
pub fn media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// An image's size in pixels, from its header.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |at: usize| {
        Some(u32::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le16 = |at: usize| {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le24 = |at: usize| {
        let b = bytes.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    let size = match media_type(bytes)? {
        "image/png" => (
            u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
            u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
        ),
        "image/gif" => (le16(6)?, le16(8)?),
        "image/webp" => match bytes.get(12..16)? {
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            b"VP8L" => {
                let b = bytes.get(21..25)?;
                let bits = u32::from_le_bytes(b.try_into().ok()?);
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            _ => return None,
        },
        // JPEG: the first start-of-frame marker.
        _ => {
            let mut at = 2;
            loop {
                if *bytes.get(at)? != 0xff {
                    return None;
                }
                let marker = *bytes.get(at + 1)?;
                if marker == 0xff {
                    at += 1;
                    continue;
                }
                let length = be16(at + 2)? as usize;
                if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                    break (be16(at + 7)?, be16(at + 5)?);
                }
                at += 2 + length;
            }
        }
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// A PNG's thumbnail: the middle square, `THUMB` px a side.
pub fn thumbnail(bytes: &[u8]) -> Option<Image> {
    if media_type(bytes) != Some("image/png") {
        return None;
    }
    let pixels = proto::decode_png(bytes)?;
    let (w, h) = (pixels.width as usize, pixels.height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let side = w.min(h);
    let (x0, y0) = ((w - side) / 2, (h - side) / 2);
    let n = THUMB as usize;
    let mut out = Vec::with_capacity(n * n);
    for ty in 0..n {
        let sy0 = y0 + ty * side / n;
        let sy1 = (y0 + (ty + 1) * side / n).max(sy0 + 1);
        for tx in 0..n {
            let sx0 = x0 + tx * side / n;
            let sx1 = (x0 + (tx + 1) * side / n).max(sx0 + 1);
            // The box's mean, channel by channel (premultiplied).
            let mut sum = [0_u64; 4];
            for sy in sy0..sy1 {
                for &p in &pixels.argb[sy * w + sx0..sy * w + sx1] {
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u64::from(p >> (24 - 8 * c) & 0xff);
                    }
                }
            }
            let count = ((sy1 - sy0) * (sx1 - sx0)) as u64;
            out.push(
                sum.iter()
                    .enumerate()
                    .fold(0, |acc, (c, s)| acc | ((s / count) as u32) << (24 - 8 * c)),
            );
        }
    }
    Some(Image {
        width: THUMB,
        height: THUMB,
        pixels: out,
    })
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// A file's text: UTF-8, else (a known text type) UTF-16 with its mark or
/// Latin-1. None for what is not text.
fn text_of(name: &str, bytes: &[u8]) -> Option<String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.contains('\0')
    {
        return Some(text.to_owned());
    }
    if !TEXT_EXTENSIONS.contains(&extension(name).as_str()) {
        return None;
    }
    let utf16 = |le: bool| {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|b| {
                if le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xff, 0xfe, ..] => utf16(true),
        [0xfe, 0xff, ..] => utf16(false),
        _ if !bytes.contains(&0) => Some(bytes.iter().map(|&b| char::from(b)).collect()),
        _ => None,
    }
}

/// A chosen file (or a pasted image) as an attachment, or why it cannot be
/// one. Its id is the model's to give.
pub fn prepare(name: &str, bytes: &[u8]) -> Result<Attachment, String> {
    let size = bytes.len() as u64;
    if size > MAX_SOURCE {
        return Err(TOO_LARGE.into());
    }
    if let Some(media_type) = media_type(bytes) {
        if size > MAX_IMAGE {
            return Err(IMAGE_TOO_LARGE.into());
        }
        let (width, height) = dimensions(bytes).unzip();
        return Ok(Attachment {
            id: 0,
            name: name.to_owned(),
            size,
            kind: Kind::Image {
                media_type,
                base64: base64(bytes),
                image_id: super::images::sha256_hex(bytes),
                width,
                height,
                thumb: thumbnail(bytes).map(Arc::new),
            },
        });
    }
    if bytes.starts_with(b"%PDF") || extension(name) == "pdf" {
        return Err(PDF.into());
    }
    let Some(text) = text_of(name, bytes) else {
        return Err(if TEXT_EXTENSIONS.contains(&extension(name).as_str()) {
            UNREADABLE.into()
        } else {
            UNSUPPORTED.into()
        });
    };
    let (text, truncated) = match text.char_indices().nth(MAX_TEXT) {
        Some((cut, _)) => (text[..cut].to_owned(), true),
        None => (text, false),
    };
    Ok(Attachment {
        id: 0,
        name: name.to_owned(),
        size,
        kind: Kind::File { text, truncated },
    })
}

/// A sent image back as an attachment (a rewind, a queued message pulled
/// back), if its bytes came with it.
pub fn from_image(image: &ImageAttachment, index: usize) -> Option<Attachment> {
    let bytes = proto::base64_decode(&image.base64).filter(|b| !b.is_empty())?;
    let media_type = media_type(&bytes)?;
    let (width, height) = match (image.width, image.height) {
        (Some(w), Some(h)) => (Some(w), Some(h)),
        _ => dimensions(&bytes).unzip(),
    };
    Some(Attachment {
        id: 0,
        name: format!("Image {}", index + 1),
        size: if image.size_bytes > 0 {
            image.size_bytes
        } else {
            bytes.len() as u64
        },
        kind: Kind::Image {
            media_type,
            base64: image.base64.clone(),
            image_id: super::images::sha256_hex(&bytes),
            width,
            height,
            thumb: thumbnail(&bytes).map(Arc::new),
        },
    })
}

/// A sent message's text and images back as a draft's text and
/// attachments (documents out of their envelopes).
pub fn editable(text: &str, images: &[ImageAttachment]) -> (String, Vec<Attachment>) {
    let (text, mut files) = split(text);
    files.extend(
        images
            .iter()
            .enumerate()
            .filter_map(|(i, image)| from_image(image, i)),
    );
    (text, files)
}

/// What a message and its attachments weigh against `BUDGET`.
pub fn weight(text: &str, attachments: &[Attachment]) -> usize {
    text.len() + attachments.iter().map(|a| a.size as usize).sum::<usize>()
}

/// A document's envelope (the iPhone's `fileEnvelope`).
pub fn envelope(name: &str, size: u64, text: &str, truncated: bool) -> String {
    let name = name.replace(['\r', '\n'], " ");
    format!(
        "<exocortex_file name_base64=\"{}\" size_bytes=\"{size}\" truncated=\"{truncated}\">\n[Attached file: {name}]\n{text}{}\n</exocortex_file>",
        base64(name.as_bytes()),
        if truncated { TRUNCATION_NOTE } else { "" },
    )
}

/// The message as sent: what was typed, then an envelope per document.
pub fn resolve_text(text: &str, attachments: &[Attachment]) -> String {
    let base = text.trim();
    let files: Vec<String> = attachments
        .iter()
        .filter_map(|a| match &a.kind {
            Kind::File { text, truncated } => Some(envelope(&a.name, a.size, text, *truncated)),
            Kind::Image { .. } => None,
        })
        .collect();
    if files.is_empty() {
        return base.to_owned();
    }
    let rendered = files.join("\n\n");
    if base.is_empty() {
        rendered
    } else {
        format!("{base}\n\n{rendered}")
    }
}

/// A sent message back into what was typed and its documents (the
/// iPhone's `splitEnvelopes`).
pub fn split(text: &str) -> (String, Vec<Attachment>) {
    const OPEN: &str = "<exocortex_file name_base64=\"";
    const CLOSE: &str = "\n</exocortex_file>";
    let mut files = Vec::new();
    let mut kept = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        let parsed = (|| {
            let tail = &rest[at + OPEN.len()..];
            let (name64, tail) = tail.split_once("\" size_bytes=\"")?;
            let (size, tail) = tail.split_once("\" truncated=\"")?;
            let (truncated, tail) = tail.split_once("\">\n[Attached file: ")?;
            let (_, tail) = tail.split_once("]\n")?;
            let end = tail.find(CLOSE)?;
            let truncated = match truncated {
                "true" => true,
                "false" => false,
                _ => return None,
            };
            let body = &tail[..end];
            let body = if truncated {
                body.strip_suffix(TRUNCATION_NOTE).unwrap_or(body)
            } else {
                body
            };
            let name = proto::base64_decode(name64)
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_else(|| "file".into());
            let consumed = rest.len() - tail.len() + end + CLOSE.len();
            Some((
                Attachment {
                    id: 0,
                    name,
                    size: size.parse().ok()?,
                    kind: Kind::File {
                        text: body.to_owned(),
                        truncated,
                    },
                },
                consumed,
            ))
        })();
        match parsed {
            Some((file, consumed)) => {
                kept.push_str(rest[..at].trim_end_matches('\n'));
                files.push(file);
                rest = &rest[consumed..];
            }
            None => {
                kept.push_str(&rest[..at + OPEN.len()]);
                rest = &rest[at + OPEN.len()..];
            }
        }
    }
    kept.push_str(rest);
    if files.is_empty() {
        (text.to_owned(), files)
    } else {
        (kept.trim().to_owned(), files)
    }
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> shift & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A PNG of a gradient, for tests.
#[cfg(test)]
pub fn test_png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let data: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [(x * 255 / width) as u8, (y * 255 / height) as u8, 200, 255]
            })
            .collect();
        writer.write_image_data(&data).unwrap();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        test_png(width, height)
    }

    #[test]
    fn base64_round_trips() {
        for text in ["", "a", "ab", "abc", "abcd", "héllo wörld"] {
            let encoded = base64(text.as_bytes());
            assert_eq!(proto::base64_decode(&encoded).unwrap(), text.as_bytes());
        }
        assert_eq!(base64(b"ab"), "YWI=");
    }

    #[test]
    fn images_and_text_are_prepared_and_the_rest_refused() {
        let image = prepare("shot.png", &png(300, 200)).unwrap();
        let Kind::Image {
            media_type,
            width,
            height,
            thumb,
            ..
        } = &image.kind
        else {
            panic!("an image");
        };
        assert_eq!(
            (*media_type, *width, *height),
            ("image/png", Some(300), Some(200))
        );
        let thumb = thumb.as_ref().unwrap();
        assert_eq!((thumb.width, thumb.height), (THUMB, THUMB));
        let wire = image.wire_image().unwrap();
        assert_eq!(wire["mediaType"], "image/png");
        assert_eq!(wire["width"], 300);
        assert!(image.shown_image().unwrap().image_id.is_some());

        let text = prepare("notes", "héllo\n".as_bytes()).unwrap();
        assert_eq!(
            text.kind,
            Kind::File {
                text: "héllo\n".into(),
                truncated: false
            }
        );
        let long = prepare("long.txt", "é".repeat(MAX_TEXT + 5).as_bytes()).unwrap();
        assert!(
            matches!(&long.kind, Kind::File { text, truncated: true } if text.chars().count() == MAX_TEXT)
        );
        assert_eq!(long.detail(), "234 KB · truncated");
        // Latin-1 in a known text type; binary refused.
        assert!(prepare("old.txt", b"caf\xe9").is_ok());
        assert_eq!(
            prepare("blob.bin", b"\x00\x01\x02").unwrap_err(),
            UNSUPPORTED
        );
        assert_eq!(prepare("a.pdf", b"%PDF-1.7").unwrap_err(), PDF);
        let big = vec![0x89; (MAX_IMAGE + 1) as usize];
        let mut big_png = png(1, 1);
        big_png.extend_from_slice(&big);
        assert_eq!(prepare("big.png", &big_png).unwrap_err(), IMAGE_TOO_LARGE);
    }

    #[test]
    fn dimensions_come_from_headers() {
        let gif = b"GIF89a\x0a\x00\x05\x00".to_vec();
        assert_eq!(dimensions(&gif), Some((10, 5)));
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0, 0];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x02, 0x58, 0x03, 0x20]);
        assert_eq!(dimensions(&jpeg), Some((800, 600)));
    }

    #[test]
    fn envelopes_go_out_and_come_back() {
        let file = prepare("a\nb.md", b"# Hi\nthere").unwrap();
        let sent = resolve_text("  look  ", std::slice::from_ref(&file));
        assert!(sent.starts_with("look\n\n<exocortex_file name_base64=\""));
        assert!(sent.contains("[Attached file: a b.md]\n# Hi\nthere\n</exocortex_file>"));
        let (text, files) = split(&sent);
        assert_eq!(text, "look");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "a b.md");
        assert_eq!(files[0].kind, file.kind);
        let cut = Attachment {
            kind: Kind::File {
                text: "abc".into(),
                truncated: true,
            },
            ..file.clone()
        };
        let sent = resolve_text("", &[cut.clone(), file.clone()]);
        let (text, files) = split(&sent);
        assert_eq!(text, "");
        assert_eq!(files[0].kind, cut.kind);
        assert_eq!(files.len(), 2);
        // Not an envelope: left alone.
        let odd = "x <exocortex_file name_base64=\"zz\"> y";
        assert_eq!(split(odd), (odd.to_owned(), Vec::new()));
    }
}
