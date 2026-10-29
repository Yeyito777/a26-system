//! Terminal layout for parsed TeX (exocortex `tui/src/markdown/mathlayout.ts`).
//!
//! Inline math is laid out on one line: Unicode scripts where every
//! character has a Unicode form (x², aᵢ), `^(...)`/`_(...)` otherwise,
//! vulgar fractions for simple digit pairs (½) and `a/b` with parentheses
//! where precedence needs them. Display math uses a small box model
//! instead: stacked fractions, stretchy delimiters, limits above and below
//! large operators, and aligned environment grids. A display expression
//! wider than the width falls back to the linear form, broken before
//! relations and binary operators.

use std::borrow::Cow;
use std::cell::Cell;

use super::parse::{self, MAX_TREE_DEPTH, Node, is_row_environment};
use super::symbols::{self, Class};
use super::text::{
    is_letter, is_mark, is_math_symbol, is_number, is_space, nfc, slice_by_width, trim, trim_end,
    utf16_len, width,
};

const NBSP: char = '\u{A0}';

/// Rendered rows sharing a baseline row (the TUI's `Box`).
#[derive(Debug, Clone)]
struct Block {
    rows: Vec<String>,
    /// Row holding the math axis/baseline used to align neighbours.
    base: usize,
    width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Atom,
    Text,
    Op,
    Group,
    Linfrac,
    Vulgar,
    Frac2d,
    Delim,
    Sqrt,
    Env,
    Scripted,
    Space,
}

#[derive(Debug, Clone)]
struct Item {
    block: Block,
    class: Class,
    kind: Kind,
    /// Source glyph for single atoms.
    text: Option<String>,
    unary: bool,
    limits: Option<bool>,
    /// A single term: safe as a denominator or script base without parentheses.
    simple: bool,
    /// Juxtaposed factors only: safe as a numerator without parentheses.
    product: bool,
    /// Ends in an unparenthesised `_x`/`^x` that a following letter would extend.
    open_script: bool,
}

#[derive(Clone, Copy)]
struct Context {
    /// Stacked (multi-row) layout is allowed.
    stacked: bool,
    /// Inside a superscript/subscript: compact spacing, always linear.
    script: bool,
    /// Use no-break spaces so prose wrapping cannot split the expression.
    nbsp: bool,
}

const LINEAR: Context = Context {
    stacked: false,
    script: false,
    nbsp: false,
};
const SCRIPT: Context = Context {
    stacked: false,
    script: true,
    nbsp: false,
};
const STACKED: Context = Context {
    stacked: true,
    script: false,
    nbsp: false,
};

/// A display expression's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLayout {
    pub rows: Vec<String>,
    /// Row holding the baseline of the last line (where trailing
    /// punctuation belongs).
    pub last_base: usize,
}

fn make_block(rows: Vec<String>, base: usize) -> Block {
    let widest = rows.iter().map(|row| width(row)).max().unwrap_or(0);
    let rows = rows
        .into_iter()
        .map(|row| {
            let pad = widest - width(&row);
            row + &" ".repeat(pad)
        })
        .collect();
    Block {
        rows,
        base,
        width: widest,
    }
}

fn line(text: &str) -> Block {
    make_block(vec![text.to_string()], 0)
}

fn empty() -> Block {
    Block {
        rows: vec![String::new()],
        base: 0,
        width: 0,
    }
}

/// Concatenate blocks left to right on a shared baseline.
fn hcat(blocks: &[Block], gaps: &[usize], space: char) -> Block {
    if blocks.is_empty() {
        return empty();
    }
    let above = blocks.iter().map(|block| block.base).max().unwrap_or(0);
    let below = blocks
        .iter()
        .map(|block| block.rows.len().saturating_sub(block.base + 1))
        .max()
        .unwrap_or(0);
    let mut rows = vec![String::new(); above + below + 1];
    let mut total = 0;
    for (index, block) in blocks.iter().enumerate() {
        let gap_count = gaps.get(index).copied().unwrap_or(0);
        let gap: String = std::iter::repeat_n(space, gap_count).collect();
        let offset = above - block.base;
        for (row, out) in rows.iter_mut().enumerate() {
            out.push_str(&gap);
            match row
                .checked_sub(offset)
                .and_then(|local| block.rows.get(local))
            {
                Some(text) => out.push_str(text),
                None => out.push_str(&" ".repeat(block.width)),
            }
        }
        total += gap_count + block.width;
    }
    Block {
        rows,
        base: above,
        width: total,
    }
}

fn center(block: &Block, width: usize) -> Vec<String> {
    let slack = width.saturating_sub(block.width);
    let left = slack / 2;
    block
        .rows
        .iter()
        .map(|row| format!("{}{row}{}", " ".repeat(left), " ".repeat(slack - left)))
        .collect()
}

/// Stack blocks vertically, centred; `base_block` selects whose baseline
/// the result uses.
fn vstack(blocks: &[Block], base_block: usize) -> Block {
    let width = blocks.iter().map(|block| block.width).max().unwrap_or(0);
    let mut rows = Vec::new();
    let mut base = 0;
    for (index, block) in blocks.iter().enumerate() {
        if index == base_block {
            base = rows.len() + block.base;
        }
        rows.extend(center(block, width));
    }
    Block { rows, base, width }
}

fn item(block: Block, class: Class, kind: Kind, simple: bool, product: bool) -> Item {
    Item {
        block,
        class,
        kind,
        text: None,
        unary: false,
        limits: None,
        simple,
        product,
        open_script: false,
    }
}

fn box_text(block: &Block) -> &str {
    block.rows.get(block.base).map_or("", String::as_str)
}

fn first_char(text: &str) -> Option<char> {
    text.chars().next()
}

fn is_single_digit(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.len() == 1 && text.as_bytes()[0].is_ascii_digit())
}

fn is_digits(item: Option<&Item>) -> bool {
    item.is_some_and(|item| item.kind == Kind::Atom && is_single_digit(item.text.as_deref()))
}

