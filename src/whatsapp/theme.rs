//! WhatsApp's dark look on Android, in this app's pixels (1080 wide: one dp
//! is 2.75 px): its colours, sizes and type.

#![allow(dead_code)]

use crate::gfx::Rgb;

/// Where the app's own bars start: Moon places the window under its
/// status bar (which it draws in the app's colour).
pub const TOP: f32 = 0.0;
/// What Moon keeps at the bottom for its home gesture: drags there are
/// Moon's, taps still come.
pub const GESTURE_RESERVE: f32 = 170.0;
/// The bottom inset the composer and the tab bar stand on.
pub const SAFE_BOTTOM: f32 = 60.0;

pub const DP: f32 = 2.75;

// Colours.
pub const BG: Rgb = 0x0b141a;
pub const BAR: Rgb = 0x0b141a;
pub const CHAT_BAR: Rgb = 0x1f2c34;
pub const SURFACE: Rgb = 0x1f2c34;
pub const SURFACE_HI: Rgb = 0x2a3942;
pub const OUT: Rgb = 0x005c4b;
pub const IN: Rgb = 0x1f2c34;
pub const GREEN: Rgb = 0x21c063;
pub const GREEN_DARK: Rgb = 0x103529;
pub const GREEN_TEXT: Rgb = 0xd9fdd3;
pub const TEAL: Rgb = 0x00a884;
pub const TEXT: Rgb = 0xe9edef;
pub const SECONDARY: Rgb = 0x8696a0;
pub const OUT_SECONDARY: Rgb = 0x99beb7;
pub const TICK_READ: Rgb = 0x53bdeb;
pub const LINK: Rgb = 0x53bdeb;
pub const DIVIDER: Rgb = 0x222d34;
pub const CHIP: Rgb = 0x182229;
pub const DANGER: Rgb = 0xf15c6d;
pub const PRESSED: Rgb = 0x202c33;
pub const AVATAR_BG: Rgb = 0x6a7175;
pub const AVATAR_FG: Rgb = 0xcfd4d6;
pub const ON_GREEN: Rgb = 0x0b141a;

/// Group members' names, by who they are (WhatsApp's palette).
pub const NAME_COLORS: [Rgb; 16] = [
    0xfc9775, 0x53bdeb, 0xa5b337, 0xffd279, 0xff72a1, 0x25d366, 0x7f66ff, 0x06cf9c, 0xf7a46d, 0xe26ab6,
    0x42c7b8, 0xffbc38, 0x00bfa5, 0xd88b6b, 0xc86bd9, 0xef6b6b,
];

pub fn name_color(id: &str) -> Rgb {
    let mut hash: u32 = 2_166_136_261;
    for byte in id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    NAME_COLORS[(hash % NAME_COLORS.len() as u32) as usize]
}

// The chat list.
pub const APP_BAR: f32 = 154.0;
pub const MARGIN: f32 = 44.0;
pub const ROW: f32 = 198.0;
pub const AVATAR: f32 = 135.0;
pub const ROW_TEXT_X: f32 = MARGIN + AVATAR + 40.0;
pub const SEARCH_H: f32 = 121.0;
pub const CHIPS_H: f32 = 88.0;
pub const TAB_BAR: f32 = 210.0;

// Type sizes (px).
pub const T_APP_TITLE: f32 = 64.0;
pub const T_TITLE: f32 = 47.0;
pub const T_PREVIEW: f32 = 40.0;
pub const T_TIME: f32 = 33.0;
pub const T_BODY: f32 = 43.0;
pub const LINE: f32 = 58.0;
pub const T_META: f32 = 30.0;
pub const T_NAME: f32 = 37.0;
pub const T_CHIP: f32 = 34.0;
pub const T_HEADER: f32 = 50.0;
pub const T_SUB: f32 = 34.0;

// The chat.
pub const HEADER: f32 = 154.0;
pub const BUBBLE_PAD_X: f32 = 24.0;
pub const BUBBLE_PAD_Y: f32 = 14.0;
pub const BUBBLE_RADIUS: f32 = 21.0;
pub const SIDE: f32 = 30.0;
pub const TAIL: f32 = 22.0;
pub const GAP_SAME: f32 = 6.0;
pub const GAP_OTHER: f32 = 22.0;
pub const COMPOSER_H: f32 = 132.0;
