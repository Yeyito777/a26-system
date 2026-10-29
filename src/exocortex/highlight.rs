//! Syntax highlighting for code blocks (WP-C1, PAR-137): the TUI's
//! highlighter (exocortex `tui/src/markdown/highlight.ts`) ported as is —
//! its rule sets, in its order, matched by a small backtracking engine with
//! JavaScript's regex semantics for the features the rules use (classes,
//! `\b`, `\d\w\s`, groups, alternation, greedy and lazy repeats,
//! lookahead, `^`/`$` with the `m` flag, `i`). Line by line: at each
//! position the first rule that matches there wins; other characters are
//! plain. Colours are the theme's `Syntax` (the TUI's palette).

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A token's class (the TUI's `TokenType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Operator,
    Punct,
    Default,
}

/// Lines longer than this are not highlighted (a minified file).
pub const MAX_LINE: usize = 500;

/// The spans of `line` (character ranges, in order, covering it) and
/// their classes; None when the language is unknown.
pub fn highlight_line(line: &str, language: &str) -> Option<Vec<(usize, usize, Token)>> {
    let rules = rules_for(language)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.len() > MAX_LINE {
        return Some(vec![(0, chars.len(), Token::Default)]);
    }
    let mut out: Vec<(usize, usize, Token)> = Vec::new();
    let mut push = |start: usize, end: usize, token: Token| match out.last_mut() {
        Some(last) if last.2 == token && last.1 == start => last.1 = end,
        _ => out.push((start, end, token)),
    };
    let mut pos = 0;
    while pos < chars.len() {
        let found = rules.iter().find_map(|(re, token)| {
            re.match_at(&chars, pos)
                .filter(|end| *end > pos)
                .map(|end| (end, *token))
        });
        match found {
            Some((end, token)) => {
                push(pos, end, token);
                pos = end;
            }
            None => {
                push(pos, pos + 1, Token::Default);
                pos += 1;
            }
        }
    }
    Some(out)
}

/// A line's spans (`highlight_line`), remembered: a streamed code block
/// is laid out again on every chunk, and only its last line is new.
pub fn highlight_cached(line: &str, language: &str) -> Option<Rc<Vec<(usize, usize, Token)>>> {
    let set = set_of(language)?;
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(spans) = cache.get(&(set, line.to_owned())) {
            return Some(spans.clone());
        }
        let spans = Rc::new(highlight_line(line, language)?);
        if cache.len() >= CACHED_LINES {
            cache.clear();
        }
        cache.insert((set, line.to_owned()), spans.clone());
        Some(spans)
    })
}

/// Lines' spans remembered at most.
const CACHED_LINES: usize = 20_000;

type Spans = Rc<Vec<(usize, usize, Token)>>;

thread_local! {
    static CACHE: RefCell<HashMap<(usize, String), Spans>> = RefCell::new(HashMap::new());
}

/// Whether `language` (a fence's info word) is highlighted.
pub fn supported(language: &str) -> bool {
    set_of(language).is_some()
}

// ----- The rules (highlight.ts, in its order) ----------------------------------

type Rules = Vec<(Re, Token)>;

fn rules(list: &[(&str, &str, Token)]) -> Rules {
    list.iter()
        .map(|(pattern, flags, token)| (Re::new(pattern, flags), *token))
        .collect()
}

use Token::{
    Comment as C, Function as F, Keyword as K, Number as N, Operator as O, Punct as P, String as S,
    Type as T,
};

fn typescript() -> Rules {
    rules(&[
        (r"\/\/.*$", "y", C),
        (r"\/\*[\s\S]*?\*\/", "y", C),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'(?:[^'\\]|\\.)*'", "y", S),
        (r"`(?:[^`\\]|\\.)*`", "y", S),
        (r"\b0[xX][0-9a-fA-F]+\b", "y", N),
        (r"\b0[oO][0-7]+\b", "y", N),
        (r"\b0[bB][01]+\b", "y", N),
        (r"\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?n?\b", "y", N),
        (
            r"\b(?:string|number|boolean|void|any|never|unknown|null|undefined|object|symbol|bigint|Array|Map|Set|Record|Promise|Partial|Required|Readonly|Pick|Omit)\b",
            "y",
            T,
        ),
        (
            r"\b(?:function|const|let|var|if|else|return|while|for|do|switch|case|break|continue|throw|try|catch|finally|new|delete|typeof|instanceof|in|of|class|extends|implements|interface|type|enum|namespace|module|import|export|from|default|as|async|await|yield|static|get|set|public|private|protected|readonly|abstract|declare|override|satisfies|this|super|true|false|null|undefined|void)\b",
            "y",
            K,
        ),
        (r"\b[a-zA-Z_$][\w$]*(?=\s*\()", "y", F),
        (
            r"=>|\.{3}|[!=]==?|[<>]=?|&&|\|\||[+\-*/%]=?|\?\?|[?!]\.?|[~^&|]=?|<<=?|>>>?=?",
            "y",
            O,
        ),
        (r"[{}()[\];,.:@#]", "y", P),
    ])
}