fn is_digit_run(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// TeX's rule: a binary operator with no left operand (or no right one)
/// is ordinary.
fn classify_binaries(items: &mut [Item]) {
    let neighbour = |items: &[Item], from: usize, forward: bool| -> Option<Class> {
        if forward {
            items[from + 1..]
                .iter()
                .find(|item| item.class != Class::Space)
                .map(|item| item.class)
        } else {
            items[..from]
                .iter()
                .rev()
                .find(|item| item.class != Class::Space)
                .map(|item| item.class)
        }
    };
    for k in 0..items.len() {
        if items[k].class != Class::Bin {
            continue;
        }
        let previous = neighbour(items, k, false);
        let next = neighbour(items, k, true);
        if previous.is_none_or(|class| {
            matches!(
                class,
                Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct
            )
        }) {
            items[k].class = Class::Ord;
            items[k].unary = true;
        } else if next.is_none_or(|class| matches!(class, Class::Rel | Class::Close | Class::Punct))
        {
            items[k].class = Class::Ord;
        }
    }
    for current in items.iter_mut() {
        if current.text.as_deref() == Some("*") {
            current.block = line(if current.class == Class::Bin {
                "×"
            } else {
                "∗"
            });
        }
    }
}

/// Columns between two adjacent items.
fn gap(items: &[Item], k: usize, ctx: Context) -> usize {
    let Some(a) = k.checked_sub(1).and_then(|index| items.get(index)) else {
        return 0;
    };
    let b = &items[k];
    if a.class == Class::Space || b.class == Class::Space {
        return 0;
    }
    if ctx.script {
        return usize::from(
            a.class == Class::Op
                && matches!(b.class, Class::Ord | Class::Op)
                && b.kind != Kind::Delim,
        );
    }
    use Class::{Bin, Close, Op, Open, Punct, Rel};
    let (x, y) = (a.class, b.class);
    if x == Rel && y == Rel {
        return 0;
    }
    if x == Rel || y == Rel {
        return usize::from(!(x == Open || y == Close || y == Punct));
    }
    if x == Bin || y == Bin {
        return 1;
    }
    if x == Punct {
        // Thousands separators: 1,000 rather than 1, 000.
        if a.text.as_deref() == Some(",")
            && is_digits(k.checked_sub(2).and_then(|index| items.get(index)))
            && is_digits(Some(b))
            && is_digits(items.get(k + 1))
            && is_digits(items.get(k + 2))
            && !is_digits(items.get(k + 3))
        {
            return 0;
        }
        return 1;
    }
    if x == Op {
        return usize::from(!(matches!(y, Open | Punct | Close) || b.kind == Kind::Delim));
    }
    if y == Op {
        return usize::from(x != Open);
    }
    if a.open_script && first_char(box_text(&b.block)).is_some_and(|c| is_letter(c) || is_number(c))
    {
        return 1;
    }
    if (a.kind == Kind::Frac2d || b.kind == Kind::Frac2d)
        && x != Open
        && y != Close
        && y != Punct
        && b.text.as_deref() != Some(".")
    {
        return 1;
    }
    0
}

/// What a linear fraction may not touch on its left without parentheses.
fn ends_like_factor(text: &str) -> bool {
    text.chars().last().is_some_and(|c| {
        is_letter(c)
            || is_number(c)
            || ")]}′²³¹ⁿ".contains(c)
            || ('\u{2070}'..='\u{2079}').contains(&c)
            || ('\u{2080}'..='\u{2089}').contains(&c)
    })
}

/// Blocks and leading gaps for a list, parenthesising linear fractions
/// that touch a neighbour.
fn arrange(items: &[Item], ctx: Context) -> (Vec<Block>, Vec<usize>) {
    let gaps: Vec<usize> = (0..items.len()).map(|k| gap(items, k, ctx)).collect();
    let blocks = items
        .iter()
        .enumerate()
        .map(|(k, current)| {
            if current.kind != Kind::Linfrac {
                return current.block.clone();
            }
            // Juxtaposed factors (C·A/T, a/b·x) need parentheses; punctuation does not.
            let previous = k.checked_sub(1).and_then(|index| items.get(index));
            let touches_left = previous.is_some_and(|previous| {
                gaps[k] == 0 && !previous.unary && ends_like_factor(box_text(&previous.block))
            });
            let touches_right = items.get(k + 1).is_some_and(|next| {
                gaps.get(k + 1) == Some(&0)
                    && first_char(box_text(&next.block))
                        .is_some_and(|c| is_letter(c) || is_number(c) || "(√∑∫".contains(c))
            });
            if touches_left || touches_right {
                line(&format!("({})", box_text(&current.block)))
            } else {
                current.block.clone()
            }
        })
        .collect();
    (blocks, gaps)
}

fn join_items(items: &[Item], ctx: Context) -> Block {
    let (blocks, gaps) = arrange(items, ctx);
    hcat(&blocks, &gaps, if ctx.nbsp { NBSP } else { ' ' })
}

fn is_product(items: &[Item]) -> bool {
    if items.is_empty() {
        return false;
    }
    // Bracketed groups are single factors whatever they contain: f(a, b).
    let mut depth = 0usize;
    for (k, current) in items.iter().enumerate() {
        if current.class == Class::Open {
            depth += 1;
            continue;
        }
        if current.class == Class::Close {
            if depth == 0 {
                return false;
            }
            depth -= 1;
            continue;
        }
        if depth > 0 {
            continue;
        }
        let call = current.class == Class::Op
            && items
                .get(k + 1)
                .is_some_and(|next| next.class == Class::Open || next.kind == Kind::Delim);
        if !current.product && !call {
            return false;
        }
    }
    depth == 0
}

fn is_simple(items: &[Item]) -> bool {
    if items.len() == 1 {
        return items[0].simple;
    }
    if items.len() > 1
        && items.iter().all(|current| {
            current.kind == Kind::Atom && matches!(current.text.as_deref(), Some(text) if text.len() == 1 && (text.as_bytes()[0].is_ascii_digit() || text == "."))
        })
    {
        return true;
    }
    let (Some(first), Some(second)) = (items.first(), items.get(1)) else {
        return false;
    };
    if first.class == Class::Op {
        if items.len() == 2 && second.kind == Kind::Delim {
            return true;
        }
        if second.text.as_deref() == Some("(")
            && items[items.len() - 1].text.as_deref() == Some(")")
        {
            let mut depth = 0isize;
            for (k, current) in items.iter().enumerate().skip(1) {
                match current.text.as_deref() {
                    Some("(") => depth += 1,
                    Some(")") => {
                        depth -= 1;
                        if depth == 0 && k < items.len() - 1 {
                            return false;
                        }
                    }
                    _ => {}
                }
            }
            return depth == 0;
        }
    }
    false
}

/// Whether the TUI's `containsWords` holds: words rather than symbols.
fn contains_words(nodes: Option<&[Node]>) -> bool {
    nodes.unwrap_or_default().iter().any(|node| match node {
        Node::Text(text) => {
            utf16_len(&text.chars().filter(|&c| !is_space(c)).collect::<String>()) > 1
        }
        Node::Op { text, .. } => {
            let mut chars = text.chars();
            let symbol =
                matches!((chars.next(), chars.next()), (Some(c), None) if is_math_symbol(c));
            !symbol && utf16_len(text) > 1 && text != "sum" && text != "prod"
        }
        Node::Group(body) => contains_words(Some(body)),
        _ => false,
    })
}

fn script_fallback(marker: char, text: &str, words: bool) -> String {
    if text.is_empty() {
        return String::new();
    }
    let bare = text.chars().count() == 1
        || text.chars().all(|c| c.is_ascii_digit() || c == '.')
        || (words && text.chars().all(|c| is_letter(c) || is_number(c)));
    if bare {
        format!("{marker}{text}")
    } else {
        format!("{marker}({text})")
    }
}

fn digits_of(nodes: &[Node]) -> Option<String> {
    let mut digits = String::new();
    for node in nodes {
        match node {
            Node::Atom { text, .. } if is_digit_run(text) => digits.push_str(text),
            Node::Group(body) => digits.push_str(&digits_of(body)?),
            _ => return None,
        }
    }
    (!digits.is_empty()).then_some(digits)
}

fn vulgar_fraction(num: &[Node], den: &[Node]) -> Option<&'static str> {
    let top = digits_of(num)?;
    let bottom = digits_of(den)?;
    symbols::vulgar_fraction(&format!("{top}/{bottom}"))
}

