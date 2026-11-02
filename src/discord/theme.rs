//! Discord's dark theme and the app's metrics. Sizes are Discord's mobile
//! dp at 2.75 px each (the window is 1080 px, 393 dp, wide); colours are
//! its dark palette.

#![allow(dead_code)]

use crate::gfx::Rgb;

pub const DP: f32 = 2.75;

pub const fn dp(value: f32) -> f32 {
    value * DP
}

// ----- Colours ---------------------------------------------------------------

/// The server rail and the tab bar.
pub const RAIL: Rgb = 0x1e1f22;
/// The channel list, cards (embeds, files), the code block.
pub const LIST: Rgb = 0x2b2d31;
/// The chat.
pub const CHAT: Rgb = 0x313338;
/// The composer's field, a pressed row.
pub const FIELD: Rgb = 0x383a40;
/// A selected channel, a pressed message.
pub const SELECTED: Rgb = 0x404249;
pub const HOVER: Rgb = 0x35373c;
pub const DIVIDER: Rgb = 0x3f4147;
pub const HAIR: Rgb = 0x26272b;

pub const HEADER: Rgb = 0xf2f3f5;
pub const TEXT: Rgb = 0xdbdee1;
pub const MUTED: Rgb = 0x949ba4;
pub const FAINT: Rgb = 0x6d6f78;
pub const DIM: Rgb = 0x4e5058;
pub const INTERACTIVE: Rgb = 0xb5bac1;

pub const BLURPLE: Rgb = 0x5865f2;
pub const RED: Rgb = 0xf23f43;
pub const GREEN: Rgb = 0x23a55a;
pub const YELLOW: Rgb = 0xf0b232;
pub const LINK: Rgb = 0x00a8fc;
pub const WHITE: Rgb = 0xffffff;

/// A mention of the user: the row's tint and its left bar.
pub const MENTION_ROW: Rgb = 0x433e33;
pub const MENTION_BAR: Rgb = 0xf0b232;
/// `@someone` in text: its pill and its letters.
pub const MENTION_PILL: Rgb = 0x3c4270;
pub const MENTION_TEXT: Rgb = 0xc9cdfb;
pub const CODE_BG: Rgb = 0x2b2d31;
pub const CODE_BORDER: Rgb = 0x1e1f22;
pub const QUOTE_BAR: Rgb = 0x4e5058;
pub const SPOILER: Rgb = 0x1e1f22;
pub const REACTION: Rgb = 0x2b2d31;
pub const REACTION_MINE: Rgb = 0x373a59;

// ----- Type (px) -------------------------------------------------------------

pub const T_BODY: f32 = 44.0;
pub const L_BODY: f32 = 60.0;
pub const T_SMALL: f32 = 38.5;
pub const L_SMALL: f32 = 52.0;
pub const T_TINY: f32 = 33.0;
pub const L_TINY: f32 = 44.0;
pub const T_TITLE: f32 = 49.5;
pub const T_H1: f32 = 66.0;
pub const T_H2: f32 = 55.0;
pub const T_H3: f32 = 48.0;
pub const T_CODE: f32 = 37.0;
pub const L_CODE: f32 = 52.0;

// ----- Metrics (px) ----------------------------------------------------------

/// Moon's status bar over the window's top.
pub const TOP: f32 = 124.0;
/// What stays clear at the bottom for Moon's home gesture (taps still come).
pub const SAFE_BOTTOM: f32 = 94.0;
/// Drags that start this close to the bottom are Moon's.
pub const GESTURE_RESERVE: f32 = 170.0;
pub const HEADER_H: f32 = dp(54.0);
pub const TAB_H: f32 = dp(56.0);

pub const RAIL_W: f32 = dp(72.0);
pub const SERVER_ICON: f32 = dp(48.0);
pub const SERVER_GAP: f32 = dp(8.0);

pub const AVATAR: f32 = dp(40.0);
pub const GUTTER: f32 = dp(16.0);
/// The message text's left edge.
pub const CONTENT_X: f32 = GUTTER + AVATAR + dp(12.0);

pub const MARGIN: f32 = dp(16.0);

/// A role colour of 0 means "no colour": the default name colour.
pub fn name_color(color: Option<u32>) -> Rgb {
    match color {
        Some(c) if c != 0 => c & 0xffffff,
        _ => HEADER,
    }
}
