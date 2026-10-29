//! A forgiving TeX math parser (exocortex `tui/src/markdown/mathparse.ts`).
//!
//! It accepts the subset of TeX that chat models emit, including TeX's
//! brace-less arguments (`\frac12`, `\sqrt2`, `x^2`), and never fails on
//! ordinary input: missing arguments become empty, stray braces are
//! skipped, and unknown commands keep their name (without the backslash)
//! so following arguments stay readable. Where the TUI's parser throws
//! (`\not` at the very end) or runs out of stack, the guard records a
//! failure and the caller shows the source instead, as the TUI does.

use std::cell::Cell;

use super::symbols::{self, Class, Font};
use super::text::{is_letter, is_letter_unit, is_space, trim, trim_start};

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Atom {
        text: String,
        class: Class,
    },
    Text(String),
    Space(usize),
    Group(Vec<Node>),
    Scripts {
        base: Option<Box<Node>>,
        sup: Option<Vec<Node>>,
        sub: Option<Vec<Node>>,
        limits: Option<bool>,
    },
    Frac {
        num: Vec<Node>,
        den: Vec<Node>,
    },
    Binom {
        top: Vec<Node>,
        bottom: Vec<Node>,
    },
    Sqrt {
        index: Option<Vec<Node>>,
        body: Vec<Node>,
    },
    Op {
        text: String,
        limits: bool,
    },
    Delim {
        left: String,
        right: String,
        body: Vec<Node>,
    },
    Accent {
        mark: &'static str,
        spanning: bool,
        body: Vec<Node>,
    },
    Boxed(Vec<Node>),
    Env {
        name: String,
        rows: Vec<Vec<Vec<Node>>>,
        columns: Option<String>,
    },
    Newline,
    Tag(Vec<Node>),
    Pmod {
        body: Vec<Node>,
        word: bool,
    },
    /// `\over`, `\choose`, … before their group is resolved; never in a
    /// parsed tree.
    Infix(Infix),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Infix {
    Over,
    Choose,
    Other,
}

impl Node {
    fn atom(text: impl Into<String>, class: Class) -> Node {
        Node::Atom {
            text: text.into(),
            class,
        }
    }

    /// Move the node's children to `out`, for walks that must not recurse.
    fn take_children(&mut self, out: &mut Vec<Node>) {
        match self {
            Node::Group(body) | Node::Boxed(body) | Node::Tag(body) => out.append(body),
            Node::Delim { body, .. } | Node::Accent { body, .. } | Node::Pmod { body, .. } => {
                out.append(body)
            }
            Node::Scripts { base, sup, sub, .. } => {
                out.extend(base.take().map(|base| *base));
                out.extend(sup.take().into_iter().flatten());
                out.extend(sub.take().into_iter().flatten());
            }
            Node::Frac { num: a, den: b } | Node::Binom { top: a, bottom: b } => {
                out.append(a);
                out.append(b);
            }
            Node::Sqrt { index, body } => {
                out.extend(index.take().into_iter().flatten());
                out.append(body);
            }
            Node::Env { rows, .. } => {
                out.extend(std::mem::take(rows).into_iter().flatten().flatten())
            }
            _ => {}
        }
    }
}

/// Parser recursion (lists and items, across nested parsers) deeper than
/// this gives up, and so does a parsed tree deeper than `MAX_TREE_DEPTH`.
/// The TUI's own limit is its stack, some thousands of levels; models never
/// come close, and a phone thread's stack would not survive it.
const MAX_NESTING: usize = 200;
pub const MAX_TREE_DEPTH: usize = 120;

/// The parse's shared state across nested parsers: recursion depth and
/// whether the TUI would have thrown.
#[derive(Default)]
pub struct Guard {
    nesting: Cell<usize>,
    pub failed: Cell<bool>,
}

impl Guard {
    /// Enter one level of recursion; false (and failed) past the limit.
    fn enter(&self) -> bool {
        let nesting = self.nesting.get() + 1;
        if nesting > MAX_NESTING {
            self.failed.set(true);
            return false;
        }
        self.nesting.set(nesting);
        true
    }

    fn leave(&self) {
        self.nesting.set(self.nesting.get() - 1);
    }
}

/// List nesting the TUI parses; deeper source stays text.
const MAX_DEPTH: usize = 60;

