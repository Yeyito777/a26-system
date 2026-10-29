//! The Exocortex app's look: the palette (Whale, the iPhone app's default,
//! and Cerberus and Tonikawa, all from its `theme/themes.ts`), the opaque
//! composites every translucent CSS colour becomes over the background it
//! sits on, the type tokens and the metrics of docs/exocortex-roadmap.md §3.
//!
//! Everything that draws takes its colours from `current()`; a piece laid
//! out with them is cached by `generation()` (it changes with the theme and
//! the text size), so switching either lays everything out again.

#![allow(dead_code)]

use std::cell::Cell;

use crate::gfx::Rgb;
use crate::text::Weight;

/// The palette tokens, named as in `themes.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Rgb,
    pub secondary_accent: Rgb,
    pub link: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub error: Rgb,
    pub warning: Rgb,
    pub success: Rgb,
    pub tool: Rgb,
    pub goal: Rgb,
    pub app_background: Rgb,
    pub sidebar_background: Rgb,
    pub surface_background: Rgb,
    pub selection_background: Rgb,
    pub user_message_background: Rgb,
    pub automated_user_background: Rgb,
    pub border: Rgb,
    pub border_focused: Rgb,
    pub notification_background: Rgb,
    pub notification_foreground: Rgb,
    pub text_selection: Rgb,
    pub search_background: Rgb,
    pub search_foreground: Rgb,
    /// Valid commands and macros inside the editor.
    pub command: Rgb,
    /// Text and icons on an accent fill.
    pub on_accent: Rgb,
    pub syntax: Syntax,
    /// Activity rows' fallback colours (exo, chrono, bash).
    pub task_exo: Rgb,
    pub task_chrono: Rgb,
    pub task_bash: Rgb,
}

/// Code highlighting colours (the TUI's palette, PAR-137).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syntax {
    pub keyword: Rgb,
    pub string: Rgb,
    /// Drawn italic.
    pub comment: Rgb,
    pub number: Rgb,
    pub type_: Rgb,
    pub function: Rgb,
    pub punctuation: Rgb,
}

const TUI_SYNTAX: Syntax = Syntax {
    keyword: 0xc792ea,
    string: 0xc3e88d,
    comment: 0x5f6978,
    number: 0xf78c6c,
    type_: 0x89ddff,
    function: 0x82aaff,
    punctuation: 0xb4b4b4,
};

pub const WHALE_PALETTE: Palette = Palette {
    accent: 0x1d9bf0,
    secondary_accent: 0x48cae4,
    link: 0x48cae4,
    text: 0xffffff,
    muted: 0x646464,
    error: 0xff453a,
    warning: 0xffd60a,
    success: 0x50c878,
    tool: 0xbf5af2,
    goal: 0xc792ea,
    app_background: 0x00050f,
    sidebar_background: 0x030814,
    surface_background: 0x090d35,
    selection_background: 0x0f193c,
    user_message_background: 0x090d35,
    automated_user_background: 0x0e1f2c,
    border: 0x555555,
    border_focused: 0x1c94e5,
    notification_background: 0xc62828,
    notification_foreground: 0xffffff,
    text_selection: 0x4f5258,
    search_background: 0xfce094,
    search_foreground: 0x00050f,
    command: 0xaed6fe,
    on_accent: 0x00050f,
    syntax: TUI_SYNTAX,
    task_exo: 0x1d9bf0,
    task_chrono: 0x4ec9b0,
    task_bash: 0xd19a66,
};

pub const CERBERUS_PALETTE: Palette = Palette {
    accent: 0xd32f2f,
    secondary_accent: 0xff6b6b,
    link: 0xef9a9a,
    text: 0xe0e0e0,
    muted: 0x666666,
    error: 0xf44336,
    warning: 0xffa726,
    success: 0x66bb6a,
    tool: 0xb06464,
    goal: 0xc792ea,
    app_background: 0x141414,
    sidebar_background: 0x1a1a1a,
    surface_background: 0x252525,
    selection_background: 0x333333,
    user_message_background: 0x252525,
    automated_user_background: 0x1d262b,
    border: 0x555555,
    border_focused: 0xd32f2f,
    notification_background: 0xb71c1c,
    notification_foreground: 0xffffff,
    text_selection: 0x4a4a4a,
    search_background: 0xfce094,
    search_foreground: 0x141414,
    command: 0xaed6fe,
    on_accent: 0xffffff,
    syntax: TUI_SYNTAX,
    task_exo: 0xd32f2f,
    task_chrono: 0x4ec9b0,
    task_bash: 0xd19a66,
};

