//! The string semantics the TUI's math code relies on: JavaScript's `\s`,
//! `trim` and UTF-16 lengths, the regex property classes it tests
//! (`\p{L}`, `\p{N}`, `\p{M}`, `\p{Sm}`), `normalize("NFC")`, and its
//! terminal column widths (exocortex `tui/src/textwidth.ts`: glibc's
//! wcwidth as st draws it, wide East Asian and emoji glyphs two columns,
//! marks and joiners none).

use std::sync::OnceLock;

use super::unicode;

fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    let cp = c as u32;
    ranges
        .binary_search_by(|&(start, end)| {
            if end < cp {
                std::cmp::Ordering::Less
            } else if start > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// JavaScript's `\s` (and what `trim` removes).
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `s.length` in JavaScript.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `\p{L}`.
pub fn is_letter(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    in_ranges(c, unicode::LETTER)
}

/// `\p{L}` tested on one UTF-16 unit (`src[i]`): half of a surrogate
/// pair is never a letter.
pub fn is_letter_unit(c: char) -> bool {
    (c as u32) <= 0xFFFF && is_letter(c)
}

/// `\p{N}`.
pub fn is_number(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    in_ranges(c, unicode::NUMBER)
}

/// `\p{M}`.
pub fn is_mark(c: char) -> bool {
    !c.is_ascii() && (in_ranges(c, unicode::ZERO_WIDTH_MARK) || in_ranges(c, unicode::SPACING_MARK))
}

/// `\p{Sm}`.
pub fn is_math_symbol(c: char) -> bool {
    in_ranges(c, unicode::MATH_SYMBOL)
}

// ── Terminal columns ───────────────────────────────────────────────────

/// Always double-width code points (textwidth.ts's `WIDE_RANGES`).
#[rustfmt::skip]
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F), (0x231A, 0x231B), (0x2329, 0x232A), (0x23E9, 0x23EC), (0x23F0, 0x23F0),
    (0x23F3, 0x23F3), (0x25FD, 0x25FE), (0x2614, 0x2615), (0x2630, 0x2637), (0x2648, 0x2653),
    (0x267F, 0x267F), (0x268A, 0x268F), (0x2693, 0x2693), (0x26A1, 0x26A1), (0x26AA, 0x26AB),
    (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4), (0x26EA, 0x26EA),
    (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD), (0x2705, 0x2705),
    (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755),
    (0x2757, 0x2757), (0x2795, 0x2797), (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x2E80, 0x303E), (0x3040, 0xA4CF), (0xA960, 0xA97C),
    (0xAC00, 0xD7AF), (0xF900, 0xFAFF), (0xFE10, 0xFE19), (0xFE30, 0xFE6F), (0xFF01, 0xFF60),
    (0xFFE0, 0xFFE6), (0x16FE0, 0x16FE3), (0x16FF0, 0x16FF6), (0x17000, 0x18CD5), (0x18CFF, 0x18D1E),
    (0x18D80, 0x18DF2), (0x1AFF0, 0x1AFF3), (0x1AFF5, 0x1AFFB), (0x1AFFD, 0x1AFFE), (0x1B000, 0x1B122),
    (0x1B132, 0x1B132), (0x1B150, 0x1B152), (0x1B155, 0x1B155), (0x1B164, 0x1B167), (0x1B170, 0x1B2FB),
    (0x1D300, 0x1D356), (0x1D360, 0x1D376), (0x1F004, 0x1F004), (0x1F0CF, 0x1F0CF), (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A), (0x1F200, 0x1F202), (0x1F210, 0x1F23B), (0x1F240, 0x1F248), (0x1F250, 0x1F251),
    (0x1F260, 0x1F265), (0x1F300, 0x1F320), (0x1F32D, 0x1F335), (0x1F337, 0x1F37C), (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA), (0x1F3CF, 0x1F3D3), (0x1F3E0, 0x1F3F0), (0x1F3F4, 0x1F3F4), (0x1F3F8, 0x1F43E),
    (0x1F440, 0x1F440), (0x1F442, 0x1F4FC), (0x1F4FF, 0x1F53D), (0x1F54B, 0x1F54E), (0x1F550, 0x1F567),
    (0x1F57A, 0x1F57A), (0x1F595, 0x1F596), (0x1F5A4, 0x1F5A4), (0x1F5FB, 0x1F64F), (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC), (0x1F6D0, 0x1F6D2), (0x1F6D5, 0x1F6D8), (0x1F6DC, 0x1F6DF), (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC), (0x1F7E0, 0x1F7EB), (0x1F7F0, 0x1F7F0), (0x1F90C, 0x1F93A), (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF), (0x1FA70, 0x1FA7C), (0x1FA80, 0x1FA8A), (0x1FA8E, 0x1FAC6), (0x1FAC8, 0x1FAC8),
    (0x1FACD, 0x1FADC), (0x1FADF, 0x1FAEA), (0x1FAEF, 0x1FAF8), (0x20000, 0x3FFFF),
];