/// Environments whose rows are lines of one display (`aligned`, `gather`, …).
pub fn is_row_environment(name: &str) -> bool {
    matches!(
        name,
        "aligned"
            | "align"
            | "align*"
            | "alignat"
            | "alignat*"
            | "alignedat"
            | "flalign"
            | "flalign*"
            | "eqnarray"
            | "eqnarray*"
            | "gather"
            | "gather*"
            | "gathered"
            | "equation"
            | "equation*"
            | "split"
            | "multline"
            | "multline*"
            | "subequations"
            | "displaymath"
            | "math"
    )
}

#[derive(Clone, Copy)]
enum Stop {
    Never,
    Right,
    Cell,
}

struct Parser<'g> {
    src: Vec<char>,
    i: usize,
    depth: usize,
    guard: &'g Guard,
}

impl<'g> Parser<'g> {
    fn new(src: &str, guard: &'g Guard) -> Parser<'g> {
        Parser {
            src: src.chars().collect(),
            i: 0,
            depth: 0,
            guard,
        }
    }

    fn len(&self) -> usize {
        self.src.len()
    }

    fn char_at(&self, index: usize) -> Option<char> {
        self.src.get(index).copied()
    }

    fn peek(&self) -> Option<char> {
        self.char_at(self.i)
    }

    /// Give up: the TUI would throw here. The rest of the source is
    /// skipped so every loop ends.
    fn fail(&mut self) {
        self.guard.failed.set(true);
        self.i = self.len();
    }

    fn parse_all(&mut self) -> Vec<Node> {
        let mut out = Vec::new();
        while self.i < self.len() {
            out.extend(self.parse_list(Stop::Never, false));
            // A stray closer stopped the list: skip it and continue.
            if self.i < self.len() {
                self.i += 1;
            }
        }
        out
    }

    fn at(&self, text: &str) -> bool {
        let mut index = self.i;
        for c in text.chars() {
            if self.char_at(index) != Some(c) {
                return false;
            }
            index += 1;
        }
        true
    }

    /// Letters from `index` on, each tested as one UTF-16 unit.
    fn letter_run_at(&self, index: usize) -> usize {
        let mut end = index;
        while self.char_at(end).is_some_and(is_letter_unit) {
            end += 1;
        }
        end - index
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(is_space) {
            self.i += 1;
        }
    }

    /// `\name` (letters, optionally `*`) at the cursor, without consuming
    /// it. The TUI looks at 40 UTF-16 units: at most 39 letters, a star
    /// only within them.
    fn peek_command(&self) -> Option<String> {
        if self.peek() != Some('\\') {
            return None;
        }
        let first = self.char_at(self.i + 1)?;
        if !first.is_ascii_alphabetic() {
            return Some(first.to_string());
        }
        let mut name = String::new();
        let mut index = self.i + 1;
        while name.len() < 39 && self.char_at(index).is_some_and(|c| c.is_ascii_alphabetic()) {
            name.push(self.src[index]);
            index += 1;
        }
        if name.len() < 39 && self.char_at(index) == Some('*') {
            name.push('*');
        }
        Some(name)
    }

    /// Move past `\name`.
    fn skip_command(&mut self, name: &str) {
        self.i += name.chars().count() + 1;
    }

    fn at_command(&self, name: &str) -> bool {
        self.at(&format!("\\{name}"))
            && !self
                .char_at(self.i + name.len() + 1)
                .is_some_and(|c| c.is_ascii_alphabetic())
    }

    fn at_environment_end(&self) -> bool {
        self.at_command("end")
    }

    fn stops(&self, stop: Stop) -> bool {
        match stop {
            Stop::Never => false,
            Stop::Right => self.at_command("right"),
            Stop::Cell => self.at("&") || self.at("\\\\") || self.at_environment_end(),
        }
    }

    /// Parse items until `stop` says so (or the input ends, or an
    /// unmatched `}` appears). Scripts and primes attach to the preceding
    /// item.
    fn parse_list(&mut self, stop: Stop, in_group: bool) -> Vec<Node> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            // Pathological nesting: keep the remaining source readable as text.
            let rest: String = self.src[self.i.min(self.len())..].iter().collect();
            self.i = self.len();
            self.depth -= 1;
            return vec![Node::Text(rest)];
        }
        if !self.guard.enter() {
            self.i = self.len();
            self.depth -= 1;
            return Vec::new();
        }
        let mut items: Vec<Node> = Vec::new();
        // Consecutive plain letters just parsed, to recognise prose inside math.
        let mut letter_run = 0usize;
        loop {
            let before = self.i;
            self.skip_space();
            let saw_space = self.i > before;
            if self.i >= self.len() || self.stops(stop) {
                break;
            }
            let c = self.src[self.i];
            // Models sometimes write prose in math mode ("\approx 4 hours for
            // one PushT-scale run"). TeX would run the words together; keep
            // the spaces between words and hyphens inside them.
            let after_wordish = letter_run > 0
                || matches!(items.last(), Some(Node::Atom { text, .. }) if is_single_digit(text));
            if after_wordish
                && saw_space
                && is_letter_unit(c)
                && (letter_run >= 2 || self.letter_run_at(self.i) >= 2)
            {
                items.push(Node::Space(1));
            }
            if c == '-' && !saw_space && letter_run >= 2 && self.letter_run_at(self.i + 1) >= 2 {
                items.push(Node::atom("-", Class::Ord));
                self.i += 1;
                letter_run = 0;
                continue;
            }
            if c == '}' {
                if in_group {
                    break;
                }
                self.i += 1;
                continue;
            }
            if c == '^' || c == '_' {
                self.attach_script(&mut items);
                letter_run = 0;
                continue;
            }
            if c == '\'' {
                let mut count = 0;
                while self.peek() == Some('\'') {
                    count += 1;
                    self.i += 1;
                }
                let primes = match count {
                    1 => "′".to_string(),
                    2 => "″".to_string(),
                    3 => "‴".to_string(),
                    _ => "′".repeat(count),
                };
                items.push(Node::atom(primes, Class::Ord));
                letter_run = 0;
                continue;
            }
            let command = self.peek_command();
            if let Some(name) = command
                .as_deref()
                .filter(|name| *name == "limits" || *name == "nolimits")
            {
                let limits = name == "limits";
                self.skip_command(name);
                match items.last_mut() {
                    Some(Node::Op {
                        limits: previous, ..
                    }) => *previous = limits,
                    Some(Node::Scripts {
                        limits: previous, ..
                    }) => *previous = Some(limits),
                    _ => {}
                }
                continue;
            }
            let node = self.parse_item();
            letter_run = match &node {
                Some(Node::Atom { text, .. }) if c != '\\' && is_single_letter(text) => {
                    letter_run + 1
                }
                _ => 0,
            };
            items.extend(node);
        }
        self.depth -= 1;
        self.guard.leave();
        resolve_infix(items)
    }

    fn attach_script(&mut self, items: &mut Vec<Node>) {
        let marker = self.src[self.i];
        self.i += 1;
        let argument = self.read_argument();
        let superscript = marker == '^';
        let extend = match items.last() {
            Some(Node::Scripts { sup, sub, .. }) => {
                if superscript {
                    sup.is_none()
                } else {
                    sub.is_none()
                }
            }
            _ => false,
        };
        if !extend {
            let wraps = matches!(items.last(), Some(last) if !matches!(last, Node::Infix(_) | Node::Space(_)));
            let base = if wraps { items.pop() } else { None };
            let limits = match &base {
                Some(Node::Op { limits, .. }) => Some(*limits),
                _ => None,
            };
            items.push(Node::Scripts {
                base: base.map(Box::new),
                sup: None,
                sub: None,
                limits,
            });
        }
        if let Some(Node::Scripts { sup, sub, .. }) = items.last_mut() {
            let slot = if superscript { sup } else { sub };
            slot.get_or_insert_with(Vec::new).extend(argument);
        }
    }

    /// A TeX macro argument: a braced group, one command (with its own
    /// arguments), or one character.
    fn read_argument(&mut self) -> Vec<Node> {
        self.skip_space();
        let Some(c) = self.peek() else {
            return Vec::new();
        };
        if c == '{' {
            self.i += 1;
            let body = self.parse_list(Stop::Never, true);
            if self.peek() == Some('}') {
                self.i += 1;
            }
            return body;
        }
        if matches!(c, '}' | '&' | '^' | '_') {
            return Vec::new();
        }
        if c == '\\' && (self.at("\\\\") || self.at_environment_end() || self.at_command("right")) {
            return Vec::new();
        }
        match self.parse_item() {
            Some(Node::Infix(_)) | None => Vec::new(),
            Some(node) => vec![node],
        }
    }

    /// Raw text of a braced group (or one character), for `\text` and names.
    fn read_raw_argument(&mut self) -> String {
        self.skip_space();
        if self.peek() != Some('{') {
            if self.i >= self.len() {
                return String::new();
            }
            if let Some(command) = self.peek_command() {
                self.skip_command(&command);
                return format!("\\{command}");
            }
            let c = self.src[self.i];
            self.i += 1;
            return c.to_string();
        }
        let mut depth = 0i64;
        let start = self.i + 1;
        let mut j = self.i;
        while j < self.len() {
            let c = self.src[j];
            if c == '\\' {
                j += 2;
                continue;
            }
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    self.i = j + 1;
                    return self.src[start..j].iter().collect();
                }
            }
            j += 1;
        }
        self.i = self.len();
        self.src[start.min(self.len())..].iter().collect()
    }

    /// `[...]` optional argument, if present.
    fn read_optional(&mut self) -> Option<String> {
        self.skip_space();
        if self.peek() != Some('[') {
            return None;
        }
        let mut depth = 0i64;
        let mut j = self.i + 1;
        while j < self.len() {
            let c = self.src[j];
            if c == '\\' {
                j += 2;
                continue;
            }
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
            } else if c == ']' && depth <= 0 {
                let content = self.src[self.i + 1..j].iter().collect();
                self.i = j + 1;
                return Some(content);
            }
            j += 1;
        }
        None
    }

    fn parse_item(&mut self) -> Option<Node> {
        if !self.guard.enter() {
            self.fail();
            return None;
        }
        let node = self.parse_item_inner();
        self.guard.leave();
        node
    }

    fn parse_item_inner(&mut self) -> Option<Node> {
        let Some(c) = self.peek() else {
            // String.fromCodePoint(undefined) throws in the TUI.
            self.fail();
            return None;
        };
        if c == '{' {
            self.i += 1;
            let body = self.parse_list(Stop::Never, true);
            if self.peek() == Some('}') {
                self.i += 1;
            }
            return Some(Node::Group(body));
        }
        if c == '\\' {
            return self.parse_command();
        }
        if c == '~' {
            self.i += 1;
            return Some(Node::Space(1));
        }
        if matches!(c, '&' | '$' | '#') {
            // Alignment marks outside an environment and stray dollars are
            // layout syntax, not content.
            self.i += 1;
            return (c == '#').then(|| Node::atom("#", Class::Ord));
        }
        self.i += 1;
        if c == '-' {
            return Some(Node::atom("−", Class::Bin));
        }
        Some(Node::atom(c, symbols::text_class(&c.to_string())))
    }

    fn parse_command(&mut self) -> Option<Node> {
        if self.at("\\\\") {
            self.i += 2;
            if self.peek() == Some('*') {
                self.i += 1;
            }
            self.read_optional();
            return Some(Node::Newline);
        }
        let Some(name) = self.peek_command() else {
            self.i += 1;
            return None;
        };
        self.skip_command(&name);
        let name = name.as_str();

        // Control symbols: \, \; \{ \} \| \$ ...
        if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            if let Some(width) = symbols::spaces(name) {
                return (width > 0).then_some(Node::Space(width));
            }
            return Some(match name {
                "{" => Node::atom("{", Class::Open),
                "}" => Node::atom("}", Class::Close),
                "|" => Node::atom("‖", Class::Ord),
                "\n" | "\t" | " " => Node::Space(1),
                _ => Node::atom(name, Class::Ord),
            });
        }

        let bare = name.strip_suffix('*').unwrap_or(name);
        match name {
            "begin" => return self.parse_environment(),
            "end" => {
                self.read_raw_argument();
                return None;
            }
            "left" => return Some(self.parse_left_right()),
            "right" => {
                self.read_delimiter();
                return None;
            }
            "middle" => {
                let delimiter = self
                    .read_delimiter()
                    .filter(|delimiter| !delimiter.is_empty());
                return delimiter.map(|text| Node::atom(text, Class::Rel));
            }
            _ => {}
        }
        if is_big(name) {
            let delimiter = self
                .read_delimiter()
                .filter(|delimiter| !delimiter.is_empty())?;
            let class = match name.chars().last() {
                Some('l') => Class::Open,
                Some('r') => Class::Close,
                Some('m') => Class::Rel,
                _ => symbols::text_class(&delimiter),
            };
            return Some(Node::atom(delimiter, class));
        }
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let num = self.read_argument();
                let den = self.read_argument();
                return Some(Node::Frac { num, den });
            }
            "binom" | "dbinom" | "tbinom" => {
                let top = self.read_argument();
                let bottom = self.read_argument();
                return Some(Node::Binom { top, bottom });
            }
            "over" => return Some(Node::Infix(Infix::Over)),
            "choose" => return Some(Node::Infix(Infix::Choose)),
            "atop" | "brack" | "brace" => return Some(Node::Infix(Infix::Other)),
            "sqrt" => {
                let index = self
                    .read_optional()
                    .map(|source| parse(&source, self.guard));
                return Some(Node::Sqrt {
                    index,
                    body: self.read_argument(),
                });
            }
            _ => {}
        }
        if symbols::is_text_command(name) {
            let raw = self.read_raw_argument();
            return Some(self.text_node(&raw));
        }
        if let Some(font) = symbols::font_command(name) {
            // An upright word (\mathrm{net}) reads as text, not as a product of letters.
            if font == Font::Plain && self.at_upright_word() {
                let raw = self.read_raw_argument();
                return Some(self.text_node(trim(&raw)));
            }
            return Some(Node::Group(with_font(self.read_argument(), font)));
        }
        if bare == "operatorname" {
            let text = plain_name(&self.read_raw_argument());
            return Some(Node::Op {
                text,
                limits: name.ends_with('*'),
            });
        }
        if let Some(mark) = symbols::accent(name) {
            let spanning = symbols::is_spanning_accent(name);
            return Some(Node::Accent {
                mark,
                spanning,
                body: self.read_argument(),
            });
        }
        match name {
            "overset" | "stackrel" | "underset" => {
                let annotation = self.read_argument();
                let mut base = self.read_argument();
                let base_node = if base.len() == 1 {
                    base.pop()
                } else {
                    Some(Node::Group(base))
                };
                let base = base_node.map(Box::new);
                return Some(if name == "underset" {
                    Node::Scripts {
                        base,
                        sup: None,
                        sub: Some(annotation),
                        limits: Some(true),
                    }
                } else {
                    Node::Scripts {
                        base,
                        sup: Some(annotation),
                        sub: None,
                        limits: Some(true),
                    }
                });
            }
            "overbrace" | "underbrace" | "boldsymbol" | "smash" => {
                return Some(Node::Group(self.read_argument()));
            }
            "xrightarrow" | "xleftarrow" | "xRightarrow" | "xLeftarrow" => {
                self.read_optional();
                let label = self.read_argument();
                let arrow = match name {
                    "xrightarrow" => "→",
                    "xleftarrow" => "←",
                    "xRightarrow" => "⇒",
                    _ => "⇐",
                };
                return Some(Node::Scripts {
                    base: Some(Box::new(Node::atom(arrow, Class::Rel))),
                    sup: Some(label),
                    sub: None,
                    limits: Some(true),
                });
            }
            _ => {}
        }
        if matches!(bare, "boxed" | "fbox" | "framebox") {
            if bare == "framebox" {
                self.read_optional();
            }
            return Some(Node::Boxed(self.read_argument()));
        }
        if bare == "tag" {
            let raw = self.read_raw_argument();
            return Some(Node::Tag(self.text_body(&raw)));
        }
        match name {
            "pmod" | "pod" => {
                return Some(Node::Pmod {
                    body: self.read_argument(),
                    word: name == "pmod",
                });
            }
            "mod" => return Some(Node::atom("mod", Class::Bin)),
            "not" => return self.parse_not(),
            "substack" => {
                let raw = self.read_raw_argument();
                let rows = split_rows(&raw)
                    .iter()
                    .map(|row| vec![parse(row, self.guard)])
                    .collect();
                return Some(Node::Env {
                    name: "substack".to_string(),
                    rows,
                    columns: None,
                });
            }
            "textcolor" | "colorbox" => {
                self.read_raw_argument();
                return Some(Node::Group(self.read_argument()));
            }
            "href" => {
                self.read_raw_argument();
                let raw = self.read_raw_argument();
                return Some(self.text_node(&raw));
            }
            "url" => {
                let raw = self.read_raw_argument();
                return Some(self.text_node(&raw));
            }
            "eqref" | "ref" => {
                let label = self.read_raw_argument();
                return Some(Node::Text(if name == "eqref" {
                    format!("({label})")
                } else {
                    label
                }));
            }
            _ => {}
        }
        if let Some(count) = symbols::dropped_arguments(name) {
            for _ in 0..count {
                self.read_raw_argument();
            }
            return name.starts_with("hspace").then_some(Node::Space(1));
        }
        if symbols::is_ignored(name) {
            return None;
        }
        if let Some(width) = symbols::spaces(name) {
            return (width > 0).then_some(Node::Space(width));
        }
        if let Some((text, limits)) = symbols::large_operator(name) {
            return Some(Node::Op {
                text: text.to_string(),
                limits,
            });
        }
        if symbols::is_function_name(name) {
            let text = symbols::operator_display_name(name).unwrap_or(name);
            return Some(Node::Op {
                text: text.to_string(),
                limits: symbols::is_limit_operator(name),
            });
        }
        if let Some((text, class)) = symbols::symbol(name) {
            return Some(Node::atom(text, class));
        }
        // Unknown command: keep its name (not the backslash) as an upright
        // operator; any braced arguments that follow render as ordinary groups.
        Some(Node::Op {
            text: bare.to_string(),
            limits: false,
        })
    }

    /// `{word}` or `{two words}` after an upright font command
    /// (`/^\{\s*[A-Za-z]{2,}(?:[ -][A-Za-z]+)*\s*\}/` on the trimmed rest).
    fn at_upright_word(&self) -> bool {
        let rest: String = self.src[self.i..].iter().collect();
        let chars: Vec<char> = trim_start(&rest).chars().collect();
        let mut k = 0;
        if chars.first() != Some(&'{') {
            return false;
        }
        k += 1;
        while chars.get(k).is_some_and(|&c| is_space(c)) {
            k += 1;
        }
        let letters = |k: &mut usize| {
            let start = *k;
            while chars.get(*k).is_some_and(|c| c.is_ascii_alphabetic()) {
                *k += 1;
            }
            *k - start
        };
        if letters(&mut k) < 2 {
            return false;
        }
        while matches!(chars.get(k), Some(' ' | '-'))
            && chars.get(k + 1).is_some_and(|c| c.is_ascii_alphabetic())
        {
            k += 1;
            letters(&mut k);
        }
        while chars.get(k).is_some_and(|&c| is_space(c)) {
            k += 1;
        }
        chars.get(k) == Some(&'}')
    }

    fn parse_not(&mut self) -> Option<Node> {
        self.skip_space();
        let next = self.parse_item()?;
        let (text, class) = match next {
            Node::Infix(_) => return None,
            Node::Atom { text, class } => (text, class),
            other => return Some(other),
        };
        if let Some(negated) = symbols::negated(&text) {
            let class = if class == Class::Ord {
                Class::Ord
            } else {
                Class::Rel
            };
            return Some(Node::atom(negated, class));
        }
        Some(Node::atom(format!("{text}\u{338}"), class))
    }

    /// A delimiter after \left, \right, \big…: a character or a delimiter command.
    fn read_delimiter(&mut self) -> Option<String> {
        self.skip_space();
        let c = self.peek()?;
        if let Some(command) = self.peek_command() {
            self.skip_command(&command);
            return Some(
                match command.as_str() {
                    "." => "",
                    "{" | "lbrace" => "{",
                    "}" | "rbrace" => "}",
                    "|" | "Vert" | "lVert" | "rVert" => "‖",
                    "vert" | "lvert" | "rvert" | "mid" => "|",
                    "backslash" => "\\",
                    other => symbols::symbol(other).map_or("", |(text, _)| text),
                }
                .to_string(),
            );
        }
        self.i += 1;
        Some(if c == '.' {
            String::new()
        } else {
            c.to_string()
        })
    }

    fn parse_left_right(&mut self) -> Node {
        let left = self.read_delimiter().unwrap_or_default();
        let body = self.parse_list(Stop::Right, true);
        let mut right = String::new();
        if self.at_command("right") {
            self.i += "\\right".len();
            right = self.read_delimiter().unwrap_or_default();
        }
        Node::Delim { left, right, body }
    }

    fn parse_environment(&mut self) -> Option<Node> {
        let name = trim(&self.read_raw_argument()).to_string();
        let mut columns = None;
        let starred_matrix = matches!(
            name.as_str(),
            "matrix*" | "pmatrix*" | "bmatrix*" | "Bmatrix*" | "vmatrix*" | "Vmatrix*"
        );
        let column_spec = matches!(
            name.as_str(),
            "array" | "subarray" | "alignat" | "alignat*" | "alignedat"
        );
        if column_spec || starred_matrix {
            self.read_optional();
            columns = if column_spec {
                Some(self.read_raw_argument())
            } else {
                self.read_optional()
            };
        } else if matches!(name.as_str(), "aligned" | "gathered" | "split") {
            self.read_optional();
        }
        let mut rows: Vec<Vec<Vec<Node>>> = Vec::new();
        let mut cells: Vec<Vec<Node>> = Vec::new();
        loop {
            let cell = self.parse_list(Stop::Cell, true);
            cells.push(cell);
            if self.at("&") {
                self.i += 1;
                continue;
            }
            if self.at("\\\\") {
                self.i += 2;
                if self.peek() == Some('*') {
                    self.i += 1;
                }
                self.read_optional();
                rows.push(std::mem::take(&mut cells));
                continue;
            }
            if self.at_environment_end() {
                self.i += 4;
                self.read_raw_argument();
            } else if self.peek() == Some('}') {
                // Unbalanced brace inside an environment: stop here.
                self.i += 1;
            }
            break;
        }
        rows.push(cells);
        // A trailing `\\` leaves an empty final row.
        while rows.len() > 1 && rows.last().is_some_and(|row| row.iter().all(Vec::is_empty)) {
            rows.pop();
        }
        Some(Node::Env {
            name,
            rows,
            columns,
        })
    }

    fn text_node(&mut self, raw: &str) -> Node {
        let mut body = self.text_body(raw);
        if body.len() == 1 {
            body.pop().unwrap_or(Node::Group(Vec::new()))
        } else {
            Node::Group(body)
        }
    }

    /// `\text{...}` content: literal text with embedded `$...$` math.
    fn text_body(&mut self, raw: &str) -> Vec<Node> {
        let raw: Vec<char> = raw.chars().collect();
        let mut out = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, out: &mut Vec<Node>| {
            if !text.is_empty() {
                out.push(Node::Text(std::mem::take(text)));
            }
        };
        let mut j = 0;
        while j < raw.len() {
            let c = raw[j];
            if c == '$' {
                if let Some(offset) = raw[j + 1..].iter().position(|&d| d == '$') {
                    let close = j + 1 + offset;
                    flush(&mut text, &mut out);
                    let inner: String = raw[j + 1..close].iter().collect();
                    out.push(Node::Group(parse(&inner, self.guard)));
                    j = close + 1;
                    continue;
                }
                j += 1;
                continue;
            }
            if c == '{' || c == '}' {
                j += 1;
                continue;
            }
            if c == '~' {
                text.push(' ');
                j += 1;
                continue;
            }
            if c == '\\' {
                let Some(&first) = raw.get(j + 1) else {
                    j += 1;
                    continue;
                };
                if !first.is_ascii_alphabetic() {
                    if matches!(first, ',' | ';' | ' ') {
                        text.push(' ');
                    } else {
                        text.push(first);
                    }
                    j += 2;
                    continue;
                }
                let mut end = j + 1;
                while raw.get(end).is_some_and(|c| c.is_ascii_alphabetic()) {
                    end += 1;
                }
                let command: String = raw[j + 1..end].iter().collect();
                j = end;
                // Nested text commands (\textbf{...}) keep their content.
                if symbols::is_text_command(&command) || symbols::font_command(&command).is_some() {
                    continue;
                }
                if symbols::is_ignored(&command) {
                    continue;
                }
                if command == "quad" || command == "qquad" {
                    text.push_str(if command == "quad" { "  " } else { "    " });
                    continue;
                }
                text.push_str(
                    symbols::symbol(&command).map_or(command.as_str(), |(glyph, _)| glyph),
                );
                continue;
            }
            text.push(c);
            j += 1;
        }
        flush(&mut text, &mut out);
        out
    }
}

