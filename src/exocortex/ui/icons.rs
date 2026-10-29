//! The icon set (roadmap §3.6.20): the iPhone app's stroke icons
//! (`web/src/ui/icons.tsx`, lucide-style, on a 24-unit grid) as the same
//! SVG path data, drawn into a square box with round caps and joins. Each
//! (icon, size, stroke) is rasterized once into a coverage mask and drawn at
//! whole pixels, so an icon moved with its row draws the same pixels.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::gfx::{Canvas, Path, Rgb};

/// Every icon the app draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    ArrowUp,
    ArrowDown,
    /// A circle with an arrow down: jump to latest, next unread.
    DownloadCircle,
    External,
    Undo,
    Redo,
    Rewind,
    Plus,
    X,
    Check,
    Search,
    Ellipsis,
    Refresh,
    Trash,
    Copy,
    Clone,
    Paste,
    Pencil,
    Share,
    Download,
    Export,
    Stop,
    /// List-plus: queue a message.
    ListPlus,
    Background,
    /// List-checks: select, activity.
    ListChecks,
    Sparkle,
    FileText,
    File,
    Folder,
    /// The list's folder glyph (filled).
    FolderFill,
    FolderOpen,
    FolderPlus,
    FolderMove,
    Tray,
    Chat,
    Pin,
    PinOff,
    /// Filled.
    Star,
    StarLine,
    StarOff,
    Bell,
    BellOff,
    Hash,
    /// Filled, with a check knocked out.
    CheckCircle,
    Circle,
    Gear,
    InfoCircle,
    /// Filled, with an exclamation knocked out.
    WarningTriangle,
    /// Filled: steer now.
    Bolt,
    /// Message end.
    Clock,
    Mic,
    Pause,
    Play,
    Image,
    Camera,
    /// Goal.
    Target,
    /// Chrono.
    Timer,
    /// Ask aside (BTW).
    MessageSquare,
    Gauge,
}

/// How an element of an icon is painted.
#[derive(Clone, Copy, PartialEq)]
enum Paint {
    Stroke,
    Fill,
    /// Stroked in the knockout colour (the background), over a fill.
    KnockStroke,
}