pub const TONIKAWA_PALETTE: Palette = Palette {
    accent: 0xe47fac,
    secondary_accent: 0xb99ad8,
    link: 0xe47fac,
    text: 0xeee9ed,
    muted: 0x8d808a,
    error: 0xe06c75,
    warning: 0xd6a65d,
    success: 0x87b695,
    tool: 0xb99ad8,
    goal: 0xc792ea,
    app_background: 0x131116,
    sidebar_background: 0x17131a,
    surface_background: 0x211a21,
    selection_background: 0x302331,
    user_message_background: 0x211a21,
    automated_user_background: 0x242031,
    border: 0x655b63,
    border_focused: 0xe47fac,
    notification_background: 0xa94371,
    notification_foreground: 0xfff7fb,
    text_selection: 0x503749,
    search_background: 0xd8b56f,
    search_foreground: 0x1a1519,
    command: 0xaed6fe,
    on_accent: 0x131116,
    syntax: TUI_SYNTAX,
    task_exo: 0xe47fac,
    task_chrono: 0x4ec9b0,
    task_bash: 0xd19a66,
};

/// `fg` at `percent` % over an opaque `bg`, per channel, rounded as the
/// iPhone's CSS colour-mix was (the roadmap's §3.2 values): what a
/// translucent colour looks like there, as an opaque colour, so partial
/// redraws stay exact. Also for runtime colours (a daemon's tool colours).
pub const fn mix(fg: Rgb, percent: u32, bg: Rgb) -> Rgb {
    let alpha = percent as f64 / 100.0;
    mix_channel(fg, bg, alpha, 16) | mix_channel(fg, bg, alpha, 8) | mix_channel(fg, bg, alpha, 0)
}

const fn mix_channel(fg: Rgb, bg: Rgb, alpha: f64, shift: u32) -> u32 {
    let f = ((fg >> shift) & 0xff) as f64;
    let b = ((bg >> shift) & 0xff) as f64;
    let value = f * alpha + b * (1.0 - alpha);
    // Rounded half to even (truncating a non-negative value is its floor),
    // as the table's values were.
    let floor = value as u32;
    let fraction = value - floor as f64;
    let rounded = if fraction > 0.5 || (fraction == 0.5 && floor % 2 == 1) {
        floor + 1
    } else {
        floor
    };
    (if rounded > 255 { 255 } else { rounded }) << shift
}

/// `#RRGGBB` (a daemon's tool colour, a notice colour) as a colour.
pub fn parse_hex(color: &str) -> Option<Rgb> {
    let hex = color.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// The opaque composites of §3.2: translucent colours over the background
/// each sits on, worked out from the palette (so every theme has them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Composites {
    pub rule_topbar: Rgb,
    pub rule_strong: Rgb,
    pub rule_table: Rgb,
    pub rule_code: Rgb,
    pub rule_code_head: Rgb,
    pub rule_box: Rgb,
    pub rule_list: Rgb,
    pub rule_sheet_row: Rgb,
    pub rule_field_row: Rgb,
    pub rule_settings: Rgb,
    pub rule_sheet_top: Rgb,
    pub island_border: Rgb,
    pub island_border_focused: Rgb,
    pub menu_border: Rgb,
    pub bubble_border: Rgb,
    pub instructions_border: Rgb,
    pub instructions_rule: Rgb,
    pub inline_code_bg: Rgb,
    pub code_bg: Rgb,
    pub thinking_bg: Rgb,
    pub thinking_text: Rgb,
    pub text_meta: Rgb,
    pub text_dim: Rgb,
    pub text_quote: Rgb,
    pub text_output: Rgb,
    pub placeholder: Rgb,
    pub grabber: Rgb,
    pub switch_off: Rgb,
    pub send_disabled_face: Rgb,
    pub send_disabled_icon: Rgb,
    pub stop_face: Rgb,
    pub row_pressed: Rgb,
    pub row_lifted: Rgb,
    pub row_checked: Rgb,
    pub drop_target: Rgb,
    pub menu_item_pressed: Rgb,
    pub search_box: Rgb,
    pub search_field_border: Rgb,
    pub folder_icon_bg: Rgb,
    pub folder_icon_border: Rgb,
    pub fold_mark: Rgb,
    pub fold_mark_open: Rgb,
    pub notice_warning_bg: Rgb,
    pub notice_warning_border: Rgb,
    pub notice_error_bg: Rgb,
    pub notice_error_border: Rgb,
    pub notice_muted_bg: Rgb,
    pub notice_muted_border: Rgb,
    pub tool_bg: Rgb,
    pub tool_border: Rgb,
    pub tool_pressed: Rgb,
    pub quote_bar: Rgb,
    pub link_underline: Rgb,
    pub flash: Rgb,
    /// A sheet's backdrop: black at 55 % over the app background.
    pub backdrop: Rgb,
    /// Read-full and open-all buttons' border (secondary 40 %).
    pub secondary_border: Rgb,
    /// Secondary buttons' border (accent 50 % on the sidebar).
    pub accent_border: Rgb,
    /// The jump-to-latest button's border.
    pub jump_border: Rgb,
    /// The editor's selection (accent 35 % on the surface; the roadmap's
    /// "#0b3e7d" is not that recipe's colour, #103f76).
    pub editor_selection: Rgb,
    /// A focused text field's border (accent 80 % on the surface).
    pub field_focused: Rgb,
    /// Toasts' and pills' border (border 60 % on the surface).
    pub toast_border: Rgb,
    /// The page rail's unlit segments (none: drawn only lit).
    pub segmented_border: Rgb,
}

