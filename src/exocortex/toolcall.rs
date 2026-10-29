//! Tool-call display (WP-C2, PAR-123, 124, 126, 146, 147): a call's label,
//! colour and detail as logical lines, ported from the TUI by way of the
//! iPhone app's verbatim ports (exocortex-phone `web/src/ui/chat/tools/`:
//! toolstyles.ts, bashsegments.ts, toolcalllogical.ts, sanitize.ts,
//! display.ts; keep in sync). Each line has its own label and colour: a
//! bash command splits per top-level segment, and a segment whose executable
//! matches an external tool style (`gmail` → Gmail) reads as that tool, its
//! setup, wrappers, heredocs, prompts, `--stdin` data and line continuations
//! kept with the command they belong to.
//!
//! Offsets are bytes where the TS counts UTF-16 units: the shell syntax is
//! ASCII, so both find the same places. Whitespace is JavaScript's (`\s`,
//! `trim`), not Rust's.
#![allow(dead_code)] // view/tools.rs calls it next.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use super::proto::{ExternalToolStyle, Presentation, ToolStyle};

/// A non-shell call longer than this many rows shows `COLLAPSED_ROWS` of
/// them until opened.
pub const COLLAPSE_AFTER_ROWS: usize = 12;
pub const COLLAPSED_ROWS: usize = 8;
/// An `exo` call's free text up to this long, on one line, stays on its
/// header line.
const EXO_INLINE_TEXT_MAX: usize = 100;
const SHELL_TOOLS: [&str; 3] = ["bash", "Bash", "exec_command"];
const SETUP_COMMANDS: [&str; 6] = ["set", "cd", "export", "unset", "source", "."];
/// Reserved words that can open a compound-command segment, e.g. `do` in
/// `for …; do tool …`.
const LEADING_KEYWORDS: [&str; 9] = [
    "!", "{", "do", "then", "else", "elif", "if", "while", "until",
];
const TIMEOUT_OPTIONS_WITH_VALUE: [&str; 4] = ["-k", "-s", "--kill-after", "--signal"];
/// The shell keyword and GNU time's usual paths; `-f FORMAT`/`-o FILE` take
/// a value.
const TIME_WRAPPERS: [&str; 3] = ["time", "/usr/bin/time", "/bin/time"];
const TIME_OPTIONS_WITH_VALUE: [&str; 4] = ["-f", "-o", "--format", "--output"];

/// A line's label and colour (`#rrggbb`; `None` is the theme's tool colour).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineStyle {
    pub label: String,
    pub color: Option<String>,
}

/// One logical line of a call (`ToolCallLogicalLine`). With `has_label`,
/// `text` starts with `style.label` (then a space and the detail, if any);
/// without, it continues the line before. It can hold hard newlines (a
/// prompt, a quoted argument).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalLine {
    pub style: LineStyle,
    pub text: String,
    pub has_label: bool,
}

/// A call as the transcript shows it (`ToolCallDisplay`): its lines, and the
/// first labelled line's colour (the rail, the disclosure) and label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallDisplay {
    pub lines: Vec<LogicalLine>,
    pub color: Option<String>,
    pub label: String,
    pub shell: bool,
}

/// What a collapsed call shows (`collapseToolCall`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collapsed<'a> {
    pub visible: Cow<'a, [LogicalLine]>,
    /// Rows left out, for "… +N more lines"; 0 when all of it shows.
    pub hidden_rows: usize,
}

/// A whole call (display.ts `toolCallDisplay`): an `exo` call's full text,
/// sanitized; its own style before the registry, its validated bash styles
/// before the global ones. `registry` is the daemon's, by name (the app keys
/// it lowercased; either works).
pub fn call_display(
    tool_name: &str,
    summary: &str,
    input: &Value,
    presentation: Option<&Presentation>,
    registry: &HashMap<String, ToolStyle>,
    external: &[ExternalToolStyle],
) -> CallDisplay {
    let summary = call_summary(tool_name, summary, input, presentation);
    let summary = sanitize(&summary);
    let lines = call_lines(
        tool_name,
        &summary,
        presentation.and_then(|p| p.tool_style.as_ref()),
        registry,
        external,
        presentation.map_or(&[][..], |p| p.bash_styles.as_slice()),
        input,
    );
    let first = lines.iter().find(|l| l.has_label).or(lines.first());
    CallDisplay {
        color: first.and_then(|l| l.style.color.clone()),
        label: first.map_or_else(|| tool_name.to_owned(), |l| l.style.label.clone()),
        shell: is_shell(tool_name),
        lines,
    }
}

/// A call's logical lines (`renderToolCallLogicalLines`) from `summary` as
/// given (`call_display` sanitizes it first). `own` is the call's
/// `presentation.toolStyle`; `local_bash` its `presentation.bashStyles` as
/// received, validated here and tried before `external`.
pub fn call_lines(
    tool_name: &str,
    summary: &str,
    own: Option<&ToolStyle>,
    registry: &HashMap<String, ToolStyle>,
    external: &[ExternalToolStyle],
    local_bash: &[ExternalToolStyle],
    input: &Value,
) -> Vec<LogicalLine> {
    let styles: Vec<&ExternalToolStyle> = local_bash_styles(local_bash).chain(external).collect();
    let registry = Registry { own, map: registry };
    render(tool_name, summary, registry, &styles, input)
        .into_iter()
        .map(Line::logical)
        .collect()
}

/// Shell calls always show in full.
pub fn is_shell(tool_name: &str) -> bool {
    SHELL_TOOLS.contains(&tool_name)
}

/// A call's invocation-local bash styles that are fit to show
/// (`callLocalBashStyles`): the first 16, each with a command and a label
/// without control characters and a `#rrggbb` colour.
pub fn local_bash_styles(
    styles: &[ExternalToolStyle],
) -> impl Iterator<Item = &ExternalToolStyle> + '_ {
    let fits = |s: &str, max: usize| {
        let len = s.encode_utf16().count();
        len > 0 && len <= max && !s.chars().any(is_control)
    };
    styles.iter().take(16).filter(move |style| {
        fits(&style.cmd, 1_024) && fits(&style.label, 64) && is_hex_color(&style.color)
    })
}

/// The text a call shows (display.ts `toolCallSummary`): an `exo` call's
/// full prompt or message from its input (older summaries cut it at 100
/// characters), else its summary.
pub fn call_summary<'a>(
    tool_name: &str,
    summary: &'a str,
    input: &Value,
    presentation: Option<&Presentation>,
) -> Cow<'a, str> {
    let detail = presentation
        .and_then(|p| p.detail.as_deref())
        .is_some_and(|d| !d.is_empty());
    let exo = ["subagent", "message", "status", "restart", "abort"]
        .iter()
        .any(|key| input.get(key).is_some());
    if tool_name == "exo" && !detail && exo {
        Cow::Owned(exo_call_detail(input))
    } else {
        Cow::Borrowed(summary)
    }
}

/// An `exo` call's detail (messages.ts `exoCallDetail`): a header naming the
/// action and its target (`message: <conversation>`, `subagent (opus)`),
/// then its free text after `: ` when short and on one line, else on the
/// lines below, in full.
pub fn exo_call_detail(input: &Value) -> String {
    let action = ["subagent", "message", "status", "restart"]
        .into_iter()
        .find(|key| input.get(key).is_some())
        .unwrap_or("abort");
    fn str_at(value: Option<&Value>) -> &str {
        value.and_then(Value::as_str).unwrap_or("")
    }
    let mut header = action.to_owned();
    let mut text = str_at(input.get(action));
    if action == "message" {
        let message = input.get("message");
        header = format!(
            "message: {}",
            str_at(message.and_then(|m| m.get("conversation")))
        );
        text = str_at(message.and_then(|m| m.get("text")));
    } else if action == "subagent" {
        let args = input.get("args");
        let options: Vec<&str> = ["model", "effort"]
            .into_iter()
            .map(|key| str_at(args.and_then(|a| a.get(key))))
            .filter(|s| !s.is_empty())
            .collect();
        if !options.is_empty() {
            header = format!("subagent ({})", options.join(", "));
        }
    }
    let text = js_trim(text);
    if text.is_empty() {
        header
    } else if !text.contains('\n') && text.encode_utf16().nth(EXO_INLINE_TEXT_MAX).is_none() {
        format!("{header}: {text}")
    } else {
        format!("{header}\n{text}")
    }
}

/// Text from a tool or the daemon made safe to draw (terminaltext.ts
/// `sanitizeUntrustedText`): CR and CRLF become LF, a tab four spaces, ESC
/// `␛`, other control characters `�`.
pub fn sanitize(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| is_control(c) && c != '\n') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\x1b' => out.push('␛'),
            c if is_control(c) => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// The TUI's collapse rule: shell calls always show in full; others over
