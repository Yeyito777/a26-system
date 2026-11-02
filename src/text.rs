//! Vendored from Moon (OSS-Phone-V1 `src/a26-shell/src`); keep in sync.
#![allow(dead_code)]
//! Anti-aliased text using the embedded Inter typeface (SIL OFL 1.1, see
//! `assets/fonts/OFL.txt`; this copy, unlike Moon's, adds JetBrains Mono NL
//! in five cuts, `OFL-JetBrainsMono.txt`, subset by
//! `scripts/subset-fonts.sh`, for the Exocortex app's Whale look, which is
//! monospaced throughout), with fallbacks for everything Inter lacks so a
//! message never shows a '?': Noto Sans (Latin extended, Greek, Cyrillic,
//! Vietnamese), Noto Sans Symbols 2, Symbols and Math, and Noto Color Emoji (colour
//! bitmaps; ZWJ sequences, skin tones, keycaps and flags through its
//! ligatures). The fallbacks are files in `FONT_DIR` (`MOON_FONTS` overrides
//! it), memory-mapped the first time a character needs them, so they cost
//! nothing until then. A character no font has draws as an empty box.
//! Glyphs are rasterized once per weight/size/character and cached for the
//! life of the process. An emoji's bitmap (a PNG in the font, decoded and
//! scaled: a few million instructions) is made on an emoji thread from the
//! moment its glyph is first wanted, so text laid out before it is drawn
//! draws it ready; drawn before that thread made it, it is made in place,
//! as before. Decoded PNGs are kept a while (`STRIKE_PIXELS`), so an
//! emoji at another size is only scaled.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use fontdue::{Font, FontSettings};
use ttf_parser::{Face, GlyphId, OutlineBuilder, RasterImageFormat};

use crate::gfx::{Canvas, Image, Path, Rgb};

pub const FONT_DIR: &str = "/opt/a26-shell/share/fonts";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Medium,
    SemiBold,
    Bold,
    /// Optical "Display" cuts are drawn tighter for large sizes.
    DisplayLight,
    DisplayMedium,
    DisplaySemiBold,
    /// JetBrains Mono NL: the Exocortex app's text, in its five cuts.
    Mono,
    MonoMedium,
    MonoSemiBold,
    MonoBold,
    MonoItalic,
}

const WEIGHTS: [Weight; 12] = [
    Weight::Regular,
    Weight::Medium,
    Weight::SemiBold,
    Weight::Bold,
    Weight::DisplayLight,
    Weight::DisplayMedium,
    Weight::DisplaySemiBold,
    Weight::Mono,
    Weight::MonoMedium,
    Weight::MonoSemiBold,
    Weight::MonoBold,
    Weight::MonoItalic,
];

fn font_bytes(weight: Weight) -> &'static [u8] {
    match weight {
        Weight::Regular => include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        Weight::Medium => include_bytes!("../assets/fonts/Inter-Medium.ttf"),
        Weight::SemiBold => include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
        Weight::Bold => include_bytes!("../assets/fonts/Inter-Bold.ttf"),
        Weight::DisplayLight => include_bytes!("../assets/fonts/InterDisplay-Light.ttf"),
        Weight::DisplayMedium => include_bytes!("../assets/fonts/InterDisplay-Medium.ttf"),
        Weight::DisplaySemiBold => include_bytes!("../assets/fonts/InterDisplay-SemiBold.ttf"),
        Weight::Mono => include_bytes!("../assets/fonts/JetBrainsMonoNL-Regular.ttf"),
        Weight::MonoMedium => include_bytes!("../assets/fonts/JetBrainsMonoNL-Medium.ttf"),
        Weight::MonoSemiBold => include_bytes!("../assets/fonts/JetBrainsMonoNL-SemiBold.ttf"),
        Weight::MonoBold => include_bytes!("../assets/fonts/JetBrainsMonoNL-Bold.ttf"),
        Weight::MonoItalic => include_bytes!("../assets/fonts/JetBrainsMonoNL-Italic.ttf"),
    }
}

/// The fallback files, in the order they are tried.
const NOTO_REGULAR: usize = 0;
const NOTO_SEMIBOLD: usize = 1;
const SYMBOLS_2: usize = 2;
const SYMBOLS: usize = 3;
const MATH: usize = 4;
const EMOJI: usize = 5;
const FALLBACK_FILES: [&str; 6] = [
    "NotoSans-Regular.ttf",
    "NotoSans-SemiBold.ttf",
    "NotoSansSymbols2-Regular.ttf",
    "NotoSansSymbols-Regular.ttf",
    "NotoSansMath-Regular.ttf",
    "NotoColorEmoji.ttf",
];

enum Shape {
    /// Coverage, drawn in the text's colour.
    Mask(Vec<u8>),
    /// An emoji: its bitmap (premultiplied ARGB, drawn as is) is made on
    /// the emoji thread once the glyph is first wanted (a layout measures
    /// text before it draws it), or here if it is drawn before that.
    Emoji(RefCell<Bitmap>),
}

enum Bitmap {
    Asked(Arc<Job>),
    Made(Option<Made>),
}

/// An emoji's bitmap and where it goes from the pen (x) and baseline (y).
struct Made {
    x: i32,
    y: i32,
    image: Image,
}

/// An emoji bitmap asked of the emoji thread.
struct Job {
    size: f32,
    ids: Vec<GlyphId>,
    asked: std::time::Instant,
    state: Mutex<JobState>,
}

enum JobState {
    Waiting,
    Making,
    Done(Option<Made>),
    /// Taken (or made by the thread that draws it).
    Gone,
}

/// Emoji bitmaps asked of the emoji thread and not yet made, at most;
/// past it they are made when drawn.
const JOBS: usize = 64;
/// How long the emoji thread leaves a bitmap after it was asked for: one
/// drawn in the frame that measured it is made there, once, not twice.
const JOB_AFTER: std::time::Duration = std::time::Duration::from_millis(12);
static WAITING: AtomicUsize = AtomicUsize::new(0);
static EMOJI_THREAD: OnceLock<Option<mpsc::Sender<Arc<Job>>>> = OnceLock::new();