#[derive(Clone, Copy)]
enum El {
    Path(&'static str, Paint),
    Circle(f32, f32, f32, Paint),
    Rect(f32, f32, f32, f32, f32, Paint),
}

use El::{Circle, Path as P, Rect};
use Paint::{Fill, KnockStroke, Stroke};

const FOLDER: &str =
    "M3.5 6.5a1 1 0 0 1 1-1h4.6l2 2.2h8.4a1 1 0 0 1 1 1v9.8a1 1 0 0 1-1 1h-15a1 1 0 0 1-1-1z";
const STAR: &str = "M12 4l2.4 5 5.4.6-4 3.7 1.1 5.3L12 15.9l-4.9 2.7 1.1-5.3-4-3.7 5.4-.6z";
const PIN: &str = "M9 4h6l-1 5 3 3v1.5H7V12l3-3z";

/// The icon's elements and its stroke width in grid units (the iPhone's,
/// scaled so the default is the roadmap's 5 px in a 60 px box).
fn spec(icon: Icon) -> (&'static [El], f32) {
    const W: f32 = 2.0;
    match icon {
        Icon::ChevronLeft => (&[P("M15 5l-7 7 7 7", Stroke)], W),
        Icon::ChevronRight => (&[P("M9 5l7 7-7 7", Stroke)], W),
        Icon::ChevronDown => (&[P("M6 9l6 6 6-6", Stroke)], W),
        Icon::ChevronUp => (&[P("M6 15l6-6 6 6", Stroke)], W),
        Icon::ArrowUp => (&[P("M12 19V5M5.5 11.5L12 5l6.5 6.5", Stroke)], 2.44),
        Icon::ArrowDown => (&[P("M12 5v14M5.5 12.5L12 19l6.5-6.5", Stroke)], 2.22),
        Icon::DownloadCircle => (
            &[
                Circle(12.0, 12.0, 8.5, Stroke),
                P("M12 7.5v8M8.5 12.5L12 16l3.5-3.5", Stroke),
            ],
            W,
        ),
        Icon::External => (
            &[P(
                "M14 4.5h5.5V10M19.5 4.5L11 13M17 14v5a1 1 0 0 1-1 1H5.5a1 1 0 0 1-1-1V8.5a1 1 0 0 1 1-1H10",
                Stroke,
            )],
            W,
        ),
        Icon::Undo => (
            &[
                P("M9 7.5L4.5 12 9 16.5", Stroke),
                P("M4.5 12h10a5 5 0 0 1 0 10H12", Stroke),
            ],
            W,
        ),
        Icon::Redo => (
            &[
                P("M15 7.5l4.5 4.5-4.5 4.5", Stroke),
                P("M19.5 12h-10a5 5 0 0 0 0 10H12", Stroke),
            ],
            W,
        ),
        Icon::Rewind => (
            &[
                P("M9 14L4.5 9.5 9 5", Stroke),
                P("M4.5 9.5H15a4.5 4.5 0 0 1 0 9h-3", Stroke),
            ],
            W,
        ),
        Icon::Plus => (&[P("M12 5v14M5 12h14", Stroke)], W),
        Icon::X => (&[P("M6 6l12 12M18 6L6 18", Stroke)], W),
        Icon::Check => (&[P("M5 12.5l4.5 4.5L19 7.5", Stroke)], 2.44),
        Icon::Search => (
            &[
                Circle(11.0, 11.0, 6.5, Stroke),
                P("M20 20l-4.2-4.2", Stroke),
            ],
            W,
        ),
        Icon::Ellipsis => (&[P("M6 12h.01M12 12h.01M18 12h.01", Stroke)], 2.67),
        Icon::Refresh => (&[P("M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5", Stroke)], W),
        Icon::Trash => (&[P("M4.5 7h15M9.5 7V4.5h5V7M6.5 7l1 13h9l1-13", Stroke)], W),
        Icon::Copy => (
            &[
                Rect(8.5, 8.5, 11.0, 11.0, 1.5, Stroke),
                P(
                    "M15.5 8.5V5.5a1 1 0 0 0-1-1h-9a1 1 0 0 0-1 1v9a1 1 0 0 0 1 1h3",
                    Stroke,
                ),
            ],
            W,
        ),
        Icon::Clone => (
            &[
                Rect(8.5, 8.5, 11.0, 11.0, 1.5, Stroke),
                P(
                    "M15.5 8.5V5.5a1 1 0 0 0-1-1h-9a1 1 0 0 0-1 1v9a1 1 0 0 0 1 1h3M14 11.5v5M11.5 14h5",
                    Stroke,
                ),
            ],
            W,
        ),
        Icon::Paste => (
            &[
                Rect(5.5, 5.0, 13.0, 15.5, 1.5, Stroke),
                P("M9 5V3.5h6V5M9 11h6M9 14.5h4", Stroke),
            ],
            W,
        ),
        Icon::Pencil => (
            &[
                P(
                    "M4.5 19.5l1-4.2L15.8 5a1.8 1.8 0 0 1 2.6 0l.6.6a1.8 1.8 0 0 1 0 2.6L8.7 18.5z",
                    Stroke,
                ),
                P("M13.8 7l3.2 3.2", Stroke),
            ],
            W,
        ),
        Icon::Share => (
            &[
                P("M12 4v11M8 8l4-4 4 4", Stroke),
                P(
                    "M6 11.5H5a1 1 0 0 0-1 1V19a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-6.5a1 1 0 0 0-1-1h-1",
                    Stroke,
                ),
            ],
            W,
        ),
        Icon::Download => (&[P("M12 4v11M7 10.5l5 5 5-5M5 20h14", Stroke)], W),
        Icon::Export => (
            &[
                P(
                    "M6 3.5h8l4 4v12.5a.5.5 0 0 1-.5.5h-11a.5.5 0 0 1-.5-.5z",
                    Stroke,
                ),
                P("M12 10v7M9 14l3 3 3-3", Stroke),
            ],
            W,
        ),
        Icon::Stop => (&[Rect(6.5, 6.5, 11.0, 11.0, 1.5, Fill)], W),
        Icon::ListPlus => (&[P("M4 7h11M4 12h11M4 17h7M17 14v6M14 17h6", Stroke)], 2.22),
        Icon::Background => (
            &[
                Rect(4.0, 4.0, 11.0, 11.0, 1.5, Stroke),
                P("M9 20h10a1 1 0 0 0 1-1V9", Stroke),
            ],
            W,
        ),
        Icon::ListChecks => (
            &[P(
                "M4 7l1.5 1.5L8 6M4 13l1.5 1.5L8 12M4 19l1.5 1.5L8 18M11 7.5h9M11 13.5h9M11 19.5h9",
                Stroke,
            )],
            W,
        ),
        Icon::Sparkle => (
            &[
                P(
                    "M12 3.5l1.8 5.2 5.2 1.8-5.2 1.8L12 17.5l-1.8-5.2-5.2-1.8 5.2-1.8z",
                    Stroke,
                ),
                P(
                    "M18.5 15.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z",
                    Stroke,
                ),
            ],
            W,
        ),
        Icon::FileText => (
            &[
                P("M6.5 3.5h7l4 4v13h-11z", Stroke),
                P("M13.5 3.5v4h4M9 12h6M9 15.5h6", Stroke),
            ],
            W,
        ),
        Icon::File => (
            &[
                P(
                    "M6.5 3.5h7l4 4V20a.5.5 0 0 1-.5.5H6.5A.5.5 0 0 1 6 20V4a.5.5 0 0 1 .5-.5z",
                    Stroke,
                ),
                P("M13.5 3.5V8h4", Stroke),
            ],
            W,
        ),
        Icon::Folder => (&[P(FOLDER, Stroke)], W),
        Icon::FolderFill => (&[P(FOLDER, Fill)], W),
        Icon::FolderOpen => (
            &[
                P(
                    "M3.5 17.5V6.5a1 1 0 0 1 1-1h4.6l2 2.2h7.4a1 1 0 0 1 1 1v1.3",
                    Stroke,
                ),
                P(
                    "M3.5 17.5l2.6-6.4a1 1 0 0 1 .9-.6h13.6a.6.6 0 0 1 .6.8l-2.4 6.2a1 1 0 0 1-.9.6H4.5",
                    Stroke,
                ),
            ],
            W,
        ),
        Icon::FolderPlus => (
            &[P(FOLDER, Stroke), P("M12 10.8v5.4M9.3 13.5h5.4", Stroke)],
            W,
        ),
        Icon::FolderMove => (
            &[
                P(FOLDER, Stroke),
                P("M9 13.5h6M12.5 11l2.5 2.5-2.5 2.5", Stroke),
            ],
            W,
        ),
        Icon::Tray => (
            &[
                P(
                    "M4 13.5l2.5-8h11l2.5 8v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1z",
                    Stroke,
                ),
                P("M4 13.5h4.5l1 2h5l1-2H20", Stroke),
            ],
            W,
        ),
        Icon::Chat => (
            &[P(
                "M5 5.5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-8l-4.5 3.5v-3.5H5a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z",
                Fill,
            )],
            W,
        ),
        Icon::Pin => (&[P(PIN, Stroke), P("M12 13.5V20", Stroke)], W),
        Icon::PinOff => (&[P(PIN, Stroke), P("M12 13.5V20M4 4l16 16", Stroke)], W),
        Icon::Star => (&[P(STAR, Fill), P(STAR, Stroke)], W),
        Icon::StarLine => (&[P(STAR, Stroke)], W),
        Icon::StarOff => (&[P(STAR, Stroke), P("M4 4l16 16", Stroke)], W),
        Icon::Bell => (
            &[P(
                "M6 15V10a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20a2 2 0 0 0 4 0",
                Stroke,
            )],
            W,
        ),
        Icon::BellOff => (
            &[P(
                "M6 15V10a6 6 0 0 1 9.5-4.9M18 10v5l1.5 2H8M10 20a2 2 0 0 0 4 0M4 4l16 16",
                Stroke,
            )],
            W,
        ),
        Icon::Hash => (&[P("M9.5 4l-2 16M16.5 4l-2 16M5 9h15M4 15h15", Stroke)], W),
        Icon::CheckCircle => (
            &[
                Circle(12.0, 12.0, 8.5, Fill),
                P("M8 12.3l2.7 2.7L16.2 9.5", KnockStroke),
            ],
            W,
        ),
        Icon::Circle => (&[Circle(12.0, 12.0, 8.5, Stroke)], W),
        Icon::Gear => (
            &[
                Circle(12.0, 12.0, 3.0, Stroke),
                P(
                    "M12 3.5v2.2M12 18.3v2.2M3.5 12h2.2M18.3 12h2.2M6 6l1.6 1.6M16.4 16.4L18 18M6 18l1.6-1.6M16.4 7.6L18 6",
                    Stroke,
                ),
            ],
            W,
        ),
        Icon::InfoCircle => (
            &[
                Circle(12.0, 12.0, 8.5, Stroke),
                P("M12 11v5.5M12 7.6v.1", Stroke),
            ],
            W,
        ),
        Icon::WarningTriangle => (
            &[
                P("M12 4l9 15.5H3z", Fill),
                P("M12 10v4.5M12 17v.01", KnockStroke),
            ],
            W,
        ),
        Icon::Bolt => (&[P("M13 3L5 13.5h6L10 21l8-10.5h-6z", Fill)], W),
        Icon::Clock => (
            &[Circle(12.0, 12.0, 8.0, Stroke), P("M12 7.5V12l3 2", Stroke)],
            W,
        ),
        Icon::Mic => (
            &[
                Rect(9.0, 3.5, 6.0, 11.0, 3.0, Stroke),
                P("M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v2.5", Stroke),
            ],
            W,
        ),
        Icon::Pause => (
            &[
                Rect(7.0, 5.5, 3.2, 13.0, 0.8, Fill),
                Rect(13.8, 5.5, 3.2, 13.0, 0.8, Fill),
            ],
            W,
        ),
        Icon::Play => (&[P("M8 5.5v13l10.5-6.5z", Fill)], W),
        Icon::Image => (
            &[
                Rect(3.5, 5.0, 17.0, 14.0, 1.5, Stroke),
                Circle(9.0, 10.0, 1.7, Stroke),
                P("M20.5 15.5l-4.5-4.5-7.5 8", Stroke),
            ],
            W,
        ),
        Icon::Camera => (
            &[
                P(
                    "M4 8.5a1 1 0 0 1 1-1h2.6l1.6-2.2h5.6l1.6 2.2H19a1 1 0 0 1 1 1V18a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1z",
                    Stroke,
                ),
                Circle(12.0, 13.0, 3.4, Stroke),
            ],
            W,
        ),
        Icon::Target => (
            &[
                Circle(12.0, 12.0, 8.0, Stroke),
                Circle(12.0, 12.0, 4.0, Stroke),
                Circle(12.0, 12.0, 0.8, Fill),
            ],
            W,
        ),
        Icon::Timer => (
            &[
                Circle(12.0, 13.0, 7.5, Stroke),
                P("M12 9v4l2.5 1.5M9.5 3h5", Stroke),
            ],
            W,
        ),
        Icon::MessageSquare => (
            &[
                P("M4.5 5.5h15v10h-8l-4 3.5v-3.5h-3z", Stroke),
                P("M8.5 10.5h7", Stroke),
            ],
            W,
        ),
        Icon::Gauge => (
            &[P("M4 17a8 8 0 1 1 16 0", Stroke), P("M12 17l4-5", Stroke)],
            W,
        ),
    }
}

// ----- SVG path data ---------------------------------------------------------

/// A subpath's points, and whether it is closed.
type Polyline = (Vec<(f32, f32)>, bool);

/// Polylines of a path's subpaths (curves and arcs flattened).
fn polylines(d: &str) -> Vec<Polyline> {
    let tokens = tokenize(d);
    let mut out: Vec<Polyline> = Vec::new();
    let mut current: Vec<(f32, f32)> = Vec::new();
    let mut pen = (0.0_f32, 0.0_f32);
    let mut start = pen;
    let mut index = 0;
    let mut command = 'M';
    let number = |index: &mut usize| -> f32 {
        match tokens.get(*index) {
            Some(Token::Number(value)) => {
                *index += 1;
                *value
            }
            _ => 0.0,
        }
    };
    while index < tokens.len() {
        if let Token::Command(c) = tokens[index] {
            command = c;
            index += 1;
        }
        let relative = command.is_ascii_lowercase();
        let base = if relative { pen } else { (0.0, 0.0) };
        match command.to_ascii_uppercase() {
            'M' => {
                if current.len() > 1 {
                    out.push((std::mem::take(&mut current), false));
                }
                current.clear();
                let x = number(&mut index) + base.0;
                let y = number(&mut index) + base.1;
                pen = (x, y);
                start = pen;
                current.push(pen);
                // Further pairs are line-tos.
                command = if relative { 'l' } else { 'L' };
            }
            'L' => {
                let x = number(&mut index) + base.0;
                let y = number(&mut index) + base.1;
                pen = (x, y);
                current.push(pen);
            }
            'H' => {
                let x = number(&mut index) + base.0;
                pen = (x, pen.1);
                current.push(pen);
            }
            'V' => {
                let y = number(&mut index) + base.1;
                pen = (pen.0, y);
                current.push(pen);
            }
            'C' => {
                let c1 = (number(&mut index) + base.0, number(&mut index) + base.1);
                let c2 = (number(&mut index) + base.0, number(&mut index) + base.1);
                let end = (number(&mut index) + base.0, number(&mut index) + base.1);
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    current.push((
                        a * pen.0 + b * c1.0 + c * c2.0 + e * end.0,
                        a * pen.1 + b * c1.1 + c * c2.1 + e * end.1,
                    ));
                }
                pen = end;
            }
            'A' => {
                let rx = number(&mut index);
                let ry = number(&mut index);
                let rotation = number(&mut index);
                let large = number(&mut index) != 0.0;
                let sweep = number(&mut index) != 0.0;
                let end = (number(&mut index) + base.0, number(&mut index) + base.1);
                arc(&mut current, pen, end, (rx, ry), rotation, large, sweep);
                pen = end;
            }
            'Z' => {
                if current.len() > 1 {
                    out.push((std::mem::take(&mut current), true));
                }
                current.clear();
                pen = start;
                current.push(pen);
            }
            _ => {
                index += 1;
            }
        }
    }
    if current.len() > 1 {
        out.push((current, false));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Command(char),
    Number(f32),
}

fn tokenize(d: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let bytes = d.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index] as char;
        if c.is_ascii_alphabetic() {
            tokens.push(Token::Command(c));
            index += 1;
        } else if c == '-' || c == '.' || c.is_ascii_digit() {
            // A number: sign, digits, one point (".5.5" is two numbers).
            let begin = index;
            index += 1;
            let mut seen_point = c == '.';
            while index < bytes.len() {
                let next = bytes[index] as char;
                if next.is_ascii_digit() {
                    index += 1;
                } else if next == '.' && !seen_point {
                    seen_point = true;
                    index += 1;
                } else {
                    break;
                }
            }
            if let Ok(value) = d[begin..index].parse::<f32>() {
                tokens.push(Token::Number(value));
            }
        } else {
            index += 1;
        }
    }
    tokens
}