fn python() -> Rules {
    rules(&[
        (r"#.*$", "y", C),
        (r#""""[\s\S]*?""""#, "y", S),
        (r"'''[\s\S]*?'''", "y", S),
        (r#"[fFrRbBuU]?"(?:[^"\\]|\\.)*""#, "y", S),
        (r"[fFrRbBuU]?'(?:[^'\\]|\\.)*'", "y", S),
        (r"\b0[xX][0-9a-fA-F_]+\b", "y", N),
        (r"\b0[oO][0-7_]+\b", "y", N),
        (r"\b0[bB][01_]+\b", "y", N),
        (r"\b\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?\d+)?j?\b", "y", N),
        (
            r"\b(?:int|float|str|bool|list|dict|tuple|set|frozenset|bytes|bytearray|memoryview|complex|NoneType|type|object)\b",
            "y",
            T,
        ),
        (
            r"\b(?:def|class|if|elif|else|return|while|for|in|not|and|or|is|with|as|try|except|finally|raise|import|from|yield|lambda|pass|break|continue|del|global|nonlocal|assert|async|await|True|False|None|self|cls)\b",
            "y",
            K,
        ),
        (r"@\w+", "y", F),
        (r"\b[a-zA-Z_]\w*(?=\s*\()", "y", F),
        (
            r"\*\*|\/\/|[+\-*/%@]=?|[<>]=?|[!=]=|<<=?|>>=?|[&|^~]",
            "y",
            O,
        ),
        (r"[{}()[\];,.:->]", "y", P),
    ])
}

fn rust() -> Rules {
    rules(&[
        (r"\/\/.*$", "y", C),
        (r"\/\*[\s\S]*?\*\/", "y", C),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r#"b"(?:[^"\\]|\\.)*""#, "y", S),
        (r"'[^'\\]'|'\\.'", "y", S),
        (
            r"\b0[xX][0-9a-fA-F_]+(?:_?[iu](?:8|16|32|64|128|size))?\b",
            "y",
            N,
        ),
        (
            r"\b0[oO][0-7_]+(?:_?[iu](?:8|16|32|64|128|size))?\b",
            "y",
            N,
        ),
        (r"\b0[bB][01_]+(?:_?[iu](?:8|16|32|64|128|size))?\b", "y", N),
        (
            r"\b\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?\d+)?(?:_?(?:f32|f64|[iu](?:8|16|32|64|128|size)))?\b",
            "y",
            N,
        ),
        (
            r"\b(?:i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize|f32|f64|bool|char|str|String|Vec|Box|Rc|Arc|Option|Result|HashMap|HashSet|BTreeMap|BTreeSet|Cow|Pin|Cell|RefCell|Mutex|RwLock)\b",
            "y",
            T,
        ),
        (r"'[a-zA-Z_]\w*", "y", T),
        (
            r"\b(?:fn|let|mut|const|static|if|else|match|loop|while|for|in|break|continue|return|struct|enum|impl|trait|type|use|mod|pub|crate|super|self|Self|where|as|ref|move|async|await|dyn|unsafe|extern|true|false|macro_rules)\b",
            "y",
            K,
        ),
        (r"\b[a-zA-Z_]\w*!", "y", F),
        (r"\b[a-zA-Z_]\w*(?=\s*[(<])", "y", F),
        (
            r"=>|->|\.{2,3}|&&|\|\||[!=]=|[<>]=?|[+\-*/%&|^!]=?|<<=?|>>=?",
            "y",
            O,
        ),
        (r"[{}()[\];,.:@#]", "y", P),
    ])
}

fn go() -> Rules {
    rules(&[
        (r"\/\/.*$", "y", C),
        (r"\/\*[\s\S]*?\*\/", "y", C),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"`[^`]*`", "y", S),
        (r"'[^'\\]'|'\\.'", "y", S),
        (r"\b0[xX][0-9a-fA-F_]+\b", "y", N),
        (r"\b0[oO][0-7_]+\b", "y", N),
        (r"\b0[bB][01_]+\b", "y", N),
        (r"\b\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?\d+)?i?\b", "y", N),
        (
            r"\b(?:int|int8|int16|int32|int64|uint|uint8|uint16|uint32|uint64|uintptr|float32|float64|complex64|complex128|bool|byte|rune|string|error|any)\b",
            "y",
            T,
        ),
        (
            r"\b(?:func|var|const|type|struct|interface|map|chan|if|else|switch|case|default|for|range|break|continue|return|go|defer|select|package|import|fallthrough|goto|nil|true|false|iota|make|new|append|len|cap|copy|delete|close|panic|recover|print|println)\b",
            "y",
            K,
        ),
        (r"\b[a-zA-Z_]\w*(?=\s*\()", "y", F),
        (
            r":=|<-|&&|\|\||[!=]=|[<>]=?|[+\-*/%&|^]=?|&\^=?|<<=?|>>=?|\.{3}",
            "y",
            O,
        ),
        (r"[{}()[\];,.:@]", "y", P),
    ])
}

fn bash() -> Rules {
    rules(&[
        (r"#.*$", "y", C),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'[^']*'", "y", S),
        (r"\$'(?:[^'\\]|\\.)*'", "y", S),
        (r"\$\{[^}]+\}", "y", T),
        (r"\$[a-zA-Z_]\w*", "y", T),
        (r"\$[0-9@#?$!*-]", "y", T),
        (r"\b\d+(?:\.\d+)?\b", "y", N),
        (
            r"\b(?:if|then|else|elif|fi|case|esac|for|while|until|do|done|in|function|return|exit|local|export|readonly|declare|typeset|unset|source|eval|exec|set|shift|trap|wait|true|false|select)\b",
            "y",
            K,
        ),
        (
            r"\b(?:echo|printf|cd|ls|cat|grep|sed|awk|find|xargs|sort|uniq|wc|head|tail|cut|tr|tee|mkdir|rmdir|rm|cp|mv|ln|chmod|chown|chgrp|touch|test|read|curl|wget|git|docker|make|sudo|pip|npm|bun|yarn|cargo|go|python|node)\b",
            "y",
            F,
        ),
        (r"\|\||&&|;;|[<>]=?|[!=]=?|[|&;]", "y", O),
        (r"[{}()[\],]", "y", P),
    ])
}

