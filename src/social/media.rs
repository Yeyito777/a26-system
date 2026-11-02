//! Pictures for the social apps: avatars, photos, stickers, a video's
//! poster. The app asks for a picture at the size it draws it, every frame
//! it is near the screen (`get`); what it has not got yet is fetched from
//! the bridge (`media.fetch`: the bridge downloads it into its cache and
//! answers with the file), decoded off the UI thread (JPEG at a reduced
//! scale when that is enough, PNG, WebP, a GIF's first frame), resampled to
//! that size and kept in a cache bounded by its pixels, least recently
//! drawn first out. A picture that fails is asked again with backoff.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::link::Link;
use super::proto::BridgeError;
use crate::exocortex::line::Waker;
use crate::gfx::Image;

/// `media.fetch` requests in flight at once.
const IN_FLIGHT: usize = 4;
/// Decoded pictures kept, in pixels (4 bytes each): about 96 MB.
const BUDGET_PIXELS: usize = 24 << 20;
const RETRY_MS: [u64; 5] = [500, 1000, 2000, 5000, 15000];
const DECODERS: usize = 2;
/// The largest picture decoded (pixels), a guard against huge files.
const MAX_SOURCE_PIXELS: u64 = 64 << 20;

/// How a picture fills the box it is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fit {
    /// Inside the box, its own shape kept (a photo in a bubble): the result
    /// may be smaller than the box on one side.
    Contain,
    /// Fills the box exactly, its middle cropped (an avatar, a grid tile).
    Cover,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub reference: String,
    pub width: u32,
    pub height: u32,
    pub fit: Fit,
}

enum Entry {
    /// Wanted, not asked for yet (or to be asked again after `retry_at`).
    Wanted {
        attempts: usize,
        retry_at: Option<Instant>,
    },
    /// `media.fetch` asked with this id.
    Fetching(u64, usize),
    Decoding(usize),
    Ready(Rc<Image>),
}

/// The pictures an app shows, and what it is still waiting for.
pub struct Media {
    entries: HashMap<Key, Entry>,
    /// When each ready picture was last asked for (eviction order).
    used: HashMap<Key, Instant>,
    /// Asked for this frame, in the order asked (nearest the screen first).
    wanted: Vec<Key>,
    wanted_set: HashSet<Key>,
    fetching: HashMap<u64, Key>,
    pixels: usize,
    budget: usize,
    decoder: Decoder,
}