/// An SVG elliptical arc, flattened (the spec's endpoint-to-centre
/// conversion; the icons use only circular, unrotated ones).
fn arc(
    out: &mut Vec<(f32, f32)>,
    from: (f32, f32),
    to: (f32, f32),
    radii: (f32, f32),
    _rotation: f32,
    large: bool,
    sweep: bool,
) {
    let (mut rx, mut ry) = (radii.0.abs(), radii.1.abs());
    if rx == 0.0 || ry == 0.0 || from == to {
        out.push(to);
        return;
    }
    let dx = (from.0 - to.0) / 2.0;
    let dy = (from.1 - to.1) / 2.0;
    let lambda = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let numerator = (rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx).max(0.0);
    let denominator = rx * rx * dy * dy + ry * ry * dx * dx;
    let mut factor = (numerator / denominator).sqrt();
    if large == sweep {
        factor = -factor;
    }
    let cx = factor * rx * dy / ry + (from.0 + to.0) / 2.0;
    let cy = -factor * ry * dx / rx + (from.1 + to.1) / 2.0;
    let angle = |x: f32, y: f32| (y / ry).atan2(x / rx);
    let start = angle(from.0 - cx, from.1 - cy);
    let mut delta = angle(to.0 - cx, to.1 - cy) - start;
    let tau = std::f32::consts::TAU;
    if sweep && delta < 0.0 {
        delta += tau;
    } else if !sweep && delta > 0.0 {
        delta -= tau;
    }
    let steps = ((delta.abs() * rx.max(ry)) * 2.0).ceil().clamp(4.0, 64.0) as usize;
    for step in 1..=steps {
        let a = start + delta * step as f32 / steps as f32;
        out.push((cx + rx * a.cos(), cy + ry * a.sin()));
    }
}