/// The emoji thread, started the first time an emoji is wanted: it makes
/// the bitmaps asked of it with a face of its own on the same font data.
fn emoji_thread(data: &'static [u8]) -> Option<&'static mpsc::Sender<Arc<Job>>> {
    EMOJI_THREAD
        .get_or_init(|| {
            let (sender, jobs) = mpsc::channel::<Arc<Job>>();
            std::thread::Builder::new()
                .name("emoji".into())
                .spawn(move || {
                    let face = Face::parse(data, 0).ok();
                    let mut strikes = Vec::new();
                    for job in jobs {
                        WAITING.fetch_sub(1, Ordering::Relaxed);
                        let Some(face) = &face else { continue };
                        if let Some(left) = JOB_AFTER.checked_sub(job.asked.elapsed()) {
                            std::thread::sleep(left);
                        }
                        {
                            let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
                            if !matches!(*state, JobState::Waiting) {
                                continue;
                            }
                            *state = JobState::Making;
                        }
                        let made = make_emoji(face, job.size, &job.ids, &mut strikes);
                        let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
                        if matches!(*state, JobState::Making) {
                            *state = JobState::Done(made);
                        }
                    }
                })
                .ok()?;
            Some(sender)
        })
        .as_ref()
}

struct Glyph {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    advance: f32,
    shape: Shape,
}

impl Glyph {
    fn empty(advance: f32) -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            advance,
            shape: Shape::Mask(Vec::new()),
        }
    }
}

/// What one glyph stands for: a character, or an emoji sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Cluster {
    Char(char),
    Emoji(Box<[char]>),
}

enum Fallback {
    Unloaded,
    Missing,
    Loaded(Box<Face<'static>>),
}

struct Typefaces {
    fallbacks: Vec<Fallback>,
    glyphs: HashMap<(Weight, u16, Cluster), Rc<Glyph>, BuildHasherDefault<GlyphHasher>>,
    gamma: [u8; 256],
    /// Emoji bitmaps as decoded from the font, the most recently used
    /// first, up to `STRIKE_PIXELS`: an emoji wanted at another size is
    /// only scaled again.
    strikes: Vec<(GlyphId, Rc<Strike>)>,
    /// The emoji font's bytes, for the emoji thread.
    emoji_data: Option<&'static [u8]>,
}

/// An emoji's bitmap at the font's strike size, premultiplied ARGB.
struct Strike {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

/// What the decoded emoji bitmaps may hold, in pixels, on each thread that
/// makes them (the UI's and the emoji thread): Noto's are 136 by 128, so
/// about thirty emoji, 2 MB.
const STRIKE_PIXELS: usize = 1 << 19;

thread_local! {
    static TYPEFACES: RefCell<Typefaces> = RefCell::new(Typefaces::load());
}

fn font_dir() -> std::path::PathBuf {
    std::env::var_os("MOON_FONTS")
        .map(Into::into)
        .unwrap_or_else(|| FONT_DIR.into())
}

/// Maps a font file read-only for the life of the process; its pages load
/// only as glyphs are used.
fn map_file(path: &std::path::Path) -> Option<&'static [u8]> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path).ok()?;
    let length = usize::try_from(file.metadata().ok()?.len()).ok()?;
    if length == 0 {
        return None;
    }
    // SAFETY: a private read-only mapping of a regular file, never unmapped,
    // so the slice stays valid for 'static. The fonts are installed files
    // nobody truncates while Moon runs.
    unsafe {
        let address = libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        );
        if address == libc::MAP_FAILED {
            return None;
        }
        Some(std::slice::from_raw_parts(address.cast::<u8>(), length))
    }
}

/// Characters that take no room of their own (joiners, selectors, tags).
fn invisible(character: char) -> bool {
    matches!(
        u32::from(character),
        0..=0x1f
            | 0x7f..=0x9f
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x2064
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xe0000..=0xe007f
    )
}

fn regional_indicator(character: char) -> bool {
    matches!(u32::from(character), 0x1f1e6..=0x1f1ff)
}

/// Code points that start an emoji sequence (pictographs, dingbats,
/// arrows and symbols with an emoji form, regional indicators).
fn pictographic(character: char) -> bool {
    matches!(
        u32::from(character),
        0xa9 | 0xae
            | 0x203c
            | 0x2049
            | 0x2122
            | 0x2139
            | 0x2194..=0x21aa
            | 0x231a..=0x23ff
            | 0x24c2
            | 0x25aa..=0x25fe
            | 0x2600..=0x27bf
            | 0x2934..=0x2935
            | 0x2b05..=0x2b55
            | 0x3030
            | 0x303d
            | 0x3297
            | 0x3299
            | 0x1f000..=0x1faff
    )
}

/// Splits text into what each glyph stands for. An emoji sequence is a
/// pictograph (or a keycap base) with its selectors, skin tone, tags and
/// ZWJ-joined parts, or a pair of regional indicators (a flag). Lazy, so a
/// caller that stops early (`fit`) never looks at the rest.
/// The glyph cache's hasher: a multiply-rotate over the key's words (the
/// default SipHash cost more than drawing a cached glyph; the keys are the
/// app's own text, not an attacker's).
#[derive(Default)]
struct GlyphHasher(u64);