fn is_zero_width(c: char) -> bool {
    let cp = c as u32;
    in_ranges(c, unicode::ZERO_WIDTH_MARK)
        || (0x200B..=0x200F).contains(&cp)
        || (0x2028..=0x202E).contains(&cp)
        || (0x2060..=0x206F).contains(&cp)
        || cp == 0xFEFF
        || cp == 0x061C
        || cp == 0x180E
        || (0xFE00..=0xFE0F).contains(&cp)
        || (0x1160..=0x11FF).contains(&cp)
        || (0xD7B0..=0xD7FF).contains(&cp)
        || cp == 0x3164
        || cp == 0xFFA0
        || (0x1BCA0..=0x1BCA3).contains(&cp)
        || (0x1D173..=0x1D17A).contains(&cp)
        || (0xE0100..=0xE01EF).contains(&cp)
        || (0x0300..=0x036F).contains(&cp)
        || (0x1AB0..=0x1AFF).contains(&cp)
        || (0x1DC0..=0x1DFF).contains(&cp)
        || (0x20D0..=0x20FF).contains(&cp)
        || (0xFE20..=0xFE2F).contains(&cp)
}

fn is_tag(c: char) -> bool {
    (0xE0020..=0xE007F).contains(&(c as u32))
}

fn is_regional(c: char) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&(c as u32))
}

/// The columns of the grapheme at `chars[i]` and the index after it.
fn next_grapheme(chars: &[char], i: usize) -> (usize, usize) {
    let c = chars[i];
    if is_zero_width(c) {
        return (0, i + 1);
    }
    if is_regional(c) {
        if chars.get(i + 1).is_some_and(|&next| is_regional(next)) {
            return (2, i + 2);
        }
        return (1, i + 1);
    }
    let width = if in_ranges(c, WIDE) { 2 } else { 1 };
    let mut end = i + 1;
    if width == 2 {
        if chars.get(end) == Some(&'\u{FE0F}') {
            end += 1;
        }
        while chars.get(end).is_some_and(|&next| is_tag(next)) {
            end += 1;
        }
    }
    while chars.get(end).is_some_and(|&next| is_zero_width(next)) {
        end += 1;
    }
    (width, end)
}

/// Terminal columns of `s` (textwidth.ts's `termWidth`).
pub fn width(s: &str) -> usize {
    if s.bytes().all(|b| (0x20..=0x7E).contains(&b)) {
        return s.len();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut total = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (' '..='~').contains(&c) {
            total += 1;
            i += 1;
            continue;
        }
        let (columns, end) = next_grapheme(&chars, i);
        total += columns;
        i = end;
    }
    total
}

/// The longest prefix of `s` within `max` columns, and the rest
/// (textwidth.ts's `sliceByWidth`).
pub fn slice_by_width(s: &str, max: usize) -> (String, String) {
    let chars: Vec<char> = s.chars().collect();
    let mut used = 0;
    let mut i = 0;
    while i < chars.len() {
        if (' '..='~').contains(&chars[i]) {
            if used + 1 > max {
                break;
            }
            used += 1;
            i += 1;
            continue;
        }
        let (columns, end) = next_grapheme(&chars, i);
        if used + columns > max {
            break;
        }
        used += columns;
        i = end;
    }
    (chars[..i].iter().collect(), chars[i..].iter().collect())
}

// ── NFC ────────────────────────────────────────────────────────────────

const HANGUL_S: u32 = 0xAC00;
const HANGUL_L: u32 = 0x1100;
const HANGUL_V: u32 = 0x1161;
const HANGUL_T: u32 = 0x11A7;
const HANGUL_V_COUNT: u32 = 21;
const HANGUL_T_COUNT: u32 = 28;
const HANGUL_COUNT: u32 = 11172;