fn json() -> Rules {
    rules(&[
        (r#""(?:[^"\\]|\\.)*"\s*(?=:)"#, "y", T),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b", "y", N),
        (r"\b(?:true|false|null)\b", "y", K),
        (r"[{}[\]:,]", "y", P),
    ])
}

fn yaml() -> Rules {
    rules(&[
        (r"#.*$", "y", C),
        (r"^[\w.-]+(?=\s*:)", "y", T),
        (r"^\s+[\w.-]+(?=\s*:)", "y", T),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'(?:[^'\\]|\\.)*'", "y", S),
        (r"\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b", "y", N),
        (r"\b(?:true|false|null|yes|no|on|off)\b", "y", K),
        (r"^---$|^\.\.\.$", "my", O),
        (r"[&*]\w+", "y", F),
        (r"!\S+", "y", T),
        (r"[[\]{},:|>-]", "y", P),
    ])
}

fn html() -> Rules {
    rules(&[
        (r"<!--[\s\S]*?-->", "y", C),
        (r"<!DOCTYPE\s[^>]+>", "iy", K),
        (r"<\/?[a-zA-Z][\w-]*", "y", K),
        (r"\/?>", "y", K),
        (r"\b[a-zA-Z][\w-]*(?==)", "y", T),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'(?:[^'\\]|\\.)*'", "y", S),
        (r"&\w+;|&#\d+;|&#x[0-9a-fA-F]+;", "y", N),
        (r"[=]", "y", P),
    ])
}