impl Hasher for GlyphHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }

    fn write_u16(&mut self, n: u16) {
        self.write_u64(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

fn clusters(text: &str) -> Clusters<'_> {
    Clusters {
        characters: text.chars().peekable(),
    }
}

struct Clusters<'a> {
    characters: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Iterator for Clusters<'_> {
    type Item = Cluster;

    fn next(&mut self) -> Option<Cluster> {
        let characters = &mut self.characters;
        let character = characters.next()?;
        let keycap = matches!(character, '0'..='9' | '#' | '*')
            && matches!(characters.peek(), Some('\u{fe0f}' | '\u{20e3}'));
        if !pictographic(character) && !keycap {
            return Some(Cluster::Char(character));
        }
        let mut sequence = vec![character];
        if regional_indicator(character) {
            if let Some(&next) = characters.peek()
                && regional_indicator(next)
            {
                sequence.push(next);
                characters.next();
            }
        } else {
            while let Some(&next) = characters.peek() {
                match u32::from(next) {
                    0xfe0e | 0xfe0f | 0x20e3 | 0x1f3fb..=0x1f3ff | 0xe0020..=0xe007f => {
                        sequence.push(next);
                        characters.next();
                    }
                    0x200d => {
                        sequence.push(next);
                        characters.next();
                        if let Some(joined) = characters.next() {
                            sequence.push(joined);
                        }
                    }
                    _ => break,
                }
            }
        }
        // A text-style selector asks for the plain character.
        if sequence.len() == 1 || sequence[1..] == ['\u{fe0e}'] {
            Some(Cluster::Char(character))
        } else {
            Some(Cluster::Emoji(sequence.into_boxed_slice()))
        }
    }
}

/// Flattens a TrueType/CFF outline into a `Path` in pixels, y down, with
/// the pen at the origin.
struct Outline {
    path: Path,
    scale: f32,
    last: (f32, f32),
}

impl Outline {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale, -y * self.scale)
    }
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let (px, py) = self.point(x, y);
        self.path.move_to(px, py);
        self.last = (px, py);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (px, py) = self.point(x, y);
        self.path.line_to(px, py);
        self.last = (px, py);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, end, start) = (self.point(x1, y1), self.point(x, y), self.last);
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            let u = 1.0 - t;
            self.path.line_to(
                u * u * start.0 + 2.0 * u * t * c.0 + t * t * end.0,
                u * u * start.1 + 2.0 * u * t * c.1 + t * t * end.1,
            );
        }
        self.last = end;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, end, start) = (
            self.point(x1, y1),
            self.point(x2, y2),
            self.point(x, y),
            self.last,
        );
        for step in 1..=12 {
            let t = step as f32 / 12.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.path.line_to(
                a * start.0 + b * c1.0 + c * c2.0 + d * end.0,
                a * start.1 + b * c1.1 + c * c2.1 + d * end.1,
            );
        }
        self.last = end;
    }

    fn close(&mut self) {
        self.path.close();
    }
}

/// Applies the font's ligatures (GSUB type 4) to a glyph run, lookup by
/// lookup, as shaping would for the emoji font's sequences.
fn ligate(face: &Face, mut glyphs: Vec<GlyphId>) -> Vec<GlyphId> {
    use ttf_parser::gsub::SubstitutionSubtable;
    let Some(gsub) = face.tables().gsub else {
        return glyphs;
    };
    for lookup in gsub.lookups {
        for index in 0..lookup.subtables.len() {
            let Some(SubstitutionSubtable::Ligature(subtable)) =
                lookup.subtables.get::<SubstitutionSubtable>(index)
            else {
                continue;
            };
            let mut position = 0;
            while position < glyphs.len() {
                let replaced = subtable
                    .coverage
                    .get(glyphs[position])
                    .and_then(|set| subtable.ligature_sets.get(set))
                    .and_then(|set| {
                        set.into_iter().find(|ligature| {
                            let count = usize::from(ligature.components.len());
                            position + count < glyphs.len()
                                && ligature
                                    .components
                                    .into_iter()
                                    .zip(&glyphs[position + 1..])
                                    .filter(|(a, b)| a == *b)
                                    .count()
                                    == count
                        })
                    });
                if let Some(ligature) = replaced {
                    let count = usize::from(ligature.components.len());
                    glyphs.splice(position..=position + count, [ligature.glyph]);
                }
                position += 1;
            }
        }
    }
    glyphs
}

/// Decodes a PNG strike glyph into premultiplied ARGB.
pub(crate) fn decode_png(data: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    let channels = info.color_type.samples();
    let pixels = buffer[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|p| {
            let (r, g, b, a) = match channels {
                4 => (p[0], p[1], p[2], p[3]),
                3 => (p[0], p[1], p[2], 255),
                2 => (p[0], p[0], p[0], p[1]),
                _ => (p[0], p[0], p[0], 255),
            };
            let a32 = u32::from(a);
            let pre = |c: u8| (u32::from(c) * a32 + 127) / 255;
            a32 << 24 | pre(r) << 16 | pre(g) << 8 | pre(b)
        })
        .collect();
    Some((info.width, info.height, pixels))
}

/// Area-averaged downscale of premultiplied pixels (bitmap emoji are drawn
/// well below their strike size).
fn resample(width: u32, height: u32, pixels: &[u32], to_w: u32, to_h: u32) -> Vec<u32> {
    let (sx, sy) = (width as f32 / to_w as f32, height as f32 / to_h as f32);
    let mut out = vec![0; (to_w * to_h) as usize];
    for y in 0..to_h {
        let (y0, y1) = (y as f32 * sy, (y + 1) as f32 * sy);
        for x in 0..to_w {
            let (x0, x1) = (x as f32 * sx, (x + 1) as f32 * sx);
            let mut sum = [0.0_f32; 4];
            let mut total = 0.0;
            let mut yy = y0.floor() as u32;
            while (yy as f32) < y1 && yy < height {
                let wy = ((yy + 1) as f32).min(y1) - (yy as f32).max(y0);
                let mut xx = x0.floor() as u32;
                while (xx as f32) < x1 && xx < width {
                    let wx = ((xx + 1) as f32).min(x1) - (xx as f32).max(x0);
                    let w = wx * wy;
                    let p = pixels[(yy * width + xx) as usize];
                    for (channel, value) in sum.iter_mut().enumerate() {
                        *value += (p >> (24 - 8 * channel) & 0xff) as f32 * w;
                    }
                    total += w;
                    xx += 1;
                }
                yy += 1;
            }
            let total = total.max(1e-6);
            out[(y * to_w + x) as usize] = sum
                .iter()
                .enumerate()
                .map(|(channel, value)| {
                    ((value / total + 0.5) as u32).min(255) << (24 - 8 * channel)
                })
                .fold(0, |pixel, part| pixel | part);
        }
    }
    out
}