impl Composites {
    pub const fn of(p: &Palette) -> Self {
        let app = p.app_background;
        let sidebar = p.sidebar_background;
        let surface = p.surface_background;
        Self {
            rule_topbar: mix(p.border, 55, app),
            rule_strong: mix(p.border, 70, app),
            rule_table: mix(p.border, 60, app),
            rule_code: mix(p.border, 45, app),
            rule_code_head: mix(p.border, 35, app),
            rule_box: mix(p.border, 40, app),
            rule_list: mix(p.border, 52, sidebar),
            rule_sheet_row: mix(p.border, 45, sidebar),
            rule_field_row: mix(p.border, 28, sidebar),
            rule_settings: mix(p.border, 40, sidebar),
            rule_sheet_top: mix(p.border, 70, sidebar),
            island_border: mix(p.border, 45, surface),
            island_border_focused: mix(p.border_focused, 55, surface),
            menu_border: mix(p.border, 68, surface),
            bubble_border: mix(p.accent, 22, p.user_message_background),
            instructions_border: mix(p.accent, 55, app),
            instructions_rule: mix(p.accent, 30, app),
            inline_code_bg: mix(p.text, 8, app),
            code_bg: mix(surface, 40, app),
            thinking_bg: mix(p.muted, 6, app),
            thinking_text: mix(p.secondary_accent, 80, p.text),
            text_meta: mix(p.text, 50, app),
            text_dim: mix(p.text, 60, app),
            text_quote: mix(p.text, 75, app),
            text_output: mix(p.text, 82, app),
            placeholder: mix(p.text, 35, surface),
            grabber: mix(p.text, 25, sidebar),
            switch_off: mix(p.text, 16, sidebar),
            send_disabled_face: mix(p.text, 7, surface),
            send_disabled_icon: mix(p.text, 30, surface),
            stop_face: mix(p.error, 14, surface),
            row_pressed: mix(p.selection_background, 80, sidebar),
            row_lifted: mix(p.selection_background, 70, sidebar),
            row_checked: mix(p.accent, 9, sidebar),
            drop_target: mix(p.accent, 10, sidebar),
            menu_item_pressed: mix(p.accent, 15, surface),
            search_box: mix(surface, 75, sidebar),
            search_field_border: mix(p.border, 49, mix(surface, 75, sidebar)),
            folder_icon_bg: mix(p.secondary_accent, 6, sidebar),
            folder_icon_border: mix(p.border, 62, sidebar),
            fold_mark: mix(p.muted, 30, app),
            fold_mark_open: mix(p.muted, 45, app),
            notice_warning_bg: mix(p.warning, 6, app),
            notice_warning_border: mix(p.warning, 35, app),
            notice_error_bg: mix(p.error, 6, app),
            notice_error_border: mix(p.error, 35, app),
            notice_muted_bg: mix(p.muted, 6, app),
            notice_muted_border: mix(p.muted, 35, app),
            tool_bg: mix(p.tool, 5, app),
            tool_border: mix(p.tool, 28, app),
            tool_pressed: mix(p.tool, 10, app),
            quote_bar: mix(p.secondary_accent, 60, app),
            link_underline: mix(p.link, 45, app),
            flash: mix(p.accent, 16, app),
            backdrop: mix(0x000000, 55, app),
            secondary_border: mix(p.secondary_accent, 40, app),
            accent_border: mix(p.accent, 50, sidebar),
            jump_border: mix(p.border, 80, app),
            editor_selection: mix(p.accent, 35, surface),
            field_focused: mix(p.accent, 80, surface),
            toast_border: mix(p.border, 60, surface),
            segmented_border: mix(p.border, 70, sidebar),
        }
    }
}