fn circle_points(cx: f32, cy: f32, r: f32) -> Vec<(f32, f32)> {
    let steps = 48;
    (0..steps)
        .map(|step| {
            let a = std::f32::consts::TAU * step as f32 / steps as f32;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn rect_points(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    if r <= 0.0 {
        return vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    }
    let mut points = Vec::new();
    let corner = |points: &mut Vec<(f32, f32)>, cx: f32, cy: f32, from: f32| {
        for step in 0..=6 {
            let a = from + std::f32::consts::FRAC_PI_2 * step as f32 / 6.0;
            points.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    };
    use std::f32::consts::PI;
    corner(&mut points, x + w - r, y + r, -PI / 2.0);
    corner(&mut points, x + w - r, y + h - r, 0.0);
    corner(&mut points, x + r, y + h - r, PI / 2.0);
    corner(&mut points, x + r, y + r, PI);
    points
}

// ----- Rasterizing and drawing ---------------------------------------------

struct Mask {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    coverage: Vec<u8>,
}

/// An icon's two coverage masks (its colour, and the knockout colour),
/// relative to its box's top left.
struct Raster {
    main: Option<Mask>,
    knock: Option<Mask>,
}

thread_local! {
    static CACHE: RefCell<HashMap<(Icon, u16, u16), Rc<Raster>>> = RefCell::new(HashMap::new());
}

fn rasterize(icon: Icon, size: f32, stroke: f32) -> Raster {
    let (elements, _) = spec(icon);
    let scale = size / 24.0;
    let width = stroke;
    let mut main = Path::new();
    let mut knock = Path::new();
    let mut any = (false, false);
    let point = |(x, y): (f32, f32)| (x * scale, y * scale);
    for element in elements {
        let (shapes, paint): (Vec<Polyline>, Paint) = match *element {
            El::Path(d, paint) => (polylines(d), paint),
            El::Circle(cx, cy, r, paint) => (vec![(circle_points(cx, cy, r), true)], paint),
            El::Rect(x, y, w, h, r, paint) => (vec![(rect_points(x, y, w, h, r), true)], paint),
        };
        let path = if paint == Paint::KnockStroke {
            any.1 = true;
            &mut knock
        } else {
            any.0 = true;
            &mut main
        };
        for (points, closed) in shapes {
            let mut points: Vec<(f32, f32)> = points.into_iter().map(point).collect();
            match paint {
                Paint::Fill => path.polygon(&points),
                Paint::Stroke | Paint::KnockStroke => {
                    if closed && let Some(&first) = points.first() {
                        points.push(first);
                    }
                    if points.len() == 2 && points[0] == points[1] {
                        // A dot (a zero-length segment's round caps).
                        path.circle(points[0].0, points[0].1, width / 2.0);
                    } else {
                        path.stroke_polyline(&points, width);
                    }
                }
            }
        }
    }
    let mask = |path: &Path, used: bool| {
        used.then(|| path.rasterize())
            .flatten()
            .map(|(x, y, w, h, coverage)| Mask {
                x,
                y,
                w,
                h,
                coverage,
            })
    };
    Raster {
        main: mask(&main, any.0),
        knock: mask(&knock, any.1),
    }
}

fn raster(icon: Icon, size: f32, stroke: Option<f32>) -> Rc<Raster> {
    let stroke = stroke.unwrap_or_else(|| spec(icon).1 * size / 24.0);
    let key = (
        icon,
        (size * 4.0).round() as u16,
        (stroke * 8.0).round() as u16,
    );
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| Rc::new(rasterize(icon, size, stroke)))
            .clone()
    })
}

/// How an icon is drawn: its box size, stroke (px; by default the icon's own
/// width at that size: 5 px in a 60 px box), colour, the colour knocked-out
/// parts take (the background under it) and opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub size: f32,
    pub stroke: Option<f32>,
    pub color: Rgb,
    pub knockout: Rgb,
    pub alpha: f32,
}