impl Media {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            entries: HashMap::new(),
            used: HashMap::new(),
            wanted: Vec::new(),
            wanted_set: HashSet::new(),
            fetching: HashMap::new(),
            pixels: 0,
            budget: BUDGET_PIXELS,
            decoder: Decoder::spawn()?,
        })
    }

    /// The picture `reference` at `width`x`height`, if it is ready; asks for
    /// it otherwise. Call it for what is drawn and what is about to be, the
    /// nearest first.
    pub fn get(
        &mut self,
        reference: &str,
        width: u32,
        height: u32,
        fit: Fit,
        now: Instant,
    ) -> Option<Rc<Image>> {
        if reference.is_empty() || width == 0 || height == 0 {
            return None;
        }
        let key = Key {
            reference: reference.to_owned(),
            width: width.min(4096),
            height: height.min(4096),
            fit,
        };
        if let Some(Entry::Ready(image)) = self.entries.get(&key) {
            let image = image.clone();
            self.used.insert(key, now);
            return Some(image);
        }
        if !self.wanted_set.contains(&key) {
            self.wanted_set.insert(key.clone());
            self.wanted.push(key.clone());
        }
        self.entries.entry(key).or_insert(Entry::Wanted {
            attempts: 0,
            retry_at: None,
        });
        None
    }

    /// Whether a picture is ready, without asking for it.
    pub fn peek(&self, reference: &str, width: u32, height: u32, fit: Fit) -> Option<Rc<Image>> {
        let key = Key {
            reference: reference.to_owned(),
            width,
            height,
            fit,
        };
        match self.entries.get(&key) {
            Some(Entry::Ready(image)) => Some(image.clone()),
            _ => None,
        }
    }

    /// After a frame: asks the bridge for what was wanted, nearest first,
    /// a few at a time. Returns when it next needs to run (a retry).
    pub fn pump(&mut self, link: &Link, connected: bool, now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let wanted = std::mem::take(&mut self.wanted);
        self.wanted_set.clear();
        if !connected {
            return None;
        }
        for key in wanted {
            if self.fetching.len() >= IN_FLIGHT {
                break;
            }
            let Some(Entry::Wanted { attempts, retry_at }) = self.entries.get(&key) else {
                continue;
            };
            let attempts = *attempts;
            if let Some(at) = *retry_at
                && at > now
            {
                next = Some(next.map_or(at, |n: Instant| n.min(at)));
                continue;
            }
            let max = key.width.max(key.height);
            let id = link.request("media.fetch", json!({"ref": key.reference, "max": max}));
            self.fetching.insert(id, key.clone());
            self.entries.insert(key, Entry::Fetching(id, attempts));
        }
        next
    }

    /// A bridge answer: true when it was a `media.fetch` of ours.
    pub fn answer(&mut self, id: u64, result: &Result<Value, BridgeError>, now: Instant) -> bool {
        let Some(key) = self.fetching.remove(&id) else {
            return false;
        };
        let attempts = match self.entries.get(&key) {
            Some(Entry::Fetching(_, attempts)) => *attempts,
            _ => 0,
        };
        let path = result
            .as_ref()
            .ok()
            .and_then(|value| value.get("path"))
            .and_then(Value::as_str);
        match path {
            Some(path) => {
                self.decoder.submit(key.clone(), path.to_owned());
                self.entries.insert(key, Entry::Decoding(attempts));
            }
            None => {
                let gone = matches!(result, Err(error) if error.code == "not_found" || error.code == "invalid_argument");
                self.failed(key, attempts, gone, now);
            }
        }
        true
    }

    fn failed(&mut self, key: Key, attempts: usize, gone: bool, now: Instant) {
        // A picture that is gone is asked again rarely (it may come back).
        let delay = if gone {
            60_000
        } else {
            RETRY_MS[attempts.min(RETRY_MS.len() - 1)]
        };
        self.entries.insert(
            key,
            Entry::Wanted {
                attempts: attempts + 1,
                retry_at: Some(now + Duration::from_millis(delay)),
            },
        );
    }

    /// The link dropped: what was in flight is asked again once it is back.
    pub fn disconnected(&mut self) {
        for (_, key) in self.fetching.drain() {
            self.entries.insert(
                key,
                Entry::Wanted {
                    attempts: 0,
                    retry_at: None,
                },
            );
        }
    }

    /// The fd that becomes readable when decoded pictures wait.
    pub fn fd(&self) -> std::os::fd::RawFd {
        self.decoder.waker.fd()
    }

    /// Takes the pictures decoded since the last call; true when any came
    /// (draw again).
    pub fn take_decoded(&mut self, now: Instant) -> bool {
        self.decoder.waker.drain();
        let mut any = false;
        while let Ok((key, image)) = self.decoder.results.try_recv() {
            let attempts = match self.entries.get(&key) {
                Some(Entry::Decoding(attempts)) => *attempts,
                _ => 0,
            };
            match image {
                Some(image) => {
                    self.pixels += image.pixels.len();
                    self.used.insert(key.clone(), now);
                    self.entries.insert(key, Entry::Ready(Rc::new(image)));
                    any = true;
                }
                None => self.failed(key, attempts, true, now),
            }
        }
        if any {
            self.evict();
        }
        any
    }

    fn evict(&mut self) {
        if self.pixels <= self.budget {
            return;
        }
        let mut ready: Vec<(Instant, Key)> = self
            .used
            .iter()
            .map(|(key, at)| (*at, key.clone()))
            .collect();
        ready.sort_by_key(|(at, _)| *at);
        for (_, key) in ready {
            if self.pixels <= self.budget * 3 / 4 {
                break;
            }
            if let Some(Entry::Ready(image)) = self.entries.remove(&key) {
                self.pixels = self.pixels.saturating_sub(image.pixels.len());
            }
            self.used.remove(&key);
        }
    }
}