/// `COLLAPSE_AFTER_ROWS` rows (hard lines wrapped at `cols` characters) show
/// their first `COLLAPSED_ROWS` until opened.
pub fn collapse(display: &CallDisplay, cols: usize) -> Collapsed<'_> {
    let cols = cols.max(8);
    let rows = |hard: &str| hard.chars().count().div_ceil(cols).max(1);
    let total: usize = display
        .lines
        .iter()
        .flat_map(|l| l.text.split('\n'))
        .map(rows)
        .sum();
    if display.shell || total <= COLLAPSE_AFTER_ROWS {
        return Collapsed {
            visible: Cow::Borrowed(&display.lines),
            hidden_rows: 0,
        };
    }
    let mut visible = Vec::new();
    let mut left = COLLAPSED_ROWS;
    for line in &display.lines {
        if left == 0 {
            break;
        }
        let mut kept = Vec::new();
        for hard in line.text.split('\n') {
            if left == 0 {
                break;
            }
            let n = rows(hard);
            kept.push(if n <= left {
                hard
            } else {
                hard.char_indices()
                    .nth(left * cols)
                    .map_or(hard, |(at, _)| &hard[..at])
            });
            left -= n.min(left);
        }
        visible.push(LogicalLine {
            style: line.style.clone(),
            text: kept.join("\n"),
            has_label: line.has_label,
        });
    }
    Collapsed {
        visible: Cow::Owned(visible),
        hidden_rows: total - COLLAPSED_ROWS,
    }
}

// ----- Character classes ------------------------------------------------

/// JavaScript's `\s` (and what `trim` removes).
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}'
    ) || matches!(
        c,
        '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_space)
}

fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

fn js_trim(s: &str) -> &str {
    js_trim_end(js_trim_start(s))
}

/// C0 and C1 controls (`[\u0000-\u001f\u007f-\u009f]`).
fn is_control(c: char) -> bool {
    matches!(c, '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// `s` from byte `at`, on a character boundary at or before it.
fn tail(s: &str, at: usize) -> &str {
    let mut at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    &s[at..]
}

// ----- Styles (toolstyles.ts) ---------------------------------------------

/// The call's own style, then the daemon's registry.
#[derive(Clone, Copy)]
struct Registry<'a> {
    own: Option<&'a ToolStyle>,
    map: &'a HashMap<String, ToolStyle>,
}

impl<'a> Registry<'a> {
    fn get(self, name: &str) -> Option<&'a ToolStyle> {
        if let Some(own) = self.own.filter(|own| own.name == name) {
            return Some(own);
        }
        // The app keys its registry by lowercased name.
        self.map.get(name).or_else(|| {
            if name.bytes().any(|b| b.is_ascii_uppercase()) {
                self.map.get(&name.to_ascii_lowercase())
            } else {
                None
            }
        })
    }
}

/// `ResolvedToolDisplay`.
#[derive(Debug, Clone, PartialEq)]
struct Display<'a> {
    label: Cow<'a, str>,
    detail: &'a str,
    color: Option<&'a str>,
    /// The external command that matched, for re-applying it on later lines.
    cmd: Option<&'a str>,
}

/// Where a bash summary's first real command is and the external tool it
/// is (`BashExternalMatch`).
#[derive(Debug, Clone, PartialEq)]
struct ExternalMatch<'a> {
    /// The summary's lines, leading whitespace trimmed.
    lines: Vec<&'a str>,
    /// The line with the command.
    line: usize,
    /// Where it starts in that line.
    start: usize,
    display: Display<'a>,
}

/// A shell word, unquoted (borrowed unless quotes or escapes changed it),
/// and where it starts.
#[derive(Debug, Clone, PartialEq)]
struct Token<'a> {
    text: Cow<'a, str>,
    start: usize,
}

impl Token<'_> {
    fn word(&self) -> &str {
        &self.text
    }
}

/// A heredoc a line opens: its delimiter, whether `<<-` strips tabs before
/// it, and where its `<<` is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Heredoc<'a> {
    terminator: &'a str,
    allow_tabs: bool,
    start: usize,
}

impl Heredoc<'_> {
    fn ends_at(&self, line: &str) -> bool {
        if self.allow_tabs {
            line.trim_start_matches('\t') == self.terminator
        } else {
            line == self.terminator
        }
    }
}

/// `resolveToolDisplay`: for bash, the external tool its first real
/// executable is; else the registry's style, an MCP server's name, or the
/// tool's name.
fn resolve_display<'a>(
    tool_name: &'a str,
    summary: &'a str,
    registry: Registry<'a>,
    external: Option<&[&'a ExternalToolStyle]>,
) -> Display<'a> {
    if matches!(tool_name, "bash" | "exec_command")
        && let Some(styles) = external
        && let Some(found) = resolve_external_match(summary, styles)
    {
        return found.display;
    }
    registry_display(tool_name, summary, registry)
}

fn registry_display<'a>(
    tool_name: &'a str,
    summary: &'a str,
    registry: Registry<'a>,
) -> Display<'a> {
    let info = registry.get(tool_name);
    let label = match info {
        Some(info) => Cow::Borrowed(info.label.as_str()),
        None => mcp_label(tool_name).map_or(Cow::Borrowed(tool_name), Cow::Owned),
    };
    // A call with nothing to add is summarized by its label alone.
    let detail = if summary == label { "" } else { summary };
    Display {
        label,
        detail,
        color: info.map(|info| info.color.as_str()),
        cmd: None,
    }
}

/// An MCP tool's label (`parseMcpToolName`): `mcp__<server>__<tool>` is
/// labelled by its server, readable.
fn mcp_label(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp__")?;
    if rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let bytes = rest.as_bytes();
    let split = (1..bytes.len()).find(|&k| bytes[k..].starts_with(b"__") && k + 2 < bytes.len())?;
    let server = &rest[..split];
    let label = server
        .strip_prefix("claude_ai_")
        .unwrap_or(server)
        .replace('_', " ");
    let label = js_trim(&label);
    Some(if label.is_empty() { server } else { label }.to_owned())
}

/// `resolveBashExternalMatch`: the external tool a bash summary's first real
/// command is, and where that command starts.
fn resolve_external_match<'a>(
    summary: &'a str,
    styles: &[&'a ExternalToolStyle],
) -> Option<ExternalMatch<'a>> {
    if styles.is_empty() {
        return None;
    }
    let (lines, line, start, candidate) = command_candidate(summary)?;
    let display = styles
        .iter()
        .find_map(|style| try_match(candidate, style))?;
    Some(ExternalMatch {
        lines,
        line,
        start,
        display,
    })
}

/// `tryMatch`: the command starts with the style's `cmd`, then ends or
/// goes on after whitespace or a redirection.
fn try_match<'a>(command: &'a str, style: &'a ExternalToolStyle) -> Option<Display<'a>> {
    let cmd = style.cmd.as_str();
    let rest = command.strip_prefix(cmd)?;
    let boundary = rest
        .chars()
        .next()
        .is_none_or(|c| is_js_space(c) || c == '<' || c == '>');
    boundary.then(|| Display {
        label: Cow::Borrowed(style.label.as_str()),
        detail: js_trim_start(rest),
        color: Some(style.color.as_str()),
        cmd: Some(cmd),
    })
}

/// `extractCommandCandidate`: the first real executable past comments,
/// setup commands (`cd`, `export`, a bare assignment), `cat > file <<EOF`
/// preludes and transparent wrappers. Returns the summary's lines, the
/// command's line and start there, and the summary from it on.
fn command_candidate(summary: &str) -> Option<(Vec<&str>, usize, usize, &str)> {
    let source = js_trim_start(summary);
    let lines: Vec<&str> = source.split('\n').collect();
    let mut index = 0;
    while index < lines.len() {
        let raw = lines[index];
        let (segments, _) = split_segments(raw, None);
        for segment in &segments {
            let trimmed = js_trim_start(segment.text);
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('#') {
                break;
            }
            let heredocs = heredoc_specs(trimmed);
            let tokens = tokenize(trimmed);
            if tokens.is_empty() {
                return None;
            }
            let first = strip_assignments(&tokens, 0);
            // `TOOL=/path/to/tool; $TOOL …`: a bare assignment is setup, not
            // the command.
            if tokens
                .get(first)
                .is_none_or(|t| SETUP_COMMANDS.contains(&t.word()))
            {
                if !heredocs.is_empty() {
                    index = skip_heredoc_bodies(&lines, index, &heredocs)?;
                    break;
                }
                continue;
            }
            let command = unwrap_wrappers(&tokens, 0)?;
            if !heredocs.is_empty()
                && tokens[command].text == "cat"
                && output_redirection_before(trimmed, heredocs[0].start)
            {
                index = skip_heredoc_bodies(&lines, index, &heredocs)?;
                break;
            }
            let indent = segment.text.len() - trimmed.len();
            let start = segment.start + indent + tokens[command].start;
            let offset = raw.as_ptr() as usize - source.as_ptr() as usize;
            let candidate = js_trim_start(&source[offset + start..]);
            return Some((lines, index, start, candidate));
        }
        index += 1;
    }
    None
}

/// `tokenizeShellWords`: just enough of a shell's word splitting to find
/// the executable. A quote left open runs past the line (a multi-line
/// argument), not an error.
fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    loop {
        while chars.next_if(|&(_, c)| is_js_space(c)).is_some() {}
        let Some(&(start, _)) = chars.peek() else {
            break;
        };
        let mut end = start;
        let mut owned: Option<String> = None;
        while let Some((at, ch)) = chars.next_if(|&(_, c)| !is_js_space(c)) {
            if !matches!(ch, '\'' | '"' | '\\') {
                match &mut owned {
                    Some(text) => text.push(ch),
                    None => end = at + ch.len_utf8(),
                }
                continue;
            }
            let text = owned.get_or_insert_with(|| line[start..at].to_owned());
            match ch {
                '\'' => {
                    let mut closed = false;
                    for (_, c) in chars.by_ref() {
                        if c == '\'' {
                            closed = true;
                            break;
                        }
                        text.push(c);
                    }
                    if !closed {
                        break;
                    }
                }
                '"' => {
                    let mut closed = false;
                    while let Some((_, c)) = chars.next() {
                        match c {
                            '"' => {
                                closed = true;
                                break;
                            }
                            '\\' => text.push(chars.next().map_or('\\', |(_, c)| c)),
                            c => text.push(c),
                        }
                    }
                    if !closed {
                        break;
                    }
                }
                _ => text.push(chars.next().map_or('\\', |(_, c)| c)),
            }
        }
        let text = owned.map_or(Cow::Borrowed(&line[start..end]), Cow::Owned);
        tokens.push(Token { text, start });
    }
    tokens
}