fn css() -> Rules {
    rules(&[
        (r"\/\*[\s\S]*?\*\/", "y", C),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'(?:[^'\\]|\\.)*'", "y", S),
        (
            r"-?\b\d+(?:\.\d+)?(?:px|em|rem|%|vh|vw|vmin|vmax|ch|ex|cm|mm|in|pt|pc|s|ms|deg|rad|grad|turn|fr)?\b",
            "y",
            N,
        ),
        (r"#[0-9a-fA-F]{3,8}\b", "y", N),
        (r"@[a-zA-Z][\w-]*", "y", K),
        (r"[a-zA-Z-]+(?=\s*:)", "y", T),
        (r"::?[a-zA-Z][\w-]*", "y", F),
        (r"[.#][a-zA-Z][\w-]*", "y", F),
        (
            r"\b(?:important|inherit|initial|unset|none|auto|normal|bold|italic)\b",
            "y",
            K,
        ),
        (r"!important", "y", K),
        (r"\b[a-zA-Z][\w-]*(?=\()", "y", F),
        (r"[{}();:,>+~*[\]=]", "y", P),
    ])
}

fn sql() -> Rules {
    rules(&[
        (r"--.*$", "y", C),
        (r"\/\*[\s\S]*?\*\/", "y", C),
        (r"'(?:[^'\\]|\\.)*'", "y", S),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"\b\d+(?:\.\d+)?\b", "y", N),
        (
            r"\b(?:SELECT|FROM|WHERE|AND|OR|NOT|IN|IS|NULL|AS|ON|JOIN|LEFT|RIGHT|INNER|OUTER|FULL|CROSS|UNION|ALL|INSERT|INTO|VALUES|UPDATE|SET|DELETE|CREATE|TABLE|INDEX|VIEW|DROP|ALTER|ADD|COLUMN|PRIMARY|KEY|FOREIGN|REFERENCES|UNIQUE|CHECK|DEFAULT|CONSTRAINT|ORDER|BY|GROUP|HAVING|LIMIT|OFFSET|ASC|DESC|DISTINCT|BETWEEN|LIKE|EXISTS|CASE|WHEN|THEN|ELSE|END|CAST|COALESCE|NULLIF|COUNT|SUM|AVG|MIN|MAX|BEGIN|COMMIT|ROLLBACK|TRANSACTION|GRANT|REVOKE|WITH|RECURSIVE|RETURNING|CONFLICT|DO|NOTHING|EXCEPT|INTERSECT|FETCH|NEXT|ROWS|ONLY|IF|REPLACE|TEMPORARY|TEMP|CASCADE|RESTRICT|TRUNCATE|EXPLAIN|ANALYZE|VACUUM|REINDEX|TRIGGER|PROCEDURE|FUNCTION|SCHEMA|DATABASE|USE|SHOW|DESCRIBE)\b",
            "iy",
            K,
        ),
        (
            r"\b(?:INT|INTEGER|BIGINT|SMALLINT|TINYINT|FLOAT|DOUBLE|DECIMAL|NUMERIC|REAL|BOOLEAN|BOOL|CHAR|VARCHAR|TEXT|BLOB|DATE|TIME|DATETIME|TIMESTAMP|SERIAL|UUID|JSON|JSONB|ARRAY|BYTEA|MONEY|INTERVAL|POINT|LINE|CIRCLE|POLYGON)\b",
            "iy",
            T,
        ),
        (r"\b[a-zA-Z_]\w*(?=\s*\()", "y", F),
        (r"[<>!=]+|::|&&|\|\|", "y", O),
        (r"[{}()[\];,.*]", "y", P),
    ])
}

fn c() -> Rules {
    rules(&[
        (r"\/\/.*$", "y", C),
        (r"\/\*[\s\S]*?\*\/", "y", C),
        (r"^\s*#\s*\w+.*$", "my", F),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'(?:[^'\\]|\\.)*'", "y", S),
        (r"\b0[xX][0-9a-fA-F]+[uUlL]*\b", "y", N),
        (r"\b0[bB][01]+[uUlL]*\b", "y", N),
        (r"\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?[fFlLuU]*\b", "y", N),
        (
            r"\b(?:int|long|short|char|float|double|void|unsigned|signed|bool|size_t|ssize_t|ptrdiff_t|intptr_t|uintptr_t|int8_t|int16_t|int32_t|int64_t|uint8_t|uint16_t|uint32_t|uint64_t|FILE|wchar_t|auto|string|vector|map|set|unique_ptr|shared_ptr|optional|variant|tuple|array|pair|nullptr_t)\b",
            "y",
            T,
        ),
        (
            r"\b(?:if|else|while|for|do|switch|case|default|break|continue|return|goto|typedef|struct|union|enum|class|template|typename|namespace|using|public|private|protected|virtual|override|final|static|extern|inline|const|constexpr|consteval|volatile|register|mutable|explicit|friend|operator|new|delete|throw|try|catch|noexcept|sizeof|alignof|decltype|static_cast|dynamic_cast|reinterpret_cast|const_cast|static_assert|concept|requires|co_await|co_yield|co_return|true|false|nullptr|NULL|this)\b",
            "y",
            K,
        ),
        (r"\b[a-zA-Z_]\w*(?=\s*[(<])", "y", F),
        (
            r"->|::|\.\.\.|&&|\|\||[!=<>]=?|[+\-*/%&|^~]=?|<<=?|>>=?|\+\+|--",
            "y",
            O,
        ),
        (r"[{}()[\];,.:?#]", "y", P),
    ])
}