impl Style {
    pub fn new(size: f32, color: Rgb) -> Self {
        Self {
            size,
            stroke: None,
            color,
            knockout: crate::exocortex::theme::p().app_background,
            alpha: 1.0,
        }
    }

    pub fn stroke(mut self, px: f32) -> Self {
        self.stroke = Some(px);
        self
    }

    pub fn knockout(mut self, color: Rgb) -> Self {
        self.knockout = color;
        self
    }

    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }
}

/// Draws `icon` centred on (`cx`, `cy`), its box at whole pixels.
pub fn draw(canvas: &mut Canvas, icon: Icon, cx: f32, cy: f32, size: f32, color: Rgb, alpha: f32) {
    draw_styled(canvas, icon, cx, cy, Style::new(size, color).alpha(alpha));
}

pub fn draw_styled(canvas: &mut Canvas, icon: Icon, cx: f32, cy: f32, style: Style) {
    let raster = raster(icon, style.size, style.stroke);
    let left = (cx - style.size / 2.0).round() as i32;
    let top = (cy - style.size / 2.0).round() as i32;
    for (mask, color) in [(&raster.main, style.color), (&raster.knock, style.knockout)] {
        if let Some(mask) = mask {
            canvas.draw_mask(
                left + mask.x,
                top + mask.y,
                mask.w,
                mask.h,
                &mask.coverage,
                color,
                style.alpha,
            );
        }
    }
}