// ── Delimiters ─────────────────────────────────────────────────────────

/// Top, middle and bottom pieces of a stretched delimiter, and the
/// extension between them for braces.
fn stretch(
    delimiter: &str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    Some(match delimiter {
        "(" => ("⎛", "⎜", "⎝", None),
        ")" => ("⎞", "⎟", "⎠", None),
        "[" => ("⎡", "⎢", "⎣", None),
        "]" => ("⎤", "⎥", "⎦", None),
        "{" => ("⎧", "⎨", "⎩", Some("⎪")),
        "}" => ("⎫", "⎬", "⎭", Some("⎪")),
        "⌈" => ("⎡", "⎢", "⎢", None),
        "⌉" => ("⎤", "⎥", "⎥", None),
        "⌊" => ("⎢", "⎢", "⎣", None),
        "⌋" => ("⎥", "⎥", "⎦", None),
        "|" => ("│", "│", "│", None),
        "‖" => ("‖", "‖", "‖", None),
        _ => return None,
    })
}

fn stretched_delimiter(delimiter: &str, height: usize, base: usize) -> Block {
    if delimiter.is_empty() {
        return Block {
            rows: vec![String::new(); height],
            base,
            width: 0,
        };
    }
    if height == 1 {
        return make_block(vec![delimiter.to_string()], 0);
    }
    let Some((top, middle, bottom, extension)) = stretch(delimiter) else {
        let blank = " ".repeat(width(delimiter));
        let rows = (0..height)
            .map(|row| {
                if row == base {
                    delimiter.to_string()
                } else {
                    blank.clone()
                }
            })
            .collect();
        return make_block(rows, base);
    };
    let centre = if base > 0 && base < height - 1 {
        base
    } else {
        height / 2
    };
    let rows = (0..height)
        .map(|row| {
            if row == 0 {
                top
            } else if row == height - 1 {
                bottom
            } else {
                match extension {
                    Some(extension) if row != centre => extension,
                    _ => middle,
                }
            }
            .to_string()
        })
        .collect();
    make_block(rows, base)
}

fn delimited(left: &str, body: Block, right: &str) -> Block {
    let height = body.rows.len();
    let base = body.base;
    let inner = if height > 1 {
        let padded = hcat(std::slice::from_ref(&body), &[1], ' ');
        make_block(
            padded.rows.into_iter().map(|row| row + " ").collect(),
            padded.base,
        )
    } else {
        body
    };
    hcat(
        &[
            stretched_delimiter(left, height, base),
            inner,
            stretched_delimiter(right, height, base),
        ],
        &[],
        ' ',
    )
}

// ── Environments ───────────────────────────────────────────────────────