type Job = (Key, String);

struct Decoder {
    jobs: Arc<(Mutex<Vec<Job>>, Condvar)>,
    results: Receiver<(Key, Option<Image>)>,
    waker: Arc<Waker>,
}

impl Decoder {
    fn spawn() -> std::io::Result<Self> {
        let jobs: Arc<(Mutex<Vec<Job>>, Condvar)> =
            Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let waker = Arc::new(Waker::new()?);
        let (tx, rx): (Sender<(Key, Option<Image>)>, _) = mpsc::channel();
        for n in 0..DECODERS {
            let (jobs, waker, tx) = (jobs.clone(), waker.clone(), tx.clone());
            thread::Builder::new()
                .name(format!("social-decode-{n}"))
                .spawn(move || {
                    loop {
                        let (key, path) = {
                            let (lock, ready) = &*jobs;
                            let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                // The newest first: it is what shows now.
                                if let Some(job) = queue.pop() {
                                    break job;
                                }
                                queue = ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                            }
                        };
                        let image = decode_file(&path, key.width, key.height, key.fit);
                        if tx.send((key, image)).is_err() {
                            return;
                        }
                        waker.wake();
                    }
                })?;
        }
        Ok(Self {
            jobs,
            results: rx,
            waker,
        })
    }

    fn submit(&self, key: Key, path: String) {
        let (lock, ready) = &*self.jobs;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((key, path));
        ready.notify_one();
    }
}