/// A theme: its palette and composites, and how Settings names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    pub p: Palette,
    pub c: Composites,
}

impl Theme {
    pub const fn new(
        id: &'static str,
        label: &'static str,
        summary: &'static str,
        p: Palette,
    ) -> Self {
        Self {
            id,
            label,
            summary,
            c: Composites::of(&p),
            p,
        }
    }
}

pub static WHALE: Theme = Theme::new("whale", "Whale", "Deep blue and cyan", WHALE_PALETTE);
pub static CERBERUS: Theme = Theme::new(
    "cerberus",
    "Cerberus",
    "Red accents on charcoal",
    CERBERUS_PALETTE,
);
pub static TONIKAWA: Theme = Theme::new(
    "tonikawa",
    "Tonikawa",
    "Rose and lavender on plum",
    TONIKAWA_PALETTE,
);
pub static THEMES: [&Theme; 3] = [&WHALE, &CERBERUS, &TONIKAWA];

/// The colour Moon draws its status bar in over the app (app-services.md):
/// the theme's background.
pub fn status_bar() -> String {
    format!("#{:06x}", p().app_background)
}

thread_local! {
    static CURRENT: Cell<&'static Theme> = const { Cell::new(&WHALE) };
    static TEXT_SCALE: Cell<f32> = const { Cell::new(1.0) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// The theme everything draws with (the UI thread's; tests each have their
/// own thread, so their own).
pub fn current() -> &'static Theme {
    CURRENT.with(Cell::get)
}

/// Shortcuts: `p()` for the palette, `c()` for the composites.
pub fn p() -> &'static Palette {
    &current().p
}

pub fn c() -> &'static Composites {
    &current().c
}

/// Switches the theme (Settings → Appearance). Layout caches keyed by
/// `generation()` lay everything out again.
pub fn set_current(theme: &'static Theme) {
    if !std::ptr::eq(current(), theme) {
        CURRENT.with(|cell| cell.set(theme));
        bump();
    }
}

/// The theme a settings id names, else Whale.
pub fn by_id(id: &str) -> &'static Theme {
    THEMES
        .iter()
        .copied()
        .find(|theme| theme.id == id)
        .unwrap_or(&WHALE)
}

/// Text size (PAR-360, P2): every type token's size and line height are
/// multiplied by it. 1.0 is the roadmap's sizes.
pub fn text_scale() -> f32 {
    TEXT_SCALE.with(Cell::get)
}

pub fn set_text_scale(scale: f32) {
    let scale = scale.clamp(0.75, 1.5);
    if (scale - text_scale()).abs() > f32::EPSILON {
        TEXT_SCALE.with(|cell| cell.set(scale));
        bump();
    }
}

fn bump() {
    GENERATION.with(|cell| cell.set(cell.get().wrapping_add(1)));
}

/// Changes whenever anything that changes how a piece looks does (theme,
/// text size): part of every layout cache's key.
pub fn generation() -> u64 {
    GENERATION.with(Cell::get)
}

// ----- Type ----------------------------------------------------------------

/// A type token (§3.3): JetBrains Mono at a size, line height, weight and
/// tracking (px between glyphs), uppercase or not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type {
    size: f32,
    line: f32,
    pub weight: Weight,
    tracking: f32,
    pub upper: bool,
}

impl Type {
    const fn new(size: f32, line: f32, weight: Weight) -> Self {
        Self {
            size,
            line,
            weight,
            tracking: 0.0,
            upper: false,
        }
    }