/// A shell assignment word (`NAME=value`).
fn is_assignment(token: &str) -> bool {
    let bytes = token.as_bytes();
    if !bytes
        .first()
        .is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_')
    {
        return false;
    }
    for &b in &bytes[1..] {
        if b == b'=' {
            return true;
        }
        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return false;
        }
    }
    false
}

fn strip_assignments(tokens: &[Token], start: usize) -> usize {
    let mut i = start;
    while i < tokens.len() && is_assignment(&tokens[i].text) {
        i += 1;
    }
    i
}

fn unwrap_env(tokens: &[Token], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < tokens.len() {
        let t = tokens[i].word();
        if t == "--" {
            i += 1;
            break;
        }
        if !t.starts_with('-') {
            break;
        }
        i += 1;
    }
    let i = strip_assignments(tokens, i);
    (i < tokens.len()).then_some(i)
}

/// `command [-pVv] cmd …`; `-v`/`-V` only describe the command.
fn unwrap_command(tokens: &[Token], start: usize) -> Option<usize> {
    let mut i = start + 1;
    let mut introspection = false;
    while i < tokens.len() && tokens[i].text.starts_with('-') {
        let flags = &tokens[i].text[1..];
        if flags
            .split('-')
            .next()
            .is_some_and(|f| f.contains(['v', 'V']))
        {
            introspection = true;
        }
        i += 1;
    }
    (!introspection && i < tokens.len()).then_some(i)
}

/// Options up to the first operand; those in `with_value` take the next
/// token, `--` ends them.
fn skip_options(tokens: &[Token], start: usize, with_value: &[&str]) -> usize {
    let mut i = start + 1;
    while i < tokens.len() && tokens[i].text.starts_with('-') {
        let option = tokens[i].word();
        i += if with_value.contains(&option) { 2 } else { 1 };
        if option == "--" {
            break;
        }
    }
    i
}

fn unwrap_time(tokens: &[Token], start: usize) -> Option<usize> {
    let i = skip_options(tokens, start, &TIME_OPTIONS_WITH_VALUE);
    (i < tokens.len()).then_some(i)
}

/// `timeout [options] DURATION cmd …` → cmd.
fn unwrap_timeout(tokens: &[Token], start: usize) -> Option<usize> {
    let i = skip_options(tokens, start, &TIMEOUT_OPTIONS_WITH_VALUE);
    if !tokens.get(i).is_some_and(|t| is_duration(&t.text)) {
        return None;
    }
    (i + 1 < tokens.len()).then_some(i + 1)
}

/// `^\d+(?:\.\d+)?[smhd]?$`.
fn is_duration(t: &str) -> bool {
    let bytes = t.as_bytes();
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut i = digits(0);
    if i == 0 {
        return false;
    }
    if bytes.get(i) == Some(&b'.') && digits(i + 1) > 0 {
        i += 1 + digits(i + 1);
    }
    if bytes
        .get(i)
        .is_some_and(|b| matches!(b, b's' | b'm' | b'h' | b'd'))
    {
        i += 1;
    }
    i == bytes.len()
}

fn unwrap_nice(tokens: &[Token], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if let Some(t) = tokens.get(i) {
        let number = t.text.strip_prefix('-').unwrap_or(&t.text);
        if t.text == "-n" {
            i += 2;
        } else if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
            i += 1;
        }
    }
    (i < tokens.len()).then_some(i)
}

/// The executable past leading keywords, assignments and the wrappers
/// `timeout`, `env`, `command`, `time`, `nohup` and `nice`.
fn unwrap_wrappers(tokens: &[Token], start: usize) -> Option<usize> {
    let mut i = start;
    while tokens
        .get(i)
        .is_some_and(|t| LEADING_KEYWORDS.contains(&t.word()))
    {
        i += 1;
    }
    i = strip_assignments(tokens, i);
    loop {
        let t = tokens.get(i)?.word();
        i = match t {
            "timeout" => unwrap_timeout(tokens, i)?,
            "env" => strip_assignments(tokens, unwrap_env(tokens, i)?),
            "command" => unwrap_command(tokens, i)?,
            "nohup" => i + 1,
            "nice" => unwrap_nice(tokens, i)?,
            t if TIME_WRAPPERS.contains(&t) => unwrap_time(tokens, i)?,
            _ => return Some(i),
        };
    }
}

/// The heredocs a line opens (`<<(-)?\s*(?:'([^']*)'|"([^"]*)"|\\?([^\s'"`]+))`,
/// every match).
fn heredoc_specs(line: &str) -> Vec<Heredoc<'_>> {
    let mut specs = Vec::new();
    if !line.contains("<<") {
        return specs;
    }
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            let after = i + 2;
            let found = (bytes.get(after) == Some(&b'-'))
                .then(|| heredoc_word(line, after + 1))
                .flatten()
                .map(|(word, end)| (word, end, true))
                .or_else(|| heredoc_word(line, after).map(|(word, end)| (word, end, false)));
            if let Some((word, end, allow_tabs)) = found {
                if !word.is_empty() {
                    specs.push(Heredoc {
                        terminator: word.strip_prefix('\\').unwrap_or(word),
                        allow_tabs,
                        start: i,
                    });
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    specs
}

/// A heredoc delimiter word from `at` (whitespace first): quoted, or a run
/// of non-space, non-quote characters after an optional backslash. Returns
/// it and where it ends.
fn heredoc_word(line: &str, at: usize) -> Option<(&str, usize)> {
    let word = js_trim_start(&line[at..]);
    let start = line.len() - word.len();
    let run = |s: &str| {
        s.find(|c: char| is_js_space(c) || matches!(c, '\'' | '"' | '`'))
            .unwrap_or(s.len())
    };
    match word.chars().next()? {
        quote @ ('\'' | '"') => {
            let close = word[1..].find(quote)?;
            Some((&word[1..1 + close], start + close + 2))
        }
        first => {
            if first == '\\' {
                let n = run(&word[1..]);
                if n > 0 {
                    return Some((&word[1..1 + n], start + 1 + n));
                }
            }
            let n = run(word);
            (n > 0).then(|| (&word[..n], start + n))
        }
    }
}

fn output_redirection_before(line: &str, before: usize) -> bool {
    let bytes = line.as_bytes();
    (0..before.min(bytes.len())).any(|i| bytes[i] == b'>' && (i == 0 || bytes[i - 1] != b'<'))
}

/// The line past the heredoc bodies `specs` open at `index`.
fn skip_heredoc_bodies(lines: &[&str], index: usize, specs: &[Heredoc]) -> Option<usize> {
    let mut i = index;
    for spec in specs {
        i += 1;
        while i < lines.len() && !spec.ends_at(lines[i]) {
            i += 1;
        }
        if i >= lines.len() {
            return None;
        }
    }
    Some(i)
}

// ----- Segments (bashsegments.ts) -----------------------------------------

/// The quote a line ends inside, if any.
type Quote = Option<u8>;

/// A command between top-level control operators, where it starts, and the
/// operator after it (`""` at the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment<'a> {
    text: &'a str,
    start: usize,
    separator: &'static str,
}

/// `splitTopLevelShellSegmentsWithState`: `line` split on `;`, `&&`, `||`,
/// `|` and `|&` outside quotes, escapes and `$( … )`, from inside `initial`
/// (a quote the line before left open); and the quote it ends inside.
fn split_segments(line: &str, initial: Quote) -> (Vec<Segment<'_>>, Quote) {
    let bytes = line.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut quote = initial;
    // Open `$(` command substitutions: their control operators belong to them.
    let mut substitution = 0usize;
    while i < bytes.len() {
        let ch = bytes[i];
        match quote {
            Some(b'\'') => {
                if ch == b'\'' {
                    quote = None;
                }
                i += 1;
                continue;
            }
            Some(_) => {
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if ch == b'"' {
                    quote = None;
                }
                i += 1;
                continue;
            }
            None => {}
        }
        if ch == b'\'' || ch == b'"' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if ch == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if ch == b'$' && bytes.get(i + 1) == Some(&b'(') {
            substitution += 1;
            i += 2;
            continue;
        }
        if substitution > 0 {
            if ch == b'(' {
                substitution += 1;
            } else if ch == b')' {
                substitution -= 1;
            }
            i += 1;
            continue;
        }
        let next = bytes.get(i + 1).copied();
        let separator = match (ch, next) {
            (b';', _) => ";",
            (b'&', Some(b'&')) => "&&",
            (b'|', Some(b'|')) => "||",
            (b'|', Some(b'&')) => "|&",
            (b'|', _) => "|",
            _ => "",
        };
        if !separator.is_empty() {
            segments.push(Segment {
                text: &line[start..i],
                start,
                separator,
            });
            i += separator.len();
            start = i;
            continue;
        }
        i += 1;
    }
    segments.push(Segment {
        text: &line[start..],
        start,
        separator: "",
    });
    (segments, quote)
}