fn is_single_digit(text: &str) -> bool {
    text.len() == 1 && text.as_bytes()[0].is_ascii_digit()
}

fn is_single_letter(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if is_letter(c))
}

/// `\big`, `\Bigl`, `\biggr`, `\Biggm`, …
fn is_big(name: &str) -> bool {
    let stem = name.strip_suffix(['l', 'r', 'm']).unwrap_or(name);
    matches!(stem, "big" | "Big" | "bigg" | "Bigg")
}

/// Resolve infix commands: the first one makes a fraction or binomial of
/// what precedes it and of the rest, resolved the same way.
fn resolve_infix(items: Vec<Node>) -> Vec<Node> {
    let mut segments: Vec<Vec<Node>> = Vec::new();
    let mut current = Vec::new();
    let mut infixes = Vec::new();
    for item in items {
        match item {
            Node::Infix(infix) => {
                infixes.push(infix);
                segments.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    let mut resolved = current;
    while let (Some(infix), Some(before)) = (infixes.pop(), segments.pop()) {
        resolved = vec![match infix {
            Infix::Over => Node::Frac {
                num: before,
                den: resolved,
            },
            // \atop and friends: a fraction without a bar is closest to a binomial.
            Infix::Choose | Infix::Other => Node::Binom {
                top: before,
                bottom: resolved,
            },
        }];
    }
    resolved
}

fn with_font(nodes: Vec<Node>, font: Font) -> Vec<Node> {
    if font == Font::Plain {
        return nodes;
    }
    nodes
        .into_iter()
        .map(|node| match node {
            Node::Atom { text, class } => Node::Atom {
                text: symbols::apply_font(&text, font),
                class,
            },
            other => other,
        })
        .collect()
}

/// `\operatorname`'s name without TeX markup.
fn plain_name(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut first = String::new();
    let mut j = 0;
    'outer: while j < chars.len() {
        if chars[j] == '\\' {
            for name in ["mathrm", "text", "textrm", "mathit", "mathbf"] {
                let end = j + 1 + name.len();
                let matches = chars
                    .get(j + 1..end)
                    .is_some_and(|run| run.iter().copied().eq(name.chars()));
                let boundary = !chars
                    .get(end)
                    .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '_');
                if matches && boundary {
                    j = end;
                    continue 'outer;
                }
            }
        }
        first.push(chars[j]);
        j += 1;
    }
    let chars: Vec<char> = first.chars().collect();
    let mut second = String::new();
    let mut j = 0;
    while j < chars.len() {
        if chars[j] == '\\' && matches!(chars.get(j + 1), Some(',' | ';' | ':' | ' ')) {
            second.push(' ');
            j += 2;
            continue;
        }
        second.push(chars[j]);
        j += 1;
    }
    let third: String = second
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | '\\'))
        .collect();
    trim(&third).to_string()
}