/// The embedded faces, each parsed the first time it is needed (fontdue
/// parses every glyph up front: Inter's nine took 63 ms on the A26's fast core,
/// a cold start's longest step), or ahead of time by `preload`.
static FACES: [OnceLock<Font>; 12] = [const { OnceLock::new() }; 12];

fn face(weight: Weight) -> &'static Font {
    let index = WEIGHTS.iter().position(|w| *w == weight).unwrap_or(0);
    FACES[index].get_or_init(|| {
        Font::from_bytes(font_bytes(WEIGHTS[index]), FontSettings::default())
            .expect("embedded font is valid")
    })
}

/// Parses `weights` on threads of their own, in parallel, so text drawn
/// later is not held up by them (a face still being parsed when it is first
/// needed is waited for, not parsed twice).
pub fn preload(weights: &[Weight]) {
    for &weight in weights {
        let _ = std::thread::Builder::new()
            .name("font-preload".into())
            .spawn(move || {
                face(weight);
            });
    }
}

impl Typefaces {
    fn load() -> Self {
        // Light text on Moon's dark surfaces reads slightly thin with linear
        // coverage; a mild curve restores the designed stroke weight.
        let mut gamma = [0_u8; 256];
        for (index, value) in gamma.iter_mut().enumerate() {
            *value = ((index as f32 / 255.0).powf(0.84) * 255.0 + 0.5) as u8;
        }
        Self {
            fallbacks: FALLBACK_FILES.iter().map(|_| Fallback::Unloaded).collect(),
            glyphs: HashMap::default(),
            gamma,
            strikes: Vec::new(),
            emoji_data: None,
        }
    }