    /// The same token with other tracking (px between glyphs).
    pub const fn tracked(mut self, tracking: f32) -> Self {
        self.tracking = tracking;
        self
    }

    const fn uppercase(mut self) -> Self {
        self.upper = true;
        self
    }

    /// The same token in another weight (a Bold caption, a SemiBold row).
    pub const fn weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }

    /// The font size in px, at the current text size.
    pub fn px(&self) -> f32 {
        (self.size * text_scale()).round()
    }

    /// The line height in px, at the current text size.
    pub fn line(&self) -> f32 {
        (self.line * text_scale()).round()
    }

    pub fn tracking(&self) -> f32 {
        self.tracking
    }

    /// Where the baseline of a line starting at `top` goes: centred in the
    /// line as CSS centres its glyph box (cap height and descent balanced).
    pub fn baseline(&self, top: f32) -> f32 {
        top + (self.line() + self.px() * 0.74) / 2.0
    }
}

use Weight::{Mono, MonoBold, MonoItalic, MonoMedium, MonoSemiBold};

/// "BEGINNING OF CONVERSATION".
pub const T_EDGE: Type = Type::new(26.0, 36.0, Mono).tracked(1.6).uppercase();
/// THINKING, SYSTEM INSTRUCTIONS, PINNED, sheet titles.
pub const T_EYEBROW: Type = Type::new(28.0, 40.0, MonoBold).tracked(2.2).uppercase();
/// Settings and inspector section titles.
pub const T_SECTION: Type = Type::new(28.0, 40.0, MonoBold).tracked(2.8).uppercase();
/// The metadata footer.
pub const T_FOOTER: Type = Type::new(28.0, 40.0, MonoMedium).tracked(1.1).uppercase();
/// List meta lines, details, live status.
pub const T_META: Type = Type::new(30.0, 44.0, MonoMedium);
/// The chat subtitle.
pub const T_SUBTITLE: Type = Type::new(30.0, 40.0, Mono);
/// Tool output, file names, previews, tool actions.
pub const T_SMALL: Type = Type::new(32.0, 46.0, Mono);
/// Tool lines, notice previews, the composer error, picker descriptions.
pub const T_CAPTION: Type = Type::new(34.0, 48.0, Mono);
/// Anchored menu labels.
pub const T_MENU: Type = Type::new(33.0, 44.0, MonoSemiBold).tracked(2.0).uppercase();
/// Thinking and notice bodies, sheet messages, buttons (SemiBold).
pub const T_NOTE: Type = Type::new(36.0, 54.0, Mono);
/// Transcript text, bubbles, settings values.
pub const T_BODY: Type = Type::new(40.0, 60.0, Mono);
/// The chat title.
pub const T_TITLE: Type = Type::new(40.0, 52.0, MonoSemiBold);
/// List titles, action items, picker names.
pub const T_ROW: Type = Type::new(42.0, 56.0, MonoMedium);
/// The composer's editor.
pub const T_COMPOSER: Type = Type::new(46.0, 62.0, Mono);
/// "SETTINGS".
pub const T_PAGE_TITLE: Type = Type::new(34.0, 48.0, MonoBold).tracked(2.7).uppercase();
/// Markdown headings, h1 to h4–6.
pub const T_H1: Type = Type::new(56.0, 73.0, MonoBold);
pub const T_H2: Type = Type::new(48.0, 62.0, MonoBold);
pub const T_H3: Type = Type::new(42.0, 55.0, MonoBold);
pub const T_H4: Type = Type::new(40.0, 52.0, MonoBold);
/// Inline code (on `inline_code_bg`).
pub const T_INLINE_CODE: Type = Type::new(36.0, 60.0, Mono);
/// Code blocks.
pub const T_CODE: Type = Type::new(34.0, 52.0, Mono);
/// Tables (header Bold).
pub const T_TABLE: Type = Type::new(36.0, 52.0, Mono);
/// Emphasis.
pub const T_ITALIC: Weight = MonoItalic;

// ----- Metrics (px, §3.4) --------------------------------------------------