fn combining_class(c: char) -> u8 {
    let cp = c as u32;
    if cp < 0x300 {
        return 0;
    }
    match unicode::COMBINING_CLASS.binary_search_by(|&(start, end, _)| {
        if end < cp {
            std::cmp::Ordering::Less
        } else if start > cp {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(index) => unicode::COMBINING_CLASS[index].2,
        Err(_) => 0,
    }
}

fn decompose(c: char, out: &mut Vec<char>) {
    let cp = c as u32;
    if cp < 0xC0 {
        out.push(c);
        return;
    }
    if (HANGUL_S..HANGUL_S + HANGUL_COUNT).contains(&cp) {
        let index = cp - HANGUL_S;
        let t = index % HANGUL_T_COUNT;
        out.extend(char::from_u32(
            HANGUL_L + index / (HANGUL_V_COUNT * HANGUL_T_COUNT),
        ));
        out.extend(char::from_u32(
            HANGUL_V + (index % (HANGUL_V_COUNT * HANGUL_T_COUNT)) / HANGUL_T_COUNT,
        ));
        if t > 0 {
            out.extend(char::from_u32(HANGUL_T + t));
        }
        return;
    }
    if let Ok(index) =
        unicode::COMPOSITION.binary_search_by_key(&cp, |&(composite, _, _)| composite)
    {
        let (_, first, second) = unicode::COMPOSITION[index];
        if let (Some(first), Some(second)) = (char::from_u32(first), char::from_u32(second)) {
            decompose(first, out);
            out.push(second);
            return;
        }
    }
    if let Ok(index) = unicode::DECOMPOSITION.binary_search_by_key(&cp, |&(code, _)| code) {
        out.extend(
            unicode::DECOMPOSITION[index]
                .1
                .iter()
                .filter_map(|&part| char::from_u32(part)),
        );
        return;
    }
    out.push(c);
}

/// The canonical pairs by (first, second), for composing.
fn pairs() -> &'static [(u32, u32, u32)] {
    static PAIRS: OnceLock<Vec<(u32, u32, u32)>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        let mut pairs: Vec<(u32, u32, u32)> = unicode::COMPOSITION
            .iter()
            .map(|&(composite, first, second)| (first, second, composite))
            .collect();
        pairs.sort_unstable();
        pairs
    })
}

fn compose_pair(first: char, second: char) -> Option<char> {
    let (a, b) = (first as u32, second as u32);
    if (HANGUL_L..HANGUL_L + 19).contains(&a) && (HANGUL_V..HANGUL_V + HANGUL_V_COUNT).contains(&b)
    {
        return char::from_u32(
            HANGUL_S + ((a - HANGUL_L) * HANGUL_V_COUNT + (b - HANGUL_V)) * HANGUL_T_COUNT,
        );
    }
    if (HANGUL_S..HANGUL_S + HANGUL_COUNT).contains(&a)
        && (a - HANGUL_S) % HANGUL_T_COUNT == 0
        && (HANGUL_T + 1..HANGUL_T + HANGUL_T_COUNT).contains(&b)
    {
        return char::from_u32(a + (b - HANGUL_T));
    }
    let pairs = pairs();
    pairs
        .binary_search_by(|&(x, y, _)| (x, y).cmp(&(a, b)))
        .ok()
        .and_then(|index| char::from_u32(pairs[index].2))
}

/// `s.normalize("NFC")`. CJK compatibility ideographs keep their code
/// points (bun maps them to the unified ones).
pub fn nfc(s: &str) -> String {
    let mut chars = Vec::with_capacity(s.len());
    for c in s.chars() {
        decompose(c, &mut chars);
    }
    // Canonical order: runs of marks sorted (stably) by combining class.
    let mut start = 0;
    while start < chars.len() {
        if combining_class(chars[start]) == 0 {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < chars.len() && combining_class(chars[end]) != 0 {
            end += 1;
        }
        chars[start..end].sort_by_key(|&c| combining_class(c));
        start = end;
    }
    if chars.is_empty() {
        return String::new();
    }
    // Canonical composition.
    let mut starter = 0;
    let mut last_class: u16 = match combining_class(chars[0]) {
        0 => 0,
        _ => 256,
    };
    let mut kept = 1;
    for index in 1..chars.len() {
        let c = chars[index];
        let class = u16::from(combining_class(c));
        match compose_pair(chars[starter], c) {
            Some(composite) if last_class < class || last_class == 0 => chars[starter] = composite,
            _ => {
                if class == 0 {
                    starter = kept;
                }
                last_class = class;
                chars[kept] = c;
                kept += 1;
            }
        }
    }
    chars[..kept].iter().collect()
}