/// Rows the icon may touch when centred at `cy` (for damage and ink).
pub fn rows(cy: f32, size: f32) -> (f32, f32) {
    (cy - size * 0.6 - 2.0, cy + size * 0.6 + 2.0)
}

/// Every icon (tests, a gallery).
pub const ALL: [Icon; 63] = [
    Icon::ChevronLeft,
    Icon::ChevronRight,
    Icon::ChevronDown,
    Icon::ChevronUp,
    Icon::ArrowUp,
    Icon::ArrowDown,
    Icon::DownloadCircle,
    Icon::External,
    Icon::Undo,
    Icon::Redo,
    Icon::Rewind,
    Icon::Plus,
    Icon::X,
    Icon::Check,
    Icon::Search,
    Icon::Ellipsis,
    Icon::Refresh,
    Icon::Trash,
    Icon::Copy,
    Icon::Clone,
    Icon::Paste,
    Icon::Pencil,
    Icon::Share,
    Icon::Download,
    Icon::Export,
    Icon::Stop,
    Icon::ListPlus,
    Icon::Background,
    Icon::ListChecks,
    Icon::Sparkle,
    Icon::FileText,
    Icon::File,
    Icon::Folder,
    Icon::FolderFill,
    Icon::FolderOpen,
    Icon::FolderPlus,
    Icon::FolderMove,
    Icon::Tray,
    Icon::Chat,
    Icon::Pin,
    Icon::PinOff,
    Icon::Star,
    Icon::StarLine,
    Icon::StarOff,
    Icon::Bell,
    Icon::BellOff,
    Icon::Hash,
    Icon::CheckCircle,
    Icon::Circle,
    Icon::Gear,
    Icon::InfoCircle,
    Icon::WarningTriangle,
    Icon::Bolt,
    Icon::Clock,
    Icon::Mic,
    Icon::Pause,
    Icon::Play,
    Icon::Image,
    Icon::Camera,
    Icon::Target,
    Icon::Timer,
    Icon::MessageSquare,
    Icon::Gauge,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_parse_relative_commands_arcs_and_packed_numbers() {
        let lines = polylines("M15 5l-7 7 7 7");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, vec![(15.0, 5.0), (8.0, 12.0), (15.0, 19.0)]);
        let dots = polylines("M6 12h.01M12 12h.01");
        assert_eq!(dots.len(), 2);
        assert!((dots[1].0[1].0 - 12.01).abs() < 1e-4);
        // `.6.6` is two numbers; an arc ends where it says.
        let arc = polylines("M3.5 6.5a1 1 0 0 1 1-1h4.6z");
        let (points, closed) = &arc[0];
        assert!(closed);
        assert!(
            points
                .iter()
                .any(|&(x, y)| (x - 4.5).abs() < 1e-3 && (y - 5.5).abs() < 1e-3)
        );
        assert_eq!(tokenize(".6.6").len(), 2);
    }

    #[test]
    fn every_icon_draws_inside_its_box_at_whole_pixels() {
        for icon in ALL {
            let mut canvas = Canvas::new(80, 80);
            draw(&mut canvas, icon, 40.0, 40.0, 60.0, 0xffffff, 1.0);
            let lit: Vec<(i32, i32)> = (0..80 * 80)
                .filter(|i| canvas.pixels[*i as usize] != 0)
                .map(|i| (i % 80, i / 80))
                .collect();
            assert!(!lit.is_empty(), "{icon:?} drew nothing");
            assert!(
                lit.iter()
                    .all(|&(x, y)| (8..73).contains(&x) && (8..73).contains(&y)),
                "{icon:?} spills out of its box"
            );
            // Moved by a whole pixel, the same pixels.
            let mut moved = Canvas::new(80, 81);
            draw(&mut moved, icon, 40.0, 41.0, 60.0, 0xffffff, 1.0);
            assert_eq!(&moved.pixels[80..], &canvas.pixels[..], "{icon:?}");
        }
        // The knocked-out exclamation is drawn in the knockout colour.
        let mut canvas = Canvas::new(80, 80);
        draw_styled(
            &mut canvas,
            Icon::WarningTriangle,
            40.0,
            40.0,
            Style::new(60.0, 0xffd60a).knockout(0x123456),
        );
        assert!(canvas.pixels.contains(&0x123456));
    }
}