/// Split `a \\ b` at top-level row separators.
fn split_rows(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut rows = Vec::new();
    let mut depth = 0i64;
    let mut start = 0;
    let mut j = 0;
    while j < chars.len() {
        let c = chars[j];
        if c == '\\' && chars.get(j + 1) == Some(&'\\') && depth == 0 {
            rows.push(chars[start..j].iter().collect());
            j += 2;
            start = j;
            continue;
        }
        if c == '\\' {
            j += 2;
            continue;
        }
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
        }
        j += 1;
    }
    rows.push(chars[start.min(chars.len())..].iter().collect());
    rows
}

/// Parse TeX math source; nested sources (`\sqrt[…]`, `$…$` in text)
/// share the guard.
pub fn parse(source: &str, guard: &Guard) -> Vec<Node> {
    Parser::new(source, guard).parse_all()
}

/// The depth of the deepest node, counted without recursion.
pub fn depth(nodes: &[Node]) -> usize {
    let mut deepest = 0;
    let mut stack: Vec<(&Node, usize)> = nodes.iter().map(|node| (node, 1)).collect();
    while let Some((node, level)) = stack.pop() {
        deepest = deepest.max(level);
        for list in child_lists(node) {
            stack.extend(list.iter().map(|child| (child, level + 1)));
        }
    }
    deepest
}

/// A node's child lists.
fn child_lists(node: &Node) -> Vec<&[Node]> {
    match node {
        Node::Group(body) | Node::Boxed(body) | Node::Tag(body) => vec![body],
        Node::Delim { body, .. } | Node::Accent { body, .. } | Node::Pmod { body, .. } => {
            vec![body]
        }
        Node::Scripts { base, sup, sub, .. } => vec![
            base.as_deref().map_or(&[][..], std::slice::from_ref),
            sup.as_deref().unwrap_or_default(),
            sub.as_deref().unwrap_or_default(),
        ],
        Node::Frac { num: a, den: b } | Node::Binom { top: a, bottom: b } => vec![a, b],
        Node::Sqrt { index, body } => vec![index.as_deref().unwrap_or_default(), body],
        Node::Env { rows, .. } => rows.iter().flatten().map(Vec::as_slice).collect(),
        _ => Vec::new(),
    }
}

/// Drop a tree without recursion (a chain of `x^a^b^…` nests deeply).
pub fn dismantle(nodes: Vec<Node>) {
    let mut stack = nodes;
    while let Some(mut node) = stack.pop() {
        node.take_children(&mut stack);
    }
}