// ----- Logical lines (toolcalllogical.ts) ---------------------------------

#[derive(Debug, Clone)]
struct Line<'a> {
    display: Display<'a>,
    text: String,
    has_label: bool,
}

impl Line<'_> {
    fn logical(self) -> LogicalLine {
        LogicalLine {
            style: LineStyle {
                label: self.display.label.into_owned(),
                color: self.display.color.map(str::to_owned),
            },
            text: self.text,
            has_label: self.has_label,
        }
    }
}

/// Whether a command goes on past its line: inside a quote, after a
/// trailing backslash, or with heredoc bodies to come
/// (`ShellCommandContinuationState`).
#[derive(Debug, Clone, Default)]
struct Continuation<'a> {
    quote: Quote,
    heredocs: VecDeque<Heredoc<'a>>,
    line_continuation: bool,
}

impl<'a> Continuation<'a> {
    fn goes_on(&self) -> bool {
        self.line_continuation || self.quote.is_some() || !self.heredocs.is_empty()
    }

    /// `advanceShellCommandContinuationState`.
    fn advance(&mut self, line: &'a str) {
        if let Some(first) = self.heredocs.front() {
            if first.ends_at(line) {
                self.heredocs.pop_front();
            }
            self.line_continuation = false;
            return;
        }
        let initial = self.quote;
        self.quote = advance_quote(line, initial);
        self.line_continuation = ends_with_continuation(line, initial);
        if initial.is_none() {
            self.heredocs.extend(heredoc_specs(js_trim_start(line)));
        }
    }
}

/// An external command whose lines are still coming
/// (`PendingExternalCommand`).
#[derive(Debug, Clone)]
struct Pending<'a> {
    display: Display<'a>,
    state: Continuation<'a>,
}

/// The lines of the call's `--stdin` value, which the daemon appends to the
/// summary: data, not commands (`BashStdinLineRange`). `start` holds the
/// value's first line, `end` its last.
#[derive(Debug, Clone, Copy)]
struct StdinRange {
    start: usize,
    end: usize,
}

impl StdinRange {
    fn continues(range: Option<Self>, index: usize) -> bool {
        range.is_some_and(|r| index > r.start && index <= r.end)
    }
}

/// `bashStdinLineRange`.
fn stdin_range(summary: &str, input: &Value) -> Option<StdinRange> {
    let stdin = sanitize(input.get("stdin")?.as_str()?);
    if !stdin.contains('\n') {
        return None;
    }
    let source = js_trim_start(summary);
    let prefix = "--stdin ";
    let marker = source.rfind(&format!("{prefix}{stdin}"))?;
    let newlines = |s: &str| s.bytes().filter(|&b| b == b'\n').count();
    let start = newlines(&source[..marker + prefix.len()]);
    Some(StdinRange {
        start,
        end: start + newlines(&stdin),
    })
}

fn is_prompted(summary: &str) -> bool {
    let mut prompted = false;
    for line in js_trim_start(summary).split('\n') {
        if js_trim(line).is_empty() {
            continue;
        }
        if !js_trim_start(line).starts_with("$ ") {
            return false;
        }
        prompted = true;
    }
    prompted
}

fn strip_prompt(line: &str) -> &str {
    js_trim_start(line).strip_prefix("$ ").unwrap_or(line)
}

fn push<'a>(logical: &mut Vec<Line<'a>>, display: &Display<'a>, detail: &str, has_label: bool) {
    let text = match (has_label, detail.is_empty()) {
        (true, true) => display.label.to_string(),
        (true, false) => format!("{} {detail}", display.label),
        (false, _) => detail.to_owned(),
    };
    logical.push(Line {
        display: display.clone(),
        text,
        has_label,
    });
}

/// `pushCommandDetail`: an external tool's detail, with its command
/// re-labelled where a later line starts with it again.
fn push_command_detail<'a>(logical: &mut Vec<Line<'a>>, display: &Display<'a>) {
    let Some(cmd) = display
        .cmd
        .filter(|cmd| !cmd.is_empty() && !display.detail.is_empty())
    else {
        push(logical, display, display.detail, true);
        return;
    };
    for (i, line) in display.detail.split('\n').enumerate() {
        if i == 0 {
            push(logical, display, line, true);
            continue;
        }
        let t = js_trim_start(line);
        match t.strip_prefix(cmd) {
            Some(args) if args.is_empty() || args.starts_with(' ') => {
                push(logical, display, js_trim_start(args), true);
            }
            _ => push(logical, display, line, false),
        }
    }
}

fn append_matched<'a>(
    logical: &mut Vec<Line<'a>>,
    bash: &Display<'a>,
    text: &str,
    found: Option<&ExternalMatch<'a>>,
) {
    if let Some(found) = found
        && found.line == 0
    {
        let prefix = found
            .lines
            .first()
            .and_then(|line| line.get(..found.start))
            .map_or("", js_trim_end);
        if !js_trim(prefix).is_empty() {
            push(logical, bash, prefix, true);
        }
        push_command_detail(logical, &found.display);
        return;
    }
    push(logical, bash, text, true);
}

/// A line of only `--timeout N`, `--await N` and `--background`: the
/// parent bash call's own options, appended to the summary.
fn is_parent_option_continuation(text: &str) -> bool {
    let mut tokens = text.split(is_js_space).filter(|t| !t.is_empty()).peekable();
    if tokens.peek().is_none() {
        return false;
    }
    while let Some(token) = tokens.next() {
        if token == "--background" {
            continue;
        }
        if token != "--timeout" && token != "--await" {
            return false;
        }
        if tokens.next().is_none_or(|value| value.starts_with("--")) {
            return false;
        }
    }
    true
}

fn append_parent_option_continuation(logical: &mut [Line], text: &str) -> bool {
    let trimmed = js_trim_start(text);
    let Some(previous) = logical.last_mut() else {
        return false;
    };
    if previous.display.cmd.is_none_or(str::is_empty) || !is_parent_option_continuation(trimmed) {
        return false;
    }
    if !previous.text.is_empty() {
        previous.text.push(' ');
    }
    previous.text.push_str(trimmed);
    true
}

/// `maybeContinueExternalCommand`.
fn continue_external<'a>(
    display: Display<'a>,
    text: &'a str,
    mut state: Continuation<'a>,
) -> Option<Pending<'a>> {
    state.advance(text);
    state.goes_on().then_some(Pending { display, state })
}

/// `shouldPreferLinewiseExternalRender`: another command follows the
/// matched one, so the summary renders line by line.
fn prefer_linewise(found: &ExternalMatch, stdin: Option<StdinRange>) -> bool {
    let first = found
        .lines
        .get(found.line)
        .map_or("", |line| tail(line, found.start));
    let mut state = Continuation::default();
    state.advance(first);
    let mut pending = state.goes_on().then_some(state);
    for (i, line) in found.lines.iter().enumerate().skip(found.line + 1) {
        // Literal stdin is flattened into the bash summary after `--stdin`.
        // Its newlines do not end the matched external command.
        if StdinRange::continues(stdin, i) {
            continue;
        }
        let Some(mut state) = pending.take() else {
            return true;
        };
        state.advance(line);
        pending = state.goes_on().then_some(state);
    }
    false
}

/// `appendRenderedBashSegment`: a segment as a continuation of `pending`,
/// an external command or bash, then what is still pending.
fn append_rendered<'a>(
    logical: &mut Vec<Line<'a>>,
    bash: &Display<'a>,
    text: &'a str,
    separator: &str,
    found: Option<&ExternalMatch<'a>>,
    pending: Option<Pending<'a>>,
) -> Option<Pending<'a>> {
    let start = logical.len();
    match &pending {
        Some(pending) => push(logical, &pending.display, text, false),
        None => append_matched(logical, bash, text, found),
    }
    if !separator.is_empty()
        && logical.len() > start
        && let Some(last) = logical.last_mut()
    {
        last.text.push(' ');
        last.text.push_str(separator);
        return None;
    }
    if let Some(pending) = pending {
        return continue_external(pending.display, text, pending.state);
    }
    let found = found?;
    continue_external(
        found.display.clone(),
        tail(text, found.start),
        Continuation::default(),
    )
}

/// One line of a bash summary, parsed once for every way it may render.
struct ParsedLine<'a> {
    raw: &'a str,
    /// Without its `$ ` prompt.
    text: &'a str,
    line_match: Option<ExternalMatch<'a>>,
    segments: Vec<Segment<'a>>,
    matches: Vec<Option<ExternalMatch<'a>>>,
    non_empty: usize,
    has_segment_match: bool,
    in_heredoc: bool,
}