/// Straight RGBA rows, as decoded.
struct Rgba {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// Decodes the picture in `path` (JPEG, PNG, WebP, GIF) and resamples it for
/// a `width`x`height` box.
pub fn decode_file(path: &str, width: u32, height: u32, fit: Fit) -> Option<Image> {
    let mut file = File::open(path).ok()?;
    let mut head = [0_u8; 16];
    let read = file.read(&mut head).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let head = &head[..read];
    let reader = BufReader::new(file);
    let source = if head.starts_with(&[0xff, 0xd8]) {
        decode_jpeg(reader, width, height, fit)?
    } else if head.starts_with(b"\x89PNG") {
        decode_png(reader)?
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        decode_webp(reader)?
    } else if head.starts_with(b"GIF8") {
        decode_gif(reader)?
    } else {
        return None;
    };
    Some(resample(&source, width, height, fit))
}

fn check_size(width: u32, height: u32) -> Option<()> {
    (width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_SOURCE_PIXELS)
        .then_some(())
}

fn decode_jpeg(reader: BufReader<File>, width: u32, height: u32, fit: Fit) -> Option<Rgba> {
    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    check_size(u32::from(info.width), u32::from(info.height))?;
    // The smallest of JPEG's 1/8..1 scales that still covers the box.
    let (want_w, want_h) = match fit {
        Fit::Contain => {
            let scale = (width as f32 / f32::from(info.width))
                .min(height as f32 / f32::from(info.height))
                .min(1.0);
            (
                (f32::from(info.width) * scale).ceil() as u16,
                (f32::from(info.height) * scale).ceil() as u16,
            )
        }
        Fit::Cover => {
            let scale = (width as f32 / f32::from(info.width))
                .max(height as f32 / f32::from(info.height))
                .min(1.0);
            (
                (f32::from(info.width) * scale).ceil() as u16,
                (f32::from(info.height) * scale).ceil() as u16,
            )
        }
    };
    let (out_w, out_h) = decoder.scale(want_w.max(1), want_h.max(1)).ok()?;
    let data = decoder.decode().ok()?;
    let info = decoder.info()?;
    let count = usize::from(out_w) * usize::from(out_h);
    let mut pixels = Vec::with_capacity(count * 4);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            for p in data.chunks_exact(3).take(count) {
                pixels.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        jpeg_decoder::PixelFormat::L8 => {
            for &l in data.iter().take(count) {
                pixels.extend_from_slice(&[l, l, l, 255]);
            }
        }
        jpeg_decoder::PixelFormat::L16 => {
            for p in data.chunks_exact(2).take(count) {
                pixels.extend_from_slice(&[p[0], p[0], p[0], 255]);
            }
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            for p in data.chunks_exact(4).take(count) {
                let k = u32::from(p[3]);
                let c = |v: u8| (u32::from(v) * k / 255) as u8;
                pixels.extend_from_slice(&[c(p[0]), c(p[1]), c(p[2]), 255]);
            }
        }
    }
    (pixels.len() == count * 4).then_some(Rgba {
        width: u32::from(out_w),
        height: u32::from(out_h),
        pixels,
    })
}

fn decode_png(reader: BufReader<File>) -> Option<Rgba> {
    let mut decoder = png::Decoder::new(reader);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let (width, height) = (reader.info().width, reader.info().height);
    check_size(width, height)?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let data = &buffer[..frame.buffer_size()];
    let count = width as usize * height as usize;
    let mut pixels = Vec::with_capacity(count * 4);
    match frame.color_type {
        png::ColorType::Rgba => pixels.extend_from_slice(&data[..count * 4]),
        png::ColorType::Rgb => {
            for p in data.chunks_exact(3).take(count) {
                pixels.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in data.chunks_exact(2).take(count) {
                pixels.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &l in data.iter().take(count) {
                pixels.extend_from_slice(&[l, l, l, 255]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    (pixels.len() == count * 4).then_some(Rgba {
        width,
        height,
        pixels,
    })
}

fn decode_webp(reader: BufReader<File>) -> Option<Rgba> {
    let mut decoder = image_webp::WebPDecoder::new(reader).ok()?;
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    let alpha = decoder.has_alpha();
    let mut data = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut data).ok()?;
    let count = width as usize * height as usize;
    let pixels = if alpha {
        data
    } else {
        let mut pixels = Vec::with_capacity(count * 4);
        for p in data.chunks_exact(3).take(count) {
            pixels.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        pixels
    };
    (pixels.len() == count * 4).then_some(Rgba {
        width,
        height,
        pixels,
    })
}

fn decode_gif(reader: BufReader<File>) -> Option<Rgba> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(reader).ok()?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    check_size(width, height)?;
    let frame = decoder.read_next_frame().ok()??;
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    let (fw, fh) = (u32::from(frame.width), u32::from(frame.height));
    let (left, top) = (u32::from(frame.left), u32::from(frame.top));
    for y in 0..fh {
        for x in 0..fw {
            let (dx, dy) = (left + x, top + y);
            if dx >= width || dy >= height {
                continue;
            }
            let src = ((y * fw + x) * 4) as usize;
            let dst = ((dy * width + dx) * 4) as usize;
            if let (Some(from), true) = (frame.buffer.get(src..src + 4), dst + 4 <= pixels.len()) {
                pixels[dst..dst + 4].copy_from_slice(from);
            }
        }
    }
    Some(Rgba {
        width,
        height,
        pixels,
    })
}

/// Area-averaged resample of `source` for the box (bilinear-free: every
/// output pixel averages the source pixels it covers, at least one), to
/// premultiplied ARGB.
fn resample(source: &Rgba, width: u32, height: u32, fit: Fit) -> Image {
    let (sw, sh) = (source.width as f32, source.height as f32);
    let (out_w, out_h, crop_x, crop_y, crop_w, crop_h) = match fit {
        Fit::Contain => {
            let scale = (width as f32 / sw).min(height as f32 / sh);
            let ow = ((sw * scale).round() as u32).clamp(1, width);
            let oh = ((sh * scale).round() as u32).clamp(1, height);
            (ow, oh, 0.0, 0.0, sw, sh)
        }
        Fit::Cover => {
            let scale = (width as f32 / sw).max(height as f32 / sh);
            let cw = width as f32 / scale;
            let ch = height as f32 / scale;
            (width, height, (sw - cw) / 2.0, (sh - ch) / 2.0, cw, ch)
        }
    };
    let mut pixels = Vec::with_capacity((out_w * out_h) as usize);
    let step_x = crop_w / out_w as f32;
    let step_y = crop_h / out_h as f32;
    let stride = source.width as usize * 4;
    for y in 0..out_h {
        let y0 = crop_y + y as f32 * step_y;
        let ya = (y0.floor() as u32).min(source.height - 1);
        let yb = (((y0 + step_y).ceil() as u32).max(ya + 1)).min(source.height);
        for x in 0..out_w {
            let x0 = crop_x + x as f32 * step_x;
            let xa = (x0.floor() as u32).min(source.width - 1);
            let xb = (((x0 + step_x).ceil() as u32).max(xa + 1)).min(source.width);
            let (mut r, mut g, mut b, mut a, mut n) = (0_u32, 0_u32, 0_u32, 0_u32, 0_u32);
            for sy in ya..yb {
                let row = sy as usize * stride;
                for sx in xa..xb {
                    let i = row + sx as usize * 4;
                    let alpha = u32::from(source.pixels[i + 3]);
                    r += u32::from(source.pixels[i]) * alpha;
                    g += u32::from(source.pixels[i + 1]) * alpha;
                    b += u32::from(source.pixels[i + 2]) * alpha;
                    a += alpha;
                    n += 1;
                }
            }
            let n = n.max(1);
            let alpha = a / n;
            // Premultiplied: each channel averaged with its alpha.
            let premultiplied = |c: u32| (c / n / 255).min(alpha);
            pixels.push(
                alpha << 24 | premultiplied(r) << 16 | premultiplied(g) << 8 | premultiplied(b),
            );
        }
    }
    Image {
        width: out_w as i32,
        height: out_h as i32,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &std::path::Path, width: u32, height: u32, rgb: [u8; 3]) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header().unwrap();
        let data: Vec<u8> = (0..width * height).flat_map(|_| rgb).collect();
        writer.write_image_data(&data).unwrap();
    }

    #[test]
    fn pictures_fit_or_cover_their_box() {
        let dir = std::env::temp_dir().join(format!("social-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        write_png(&path, 400, 200, [255, 0, 0]);
        let contain = decode_file(path.to_str().unwrap(), 100, 100, Fit::Contain).unwrap();
        assert_eq!((contain.width, contain.height), (100, 50));
        assert_eq!(contain.pixels[0], 0xffff_0000);
        let cover = decode_file(path.to_str().unwrap(), 100, 100, Fit::Cover).unwrap();
        assert_eq!((cover.width, cover.height), (100, 100));
        assert!(decode_file(dir.join("missing").to_str().unwrap(), 10, 10, Fit::Cover).is_none());
        std::fs::write(dir.join("junk"), b"not a picture").unwrap();
        assert!(decode_file(dir.join("junk").to_str().unwrap(), 10, 10, Fit::Cover).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn half_transparent_pixels_are_premultiplied() {
        let source = Rgba {
            width: 2,
            height: 1,
            pixels: vec![255, 255, 255, 255, 255, 255, 255, 0],
        };
        let image = resample(&source, 1, 1, Fit::Contain);
        let pixel = image.pixels[0];
        assert_eq!(pixel >> 24, 127);
        assert_eq!(pixel & 0xff, 127);
    }
}