fn toml() -> Rules {
    rules(&[
        (r"#.*$", "y", C),
        (r#""""[\s\S]*?""""#, "y", S),
        (r"'''[\s\S]*?'''", "y", S),
        (r#""(?:[^"\\]|\\.)*""#, "y", S),
        (r"'[^']*'", "y", S),
        (r"^\s*\[\[?[\w.-]+\]?\]", "my", K),
        (r"^[\w.-]+(?=\s*=)", "my", T),
        (
            r"\b\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?)?\b",
            "y",
            N,
        ),
        (r"[+-]?\b\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?\d+)?\b", "y", N),
        (r"\b0[xX][0-9a-fA-F_]+\b", "y", N),
        (r"\b0[oO][0-7_]+\b", "y", N),
        (r"\b0[bB][01_]+\b", "y", N),
        (r"\b(?:true|false|inf|nan)\b", "y", K),
        (r"[[\]{}=,.]", "y", P),
    ])
}

fn diff() -> Rules {
    rules(&[
        (r"^(?:---|\+\+\+) .*$", "my", K),
        (r"^diff --.*$", "my", K),
        (r"^index [0-9a-f]+\.\.[0-9a-f]+.*$", "my", C),
        (r"^@@.*@@.*$", "my", T),
        (r"^\+.*$", "my", S),
        (r"^-.*$", "my", N),
    ])
}

fn markdown() -> Rules {
    rules(&[
        (r"^#{1,6}\s+.*$", "my", K),
        (r"`[^`]+`", "y", S),
        (r"\*\*[^*]+\*\*", "y", K),
        (r"\*[^*]+\*", "y", T),
        (r"\[([^\]]+)\]\([^)]+\)", "y", F),
        (r"https?:\/\/\S+", "y", F),
        (r"^\s*[-*+]\s", "my", O),
        (r"^\s*\d+\.\s", "my", O),
        (r"^\s*>+", "my", C),
        (r"^[-*_]{3,}\s*$", "my", O),
    ])
}

/// The rule set a language name (or alias) uses.
fn set_of(language: &str) -> Option<usize> {
    let set = match language.to_ascii_lowercase().as_str() {
        "typescript" | "ts" | "javascript" | "js" | "jsx" | "tsx" | "mjs" | "cjs" => 0,
        "python" | "py" => 1,
        "rust" | "rs" => 2,
        "go" | "golang" => 3,
        "bash" | "sh" | "shell" | "zsh" | "fish" | "console" => 4,
        "json" | "jsonc" | "json5" => 5,
        "yaml" | "yml" => 6,
        "html" | "htm" | "xml" | "svg" | "vue" | "svelte" => 7,
        "css" | "scss" | "sass" | "less" => 8,
        "sql" | "psql" | "mysql" | "sqlite" | "pgsql" | "plpgsql" => 9,
        "c" | "cpp" | "c++" | "cc" | "cxx" | "h" | "hpp" | "h++" | "hh" | "hxx" | "java"
        | "kotlin" | "kt" | "csharp" | "cs" | "swift" => 10,
        "toml" => 11,
        "diff" | "patch" => 12,
        "markdown" | "md" => 13,
        _ => return None,
    };
    Some(set)
}

thread_local! {
    static SETS: RefCell<HashMap<usize, &'static Rules>> = RefCell::new(HashMap::new());
}

fn rules_for(language: &str) -> Option<&'static Rules> {
    let set = set_of(language)?;
    Some(SETS.with(|sets| {
        *sets.borrow_mut().entry(set).or_insert_with(|| {
            let rules = match set {
                0 => typescript(),
                1 => python(),
                2 => rust(),
                3 => go(),
                4 => bash(),
                5 => json(),
                6 => yaml(),
                7 => html(),
                8 => css(),
                9 => sql(),
                10 => c(),
                11 => toml(),
                12 => diff(),
                _ => markdown(),
            };
            Box::leak(Box::new(rules))
        })
    }))
}

// ----- A small backtracking regex engine ---------------------------------------

#[derive(Debug, Clone)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    Boundary(bool),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: usize,
        max: Option<usize>,
        greedy: bool,
    },
    Look {
        node: Box<Node>,
        negate: bool,
    },
}