/// The parsing half of `renderSegmentedBashLines`.
fn parse_lines<'a>(summary: &'a str, styles: &[&'a ExternalToolStyle]) -> Vec<ParsedLine<'a>> {
    let mut parsed = Vec::new();
    let mut heredocs: VecDeque<Heredoc<'a>> = VecDeque::new();
    let mut quote: Quote = None;
    for raw in js_trim_start(summary).split('\n') {
        let text = strip_prompt(raw);
        let mut line = ParsedLine {
            raw,
            text,
            line_match: None,
            segments: Vec::new(),
            matches: Vec::new(),
            non_empty: 0,
            has_segment_match: false,
            in_heredoc: !heredocs.is_empty(),
        };
        if line.in_heredoc {
            while heredocs.front().is_some_and(|h| h.ends_at(text)) {
                heredocs.pop_front();
            }
        } else {
            line.line_match = resolve_external_match(text, styles);
            let (segments, ending) = split_segments(text, quote);
            quote = ending;
            for segment in &segments {
                let t = js_trim(segment.text);
                line.non_empty += usize::from(!t.is_empty());
                line.matches.push(if t.is_empty() {
                    None
                } else {
                    resolve_external_match(t, styles)
                });
            }
            line.has_segment_match = line.matches.iter().any(Option::is_some);
            line.segments = segments;
            heredocs.extend(heredoc_specs(js_trim_start(text)));
        }
        parsed.push(line);
    }
    parsed
}

/// The rendering half of `renderSegmentedBashLines` (prompts always
/// stripped, as every caller asks): `None` when the summary is not a
/// prompted transcript (`require_prompts`), or has no line mixing bash and
/// an external tool (nor, with `allow_simple`, an external command line).
fn render_parsed<'a>(
    summary: &str,
    parsed: &[ParsedLine<'a>],
    registry: Registry<'a>,
    styles: &[&'a ExternalToolStyle],
    require_prompts: bool,
    allow_simple: bool,
    stdin: Option<StdinRange>,
) -> Option<Vec<Line<'a>>> {
    if require_prompts && !is_prompted(summary) {
        return None;
    }
    if !require_prompts {
        let mixed = parsed
            .iter()
            .any(|l| l.non_empty > 1 && l.has_segment_match);
        let simple = parsed.iter().any(|l| l.line_match.is_some());
        if !(mixed || allow_simple && simple) {
            return None;
        }
    }

    let mut logical = Vec::new();
    let bash = registry_display("bash", "", registry);
    let mut pending: Option<Pending<'a>> = None;
    for (index, line) in parsed.iter().enumerate() {
        if let Some(p) = &pending
            && StdinRange::continues(stdin, index)
        {
            push(&mut logical, &p.display, line.text, false);
            if stdin.is_some_and(|r| index >= r.end) {
                pending = None;
            }
            continue;
        }

        if let Some(p) = pending.take() {
            if !p.state.heredocs.is_empty() {
                push(&mut logical, &p.display, line.text, false);
                pending = continue_external(p.display, line.text, p.state);
                continue;
            }
            let (segments, _) = split_segments(line.text, p.state.quote);
            let mut p = Some(p);
            for (i, segment) in segments.iter().enumerate() {
                let first = i == 0;
                let text = if first {
                    segment.text
                } else {
                    js_trim(segment.text)
                };
                if !first && text.is_empty() {
                    continue;
                }
                let found = if first {
                    None
                } else {
                    resolve_external_match(text, styles)
                };
                pending = append_rendered(
                    &mut logical,
                    &bash,
                    text,
                    segment.separator,
                    found.as_ref(),
                    if first { p.take() } else { None },
                );
            }
            continue;
        }

        if js_trim(line.raw).is_empty() {
            push(&mut logical, &bash, "", false);
            continue;
        }

        if !line.in_heredoc && append_parent_option_continuation(&mut logical, line.text) {
            continue;
        }

        if line.in_heredoc
            || line.line_match.is_some()
            || line.non_empty <= 1
            || !line.has_segment_match
        {
            let found = if line.in_heredoc {
                None
            } else {
                line.line_match.as_ref()
            };
            pending = append_rendered(&mut logical, &bash, line.text, "", found, None);
            if let Some(found) = &line.line_match
                && let Some(range) = stdin
                && index == range.start
                && range.end > index
            {
                // The first stdin line is appended to the real command line.
                // Keep its external-tool display active for the remaining
                // literal data lines; do not derive shell quote/heredoc
                // state from that unquoted data.
                pending = Some(Pending {
                    display: found.display.clone(),
                    state: Continuation::default(),
                });
            }
            continue;
        }

        pending = None;
        for (segment, found) in line.segments.iter().zip(&line.matches) {
            let text = js_trim(segment.text);
            if text.is_empty() {
                continue;
            }
            pending = append_rendered(
                &mut logical,
                &bash,
                text,
                segment.separator,
                found.as_ref(),
                None,
            );
        }
    }
    Some(logical)
}

/// `renderToolCallLogicalLines`. Each line carries its own display, so bash
/// can mix plain prelude lines with styled external-tool lines without
/// dropping the setup commands.
fn render<'a>(
    tool_name: &'a str,
    summary: &'a str,
    registry: Registry<'a>,
    styles: &[&'a ExternalToolStyle],
    input: &Value,
) -> Vec<Line<'a>> {
    // Codex's exec_command and Claude Code's Bash take the same command.
    let tool_name = if matches!(tool_name, "exec_command" | "Bash") {
        "bash"
    } else {
        tool_name
    };
    let mut logical = Vec::new();
    if tool_name == "bash" {
        let stdin = stdin_range(summary, input);
        let parsed = parse_lines(summary, styles);
        let segmented = |require_prompts, allow_simple| {
            render_parsed(
                summary,
                &parsed,
                registry,
                styles,
                require_prompts,
                allow_simple,
                stdin,
            )
            .filter(|lines| !lines.is_empty())
        };
        if let Some(lines) = segmented(true, false).or_else(|| segmented(false, false)) {
            return lines;
        }

        if let Some(found) = resolve_external_match(summary, styles) {
            if prefer_linewise(&found, stdin)
                && let Some(lines) = segmented(false, true)
            {
                return lines;
            }
            let bash = registry_display("bash", "", registry);
            for (index, raw) in found.lines.iter().enumerate() {
                if index < found.line {
                    if js_trim_start(raw).is_empty() {
                        push(&mut logical, &bash, "", false);
                    } else {
                        push(&mut logical, &bash, raw, true);
                    }
                    continue;
                }
                let prefix = js_trim_end(raw.get(..found.start).unwrap_or(""));
                if !js_trim(prefix).is_empty() {
                    push(&mut logical, &bash, prefix, true);
                }
                push_command_detail(&mut logical, &found.display);
                break;
            }
            return logical;
        }

        if let Some(lines) = segmented(false, true) {
            return lines;
        }
    }
    // Bash with no external tool in it reads as bash, everything else by
    // the registry.
    push_command_detail(
        &mut logical,
        &registry_display(tool_name, summary, registry),
    );
    logical
}