    fn fallback(&mut self, index: usize) -> Option<&Face<'static>> {
        if let Fallback::Unloaded = self.fallbacks[index] {
            let path = font_dir().join(FALLBACK_FILES[index]);
            let data = map_file(&path);
            if index == EMOJI {
                self.emoji_data = data;
            }
            self.fallbacks[index] = match data.and_then(|data| Face::parse(data, 0).ok()) {
                Some(face) => Fallback::Loaded(Box::new(face)),
                None => {
                    eprintln!("font fallback unavailable: {}", path.display());
                    Fallback::Missing
                }
            };
        }
        match &self.fallbacks[index] {
            Fallback::Loaded(face) => Some(face),
            _ => None,
        }
    }

    fn glyph(&mut self, weight: Weight, size: f32, cluster: &Cluster) -> Rc<Glyph> {
        let key = (weight, (size * 2.0).round() as u16, cluster.clone());
        if let Some(glyph) = self.glyphs.get(&key) {
            return glyph.clone();
        }
        let size = f32::from(key.1) / 2.0;
        let glyph = Rc::new(match cluster {
            Cluster::Char(character) => self.char_glyph(weight, size, *character),
            Cluster::Emoji(sequence) => self.emoji_glyph(weight, size, sequence),
        });
        self.glyphs.insert(key, glyph.clone());
        glyph
    }

    fn inter(&self, weight: Weight) -> &'static Font {
        face(weight)
    }

    fn char_glyph(&mut self, weight: Weight, size: f32, character: char) -> Glyph {
        if invisible(character) {
            return Glyph::empty(0.0);
        }
        if self.inter(weight).lookup_glyph_index(character) != 0 {
            return self.inter_glyph(weight, size, character);
        }
        if character.is_whitespace() {
            return Glyph::empty(self.inter(weight).metrics(' ', size).advance_width);
        }
        let noto = match weight {
            Weight::SemiBold
            | Weight::Bold
            | Weight::DisplaySemiBold
            | Weight::MonoSemiBold
            | Weight::MonoBold => NOTO_SEMIBOLD,
            _ => NOTO_REGULAR,
        };
        for index in [noto, SYMBOLS_2, SYMBOLS, MATH] {
            if let Some(glyph) = self.outline_glyph(index, size, character) {
                return glyph;
            }
        }
        if let Some(face) = self.fallback(EMOJI)
            && let Some(id) = face.glyph_index(character)
            && let Some(glyph) = self.color_glyph(size, &[id])
        {
            return glyph;
        }
        self.tofu(weight, size)
    }

    fn emoji_glyph(&mut self, weight: Weight, size: f32, sequence: &[char]) -> Glyph {
        let ids = self.fallback(EMOJI).and_then(|face| {
            let first = face.glyph_index(sequence[0])?;
            let mut ids = vec![first];
            ids.extend(sequence[1..].iter().filter_map(|c| face.glyph_index(*c)));
            Some(ligate(face, ids))
        });
        if let Some(glyph) = ids.and_then(|ids| self.color_glyph(size, &ids)) {
            return glyph;
        }
        // No emoji font: the parts as text, joiners and selectors dropped.
        let mut advance = 0.0;
        let mut first = None;
        for character in sequence.iter().copied().filter(|c| !invisible(*c)) {
            let glyph = self.char_glyph(weight, size, character);
            advance += glyph.advance;
            first.get_or_insert(glyph);
        }
        let mut glyph = first.unwrap_or_else(|| Glyph::empty(0.0));
        glyph.advance = advance;
        glyph
    }

    fn inter_glyph(&self, weight: Weight, size: f32, character: char) -> Glyph {
        let (metrics, mut mask) = self.inter(weight).rasterize(character, size);
        for value in &mut mask {
            *value = self.gamma[usize::from(*value)];
        }
        Glyph {
            x: metrics.xmin,
            y: -(metrics.height as i32) - metrics.ymin,
            width: metrics.width as i32,
            height: metrics.height as i32,
            advance: metrics.advance_width,
            shape: Shape::Mask(mask),
        }
    }

    fn outline_glyph(&mut self, index: usize, size: f32, character: char) -> Option<Glyph> {
        let gamma = self.gamma;
        let face = self.fallback(index)?;
        let id = face.glyph_index(character)?;
        let scale = size / f32::from(face.units_per_em());
        let advance = f32::from(face.glyph_hor_advance(id).unwrap_or(0)) * scale;
        let mut outline = Outline {
            path: Path::new(),
            scale,
            last: (0.0, 0.0),
        };
        if face.outline_glyph(id, &mut outline).is_none() {
            return Some(Glyph::empty(advance));
        }
        outline.path.close();
        let Some((x, y, width, height, mut mask)) = outline.path.rasterize() else {
            return Some(Glyph::empty(advance));
        };
        for value in &mut mask {
            *value = gamma[usize::from(*value)];
        }
        Some(Glyph {
            x,
            y,
            width,
            height,
            advance,
            shape: Shape::Mask(mask),
        })
    }

    /// Emoji glyphs side by side (a sequence the font has no single
    /// ligature for), scaled from the bitmap strike to `size`: their
    /// advance now, their bitmap asked of the emoji thread.
    fn color_glyph(&mut self, size: f32, ids: &[GlyphId]) -> Option<Glyph> {
        let face = self.fallback(EMOJI)?;
        let upem = f32::from(face.units_per_em());
        let strike = face.glyph_raster_image(ids[0], u16::MAX)?.pixels_per_em;
        let mut pen = 0.0;
        let mut any = false;
        for id in ids {
            pen += f32::from(face.glyph_hor_advance(*id).unwrap_or(0)) * size / upem;
            any |= face
                .glyph_raster_image(*id, strike)
                .is_some_and(|image| image.format == RasterImageFormat::PNG);
        }
        // (A PNG that then fails to decode draws nothing, where text drew
        // its parts before; the font's all decode.)
        if !any {
            return None;
        }
        let job = Arc::new(Job {
            size,
            ids: ids.to_vec(),
            asked: std::time::Instant::now(),
            state: Mutex::new(JobState::Waiting),
        });
        if WAITING.load(Ordering::Relaxed) < JOBS
            && let Some(thread) = self.emoji_data.and_then(emoji_thread)
        {
            WAITING.fetch_add(1, Ordering::Relaxed);
            if thread.send(job.clone()).is_err() {
                WAITING.fetch_sub(1, Ordering::Relaxed);
            }
        }
        Some(Glyph {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            advance: pen,
            shape: Shape::Emoji(RefCell::new(Bitmap::Asked(job))),
        })
    }

    /// An emoji's bitmap, to draw it: the emoji thread's if it is made,
    /// else made here (the UI thread does not wait for one that may run
    /// on a slower core).
    fn made(&mut self, bitmap: &RefCell<Bitmap>) {
        let job = match &*bitmap.borrow() {
            Bitmap::Made(_) => return,
            Bitmap::Asked(job) => job.clone(),
        };
        let done = {
            let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
            match std::mem::replace(&mut *state, JobState::Gone) {
                JobState::Done(made) => Some(made),
                _ => None,
            }
        };
        let made = match done {
            Some(made) => made,
            None => {
                let Fallback::Loaded(face) = &self.fallbacks[EMOJI] else {
                    *bitmap.borrow_mut() = Bitmap::Made(None);
                    return;
                };
                make_emoji(face, job.size, &job.ids, &mut self.strikes)
            }
        };
        *bitmap.borrow_mut() = Bitmap::Made(made);
    }

    /// An empty box: a character no font has.
    fn tofu(&self, weight: Weight, size: f32) -> Glyph {
        let advance = self.inter(weight).metrics('0', size).advance_width;
        let width = (size * 0.5).round().max(3.0) as i32;
        let height = (size * 0.7).round().max(4.0) as i32;
        let stroke = (size / 16.0).round().max(1.0) as i32;
        let mut mask = vec![0_u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                let edge = x < stroke || y < stroke || x >= width - stroke || y >= height - stroke;
                if edge {
                    mask[(y * width + x) as usize] = 200;
                }
            }
        }
        Glyph {
            x: ((advance - width as f32) / 2.0).round() as i32,
            y: -height,
            width,
            height,
            advance,
            shape: Shape::Mask(mask),
        }
    }
}

/// The bitmap of emoji glyphs `ids` side by side at `size`: each scaled
/// from the font's strike, one image covering them all.
fn make_emoji(
    face: &Face,
    size: f32,
    ids: &[GlyphId],
    strikes: &mut Vec<(GlyphId, Rc<Strike>)>,
) -> Option<Made> {
    let upem = f32::from(face.units_per_em());
    let strike = face.glyph_raster_image(ids[0], u16::MAX)?.pixels_per_em;
    let scale = size / f32::from(strike);
    let mut parts = Vec::new();
    let mut pen = 0.0;
    for id in ids {
        let advance = f32::from(face.glyph_hor_advance(*id).unwrap_or(0)) * size / upem;
        if let Some(image) = face.glyph_raster_image(*id, strike)
            && image.format == RasterImageFormat::PNG
            && let Some(decoded) = decoded_strike(strikes, *id, image.data)
        {
            let (w, h) = (decoded.width, decoded.height);
            let to_w = ((w as f32 * scale).round() as u32).max(1);
            let to_h = ((h as f32 * scale).round() as u32).max(1);
            let left = (pen + f32::from(image.x) * scale).round() as i32;
            let top = -((f32::from(image.y) + f32::from(image.height)) * scale).round() as i32;
            parts.push((
                left,
                top,
                to_w,
                to_h,
                resample(w, h, &decoded.pixels, to_w, to_h),
            ));
        }
        pen += advance;
    }
    if parts.is_empty() {
        return None;
    }
    // One image covering every part.
    let x0 = parts.iter().map(|p| p.0).min()?;
    let y0 = parts.iter().map(|p| p.1).min()?;
    let x1 = parts.iter().map(|p| p.0 + p.2 as i32).max()?;
    let y1 = parts.iter().map(|p| p.1 + p.3 as i32).max()?;
    let (width, height) = (x1 - x0, y1 - y0);
    let mut pixels = vec![0_u32; (width * height) as usize];
    for (left, top, w, h, part) in parts {
        for row in 0..h as i32 {
            for column in 0..w as i32 {
                let source = part[(row * w as i32 + column) as usize];
                let index = ((top - y0 + row) * width + left - x0 + column) as usize;
                if source >> 24 > pixels[index] >> 24 {
                    pixels[index] = source;
                }
            }
        }
    }
    Some(Made {
        x: x0,
        y: y0,
        image: Image {
            width,
            height,
            pixels,
        },
    })
}