/// A compiled pattern, matched sticky (at one position).
#[derive(Debug)]
pub struct Re {
    node: Node,
    ignore_case: bool,
    multiline: bool,
}

impl Re {
    pub fn new(pattern: &str, flags: &str) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let mut parser = Parser { chars, at: 0 };
        let node = parser.alternation();
        assert!(parser.at == parser.chars.len(), "bad pattern {pattern}");
        Self {
            node,
            ignore_case: flags.contains('i'),
            multiline: flags.contains('m'),
        }
    }

    /// Where a match starting exactly at `pos` ends, if one does.
    pub fn match_at(&self, text: &[char], pos: usize) -> Option<usize> {
        let mut end = None;
        let cx = Cx { re: self, text };
        cx.node(&self.node, pos, &mut |at| {
            end = Some(at);
            true
        });
        end
    }
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn alternation(&mut self) -> Node {
        let mut branches = vec![self.concat()];
        while self.peek() == Some('|') {
            self.at += 1;
            branches.push(self.concat());
        }
        if branches.len() == 1 {
            branches.pop().unwrap()
        } else {
            Node::Alt(branches)
        }
    }

    fn concat(&mut self) -> Node {
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom();
            nodes.push(self.quantified(atom));
        }
        Node::Concat(nodes)
    }

    fn quantified(&mut self, atom: Node) -> Node {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let rest: String = self.chars[self.at..].iter().collect();
                let Some(close) = rest.find('}') else {
                    return atom;
                };
                let inner = &rest[1..close];
                let (min, max) = match inner.split_once(',') {
                    Some((a, "")) => (a.parse().unwrap_or(0), None),
                    Some((a, b)) => (a.parse().unwrap_or(0), b.parse().ok()),
                    None => {
                        let n = inner.parse().unwrap_or(0);
                        (n, Some(n))
                    }
                };
                self.at += close;
                (min, max)
            }
            _ => return atom,
        };
        self.at += 1;
        let greedy = if self.peek() == Some('?') {
            self.at += 1;
            false
        } else {
            true
        };
        Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        }
    }

    fn atom(&mut self) -> Node {
        let c = self.chars[self.at];
        self.at += 1;
        match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let mut look = None;
                if self.peek() == Some('?') {
                    match self.chars.get(self.at + 1) {
                        Some(':') => self.at += 2,
                        Some('=') => {
                            self.at += 2;
                            look = Some(false);
                        }
                        Some('!') => {
                            self.at += 2;
                            look = Some(true);
                        }
                        _ => {}
                    }
                }
                let inner = self.alternation();
                self.at += 1; // ')'
                match look {
                    Some(negate) => Node::Look {
                        node: Box::new(inner),
                        negate,
                    },
                    None => inner,
                }
            }
            '[' => self.class(),
            '\\' => {
                let e = self.chars[self.at];
                self.at += 1;
                match e {
                    'b' => Node::Boundary(true),
                    'B' => Node::Boundary(false),
                    'd' => Node::Class(vec![ClassItem::Digit(true)], false),
                    'D' => Node::Class(vec![ClassItem::Digit(false)], false),
                    'w' => Node::Class(vec![ClassItem::Word(true)], false),
                    'W' => Node::Class(vec![ClassItem::Word(false)], false),
                    's' => Node::Class(vec![ClassItem::Space(true)], false),
                    'S' => Node::Class(vec![ClassItem::Space(false)], false),
                    other => Node::Char(escaped(other)),
                }
            }
            other => Node::Char(other),
        }
    }

    fn class(&mut self) -> Node {
        let negated = self.peek() == Some('^');
        if negated {
            self.at += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek() else { break };
            if c == ']' && !first {
                self.at += 1;
                break;
            }
            first = false;
            self.at += 1;
            let item = if c == '\\' {
                let e = self.chars[self.at];
                self.at += 1;
                match e {
                    'd' => ClassItem::Digit(true),
                    'D' => ClassItem::Digit(false),
                    'w' => ClassItem::Word(true),
                    'W' => ClassItem::Word(false),
                    's' => ClassItem::Space(true),
                    'S' => ClassItem::Space(false),
                    other => ClassItem::Char(escaped(other)),
                }
            } else {
                ClassItem::Char(c)
            };
            // A range: `a-z` (a dash before `]` is itself).
            if let ClassItem::Char(from) = item
                && self.peek() == Some('-')
                && self.chars.get(self.at + 1).is_some_and(|n| *n != ']')
            {
                self.at += 1;
                let mut to = self.chars[self.at];
                self.at += 1;
                if to == '\\' {
                    to = escaped(self.chars[self.at]);
                    self.at += 1;
                }
                items.push(ClassItem::Range(from, to));
                continue;
            }
            items.push(item);
        }
        Node::Class(items, negated)
    }
}