/// The quote a line leaves open, from inside `initial` (`advanceShellQuoteState`).
fn advance_quote(line: &str, initial: Quote) -> Quote {
    let bytes = line.as_bytes();
    let mut quote = initial;
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        match quote {
            Some(b'\'') => {
                if ch == b'\'' {
                    quote = None;
                }
            }
            Some(_) => {
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if ch == b'"' {
                    quote = None;
                }
            }
            None => {
                if ch == b'\'' || ch == b'"' {
                    quote = Some(ch);
                } else if ch == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    quote
}

/// Whether a line ends in a backslash that continues it (outside single
/// quotes).
fn ends_with_continuation(line: &str, initial: Quote) -> bool {
    let bytes = line.as_bytes();
    let mut quote = initial;
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        match quote {
            Some(b'\'') => {
                if ch == b'\'' {
                    quote = None;
                }
            }
            Some(_) => {
                if ch == b'\\' {
                    if i + 1 >= bytes.len() {
                        return true;
                    }
                    i += 2;
                    continue;
                }
                if ch == b'"' {
                    quote = None;
                }
            }
            None => {
                if ch == b'\'' || ch == b'"' {
                    quote = Some(ch);
                } else if ch == b'\\' {
                    if i + 1 >= bytes.len() {
                        return true;
                    }
                    i += 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn styles(list: &[(&str, &str, &str)]) -> Vec<ExternalToolStyle> {
        list.iter()
            .map(|&(cmd, label, color)| ExternalToolStyle {
                cmd: cmd.into(),
                label: label.into(),
                color: color.into(),
            })
            .collect()
    }

    /// Keyed like the app's (lowercased names).
    fn reg(list: &[(&str, &str, &str)]) -> HashMap<String, ToolStyle> {
        list.iter()
            .map(|&(name, label, color)| {
                (
                    name.to_ascii_lowercase(),
                    ToolStyle {
                        name: name.into(),
                        label: label.into(),
                        color: color.into(),
                    },
                )
            })
            .collect()
    }

    fn bash_reg() -> HashMap<String, ToolStyle> {
        reg(&[("bash", "$", "#d19a66")])
    }

    fn ext() -> Vec<ExternalToolStyle> {
        styles(&[
            ("exo", "Exocortex", "#1d9bf0"),
            ("gmail", "Gmail", "#4ddbb7"),
            ("whatsapp", "WhatsApp", "#25d366"),
        ])
    }

    fn lines(
        tool: &str,
        summary: &str,
        registry: &HashMap<String, ToolStyle>,
        external: &[ExternalToolStyle],
        input: &Value,
    ) -> Vec<LogicalLine> {
        call_lines(tool, summary, None, registry, external, &[], input)
    }

    fn texts(lines: &[LogicalLine]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    fn labels(lines: &[LogicalLine]) -> Vec<&str> {
        lines.iter().map(|l| l.style.label.as_str()).collect()
    }

    /// `resolveToolDisplay` as the TS tests call it: (label, detail, color, cmd).
    fn resolve(
        tool: &str,
        summary: &str,
        registry: &HashMap<String, ToolStyle>,
        external: Option<&[ExternalToolStyle]>,
    ) -> (String, String, Option<String>, Option<String>) {
        let refs: Option<Vec<&ExternalToolStyle>> = external.map(|e| e.iter().collect());
        let d = resolve_display(
            tool,
            summary,
            Registry {
                own: None,
                map: registry,
            },
            refs.as_deref(),
        );
        (
            d.label.into_owned(),
            d.detail.to_owned(),
            d.color.map(str::to_owned),
            d.cmd.map(str::to_owned),
        )
    }

    fn matched(
        summary: &str,
        external: &[ExternalToolStyle],
    ) -> Option<(Vec<String>, usize, usize)> {
        let refs: Vec<&ExternalToolStyle> = external.iter().collect();
        resolve_external_match(summary, &refs).map(|m| {
            (
                m.lines.iter().map(|l| l.to_string()).collect(),
                m.line,
                m.start,
            )
        })
    }

    fn display(
        tool: &str,
        input: Value,
        summary: &str,
        presentation: Option<Presentation>,
        registry: &HashMap<String, ToolStyle>,
        external: &[ExternalToolStyle],
    ) -> CallDisplay {
        call_display(
            tool,
            summary,
            &input,
            presentation.as_ref(),
            registry,
            external,
        )
    }

    // ----- bashsegments-port.test.ts ------------------------------------

    fn segs(line: &str, initial: Quote) -> (Vec<(&str, usize, &str)>, Quote) {
        let (segments, quote) = split_segments(line, initial);
        (
            segments
                .iter()
                .map(|s| (s.text, s.start, s.separator))
                .collect(),
            quote,
        )
    }

    #[test]
    fn splits_on_top_level_control_operators() {
        assert_eq!(
            segs(
                "echo hi && gmail search \"from:alice\" --limit 5; false || whatsapp messages Mom -n 5",
                None
            )
            .0,
            [
                ("echo hi ", 0, "&&"),
                (" gmail search \"from:alice\" --limit 5", 10, ";"),
                (" false ", 47, "||"),
                (" whatsapp messages Mom -n 5", 56, ""),
            ]
        );
    }

    #[test]
    fn splits_single_pipelines_at_top_level() {
        assert_eq!(
            segs(
                "cat /tmp/prompt.txt | exo llm -- --model openai/gpt-5.4 | sed -n '1,5p'",
                None
            )
            .0,
            [
                ("cat /tmp/prompt.txt ", 0, "|"),
                (" exo llm -- --model openai/gpt-5.4 ", 21, "|"),
                (" sed -n '1,5p'", 57, ""),
            ]
        );
    }

    #[test]
    fn keeps_separators_inside_command_substitutions() {
        assert_eq!(
            segs(
                "Q=$(command -v quercus || echo /opt/quercus); $Q courses",
                None
            )
            .0,
            [
                ("Q=$(command -v quercus || echo /opt/quercus)", 0, ";"),
                (" $Q courses", 45, ""),
            ]
        );
    }

    #[test]
    fn keeps_separators_inside_quotes() {
        assert_eq!(
            segs("echo \"a && b\" && printf 'x || y; z'", None).0,
            [
                ("echo \"a && b\" ", 0, "&&"),
                (" printf 'x || y; z'", 16, "")
            ]
        );
    }

    #[test]
    fn continues_from_an_open_quote() {
        assert_eq!(
            segs("--- DAD ---' && whatsapp messages Mom -n 8", Some(b'\'')),
            (
                vec![
                    ("--- DAD ---' ", 0, "&&"),
                    (" whatsapp messages Mom -n 8", 15, ""),
                ],
                None
            )
        );
    }

    // ----- toolstyles-port.test.ts ----------------------------------------

    #[test]
    fn a_call_whose_summary_is_its_label_shows_it_once() {
        let tools = reg(&[("ListAgents", "ListAgents", "#1d9bf0")]);
        assert_eq!(resolve("ListAgents", "ListAgents", &tools, None).1, "");
        let (label, detail, ..) = resolve("ExitPlanMode", "ExitPlanMode", &tools, None);
        assert_eq!((label.as_str(), detail.as_str()), ("ExitPlanMode", ""));
        assert_eq!(
            texts(&lines(
                "ExitPlanMode",
                "ExitPlanMode",
                &tools,
                &[],
                &Value::Null
            )),
            ["ExitPlanMode"]
        );
    }

    #[test]
    fn mcp_calls_are_labelled_by_their_server() {
        let (label, detail, ..) = resolve(
            "mcp__claude_ai_Claude_Docs__batch",
            "batch",
            &bash_reg(),
            None,
        );
        assert_eq!((label.as_str(), detail.as_str()), ("Claude Docs", "batch"));
    }

    #[test]
    fn exec_command_keeps_cli_labels_and_segments() {
        let (registry, external) = (bash_reg(), ext());
        let (label, ..) = resolve(
            "exec_command",
            "gmail search newer_than:1d",
            &registry,
            Some(&external),
        );
        assert_eq!(label, "Gmail");
        let command = "pwd; gmail search newer_than:1d";
        assert_eq!(
            lines(
                "exec_command",
                command,
                &registry,
                &external,
                &json!({ "cmd": command })
            ),
            lines(
                "bash",
                command,
                &registry,
                &external,
                &json!({ "command": command })
            )
        );
    }

    #[test]
    fn claude_code_bash_renders_like_bash() {
        let (registry, external) = (bash_reg(), ext());
        let command = "cd /tmp\ngmail search newer_than:1d";
        let input = json!({ "command": command, "description": "Search mail" });
        let got = lines("Bash", command, &registry, &external, &input);
        assert_eq!(
            got,
            lines(
                "bash",
                command,
                &registry,
                &external,
                &json!({ "command": command })
            )
        );
        assert_eq!(labels(&got), ["$", "Gmail"]);
    }

    #[test]
    fn matches_a_direct_external_invocation() {
        assert_eq!(
            resolve("bash", "exo status --json", &bash_reg(), Some(&ext())),
            (
                "Exocortex".into(),
                "status --json".into(),
                Some("#1d9bf0".into()),
                Some("exo".into())
            )
        );
    }

    #[test]
    fn keeps_each_manifests_label_and_colour() {
        assert_eq!(
            resolve(
                "bash",
                "gmail search newer_than:1d",
                &bash_reg(),
                Some(&ext())
            ),
            (
                "Gmail".into(),
                "search newer_than:1d".into(),
                Some("#4ddbb7".into()),
                Some("gmail".into())
            )
        );
    }

    #[test]
    fn matches_through_setup_lines() {
        let summary =
            "set -euo pipefail\ncd /home/yeyito/Workspace/Exocortex/daemon\nexo status --json";
        let (label, detail, _, cmd) = resolve("bash", summary, &bash_reg(), Some(&ext()));
        assert_eq!(
            (label.as_str(), detail.as_str()),
            ("Exocortex", "status --json")
        );
        assert_eq!(cmd.as_deref(), Some("exo"));
        assert_eq!(
            matched(summary, &ext()),
            Some((
                vec![
                    "set -euo pipefail".into(),
                    "cd /home/yeyito/Workspace/Exocortex/daemon".into(),
                    "exo status --json".into(),
                ],
                2,
                0
            ))
        );
    }

    #[test]
    fn matches_after_a_same_line_setup_command() {
        let summary = "cd /home/yeyito/Workspace/Exocortex/external-tools/exo-cli && exo --help --timeout 120000";
        let (label, detail, _, cmd) = resolve("bash", summary, &bash_reg(), Some(&ext()));
        assert_eq!(
            (label.as_str(), detail.as_str()),
            ("Exocortex", "--help --timeout 120000")
        );
        assert_eq!(cmd.as_deref(), Some("exo"));
        assert_eq!(
            matched(summary, &ext()),
            Some((vec![summary.into()], 0, summary.find("exo --help").unwrap()))
        );
    }

    #[test]
    fn matches_through_comments_and_exports() {
        let (label, detail, _, cmd) = resolve(
            "bash",
            "# check mailbox\nexport FOO=1\ngmail search --query from:alice",
            &bash_reg(),
            Some(&ext()),
        );
        assert_eq!(
            (label.as_str(), detail.as_str()),
            ("Gmail", "search --query from:alice")
        );
        assert_eq!(cmd.as_deref(), Some("gmail"));
    }

    #[test]
    fn matches_after_a_cat_heredoc_prelude() {
        let summary = [
            "cat > /tmp/kittenml-reply.txt <<'EOF'",
            "Hi!",
            "EOF",
            "",
            "gmail reply -f /tmp/kittenml-reply.txt 19d68e0c3d19ece3 --timeout 120000",
        ]
        .join("\n");
        let (label, detail, _, cmd) = resolve("bash", &summary, &bash_reg(), Some(&ext()));
        assert_eq!(
            (label.as_str(), detail.as_str()),
            (
                "Gmail",
                "reply -f /tmp/kittenml-reply.txt 19d68e0c3d19ece3 --timeout 120000"
            )
        );
        assert_eq!(cmd.as_deref(), Some("gmail"));
        let (_, line, start) = matched(&summary, &ext()).unwrap();
        assert_eq!((line, start), (4, 0));
    }

    #[test]
    fn matches_through_assignments_and_wrappers() {
        let summary = "env FOO=1 command time exo ls | sed -n '1,5p'";
        let (label, detail, _, cmd) = resolve("bash", summary, &bash_reg(), Some(&ext()));
        assert_eq!(
            (label.as_str(), detail.as_str()),
            ("Exocortex", "ls | sed -n '1,5p'")
        );
        assert_eq!(cmd.as_deref(), Some("exo"));
        let (_, line, start) = matched(summary, &ext()).unwrap();
        assert_eq!((line, start), (0, 23));
    }

    #[test]
    fn matches_when_the_first_line_ends_inside_a_quoted_argument() {
        let summary = [
            "whatsapp send Mom \"Hola ma, update rápido 🙏",
            "",
            "Ya nos sirvió mucho lo que mandaste por email:",
            "- ✅ la referencia bancaria de Banco General",
            "Gracias 💙\"",
        ]
        .join("\n");
        let (label, detail, _, cmd) = resolve("bash", &summary, &bash_reg(), Some(&ext()));
        assert_eq!(label, "WhatsApp");
        assert_eq!(
            detail,
            "send Mom \"Hola ma, update rápido 🙏\n\nYa nos sirvió mucho lo que mandaste por email:\n- ✅ la referencia bancaria de Banco General\nGracias 💙\""
        );
        assert_eq!(cmd.as_deref(), Some("whatsapp"));
        let (_, line, start) = matched(&summary, &ext()).unwrap();
        assert_eq!((line, start), (0, 0));
    }

    #[test]
    fn falls_back_to_bash_when_another_command_comes_first() {
        for summary in [
            "git fetch\nexo status --json",
            "git fetch && exo status --json",
            "command -v exo",
        ] {
            assert_eq!(
                resolve("bash", summary, &bash_reg(), Some(&ext())),
                ("$".into(), summary.into(), Some("#d19a66".into()), None),
                "{summary}"
            );
        }
    }

    #[test]
    fn a_bare_assignment_before_the_tool_is_setup() {
        let dollar = styles(&[("$G", "Gmail", "#4ddbb7")]);
        let (label, detail, ..) = resolve(
            "bash",
            "G=/opt/gmail-cli/bin/gmail; $G search x",
            &bash_reg(),
            Some(&dollar),
        );
        assert_eq!((label.as_str(), detail.as_str()), ("Gmail", "search x"));
        let (label, detail, ..) = resolve(
            "bash",
            "TZ=America/Toronto; gmail search x",
            &bash_reg(),
            Some(&ext()),
        );
        assert_eq!((label.as_str(), detail.as_str()), ("Gmail", "search x"));
    }

    #[test]
    fn timeout_and_compound_keywords_are_transparent() {
        let (registry, external) = (bash_reg(), ext());
        let (label, detail, ..) = resolve(
            "bash",
            "timeout 60 gmail search x",
            &registry,
            Some(&external),
        );
        assert_eq!((label.as_str(), detail.as_str()), ("Gmail", "search x"));
        let (label, detail, ..) = resolve(
            "bash",
            "timeout -k 5 --foreground 2m whatsapp chats",
            &registry,
            Some(&external),
        );
        assert_eq!((label.as_str(), detail.as_str()), ("WhatsApp", "chats"));
        let looped = "for id in a b; do exo status $id; done";
        let got = lines(
            "bash",
            looped,
            &registry,
            &external,
            &json!({ "command": looped }),
        );
        assert!(labels(&got).contains(&"Exocortex"));
    }

    #[test]
    fn gnu_time_by_path_is_transparent() {
        let (label, detail, ..) = resolve(
            "bash",
            "/usr/bin/time -f \"WALL %e s\" gmail search x",
            &bash_reg(),
            Some(&ext()),
        );
        assert_eq!((label.as_str(), detail.as_str()), ("Gmail", "search x"));
        let (label, detail, ..) =
            resolve("bash", "time -p whatsapp chats", &bash_reg(), Some(&ext()));
        assert_eq!((label.as_str(), detail.as_str()), ("WhatsApp", "chats"));
    }

    #[test]
    fn timeout_without_a_duration_is_not_a_wrapper() {
        assert_eq!(
            resolve("bash", "timeout gmail", &bash_reg(), Some(&ext())).0,
            "$"
        );
    }

    // ----- toolcall-display.test.ts ---------------------------------------

    fn exo_reg() -> HashMap<String, ToolStyle> {
        reg(&[("exo", "Exocortex", "#1d9bf0")])
    }

    #[test]
    fn bash_is_labelled_dollar_in_the_registry_colour() {
        let d = display(
            "bash",
            json!({ "command": "ls -la" }),
            "ls -la",
            None,
            &bash_reg(),
            &[],
        );
        assert_eq!(texts(&d.lines), ["$ ls -la"]);
        assert_eq!(d.label, "$");
        assert_eq!(d.color.as_deref(), Some("#d19a66"));
    }

    #[test]
    fn an_unknown_tool_takes_the_theme_colour() {
        let d = display("frobnicate", json!({}), "x", None, &reg(&[]), &[]);
        assert_eq!(d.color, None);
        assert_eq!(d.lines[0].text, "frobnicate x");
    }

    #[test]
    fn mcp_tools_are_labelled_with_their_server() {
        let d = display(
            "mcp__claude_ai_Claude_Docs__batch",
            json!({}),
            "batch",
            None,
            &reg(&[]),
            &[],
        );
        assert_eq!(d.label, "Claude Docs");
    }

    #[test]
    fn a_long_subagent_prompt_collapses_to_eight_rows() {
        let steps: Vec<String> = (1..=10).map(|i| format!("Step {i}.")).collect();
        let mut parts = vec![
            "Small TUI fix.".to_owned(),
            String::new(),
            "Repo: /tmp/x".into(),
        ];
        parts.extend(steps);
        parts.push("Report back.".into());
        let prompt = parts.join("\n");
        let d = display(
            "exo",
            json!({ "subagent": prompt, "args": { "model": "opus" } }),
            &format!("subagent: {}", &prompt[..40]),
            None,
            &exo_reg(),
            &[],
        );
        assert_eq!(
            d.lines[0].text,
            format!("Exocortex subagent (opus)\n{prompt}")
        );
        let c = collapse(&d, 60);
        assert_eq!(
            c.visible[0].text.split('\n').collect::<Vec<_>>(),
            [
                "Exocortex subagent (opus)",
                "Small TUI fix.",
                "",
                "Repo: /tmp/x",
                "Step 1.",
                "Step 2.",
                "Step 3.",
                "Step 4."
            ]
        );
        assert_eq!(c.hidden_rows, 15 - 8);
    }

    #[test]
    fn a_long_exo_message_shows_in_full_under_its_header() {
        let mut parts = vec![
            "Two more from the owner for the same build (Oct 21 ~13:50Z):".to_owned(),
            String::new(),
        ];
        parts.extend((1..=12).map(|i| format!("{i}. \"We probably don't need item {i}.\"")));
        let text = parts.join("\n");
        // What an older daemon recorded: flattened and cut at 100 characters.
        let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let d = display(
            "exo",
            json!({ "message": { "conversation": "1792550206710-ixtxlt", "text": text } }),
            &format!("message: 1792550206710-ixtxlt: {}", &flat[..100]),
            None,
            &exo_reg(),
            &[],
        );
        assert_eq!(
            d.lines[0].text,
            format!("Exocortex message: 1792550206710-ixtxlt\n{text}")
        );
        let c = collapse(&d, 60);
        assert_eq!(
            c.visible[0].text.split('\n').next(),
            Some("Exocortex message: 1792550206710-ixtxlt")
        );
        assert_eq!(c.hidden_rows, 15 - 8);
    }

    #[test]
    fn short_exo_calls_stay_on_one_line() {
        let one =
            |input: Value| texts(&display("exo", input, "", None, &exo_reg(), &[]).lines).join("|");
        assert_eq!(
            one(json!({ "message": { "conversation": "c1", "text": "Thanks, merged." } })),
            "Exocortex message: c1: Thanks, merged."
        );
        assert_eq!(
            one(json!({ "subagent": "Fix the typo." })),
            "Exocortex subagent: Fix the typo."
        );
        assert_eq!(one(json!({ "status": "c1" })), "Exocortex status: c1");
    }

    #[test]
    fn shell_commands_always_render_in_full() {
        let command: Vec<String> = (0..20).map(|i| format!("echo {i}")).collect();
        let command = command.join("\n");
        let d = display(
            "Bash",
            json!({ "command": command }),
            &command,
            None,
            &reg(&[]),
            &[],
        );
        assert_eq!(collapse(&d, 40).hidden_rows, 0);
    }

    #[test]
    fn the_calls_own_style_wins_over_the_registry() {
        let presentation = Presentation {
            tool_style: Some(ToolStyle {
                name: "minecraft_grep".into(),
                label: "Minecraft Grep".into(),
                color: "#12abef".into(),
            }),
            ..Presentation::default()
        };
        let d = display(
            "minecraft_grep",
            json!({ "query": "copper" }),
            "copper",
            Some(presentation),
            &reg(&[("minecraft_grep", "Old", "#000000")]),
            &[],
        );
        assert_eq!(d.lines[0].text, "Minecraft Grep copper");
        assert_eq!(d.color.as_deref(), Some("#12abef"));
    }

    #[test]
    fn snapshotted_bash_styles_come_before_global_ones() {
        let presentation = Presentation {
            bash_styles: styles(&[("./scripts/exo-deploy", "Deploy", "#7aa2f7")]),
            ..Presentation::default()
        };
        let d = display(
            "bash",
            json!({ "command": "./scripts/exo-deploy production" }),
            "./scripts/exo-deploy production",
            Some(presentation),
            &bash_reg(),
            &styles(&[("./scripts/exo-deploy", "Global", "#ffffff")]),
        );
        assert_eq!(texts(&d.lines), ["Deploy production"]);
    }

    #[test]
    fn malformed_persisted_styles_are_rejected() {
        let presentation = Presentation {
            bash_styles: styles(&[("./exo-bad", "Bad\nLabel", "not-a-color")]),
            ..Presentation::default()
        };
        let d = display(
            "bash",
            json!({ "command": "./exo-bad" }),
            "./exo-bad",
            Some(presentation),
            &bash_reg(),
            &[],
        );
        assert_eq!(texts(&d.lines), ["$ ./exo-bad"]);
    }

    #[test]
    fn multiline_stdin_stays_with_its_external_tool() {
        let stdin = "first line\n\nprintf 'data, not bash'\nlast line";
        let d = display(
            "bash",
            json!({ "command": "image generate", "stdin": stdin, "timeout": 30_000 }),
            &format!("image generate --stdin {stdin} --timeout 30000"),
            None,
            &bash_reg(),
            &styles(&[("image", "Image", "#ff79c6")]),
        );
        assert_eq!(
            texts(&d.lines),
            [
                "Image generate --stdin first line",
                "",
                "printf 'data, not bash'",
                "last line --timeout 30000"
            ]
        );
        assert!(d.lines.iter().all(|l| l.style.label == "Image"));
    }

    #[test]
    fn setup_and_wrappers_are_skipped() {
        let cmd = "cd /tmp && timeout 30 gmail search newer_than:1d | head";
        let d = display(
            "bash",
            json!({ "command": cmd }),
            cmd,
            None,
            &bash_reg(),
            &styles(&[("gmail", "Gmail", "#4ddbb7")]),
        );
        let got: Vec<(&str, &str)> = d
            .lines
            .iter()
            .map(|l| (l.style.label.as_str(), l.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("$", "$ cd /tmp && timeout 30"),
                ("Gmail", "Gmail search newer_than:1d | head")
            ]
        );
    }

    // ----- Beyond the TS tests: outputs of the TS port for the same input --

    fn bash_lines(summary: &str) -> Vec<(String, String, bool)> {
        let d = display(
            "bash",
            json!({ "command": summary }),
            summary,
            None,
            &bash_reg(),
            &ext(),
        );
        d.lines
            .into_iter()
            .map(|l| (l.style.label, l.text, l.has_label))
            .collect()
    }

    fn rows(list: &[(&str, &str, bool)]) -> Vec<(String, String, bool)> {
        list.iter()
            .map(|&(label, text, has)| (label.into(), text.into(), has))
            .collect()
    }

    #[test]
    fn a_prompted_transcript_drops_its_prompts() {
        assert_eq!(
            bash_lines("$ cd /tmp\n$ gmail search x && exo status"),
            rows(&[
                ("$", "$ cd /tmp", true),
                ("Gmail", "Gmail search x && exo status", true)
            ])
        );
    }

    #[test]
    fn a_heredoc_body_is_not_commands() {
        assert_eq!(
            bash_lines(
                "cat > /tmp/r.txt <<'EOF'\nHi && gmail nope\nEOF\ngmail reply -f /tmp/r.txt 19d6 && echo sent"
            ),
            rows(&[
                ("$", "$ cat > /tmp/r.txt <<'EOF'", true),
                ("$", "$ Hi && gmail nope", true),
                ("$", "$ EOF", true),
                ("Gmail", "Gmail reply -f /tmp/r.txt 19d6 && echo sent", true)
            ])
        );
        assert_eq!(
            bash_lines("echo start; gmail send <<EOF\nline one; exo not-a-command\nEOF\necho done"),
            rows(&[
                ("$", "$ echo start ;", true),
                ("Gmail", "Gmail send <<EOF", true),
                ("Gmail", "line one; exo not-a-command", false),
                ("Gmail", "EOF", false),
                ("$", "$ echo done", true)
            ])
        );
    }

    #[test]
    fn continued_lines_stay_with_their_tool() {
        assert_eq!(
            bash_lines("echo a && exo llm \\\n  --model x \\\n  --prompt hi\necho b"),
            rows(&[
                ("$", "$ echo a &&", true),
                ("Exocortex", "Exocortex llm \\", true),
                ("Exocortex", "  --model x \\", false),
                ("Exocortex", "  --prompt hi", false),
                ("$", "$ echo b", true)
            ])
        );
        assert_eq!(
            bash_lines("echo hi; whatsapp send Mom \"Hola\n\nGracias\" && echo ok"),
            rows(&[
                ("$", "$ echo hi ;", true),
                ("WhatsApp", "WhatsApp send Mom \"Hola", true),
                ("WhatsApp", "", false),
                ("WhatsApp", "Gracias\"  &&", false),
                ("$", "$ echo ok", true)
            ])
        );
    }

    #[test]
    fn the_parent_calls_options_join_the_line_before() {
        assert_eq!(
            bash_lines("cd /tmp && exo status\n--timeout 120000 --background"),
            rows(&[
                ("$", "$ cd /tmp &&", true),
                (
                    "Exocortex",
                    "Exocortex status --timeout 120000 --background",
                    true
                )
            ])
        );
        assert_eq!(
            bash_lines("gmail search a\ngmail search b\n"),
            rows(&[
                ("Gmail", "Gmail search a", true),
                ("Gmail", "Gmail search b", true),
                ("$", "", false)
            ])
        );
    }

    #[test]
    fn untrusted_text_is_sanitized() {
        assert_eq!(
            sanitize("a\r\nb\rc\td\x1b[0m\x07\u{85}é"),
            "a\nb\nc    d␛[0m\u{fffd}\u{fffd}é"
        );
        assert!(matches!(sanitize("plain\ntext"), Cow::Borrowed(_)));
    }

    #[test]
    fn local_bash_styles_are_capped_and_checked() {
        let mut list = styles(&[
            ("ok", "Ok", "#a1B2c3"),
            ("", "Empty", "#000000"),
            ("x", "", "#000000"),
        ]);
        list.push(ExternalToolStyle {
            cmd: "x".repeat(1_025),
            label: "Long".into(),
            color: "#000000".into(),
        });
        list.extend(styles(&[
            ("y", "Y", "#00000g"),
            ("z\u{9f}", "Z", "#000000"),
        ]));
        list.extend((0..20).map(|i| ExternalToolStyle {
            cmd: format!("c{i}"),
            label: "C".into(),
            color: "#123456".into(),
        }));
        let kept: Vec<&str> = local_bash_styles(&list).map(|s| s.cmd.as_str()).collect();
        // The first 16 are looked at: the one good one and c0..c9.
        let mut want = vec!["ok".to_owned()];
        want.extend((0..10).map(|i| format!("c{i}")));
        assert_eq!(kept, want);
    }

    #[test]
    fn exo_text_over_100_characters_goes_below_its_header() {
        let long = "x".repeat(101);
        assert_eq!(
            exo_call_detail(&json!({ "restart": long })),
            format!("restart\n{long}")
        );
        assert_eq!(
            exo_call_detail(&json!({ "restart": "x".repeat(100) })),
            format!("restart: {}", "x".repeat(100))
        );
        assert_eq!(
            exo_call_detail(
                &json!({ "subagent": "go", "args": { "model": "opus", "effort": "xhigh" } })
            ),
            "subagent (opus, xhigh): go"
        );
        assert_eq!(exo_call_detail(&json!({ "abort": "c2" })), "abort: c2");
        // presentation.detail keeps the summary.
        let presentation = Presentation {
            detail: Some("shown".into()),
            ..Presentation::default()
        };
        assert_eq!(
            call_summary(
                "exo",
                "shown",
                &json!({ "status": "c1" }),
                Some(&presentation)
            ),
            "shown"
        );
    }

    #[test]
    fn a_short_call_never_collapses() {
        let d = display("read", json!({}), "a\nb", None, &reg(&[]), &[]);
        let c = collapse(&d, 40);
        assert!(matches!(c.visible, Cow::Borrowed(_)));
        assert_eq!(c.hidden_rows, 0);
    }

    #[test]
    fn heredoc_delimiters_follow_the_regex() {
        let specs = |line| {
            heredoc_specs(line)
                .into_iter()
                .map(|h| (h.terminator, h.allow_tabs, h.start))
                .collect::<Vec<_>>()
        };
        assert_eq!(specs("cat <<'EOF' x"), [("EOF", false, 4)]);
        assert_eq!(
            specs("cat <<-\"END\" <<\\Q"),
            [("END", true, 4), ("Q", false, 13)]
        );
        // `<<-` with nothing after it reads `-` as the delimiter, as the
        // regex backtracks to.
        assert_eq!(specs("cat <<-"), [("-", false, 4)]);
        assert_eq!(specs("cat <<''"), []);
        assert_eq!(specs("cat << 'open"), []);
    }
}