fn matrix_delimiters(bare: &str) -> Option<(&'static str, &'static str)> {
    Some(match bare {
        "matrix" | "smallmatrix" => ("", ""),
        "pmatrix" => ("(", ")"),
        "bmatrix" => ("[", "]"),
        "Bmatrix" => ("{", "}"),
        "vmatrix" => ("|", "|"),
        "Vmatrix" => ("‖", "‖"),
        "cases" | "dcases" => ("{", ""),
        "rcases" => ("", "}"),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvKind {
    Matrix,
    Cases,
    Rows,
    Array,
    Substack,
}

fn bare_name(name: &str) -> &str {
    name.strip_suffix('*').unwrap_or(name)
}

fn environment_kind(name: &str) -> EnvKind {
    let bare = bare_name(name);
    if matches!(bare, "cases" | "dcases" | "rcases") {
        EnvKind::Cases
    } else if matrix_delimiters(bare).is_some() || symbols::inherited(bare) {
        EnvKind::Matrix
    } else if bare == "array" || bare == "subarray" {
        EnvKind::Array
    } else if bare == "substack" {
        EnvKind::Substack
    } else {
        EnvKind::Rows
    }
}

/// An array's column letters, without rules and `p{…}`/`>{…}` specs.
fn column_letters(spec: &str) -> Vec<char> {
    let chars: Vec<char> = spec.chars().collect();
    let mut kept = Vec::new();
    let mut j = 0;
    while j < chars.len() {
        let c = chars[j];
        if "@!>pmb".contains(c) && chars.get(j + 1) == Some(&'{') {
            if let Some(close) = chars[j + 2..].iter().position(|&d| d == '}') {
                j += close + 3;
                continue;
            }
        }
        if c != '|' {
            kept.push(c);
        }
        j += 1;
    }
    kept.into_iter().filter(|c| "lcr".contains(*c)).collect()
}

fn column_alignments(name: &str, columns_spec: Option<&str>, columns: usize) -> Vec<char> {
    let kind = environment_kind(name);
    let bare = bare_name(name);
    let mut align: Vec<char> = match kind {
        EnvKind::Cases => vec!['l'; columns],
        EnvKind::Array if columns_spec.is_some_and(|spec| !spec.is_empty()) => {
            let letters = column_letters(columns_spec.unwrap_or_default());
            (0..columns)
                .map(|k| letters.get(k).copied().unwrap_or('c'))
                .collect()
        }
        EnvKind::Rows => {
            if matches!(
                bare,
                "gather"
                    | "gathered"
                    | "equation"
                    | "multline"
                    | "subequations"
                    | "displaymath"
                    | "math"
            ) {
                vec!['c'; columns]
            } else if bare == "eqnarray" {
                vec!['r', 'c', 'l']
            } else {
                (0..columns)
                    .map(|k| if k % 2 == 0 { 'r' } else { 'l' })
                    .collect()
            }
        }
        _ => vec!['c'; columns],
    };
    align.resize(align.len().max(columns), 'c');
    align
}

fn column_gap(name: &str, column: usize) -> usize {
    if column == 0 {
        return 0;
    }
    if environment_kind(name) == EnvKind::Rows {
        return if name.starts_with("eqnarray") {
            1
        } else if column % 2 == 0 {
            3
        } else {
            0
        };
    }
    2
}

fn strip_trailing_punctuation(text: &str) -> String {
    let trimmed = trim_end(text);
    match trimmed.strip_suffix([',', ';', '.']) {
        Some(stripped) => stripped.to_string(),
        None => text.to_string(),
    }
}

// ── Lines ──────────────────────────────────────────────────────────────

fn split_lines<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<Vec<&'a Node>> {
    let mut lines: Vec<Vec<&Node>> = vec![Vec::new()];
    for node in nodes {
        if matches!(node, Node::Newline) {
            lines.push(Vec::new());
        } else if let Some(last) = lines.last_mut() {
            last.push(node);
        }
    }
    lines
        .into_iter()
        .enumerate()
        .filter(|(index, current)| !current.is_empty() || *index == 0)
        .map(|(_, current)| current)
        .collect()
}

/// One piece of a linear line to break.
struct Piece {
    text: String,
    gap: usize,
    /// Preference for breaking before this piece: 4 after \quad, 3 relation,
    /// 2 binary operator, 1 after punctuation.
    priority: u8,
}

fn line_text(list: &[&Piece], first: bool, indent: &str) -> String {
    let joined: String = list
        .iter()
        .enumerate()
        .map(|(k, piece)| {
            format!(
                "{}{}",
                if k == 0 {
                    String::new()
                } else {
                    " ".repeat(piece.gap)
                },
                piece.text
            )
        })
        .collect();
    format!("{}{}", if first { "" } else { indent }, trim_end(&joined))
}

/// The layout of one expression, with the switches and failure state the
/// TUI keeps globally.
pub struct Layout {
    /// Whether the renderer draws combining marks (x̄); otherwise accents
    /// without a precomposed character are written out: bar(x).
    combining_marks: bool,
    failed: Cell<bool>,
}

impl Layout {
    pub fn new(combining_marks: bool) -> Layout {
        Layout {
            combining_marks,
            failed: Cell::new(false),
        }
    }

    /// Whether the TUI would have thrown somewhere (its caller then shows
    /// the source).
    pub fn failed(&self) -> bool {
        self.failed.get()
    }

    fn render_list<'a>(
        &self,
        nodes: impl IntoIterator<Item = &'a Node>,
        ctx: Context,
    ) -> Vec<Item> {
        let prepared: Vec<Cow<'a, Node>> = if ctx.stacked && !ctx.script {
            self.auto_delimiters(nodes)
        } else {
            nodes.into_iter().map(Cow::Borrowed).collect()
        };
        let mut items = Vec::new();
        for node in &prepared {
            items.extend(self.render_node(node, ctx));
        }
        classify_binaries(&mut items);
        items
    }

    fn linear<'a>(&self, nodes: impl IntoIterator<Item = &'a Node>, ctx: Context) -> String {
        box_text(&join_items(&self.render_list(nodes, ctx), ctx)).to_string()
    }

    fn join<'a>(&self, nodes: impl IntoIterator<Item = &'a Node>, ctx: Context) -> Block {
        join_items(&self.render_list(nodes, ctx), ctx)
    }

    fn render_node(&self, node: &Node, ctx: Context) -> Vec<Item> {
        match node {
            Node::Atom { text, class } => {
                let mut atom = item(
                    line(text),
                    *class,
                    Kind::Atom,
                    *class == Class::Ord,
                    *class == Class::Ord,
                );
                atom.text = Some(text.clone());
                vec![atom]
            }
            Node::Text(text) => {
                let spaced = trim(text).chars().any(is_space);
                vec![item(line(text), Class::Ord, Kind::Text, !spaced, !spaced)]
            }
            Node::Space(count) => {
                let space = if ctx.nbsp && *count == 1 { NBSP } else { ' ' };
                let text: String = std::iter::repeat_n(space, *count).collect();
                vec![item(line(&text), Class::Space, Kind::Space, false, false)]
            }
            Node::Group(body) => {
                let mut inner = self.render_list(body, ctx);
                if inner.len() > 1 {
                    let simple = is_simple(&inner);
                    let product = is_product(&inner);
                    return vec![item(
                        join_items(&inner, ctx),
                        Class::Ord,
                        Kind::Group,
                        simple,
                        product,
                    )];
                }
                let Some(mut only) = inner.pop() else {
                    return Vec::new();
                };
                if !matches!(only.class, Class::Op | Class::Space) {
                    only.class = Class::Ord;
                }
                vec![only]
            }
            Node::Scripts {
                base,
                sup,
                sub,
                limits,
            } => {
                vec![self.render_scripts(
                    base.as_deref(),
                    sup.as_deref(),
                    sub.as_deref(),
                    *limits,
                    ctx,
                )]
            }
            Node::Frac { num, den } => vec![self.render_fraction(num, den, ctx)],
            Node::Binom { top, bottom } => {
                if ctx.stacked && !ctx.script {
                    let mut stack = vstack(&[self.join(top, ctx), self.join(bottom, ctx)], 0);
                    stack.base = (stack.rows.len() - 1) / 2;
                    return vec![item(
                        delimited("(", stack, ")"),
                        Class::Ord,
                        Kind::Delim,
                        true,
                        true,
                    )];
                }
                let text = format!("C({}, {})", self.linear(top, ctx), self.linear(bottom, ctx));
                vec![item(line(&text), Class::Ord, Kind::Delim, true, true)]
            }
            Node::Sqrt { index, body } => vec![self.render_sqrt(index.as_deref(), body, ctx)],
            Node::Op { text, limits } => {
                let mut op = item(line(text), Class::Op, Kind::Op, true, false);
                op.limits = Some(*limits);
                vec![op]
            }
            Node::Delim { left, right, body } => {
                let body = self.join(body, ctx);
                vec![item(
                    delimited(left, body, right),
                    Class::Ord,
                    Kind::Delim,
                    true,
                    true,
                )]
            }
            Node::Accent {
                mark,
                spanning,
                body,
            } => {
                let inner = self.render_list(body, ctx);
                let block = join_items(&inner, ctx);
                if block.rows.len() > 1 {
                    return vec![item(block, Class::Ord, Kind::Group, true, true)];
                }
                let text = self.accent(box_text(&block), mark, *spanning);
                let simple = is_simple(&inner) || inner.is_empty();
                vec![item(line(&text), Class::Ord, Kind::Atom, simple, true)]
            }
            Node::Boxed(body) => {
                let block = self.join(body, ctx);
                if !ctx.stacked || ctx.script {
                    return vec![item(block, Class::Ord, Kind::Group, false, false)];
                }
                let bar = "─".repeat(block.width + 2);
                let mut framed = vec![format!("┌{bar}┐")];
                framed.extend(block.rows.iter().map(|row| format!("│ {row} │")));
                framed.push(format!("└{bar}┘"));
                vec![item(
                    make_block(framed, block.base + 1),
                    Class::Ord,
                    Kind::Delim,
                    true,
                    true,
                )]
            }
            Node::Env {
                name,
                rows,
                columns,
            } => vec![self.render_environment(name, rows, columns.as_deref(), ctx)],
            Node::Newline => {
                if ctx.script {
                    return Vec::new();
                }
                vec![item(line("  "), Class::Space, Kind::Space, false, false)]
            }
            Node::Tag(body) => vec![
                item(line("  "), Class::Space, Kind::Space, false, false),
                item(
                    line(&format!("({})", self.linear(body, LINEAR))),
                    Class::Ord,
                    Kind::Text,
                    true,
                    true,
                ),
            ],
            Node::Pmod { body, word } => {
                let body = self.linear(body, ctx);
                let text = if *word {
                    format!("(mod {body})")
                } else {
                    format!("({body})")
                };
                vec![
                    item(line(" "), Class::Space, Kind::Space, false, false),
                    item(line(&text), Class::Ord, Kind::Text, true, true),
                ]
            }
            Node::Infix(_) => Vec::new(),
        }
    }

    fn accent(&self, text: &str, mark: &str, spanning: bool) -> String {
        let marked = if text.chars().count() <= 1 || !spanning {
            format!("{text}{mark}")
        } else {
            text.chars()
                .map(|c| {
                    if is_space(c) || is_mark(c) {
                        c.to_string()
                    } else {
                        format!("{c}{mark}")
                    }
                })
                .collect()
        };
        let composed = nfc(&marked);
        if self.combining_marks || !composed.chars().any(is_mark) {
            return composed;
        }
        format!("{}({text})", symbols::accent_name(mark).unwrap_or("accent"))
    }

    fn render_scripts(
        &self,
        base: Option<&Node>,
        sup_nodes: Option<&[Node]>,
        sub_nodes: Option<&[Node]>,
        limits: Option<bool>,
        ctx: Context,
    ) -> Item {
        let mut base_items = base
            .map(|base| self.render_node(base, ctx))
            .unwrap_or_default();
        let base = if base_items.len() == 1 {
            base_items.remove(0)
        } else if base_items.is_empty() {
            item(empty(), Class::Ord, Kind::Atom, true, true)
        } else {
            let simple = is_simple(&base_items);
            let product = is_product(&base_items);
            item(
                join_items(&base_items, ctx),
                Class::Ord,
                Kind::Group,
                simple,
                product,
            )
        };
        let sup = sup_nodes.map(|nodes| self.linear(nodes, SCRIPT));
        let sub = sub_nodes.map(|nodes| self.linear(nodes, SCRIPT));
        let class = if base.class == Class::Space {
            Class::Ord
        } else {
            base.class
        };
        let kind = if base.kind == Kind::Op {
            Kind::Op
        } else {
            Kind::Scripted
        };
        let stack_limits = ctx.stacked && !ctx.script && limits.or(base.limits).unwrap_or(false);

        if stack_limits {
            let mut parts = Vec::new();
            let raised = sup.as_deref().is_some_and(|sup| !sup.is_empty());
            if let Some(sup) = sup.as_deref().filter(|sup| !sup.is_empty()) {
                parts.push(line(sup));
            }
            parts.push(base.block.clone());
            if let Some(sub) = sub.as_deref().filter(|sub| !sub.is_empty()) {
                parts.push(line(sub));
            }
            let mut stacked = item(
                vstack(&parts, usize::from(raised)),
                class,
                kind,
                true,
                false,
            );
            stacked.limits = base.limits;
            return stacked;
        }

        let script = |text: &Option<String>, nodes: Option<&[Node]>, marker: char| -> String {
            let Some(text) = text else {
                return String::new();
            };
            let words = contains_words(nodes);
            let unicode = if words {
                None
            } else if marker == '^' {
                symbols::superscript(text)
            } else {
                symbols::subscript(text)
            };
            unicode.unwrap_or_else(|| script_fallback(marker, text, words))
        };
        let sup_text = script(&sup, sup_nodes, '^');
        let sub_text = script(&sub, sub_nodes, '_');

        if base.block.rows.len() > 1 {
            // Tall base: superscript beside the top row, subscript beside the bottom.
            let height = base.block.rows.len();
            let column = (0..height)
                .map(|row| {
                    if row == 0 && !sup_text.is_empty() {
                        sup_text.clone()
                    } else if row == height - 1 && !sub_text.is_empty() {
                        sub_text.clone()
                    } else {
                        String::new()
                    }
                })
                .collect();
            let block = hcat(
                &[base.block.clone(), make_block(column, base.block.base)],
                &[],
                ' ',
            );
            return item(block, class, kind, true, base.product);
        }

        let mut base_text = box_text(&base.block).to_string();
        if base.kind == Kind::Linfrac || (base.kind == Kind::Group && !base.simple) {
            base_text = format!("({base_text})");
        }
        let tail = if sup_text.is_empty() {
            &sub_text
        } else {
            &sup_text
        };
        let mut chars = tail.chars();
        let open_script = matches!((chars.next(), chars.next()), (Some('_' | '^'), Some(second)) if second != '(')
            && !tail.ends_with(')');
        let product = base.product || base.kind == Kind::Linfrac || base.kind == Kind::Group;
        let mut scripted = item(
            line(&format!("{base_text}{sub_text}{sup_text}")),
            class,
            kind,
            true,
            product,
        );
        scripted.limits = base.limits;
        scripted.open_script = open_script;
        scripted
    }

    fn render_fraction(&self, num: &[Node], den: &[Node], ctx: Context) -> Item {
        if let Some(vulgar) = vulgar_fraction(num, den) {
            return item(line(vulgar), Class::Ord, Kind::Vulgar, true, true);
        }
        if ctx.stacked && !ctx.script {
            let top = self.join(num, ctx);
            let bottom = self.join(den, ctx);
            let width = top.width.max(bottom.width).max(1);
            let mut rows = center(&top, width);
            rows.push("─".repeat(width));
            rows.extend(center(&bottom, width));
            let block = Block {
                rows,
                base: top.rows.len(),
                width,
            };
            return item(block, Class::Ord, Kind::Frac2d, true, true);
        }
        let num_items = self.render_list(num, ctx);
        let den_items = self.render_list(den, ctx);
        if den_items.is_empty() {
            let simple = is_simple(&num_items);
            let product = is_product(&num_items);
            return item(
                join_items(&num_items, ctx),
                Class::Ord,
                Kind::Group,
                simple,
                product,
            );
        }
        let top = box_text(&join_items(&num_items, ctx)).to_string();
        let top = if top.is_empty() { "1".to_string() } else { top };
        let bottom = box_text(&join_items(&den_items, ctx)).to_string();
        let numerator = if is_product(&num_items) || num_items.is_empty() {
            top
        } else {
            format!("({top})")
        };
        let denominator = if is_simple(&den_items) {
            bottom
        } else {
            format!("({bottom})")
        };
        item(
            line(&format!("{numerator}/{denominator}")),
            Class::Ord,
            Kind::Linfrac,
            false,
            false,
        )
    }

    fn render_sqrt(&self, index_nodes: Option<&[Node]>, body: &[Node], ctx: Context) -> Item {
        let inner = self.render_list(body, ctx);
        let block = join_items(&inner, ctx);
        let index = index_nodes
            .map(|nodes| self.linear(nodes, SCRIPT))
            .unwrap_or_default();
        let radical = match index.as_str() {
            "3" => "∛".to_string(),
            "4" => "∜".to_string(),
            "" | "2" => "√".to_string(),
            _ => match symbols::superscript(&index) {
                Some(raised) => format!("{raised}√"),
                None => {
                    let text = format!("root({index}, {})", box_text(&block));
                    return item(line(&text), Class::Ord, Kind::Sqrt, true, true);
                }
            },
        };
        if block.rows.len() > 1 {
            let block = hcat(&[line(&radical), delimited("(", block, ")")], &[], ' ');
            return item(block, Class::Ord, Kind::Sqrt, true, true);
        }
        let text = box_text(&block);
        let text = if is_simple(&inner) {
            format!("{radical}{text}")
        } else {
            format!("{radical}({text})")
        };
        item(line(&text), Class::Ord, Kind::Sqrt, true, true)
    }

    fn may_be_tall(&self, node: &Node) -> bool {
        match node {
            Node::Frac { num, den } => vulgar_fraction(num, den).is_none(),
            Node::Binom { .. } | Node::Env { .. } | Node::Boxed(_) => true,
            Node::Delim { body, .. } | Node::Group(body) | Node::Sqrt { body, .. } => {
                body.iter().any(|child| self.may_be_tall(child))
            }
            Node::Scripts { base, limits, .. } => {
                limits.unwrap_or(false)
                    || matches!(base.as_deref(), Some(Node::Op { limits: true, .. }))
                    || base.as_deref().is_some_and(|base| self.may_be_tall(base))
            }
            _ => false,
        }
    }

    /// In stacked layout, grow plain `(...)`/`[...]` around tall content
    /// like `\left(...\right)`.
    fn auto_delimiters<'a>(&self, nodes: impl IntoIterator<Item = &'a Node>) -> Vec<Cow<'a, Node>> {
        let mut out: Vec<Cow<'a, Node>> = Vec::new();
        // Each entry's tree depth, known once it wraps (bounds the nesting
        // this adds: `((((…))))` around a fraction).
        let mut depths: Vec<Option<usize>> = Vec::new();
        let mut stack: Vec<(usize, &str)> = Vec::new();
        for node in nodes {
            if let Node::Atom { text, .. } = node
                && (text == "(" || text == "[")
            {
                stack.push((out.len(), if text == "(" { "(" } else { "[" }));
                out.push(Cow::Borrowed(node));
                depths.push(None);
                continue;
            }
            let close_atom = match node {
                Node::Atom { .. } => Some(node),
                Node::Scripts {
                    base: Some(base), ..
                } if matches!(base.as_ref(), Node::Atom { .. }) => Some(base.as_ref()),
                _ => None,
            };
            let closer = match close_atom {
                Some(Node::Atom { text, .. }) if text == ")" => Some(")"),
                Some(Node::Atom { text, .. }) if text == "]" => Some("]"),
                _ => None,
            };
            if let Some(closer) = closer {
                let opener = if closer == ")" { "(" } else { "[" };
                while stack.last().is_some_and(|&(_, open)| open != opener) {
                    stack.pop();
                }
                if let Some((position, _)) = stack.pop()
                    && out[position + 1..]
                        .iter()
                        .any(|child| self.may_be_tall(child))
                {
                    let depth = 1
                        + (position + 1..out.len())
                            .map(|index| {
                                *depths[index].get_or_insert_with(|| {
                                    parse::depth(std::slice::from_ref(out[index].as_ref()))
                                })
                            })
                            .max()
                            .unwrap_or(0);
                    if depth > MAX_TREE_DEPTH {
                        self.failed.set(true);
                    } else {
                        let body: Vec<Node> =
                            out.drain(position..).skip(1).map(Cow::into_owned).collect();
                        depths.truncate(position);
                        let delimiter = Node::Delim {
                            left: opener.to_string(),
                            right: closer.to_string(),
                            body,
                        };
                        let wrapped = match node {
                            Node::Scripts {
                                sup, sub, limits, ..
                            } => Node::Scripts {
                                base: Some(Box::new(delimiter)),
                                sup: sup.clone(),
                                sub: sub.clone(),
                                limits: *limits,
                            },
                            _ => delimiter,
                        };
                        out.push(Cow::Owned(wrapped));
                        depths.push(Some(depth + 1));
                        continue;
                    }
                }
            }
            out.push(Cow::Borrowed(node));
            depths.push(None);
        }
        out
    }

    /// Render one environment cell; `&=` keeps the relation's leading space.
    fn render_cell(&self, cell: &[Node], ctx: Context, leading_relation_space: bool) -> Block {
        let items = self.render_list(cell, ctx);
        let block = join_items(&items, ctx);
        if leading_relation_space
            && items
                .first()
                .is_some_and(|first| matches!(first.class, Class::Rel | Class::Bin))
        {
            return hcat(&[block], &[1], ' ');
        }
        block
    }

    fn render_environment(
        &self,
        name: &str,
        rows: &[Vec<Vec<Node>>],
        columns_spec: Option<&str>,
        ctx: Context,
    ) -> Item {
        let kind = environment_kind(name);
        let bare = bare_name(name);
        if symbols::inherited(bare) {
            // MATRIX_DELIMITERS[bare] is an object method: destructuring it throws.
            self.failed.set(true);
            return item(empty(), Class::Ord, Kind::Env, true, true);
        }
        let (left, right) = matrix_delimiters(bare).unwrap_or(("", ""));

        if !ctx.stacked || ctx.script {
            let text = match kind {
                EnvKind::Rows => rows
                    .iter()
                    .map(|row| self.linear(row.iter().flatten(), ctx))
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("; "),
                EnvKind::Substack => rows
                    .iter()
                    .map(|row| self.linear(row.iter().flatten(), ctx))
                    .collect::<Vec<_>>()
                    .join(", "),
                EnvKind::Cases => {
                    let body = rows
                        .iter()
                        .map(|row| {
                            let cells: Vec<String> =
                                row.iter().map(|cell| self.linear(cell, ctx)).collect();
                            let value = cells.first().map_or("", String::as_str);
                            let conditions: Vec<&str> = cells
                                .iter()
                                .skip(1)
                                .map(String::as_str)
                                .filter(|cell| !cell.is_empty())
                                .collect();
                            let condition = strip_trailing_punctuation(&conditions.join(" "));
                            let head = strip_trailing_punctuation(value);
                            if condition.is_empty() {
                                head
                            } else {
                                format!("{head}, {condition}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    let open = if left.is_empty() { "{" } else { left };
                    let close = if !right.is_empty() {
                        right
                    } else if !left.is_empty() {
                        "}"
                    } else {
                        ""
                    };
                    format!("{open}{body}{close}")
                }
                EnvKind::Matrix | EnvKind::Array => {
                    let (open, close) =
                        if kind == EnvKind::Array || (left.is_empty() && right.is_empty()) {
                            ("[", "]")
                        } else {
                            (left, right)
                        };
                    let body = rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|cell| self.linear(cell, ctx))
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!("{open}{body}{close}")
                }
            };
            return item(line(&text), Class::Ord, Kind::Env, true, true);
        }

        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        let align = column_alignments(name, columns_spec, columns);
        let cells: Vec<Vec<Block>> = rows
            .iter()
            .map(|row| {
                (0..columns)
                    .map(|column| {
                        let cell = row.get(column).map_or(&[][..], Vec::as_slice);
                        self.render_cell(cell, ctx, kind == EnvKind::Rows && column % 2 == 1)
                    })
                    .collect()
            })
            .collect();
        let widths: Vec<usize> = (0..columns)
            .map(|column| cells.iter().map(|row| row[column].width).max().unwrap_or(0))
            .collect();
        let tall = cells
            .iter()
            .any(|row| row.iter().any(|cell| cell.rows.len() > 1));
        let mut out_rows: Vec<String> = Vec::new();
        for (index, row) in cells.into_iter().enumerate() {
            if index > 0 && tall {
                out_rows.push(String::new());
            }
            let aligned: Vec<Block> = row
                .into_iter()
                .enumerate()
                .map(|(column, cell)| {
                    let slack = widths[column] - cell.width;
                    let left_pad = match align[column] {
                        'l' => 0,
                        'r' => slack,
                        _ => slack / 2,
                    };
                    hcat(&[cell, line("")], &[left_pad, slack - left_pad], ' ')
                })
                .collect();
            let gaps: Vec<usize> = (0..aligned.len())
                .map(|column| column_gap(name, column))
                .collect();
            out_rows.extend(hcat(&aligned, &gaps, ' ').rows);
        }
        let base = (out_rows.len().max(1) - 1) / 2;
        let grid = make_block(
            out_rows
                .iter()
                .map(|row| trim_end(row).to_string())
                .collect(),
            base,
        );
        if left.is_empty() && right.is_empty() {
            return item(grid, Class::Ord, Kind::Env, true, true);
        }
        item(
            delimited(left, grid, right),
            Class::Ord,
            Kind::Env,
            true,
            true,
        )
    }

    /// Single-line Unicode rendering for inline math. With `nbsp`, spaces
    /// are no-break spaces except after top-level relations and binary
    /// operators, the only places TeX breaks inline math across lines.
    pub fn inline(&self, nodes: &[Node], nbsp: bool) -> String {
        let ctx = Context { nbsp, ..LINEAR };
        split_lines(nodes)
            .into_iter()
            .map(|current| {
                let items = self.render_list(current, ctx);
                let (blocks, gaps) = arrange(&items, ctx);
                let joined: String = blocks
                    .iter()
                    .enumerate()
                    .map(|(k, block)| {
                        let previous = k.checked_sub(1).map(|index| items[index].class);
                        let breakable = !nbsp || matches!(previous, Some(Class::Rel | Class::Bin));
                        let space = if breakable { ' ' } else { NBSP };
                        let gap: String = std::iter::repeat_n(space, gaps[k]).collect();
                        gap + box_text(block)
                    })
                    .collect();
                trim(&joined).to_string()
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Break a linear expression before relations/operators to fit `width` columns.
    fn break_linear(&self, nodes: &[&Node], max: usize) -> Vec<String> {
        let items = self.render_list(nodes.iter().copied(), LINEAR);
        let (blocks, gaps) = arrange(&items, LINEAR);
        let pieces: Vec<Piece> = items
            .iter()
            .enumerate()
            .map(|(k, current)| {
                let previous = k.checked_sub(1).map(|index| &items[index]);
                let priority = if current.class == Class::Space {
                    0
                } else if previous.is_some_and(|previous| {
                    previous.class == Class::Space && width(box_text(&previous.block)) >= 2
                }) {
                    4
                } else if current.class == Class::Rel {
                    3
                } else if current.class == Class::Bin {
                    2
                } else if previous.is_some_and(|previous| previous.class == Class::Punct) {
                    1
                } else {
                    0
                };
                Piece {
                    text: box_text(&blocks[k]).to_string(),
                    gap: gaps[k],
                    priority,
                }
            })
            .collect();
        let indent = if max >= 24 { "  " } else { "" };
        let all: Vec<&Piece> = pieces.iter().collect();
        let whole = line_text(&all, true, indent);
        if width(&whole) <= max {
            return vec![whole];
        }

        let mut lines: Vec<String> = Vec::new();
        let flush = |lines: &mut Vec<String>, list: &[&Piece]| {
            let mut rest = line_text(list, lines.is_empty(), indent);
            // A single piece wider than the line is cut as a last resort,
            // preferably at a space inside it.
            while width(&rest) > max {
                let (mut chunk, mut tail) = slice_by_width(&rest, max);
                if chunk.is_empty() {
                    let mut chars = rest.chars();
                    chunk = chars.next().map(String::from).unwrap_or_default();
                    tail = chars.collect();
                }
                if let Some(space) = chunk.rfind(' ') {
                    let units = utf16_len(&chunk[..space]) as f64;
                    if units > (indent.len() as f64).max(utf16_len(&chunk) as f64 * 0.3) {
                        tail = format!("{}{tail}", &chunk[space + 1..]);
                        chunk.truncate(space);
                    }
                }
                lines.push(trim_end(&chunk).to_string());
                rest = format!("{indent}{tail}");
            }
            if !trim(&rest).is_empty() {
                lines.push(rest);
            }
        };

        let mut current: Vec<&Piece> = Vec::new();
        for piece in &pieces {
            // Explicit spacing never starts a continuation line.
            if current.is_empty() && !lines.is_empty() && trim(&piece.text).is_empty() {
                continue;
            }
            // A piece too wide for any line stays with what precedes it ("= …")
            // and is cut when flushed, rather than leaving a lone operator behind.
            let fits_alone = width(&piece.text) <= max - indent.len();
            while !current.is_empty() && fits_alone {
                let available = max - if lines.is_empty() { 0 } else { indent.len() };
                let mut candidate = current.clone();
                candidate.push(piece);
                if width(&line_text(&candidate, true, indent)) <= available {
                    break;
                }
                // Prefer the strongest break point that keeps a reasonably full line.
                let mut best: Option<usize> = None;
                for k in (1..current.len()).rev() {
                    if (width(&line_text(&current[..k], true, indent)) as f64)
                        < available as f64 * 0.3
                    {
                        break;
                    }
                    if current[k].priority > 0
                        && best.is_none_or(|best| current[k].priority > current[best].priority)
                    {
                        best = Some(k);
                    }
                }
                match best {
                    Some(best) => {
                        flush(&mut lines, &current[..best]);
                        current.drain(..best);
                        while current
                            .first()
                            .is_some_and(|first| trim(&first.text).is_empty())
                        {
                            current.remove(0);
                        }
                    }
                    None => {
                        flush(&mut lines, &current);
                        current.clear();
                    }
                }
            }
            current.push(piece);
        }
        if !current.is_empty() {
            flush(&mut lines, &current);
        }
        lines
    }

    /// Display rows, at most `max` columns wide when a narrower layout exists.
    pub fn display(&self, nodes: &[Node], max: usize) -> DisplayLayout {
        // Lines of a display expression: top-level `\\` rows or the rows of
        // one row environment.
        let significant: Vec<&Node> = nodes
            .iter()
            .filter(|node| !matches!(node, Node::Space(_)))
            .collect();
        let grid = match significant.as_slice() {
            [
                Node::Env {
                    name,
                    rows,
                    columns,
                },
            ] if is_row_environment(name) => Some((name, rows, columns)),
            _ => None,
        };
        let lines: Vec<Vec<&Node>> = match grid {
            Some((_, rows, _)) => rows
                .iter()
                .map(|row| row.iter().flatten().collect())
                .collect(),
            None => split_lines(nodes),
        };
        let done = |rows: Vec<String>, last_base: usize| DisplayLayout {
            rows: rows.iter().map(|row| trim_end(row).to_string()).collect(),
            last_base,
        };
        if let Some((name, rows, columns)) = grid {
            let block = self
                .render_environment(name, rows, columns.as_deref(), STACKED)
                .block;
            if block.width <= max {
                // The last row of a grid sits at its bottom; find its baseline
                // by laying out that row alone.
                let last_row = self.join(lines.last().into_iter().flatten().copied(), STACKED);
                let below = last_row.rows.len() - last_row.base;
                let last_base = block.rows.len().saturating_sub(below);
                return done(block.rows, last_base);
            }
        } else {
            let blocks: Vec<Block> = lines
                .iter()
                .map(|current| self.join(current.iter().copied(), STACKED))
                .collect();
            if blocks.iter().all(|block| block.width <= max) {
                let mut rows = Vec::new();
                let mut last_base = 0;
                for (index, block) in blocks.iter().enumerate() {
                    if index > 0 && (block.rows.len() > 1 || blocks[index - 1].rows.len() > 1) {
                        rows.push(String::new());
                    }
                    last_base = rows.len() + block.base;
                    rows.extend(block.rows.iter().cloned());
                }
                return done(rows, last_base);
            }
        }
        let mut rows = Vec::new();
        let mut last_base = 0;
        for current in &lines {
            let stacked = self.join(current.iter().copied(), STACKED);
            if stacked.width <= max {
                last_base = rows.len() + stacked.base;
                rows.extend(stacked.rows);
            } else {
                rows.extend(self.break_linear(current, max));
                last_base = rows.len().saturating_sub(1);
            }
        }
        done(rows, last_base)
    }
}