/// The window's width (Moon gives every app the whole screen's).
pub const W: f32 = 1080.0;
/// Moon owns the bottom of the window for its home gesture.
pub const GESTURE_RESERVE: f32 = 170.0;
/// Spacing scale (4/8/12/16/20 pt).
pub const S1: f32 = 11.0;
pub const S2: f32 = 22.0;
pub const S3: f32 = 34.0;
pub const S4: f32 = 44.0;
pub const S5: f32 = 56.0;
/// Hairlines and borders.
pub const HAIRLINE: f32 = 2.0;
/// Colour rails (user bubbles, tool rows, the open list row, the page rail).
pub const RAIL: f32 = 6.0;
/// Top bars: window y 0 to 122, then a hairline.
pub const TOP_BAR: f32 = 122.0;
/// Every touch target is at least this square (44 pt).
pub const TARGET: f32 = 122.0;
/// An icon inside a button.
pub const ICON: f32 = 60.0;
pub const BAR_PAD: f32 = 11.0;
/// The transcript.
pub const TRANSCRIPT_PAD: f32 = 40.0;
pub const ROW_GAP: f32 = 40.0;
pub const TRANSCRIPT_TOP: f32 = 34.0;
pub const TRANSCRIPT_BOTTOM: f32 = 40.0;
pub const BLOCK_GAP: f32 = 25.0;
pub const MARKDOWN_GAP: f32 = 24.0;
pub const LIST_INDENT: f32 = 34.0;
pub const MARKER_GAP: f32 = 30.0;
pub const QUOTE_BAR: f32 = 6.0;
pub const QUOTE_INDENT: f32 = 34.0;
/// The conversation list.
pub const LIST_ROW: f32 = 160.0;
pub const HEADER_ROW: f32 = 88.0;
pub const DIVIDER_ROW: f32 = 30.0;
pub const LIST_PAD: f32 = 34.0;
pub const FOLDER_INDENT: f32 = 40.0;
pub const FOLDER_ICON: f32 = 83.0;
/// User bubbles: padding (y, x), the gap on their left, the rail.
pub const BUBBLE_PAD_Y: f32 = 25.0;
pub const BUBBLE_PAD_X: f32 = 34.0;
pub const BUBBLE_GAP: f32 = 66.0;
/// Tool rows: padding top, right, bottom, left.
pub const TOOL_PAD: (f32, f32, f32, f32) = (34.0, 28.0, 34.0, 34.0);
/// Disclosure heads (thinking, notices, system instructions).
pub const DISCLOSURE_HEAD: f32 = 122.0;
pub const DISCLOSURE_PAD: (f32, f32) = (17.0, 28.0);
/// The composer island.
pub const COMPOSER_MARGIN: f32 = 34.0;
pub const COMPOSER_GAP: f32 = 22.0;
pub const ISLAND_MIN: f32 = 144.0;
pub const ISLAND_RADIUS: f32 = 72.0;
pub const ISLAND_PAD: f32 = 11.0;
pub const COMPOSER_BUTTON: f32 = 122.0;
pub const COMPOSER_FACE: f32 = 94.0;
pub const EDITOR_INSET: f32 = 28.0;
pub const EDITOR_PAD_Y: f32 = 29.0;
pub const EDITOR_LINES: usize = 6;
/// Jump to latest: its size, its inset from the right and from the island.
pub const JUMP: f32 = 122.0;
pub const JUMP_INSET: f32 = 34.0;
/// Sheets: at most this share of the window; the grabber.
pub const SHEET_MAX: f32 = 0.82;
pub const GRABBER: (f32, f32) = (100.0, 11.0);
pub const GRABBER_TOP: f32 = 22.0;
/// Action list items: height, side padding, icon.
pub const ACTION_ITEM: f32 = 144.0;
pub const ACTION_PAD: f32 = 44.0;
pub const ACTION_ICON: f32 = 60.0;
/// The anchored menu: width, item, quick row; its offset from the row and
/// the screen edges.
pub const MENU_W: f32 = 676.0;
pub const MENU_ITEM: f32 = 138.0;
pub const MENU_QUICK: f32 = 172.0;
pub const MENU_OFFSET: f32 = 25.0;
pub const MENU_CLAMP: f32 = 39.0;
/// Settings.
pub const SETTINGS_PAD: (f32, f32) = (50.0, 44.0);
pub const TOGGLE_ROW: f32 = 144.0;
/// The switch and its knob.
pub const SWITCH: (f32, f32) = (128.0, 78.0);
pub const KNOB: f32 = 61.0;
/// The command picker: at most this tall; hidden below the second.
pub const PICKER_MAX: f32 = 664.0;
pub const PICKER_MIN: f32 = 244.0;
/// Spinners: diameter and stroke.
pub const SPINNER: f32 = 39.0;
pub const SPINNER_STROKE: f32 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// The roadmap's §3.2 composite table, as written there.
    #[test]
    fn whale_composites_are_the_specified_colours() {
        let c = WHALE.c;
        let table = [
            (c.rule_topbar, 0x2f3136),
            (c.rule_strong, 0x3b3d40),
            (c.rule_table, 0x333539),
            (c.rule_code, 0x26292e),
            (c.rule_code_head, 0x1e2128),
            (c.rule_box, 0x22252b),
            (c.rule_list, 0x2e3036),
            (c.rule_sheet_row, 0x282b31),
            (c.rule_field_row, 0x1a1e26),
            (c.rule_settings, 0x24272e),
            (c.rule_sheet_top, 0x3c3e42),
            (c.island_border, 0x2b2d43),
            (c.island_border_focused, 0x135796),
            (c.menu_border, 0x3d3e4b),
            (c.bubble_border, 0x0d2c5e),
            (c.instructions_border, 0x10588b),
            (c.instructions_rule, 0x093252),
            (c.inline_code_bg, 0x141922),
            (c.code_bg, 0x04081e),
            (c.thinking_bg, 0x060b14),
            (c.thinking_text, 0x6dd5e9),
            (c.text_meta, 0x808287),
            (c.text_dim, 0x999b9f),
            (c.text_quote, 0xbfc0c3),
            (c.text_output, 0xd1d2d4),
            (c.placeholder, 0x5f627c),
            (c.grabber, 0x42464f),
            (c.switch_off, 0x2b303a),
            (c.send_disabled_face, 0x1a1e43),
            (c.send_disabled_icon, 0x535672),
            (c.stop_face, 0x2b1536),
            (c.row_pressed, 0x0d1634),
            (c.row_lifted, 0x0b1430),
            (c.row_checked, 0x051528),
            (c.drop_target, 0x06172a),
            (c.menu_item_pressed, 0x0c2251),
            (c.search_box, 0x080c2d),
            (c.folder_icon_bg, 0x071420),
            (c.folder_icon_border, 0x36383c),
            (c.fold_mark, 0x1e2228),
            (c.fold_mark_open, 0x2d3035),
            (c.notice_warning_bg, 0x0f120f),
            (c.notice_warning_border, 0x594e0d),
            (c.notice_error_bg, 0x0f0912),
            (c.notice_error_border, 0x591b1e),
            (c.notice_muted_bg, 0x060b14),
            (c.notice_muted_border, 0x23262d),
            (c.tool_bg, 0x0a091a),
            (c.tool_border, 0x351d4f),
            (c.tool_pressed, 0x130e26),
            (c.quote_bar, 0x2b7b8f),
            (c.link_underline, 0x205e6f),
            (c.flash, 0x051d33),
            (c.backdrop, 0x000207),
            (c.secondary_border, 0x1d5464),
            (c.search_field_border, 0x2e3041),
            (c.jump_border, 0x444547),
        ];
        for (index, (computed, specified)) in table.into_iter().enumerate() {
            assert_eq!(
                computed, specified,
                "composite {index}: {computed:06x} is not {specified:06x}"
            );
        }
    }

    #[test]
    fn switching_themes_changes_the_generation() {
        let before = generation();
        assert!(std::ptr::eq(current(), &WHALE));
        set_current(&WHALE);
        assert_eq!(generation(), before, "the same theme is no change");
        set_current(by_id("cerberus"));
        assert_eq!(p().accent, 0xd32f2f);
        assert_ne!(generation(), before);
        set_current(by_id("nonsense"));
        assert!(std::ptr::eq(current(), &WHALE));
        assert_eq!(parse_hex("#1d9bf0"), Some(0x1d9bf0));
        assert_eq!(parse_hex("1d9bf0"), None);
    }

    #[test]
    fn type_tokens_scale_with_the_text_size() {
        assert_eq!(T_BODY.px(), 40.0);
        assert_eq!(T_BODY.line(), 60.0);
        set_text_scale(1.25);
        assert_eq!(T_BODY.px(), 50.0);
        set_text_scale(1.0);
        assert_eq!(T_EYEBROW.tracking(), 2.2);
        const { assert!(T_EYEBROW.upper && !T_BODY.upper) };
    }
}