/// Glyph `id`'s bitmap `data` decoded, from the cache when it was
/// decoded lately.
fn decoded_strike(
    strikes: &mut Vec<(GlyphId, Rc<Strike>)>,
    id: GlyphId,
    data: &[u8],
) -> Option<Rc<Strike>> {
    if let Some(at) = strikes.iter().position(|(cached, _)| *cached == id) {
        let entry = strikes.remove(at);
        let strike = entry.1.clone();
        strikes.insert(0, entry);
        return Some(strike);
    }
    let (width, height, pixels) = decode_png(data)?;
    let strike = Rc::new(Strike {
        width,
        height,
        pixels,
    });
    strikes.insert(0, (id, strike.clone()));
    let mut held = 0;
    strikes.retain(|(_, kept)| {
        held += kept.pixels.len();
        held <= STRIKE_PIXELS
    });
    Some(strike)
}

/// Typographic advance width of `text`, including `tracking` between glyphs.
pub fn measure_tracked(text: &str, weight: Weight, size: f32, tracking: f32) -> f32 {
    TYPEFACES.with(|cell| {
        let mut faces = cell.borrow_mut();
        let mut width = 0.0;
        let mut count = 0;
        for cluster in clusters(text) {
            width += faces.glyph(weight, size, &cluster).advance;
            count += 1;
        }
        width + tracking * (count.max(1) - 1) as f32
    })
}

pub fn measure(text: &str, weight: Weight, size: f32) -> f32 {
    measure_tracked(text, weight, size, 0.0)
}

/// Draws `text` with its alphabetic baseline at `baseline`; returns its width.
#[allow(clippy::too_many_arguments)]
pub fn draw_tracked(
    canvas: &mut Canvas,
    text: &str,
    x: f32,
    baseline: f32,
    weight: Weight,
    size: f32,
    tracking: f32,
    color: Rgb,
    alpha: f32,
) -> f32 {
    TYPEFACES.with(|cell| {
        let mut faces = cell.borrow_mut();
        let mut pen = x;
        for cluster in clusters(text) {
            let glyph = faces.glyph(weight, size, &cluster);
            if let Shape::Emoji(bitmap) = &glyph.shape {
                faces.made(bitmap);
                if let Bitmap::Made(Some(made)) = &*bitmap.borrow() {
                    let (gx, gy) = (
                        pen.round() as i32 + made.x,
                        baseline.round() as i32 + made.y,
                    );
                    canvas.draw_image(&made.image, gx, gy, alpha);
                }
            } else if glyph.width > 0 && glyph.height > 0 {
                let (gx, gy) = (
                    pen.round() as i32 + glyph.x,
                    baseline.round() as i32 + glyph.y,
                );
                match &glyph.shape {
                    Shape::Mask(mask) => {
                        canvas.draw_mask(gx, gy, glyph.width, glyph.height, mask, color, alpha);
                    }
                    Shape::Emoji(_) => {}
                }
            }
            pen += glyph.advance + tracking;
        }
        pen - tracking - x
    })
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas,
    text: &str,
    x: f32,
    baseline: f32,
    weight: Weight,
    size: f32,
    color: Rgb,
    alpha: f32,
) -> f32 {
    draw_tracked(canvas, text, x, baseline, weight, size, 0.0, color, alpha)
}