fn escaped(c: char) -> char {
    match c {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        other => other,
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JavaScript's `\s`.
fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

struct Cx<'a> {
    re: &'a Re,
    text: &'a [char],
}

impl Cx<'_> {
    fn same(&self, a: char, b: char) -> bool {
        a == b || (self.re.ignore_case && a.eq_ignore_ascii_case(&b))
    }

    fn class_has(&self, items: &[ClassItem], c: char) -> bool {
        items.iter().any(|item| match *item {
            ClassItem::Char(x) => self.same(x, c),
            ClassItem::Range(a, b) => {
                (a..=b).contains(&c)
                    || (self.re.ignore_case
                        && ((a..=b).contains(&c.to_ascii_lowercase())
                            || (a..=b).contains(&c.to_ascii_uppercase())))
            }
            ClassItem::Digit(yes) => c.is_ascii_digit() == yes,
            ClassItem::Word(yes) => is_word(c) == yes,
            ClassItem::Space(yes) => is_space(c) == yes,
        })
    }

    /// Whether a one-character node matches `c`.
    fn single(&self, node: &Node, c: char) -> Option<bool> {
        match node {
            Node::Char(x) => Some(self.same(*x, c)),
            Node::Any => Some(!is_line_end(c)),
            Node::Class(items, negated) => Some(self.class_has(items, c) != *negated),
            _ => None,
        }
    }

    fn node(&self, node: &Node, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        let text = self.text;
        match node {
            Node::Char(_) | Node::Any | Node::Class(..) => match text.get(pos) {
                Some(&c) if self.single(node, c) == Some(true) => k(pos + 1),
                _ => false,
            },
            Node::Start => {
                let at_start = pos == 0
                    || (self.re.multiline && text.get(pos - 1).is_some_and(|c| is_line_end(*c)));
                at_start && k(pos)
            }
            Node::End => {
                let at_end = pos == text.len()
                    || (self.re.multiline && text.get(pos).is_some_and(|c| is_line_end(*c)));
                at_end && k(pos)
            }
            Node::Boundary(yes) => {
                let before = pos > 0 && is_word(text[pos - 1]);
                let after = text.get(pos).is_some_and(|c| is_word(*c));
                ((before != after) == *yes) && k(pos)
            }
            Node::Concat(nodes) => self.seq(nodes, pos, k),
            Node::Alt(branches) => branches.iter().any(|branch| self.node(branch, pos, k)),
            Node::Look { node, negate } => {
                let found = self.node(node, pos, &mut |_| true);
                (found != *negate) && k(pos)
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => {
                if let Some(first) = text.get(pos)
                    && self.single(node, *first).is_some()
                {
                    // One character at a time: count, then try each length.
                    let limit = max.unwrap_or(usize::MAX);
                    let mut n = 0;
                    while n < limit
                        && text
                            .get(pos + n)
                            .is_some_and(|c| self.single(node, *c) == Some(true))
                    {
                        n += 1;
                    }
                    if n < *min {
                        return false;
                    }
                    if *greedy {
                        (*min..=n).rev().any(|count| k(pos + count))
                    } else {
                        (*min..=n).any(|count| k(pos + count))
                    }
                } else if self.single(node, ' ').is_some() {
                    // At the end of the text: only zero repeats.
                    *min == 0 && k(pos)
                } else {
                    self.repeat(node, *min, *max, *greedy, 0, pos, k)
                }
            }
        }
    }

    fn seq(&self, nodes: &[Node], pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        match nodes.split_first() {
            None => k(pos),
            Some((first, rest)) => self.node(first, pos, &mut |at| self.seq(rest, at, k)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(
        &self,
        node: &Node,
        min: usize,
        max: Option<usize>,
        greedy: bool,
        count: usize,
        pos: usize,
        k: &mut dyn FnMut(usize) -> bool,
    ) -> bool {
        let more = max.is_none_or(|max| count < max);
        let again = |k: &mut dyn FnMut(usize) -> bool| {
            more && self.node(node, pos, &mut |at| {
                // An empty repeat ends the loop.
                (at != pos || count < min) && self.repeat(node, min, max, greedy, count + 1, at, k)
            })
        };
        if count < min {
            return again(k);
        }
        if greedy {
            again(k) || k(pos)
        } else {
            k(pos) || again(k)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(line: &str, language: &str) -> String {
        let spans = highlight_line(line, language).unwrap();
        let mut out = String::new();
        for (start, end, token) in spans {
            let c = match token {
                Token::Keyword => 'k',
                Token::String => 's',
                Token::Comment => 'c',
                Token::Number => 'n',
                Token::Type => 't',
                Token::Function => 'f',
                Token::Operator => 'o',
                Token::Punct => 'p',
                Token::Default => '.',
            };
            out.extend(std::iter::repeat_n(c, end - start));
        }
        out
    }

    #[test]
    fn the_engine_follows_javascript() {
        let m = |p: &str, f: &str, t: &str, at: usize| {
            Re::new(p, f).match_at(&t.chars().collect::<Vec<_>>(), at)
        };
        // Backtracking into an optional part to reach a boundary.
        assert_eq!(m(r"\b\d+(?:\.\d+)?\b", "y", "1.5x", 0), Some(1));
        assert_eq!(m(r"\b\d+(?:\.\d+)?\b", "y", "12abc", 0), None);
        // A word in a list must be the whole word.
        assert_eq!(
            m(r"\b(?:in|instanceof)\b", "y", "instanceof x", 0),
            Some(10)
        );
        assert_eq!(m(r"\b(?:class)\b", "y", "dataclass", 4), None);
        // Lazy, lookahead, ranges (`:->` is `:` to `>`), case-insensitive.
        assert_eq!(m(r"\/\*[\s\S]*?\*\/", "y", "/* a */ b */", 0), Some(7));
        assert_eq!(m(r"\b[a-zA-Z_]\w*(?=\s*\()", "y", "foo  (1)", 0), Some(3));
        assert_eq!(m(r"[{}()[\];,.:->]", "y", "<", 0), Some(1));
        assert_eq!(m(r"\b(?:SELECT)\b", "iy", "select x", 0), Some(6));
        assert_eq!(m(r"#[0-9a-fA-F]{3,8}\b", "y", "#ffcc00;", 0), Some(7));
        assert_eq!(m(r"^[\w.-]+(?=\s*:)", "y", "a: b", 0), Some(1));
        assert_eq!(m(r"^[\w.-]+(?=\s*:)", "y", " a: b", 1), None);
    }

    #[test]
    fn lines_are_coloured_as_the_tui_does() {
        let tui = |line: &str, language: &str| classes(line, language).replace('o', "t");
        assert_eq!(
            tui("export function grow(rate: number): number {", "ts"),
            "kkkkkk.kkkkkkkk.ffffp....p.ttttttpp.tttttt.p"
        );
        assert_eq!(
            tui("  return 1 + rate / 100; // a long trailing comment", "ts"),
            "..kkkkkk.n.t......t.nnnp.cccccccccccccccccccccccccc"
        );
        assert_eq!(
            tui("fn main() { let s = 'c'; println!(\"{}\", x); }", "rust"),
            "kk.ffffpp.p.kkk.....sssp.ffffffffpssssp..pp.p"
        );
        assert!(highlight_line("x", "brainfuck").is_none());
    }

    /// The TUI's own colours of its test corpus (`highlight_fixture.txt`,
    /// made by `highlightLineByRules` in exocortex `tui/src/markdown`): one
    /// class per character, `k s c n t f p .` (`t` for types and operators,
    /// which share a colour).
    #[test]
    fn the_tuis_corpus_matches_class_for_class() {
        let fixture = include_str!("highlight_fixture.txt");
        let mut checked = 0;
        let mut lines = fixture.lines();
        while let Some(language) = lines.next() {
            let (Some(line), Some(expected)) = (lines.next(), lines.next()) else {
                break;
            };
            let line = line.strip_prefix('>').unwrap_or(line);
            let expected = expected.strip_prefix('=').unwrap_or(expected);
            // Types and operators are one colour, one class there.
            assert_eq!(
                classes(line, language).replace('o', "t"),
                expected,
                "{language}: {line:?}"
            );
            checked += 1;
        }
        assert!(checked > 500, "{checked}");
    }
}