#[allow(clippy::too_many_arguments)]
pub fn draw_centered(
    canvas: &mut Canvas,
    text: &str,
    center_x: f32,
    baseline: f32,
    weight: Weight,
    size: f32,
    color: Rgb,
    alpha: f32,
) {
    let width = measure(text, weight, size);
    draw(
        canvas,
        text,
        center_x - width / 2.0,
        baseline,
        weight,
        size,
        color,
        alpha,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn draw_right(
    canvas: &mut Canvas,
    text: &str,
    right_x: f32,
    baseline: f32,
    weight: Weight,
    size: f32,
    color: Rgb,
    alpha: f32,
) {
    let width = measure(text, weight, size);
    draw(
        canvas,
        text,
        right_x - width,
        baseline,
        weight,
        size,
        color,
        alpha,
    );
}

/// Height of capital letters above the baseline; used to optically center
/// labels inside controls.
pub fn cap_height(weight: Weight, size: f32) -> f32 {
    TYPEFACES.with(|cell| {
        cell.borrow_mut()
            .glyph(weight, size, &Cluster::Char('H'))
            .height as f32
    })
}

/// Baseline that vertically centers capitals around `center_y`.
pub fn centered_baseline(center_y: f32, weight: Weight, size: f32) -> f32 {
    center_y + cap_height(weight, size) / 2.0
}

/// Shortens `text` with an ellipsis so it fits in `max_width`, never
/// splitting an emoji sequence.
pub fn fit(text: &str, weight: Weight, size: f32, max_width: f32) -> String {
    // One pass, stopping once the text overflows: a tool call's summary can
    // be a 10 KB command, and a row shows a few dozen characters of it.
    TYPEFACES.with(|cell| {
        let mut faces = cell.borrow_mut();
        let ellipsis = faces.glyph(weight, size, &Cluster::Char('…')).advance;
        let mut width = 0.0;
        let mut end = 0;
        // The longest prefix that fits with an ellipsis after it.
        let mut kept = 0;
        for cluster in clusters(text) {
            width += faces.glyph(weight, size, &cluster).advance;
            if width > max_width {
                return format!("{}…", &text[..kept]);
            }
            end += match &cluster {
                Cluster::Char(character) => character.len_utf8(),
                Cluster::Emoji(sequence) => sequence.iter().map(|c| c.len_utf8()).sum(),
            };
            if width + ellipsis <= max_width {
                kept = end;
            }
        }
        text.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_measure_and_render() {
        for weight in WEIGHTS {
            assert!(measure("Moon", weight, 40.0) > 60.0);
            assert!(cap_height(weight, 40.0) > 20.0);
        }
        let mut canvas = Canvas::new(200, 80);
        let width = draw(
            &mut canvas,
            "Hi",
            10.0,
            60.0,
            Weight::SemiBold,
            48.0,
            0xffffff,
            1.0,
        );
        assert!(width > 30.0);
        assert!(canvas.pixels.contains(&0xffffff));
    }

    #[test]
    fn fit_truncates_with_an_ellipsis() {
        let long = "An extremely long network name that will not fit";
        let fitted = fit(long, Weight::Regular, 40.0, 300.0);
        assert!(fitted.ends_with('…'));
        assert!(measure(&fitted, Weight::Regular, 40.0) <= 300.0);
        assert_eq!(fit("Short", Weight::Regular, 40.0, 300.0), "Short");
    }

    #[test]
    fn emoji_sequences_are_one_cluster() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let flag = "\u{1f1e8}\u{1f1e6}";
        let text = format!(
            "a{family}{flag}\u{2764}\u{fe0f}1\u{fe0f}\u{20e3}\u{1f44d}\u{1f3fd}\u{263a}\u{fe0e}b"
        );
        let parts: Vec<Cluster> = clusters(&text).collect();
        assert_eq!(parts.len(), 8, "{parts:?}");
        assert_eq!(parts[0], Cluster::Char('a'));
        assert!(matches!(&parts[1], Cluster::Emoji(s) if s.len() == 5));
        assert!(matches!(&parts[2], Cluster::Emoji(s) if s.len() == 2));
        assert!(matches!(&parts[3], Cluster::Emoji(s) if s.len() == 2));
        assert!(matches!(&parts[4], Cluster::Emoji(s) if s.len() == 3));
        assert!(matches!(&parts[5], Cluster::Emoji(s) if s.len() == 2));
        assert_eq!(parts[6], Cluster::Char('\u{263a}'));
        assert_eq!(parts[7], Cluster::Char('b'));
        // Digits stay text unless they are keycaps.
        assert_eq!(
            clusters("12").collect::<Vec<_>>(),
            vec![Cluster::Char('1'), Cluster::Char('2')]
        );
    }

    /// Emoji as 9d191cb drew them, to compare with: the glyphs' ids as
    /// `char_glyph` and `emoji_glyph` found them, each part decoded and
    /// scaled on the spot. (x, y, image, advance)
    fn reference_emoji(
        face: &Face,
        size: f32,
        cluster: &Cluster,
    ) -> Option<(i32, i32, Image, f32)> {
        let ids = match cluster {
            Cluster::Char(character) => vec![face.glyph_index(*character)?],
            Cluster::Emoji(sequence) => {
                let mut ids = vec![face.glyph_index(sequence[0])?];
                ids.extend(sequence[1..].iter().filter_map(|c| face.glyph_index(*c)));
                ligate(face, ids)
            }
        };
        let upem = f32::from(face.units_per_em());
        let strike = face.glyph_raster_image(ids[0], u16::MAX)?.pixels_per_em;
        let scale = size / f32::from(strike);
        let mut parts = Vec::new();
        let mut pen = 0.0;
        for id in &ids {
            let advance = f32::from(face.glyph_hor_advance(*id).unwrap_or(0)) * size / upem;
            if let Some(image) = face.glyph_raster_image(*id, strike)
                && image.format == RasterImageFormat::PNG
                && let Some((w, h, pixels)) = decode_png(image.data)
            {
                let to_w = ((w as f32 * scale).round() as u32).max(1);
                let to_h = ((h as f32 * scale).round() as u32).max(1);
                let left = (pen + f32::from(image.x) * scale).round() as i32;
                let top = -((f32::from(image.y) + f32::from(image.height)) * scale).round() as i32;
                parts.push((left, top, to_w, to_h, resample(w, h, &pixels, to_w, to_h)));
            }
            pen += advance;
        }
        if parts.is_empty() {
            return None;
        }
        let x0 = parts.iter().map(|p| p.0).min()?;
        let y0 = parts.iter().map(|p| p.1).min()?;
        let x1 = parts.iter().map(|p| p.0 + p.2 as i32).max()?;
        let y1 = parts.iter().map(|p| p.1 + p.3 as i32).max()?;
        let (width, height) = (x1 - x0, y1 - y0);
        let mut pixels = vec![0_u32; (width * height) as usize];
        for (left, top, w, h, part) in parts {
            for row in 0..h as i32 {
                for column in 0..w as i32 {
                    let source = part[(row * w as i32 + column) as usize];
                    let index = ((top - y0 + row) * width + left - x0 + column) as usize;
                    if source >> 24 > pixels[index] >> 24 {
                        pixels[index] = source;
                    }
                }
            }
        }
        Some((
            x0,
            y0,
            Image {
                width,
                height,
                pixels,
            },
            pen,
        ))
    }

    /// Emoji draw as they did before the decoded bitmaps were kept: at the
    /// Exocortex app's sizes (its type scale, at small and large text) and
    /// Discord's, at whole and fractional positions, clipped or not, faded
    /// or not, measured first or drawn straight away, and drawn again.
    /// Needs the emoji font (MOON_FONTS or the phone's); without it there
    /// is nothing to compare.
    #[test]
    fn emoji_draw_as_they_did() {
        // (A lone pictograph without U+FE0F is text: Noto Sans Symbols 2.)
        let emoji = [
            "\u{1f44d}\u{fe0f}",
            "\u{1f602}\u{fe0f}",
            "\u{1f389}\u{fe0f}",
            "\u{1f525}\u{fe0f}",
            "\u{2764}\u{fe0f}",
            "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
            "\u{1f1e8}\u{1f1e6}",
            "\u{1f44d}\u{1f3fd}",
            "1\u{fe0f}\u{20e3}",
            "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}",
            // No ligature: two bitmaps side by side.
            "\u{1f44d}\u{200d}\u{1f525}",
        ];
        // The Exocortex app's type scale (px at the default text size, and
        // at its smallest and largest), Discord's (dp 12 to 48 at 2.75).
        let sizes = [
            26.0, 28.0, 30.0, 32.0, 33.0, 34.0, 36.0, 40.0, 42.0, 46.0, 48.0, 56.0, 20.0, 52.0,
            90.0, 126.0, 33.0, 38.5, 44.0, 60.5, 132.0, 13.0,
        ];
        let places = [
            (10.0, 60.0),
            (10.3, 60.5),
            (10.5, 61.4),
            (10.75, 120.0),
            (-20.0, 5.0),
        ];
        let present = TYPEFACES.with(|cell| cell.borrow_mut().fallback(EMOJI).is_some());
        if !present {
            eprintln!(
                "emoji_draw_as_they_did: no emoji font in {}, nothing compared",
                font_dir().display()
            );
            return;
        }
        let mut compared = 0;
        let mut made_there = 0;
        for (round, &size) in sizes.iter().enumerate() {
            for (n, text) in emoji.iter().enumerate() {
                let cluster = clusters(text).next().unwrap();
                let size = (size * 2.0_f32).round() / 2.0;
                let Some((gx, gy, image, advance)) = TYPEFACES.with(|cell| {
                    let mut faces = cell.borrow_mut();
                    reference_emoji(faces.fallback(EMOJI).unwrap(), size, &cluster)
                }) else {
                    panic!("{text}: no bitmap");
                };
                // Some measured before they are drawn (as a layout does).
                if (round + n) % 2 == 0 {
                    measure(&format!("a{text}b"), Weight::Regular, size);
                    // And drawn once the emoji thread made it.
                    let start = std::time::Instant::now();
                    while start.elapsed().as_secs() < 5 {
                        let done = TYPEFACES.with(|cell| {
                            let glyph = cell.borrow_mut().glyph(Weight::Regular, size, &cluster);
                            match &glyph.shape {
                                Shape::Emoji(bitmap) => match &*bitmap.borrow() {
                                    Bitmap::Asked(job) => {
                                        matches!(*job.state.lock().unwrap(), JobState::Done(_))
                                    }
                                    Bitmap::Made(_) => true,
                                },
                                Shape::Mask(_) => panic!("{text}: not an emoji"),
                            }
                        });
                        if done {
                            made_there += 1;
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                for again in 0..2 {
                    for (p, &(x, baseline)) in places.iter().enumerate() {
                        let alpha = if p % 2 == 0 { 1.0 } else { 0.6 };
                        let mut drawn = Canvas::new(500, 300);
                        for (i, pixel) in drawn.pixels.iter_mut().enumerate() {
                            *pixel = (i as u32).wrapping_mul(2654435761) & 0x00ff_ffff;
                        }
                        let mut expected = Canvas::new(500, 300);
                        expected.pixels.copy_from_slice(&drawn.pixels);
                        let line = format!("a{text}b");
                        let width = draw(
                            &mut drawn,
                            &line,
                            x,
                            baseline,
                            Weight::Regular,
                            size,
                            0xe0e0e0,
                            alpha,
                        );
                        let mut pen = x + draw(
                            &mut expected,
                            "a",
                            x,
                            baseline,
                            Weight::Regular,
                            size,
                            0xe0e0e0,
                            alpha,
                        );
                        let before = expected.pixels.clone();
                        expected.draw_image(
                            &image,
                            pen.round() as i32 + gx,
                            baseline.round() as i32 + gy,
                            alpha,
                        );
                        assert!(
                            expected.pixels != before || x < 0.0,
                            "{text} at {size}: nothing drawn"
                        );
                        pen += advance;
                        pen += draw(
                            &mut expected,
                            "b",
                            pen,
                            baseline,
                            Weight::Regular,
                            size,
                            0xe0e0e0,
                            alpha,
                        );
                        assert_eq!(width, pen - x, "{text} at {size}: advance");
                        assert!(
                            drawn.pixels == expected.pixels,
                            "{text} at {size}, ({x}, {baseline}), alpha {alpha}, drawn {again}"
                        );
                        compared += 1;
                    }
                }
            }
        }
        eprintln!(
            "emoji_draw_as_they_did: {compared} drawings compared, {made_there} emoji made on the emoji thread"
        );
        assert!(made_there > 0);
    }

    #[test]
    fn missing_characters_never_draw_a_question_mark() {
        // With no fallback fonts installed, an unknown character is a box
        // as wide as a digit, and joiners take no room.
        let digit = measure("0", Weight::Regular, 40.0);
        let unknown = measure("\u{4e2d}", Weight::Regular, 40.0);
        assert!((unknown - digit).abs() < 0.5);
        assert_eq!(measure("\u{200d}\u{fe0f}", Weight::Regular, 40.0), 0.0);
        assert_eq!(
            measure("a\nb", Weight::Regular, 40.0),
            measure("ab", Weight::Regular, 40.0)
        );
    }
}
