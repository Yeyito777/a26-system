//! Symbol tables (exocortex `tui/src/markdown/mathsymbols.ts`): every TeX
//! command maps to a glyph and a TeX atom class. The class drives spacing:
//! binary operators and relations get a space on each side, as in typeset
//! TeX, while ordinary symbols sit next to each other.

/// A TeX atom class; `Space` is the layout's class for explicit spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Space,
}

use Class::{Bin, Close, Open, Ord, Punct, Rel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Plain,
    Double,
    Script,
    Fraktur,
    /// A name JavaScript finds on every object (`\constructor`, `\toString`):
    /// `name in FONT_COMMANDS` holds for it, but it maps nothing.
    Inherited,
}

use Font::{Double, Fraktur, Plain, Script};

#[rustfmt::skip]
const GREEK: &[(&str, &str)] = &[
    ("alpha", "α"), ("beta", "β"), ("gamma", "γ"), ("delta", "δ"), ("epsilon", "ϵ"),
    ("varepsilon", "ε"), ("zeta", "ζ"), ("eta", "η"), ("theta", "θ"), ("vartheta", "ϑ"),
    ("iota", "ι"), ("kappa", "κ"), ("varkappa", "ϰ"), ("lambda", "λ"), ("mu", "μ"), ("nu", "ν"),
    ("xi", "ξ"), ("omicron", "ο"), ("pi", "π"), ("varpi", "ϖ"), ("rho", "ρ"), ("varrho", "ϱ"),
    ("sigma", "σ"), ("varsigma", "ς"), ("tau", "τ"), ("upsilon", "υ"), ("phi", "ϕ"),
    ("varphi", "φ"), ("chi", "χ"), ("psi", "ψ"), ("omega", "ω"), ("Gamma", "Γ"), ("Delta", "Δ"),
    ("Theta", "Θ"), ("Lambda", "Λ"), ("Xi", "Ξ"), ("Pi", "Π"), ("Sigma", "Σ"), ("Upsilon", "Υ"),
    ("Phi", "Φ"), ("Psi", "Ψ"), ("Omega", "Ω"), ("varGamma", "Γ"), ("varDelta", "Δ"),
    ("varTheta", "Θ"), ("varLambda", "Λ"), ("varXi", "Ξ"), ("varPi", "Π"), ("varSigma", "Σ"),
    ("varUpsilon", "Υ"), ("varPhi", "Φ"), ("varPsi", "Ψ"), ("varOmega", "Ω"), ("digamma", "ϝ"),
];

#[rustfmt::skip]
const ORDINARY: &[(&str, &str)] = &[
    ("infty", "∞"), ("partial", "∂"), ("nabla", "∇"), ("emptyset", "∅"), ("varnothing", "∅"),
    ("forall", "∀"), ("exists", "∃"), ("nexists", "∄"), ("ell", "ℓ"), ("hbar", "ℏ"),
    ("hslash", "ℏ"), ("Re", "ℜ"), ("Im", "ℑ"), ("aleph", "ℵ"), ("beth", "ℶ"), ("wp", "℘"),
    ("angle", "∠"), ("measuredangle", "∡"), ("triangle", "△"), ("degree", "°"), ("prime", "′"),
    ("top", "⊤"), ("bot", "⊥"), ("dagger", "†"), ("ddagger", "‡"), ("therefore", "∴"),
    ("because", "∵"), ("checkmark", "✓"), ("square", "□"), ("Box", "□"), ("blacksquare", "■"),
    ("qed", "∎"), ("S", "§"), ("P", "¶"), ("dots", "…"), ("ldots", "…"), ("cdots", "⋯"),
    ("vdots", "⋮"), ("ddots", "⋱"), ("dotsc", "…"), ("dotsb", "⋯"), ("dotsm", "⋯"), ("dotsi", "⋯"),
    ("dotso", "…"), ("imath", "ı"), ("jmath", "ȷ"), ("neg", "¬"), ("lnot", "¬"), ("flat", "♭"),
    ("natural", "♮"), ("sharp", "♯"), ("clubsuit", "♣"), ("diamondsuit", "♢"), ("heartsuit", "♡"),
    ("spadesuit", "♠"), ("infin", "∞"), ("backslash", "\\"), ("surd", "√"), ("complement", "∁"),
    ("diagup", "╱"), ("diagdown", "╲"), ("lozenge", "◊"), ("star", "⋆"), ("bigstar", "★"),
    ("copyright", "©"), ("pounds", "£"), ("euro", "€"), ("yen", "¥"), ("textdollar", "$"),
    ("dollar", "$"), ("{", "{"), ("}", "}"), ("$", "$"), ("%", "%"), ("#", "#"), ("&", "&"),
    ("_", "_"),
];

#[rustfmt::skip]
const BINARY: &[(&str, &str)] = &[
    ("pm", "±"), ("mp", "∓"), ("times", "×"), ("div", "÷"), ("cdot", "·"), ("ast", "∗"),
    ("circ", "∘"), ("bullet", "•"), ("oplus", "⊕"), ("ominus", "⊖"), ("otimes", "⊗"),
    ("oslash", "⊘"), ("odot", "⊙"), ("cup", "∪"), ("cap", "∩"), ("sqcup", "⊔"), ("sqcap", "⊓"),
    ("uplus", "⊎"), ("setminus", "∖"), ("smallsetminus", "∖"), ("land", "∧"), ("lor", "∨"),
    ("wedge", "∧"), ("vee", "∨"), ("diamond", "⋄"), ("bigtriangleup", "△"),
    ("bigtriangledown", "▽"), ("triangleleft", "◁"), ("triangleright", "▷"), ("wr", "≀"),
    ("amalg", "⨿"), ("dotplus", "∔"), ("ltimes", "⋉"), ("rtimes", "⋊"), ("centerdot", "·"),
    ("divideontimes", "⋇"), ("boxplus", "⊞"), ("boxtimes", "⊠"), ("intercal", "⊺"), ("bmod", "mod"),
];

#[rustfmt::skip]
const RELATIONS: &[(&str, &str)] = &[
    ("le", "≤"), ("leq", "≤"), ("leqslant", "≤"), ("ge", "≥"), ("geq", "≥"), ("geqslant", "≥"),
    ("ne", "≠"), ("neq", "≠"), ("lt", "<"), ("gt", ">"), ("approx", "≈"), ("approxeq", "≊"),
    ("equiv", "≡"), ("sim", "∼"), ("simeq", "≃"), ("cong", "≅"), ("propto", "∝"),
    ("varpropto", "∝"), ("subset", "⊂"), ("subseteq", "⊆"), ("subsetneq", "⊊"), ("supset", "⊃"),
    ("supseteq", "⊇"), ("supsetneq", "⊋"), ("in", "∈"), ("notin", "∉"), ("ni", "∋"), ("owns", "∋"),
    ("ll", "≪"), ("gg", "≫"), ("lll", "⋘"), ("ggg", "⋙"), ("lesssim", "≲"), ("gtrsim", "≳"),
    ("lessapprox", "⪅"), ("gtrapprox", "⪆"), ("prec", "≺"), ("succ", "≻"), ("preceq", "⪯"),
    ("succeq", "⪰"), ("nless", "≮"), ("ngtr", "≯"), ("nleq", "≰"), ("ngeq", "≱"),
    ("nsubseteq", "⊈"), ("nsupseteq", "⊉"), ("parallel", "∥"), ("nparallel", "∦"), ("perp", "⟂"),
    ("mid", "∣"), ("nmid", "∤"), ("vdash", "⊢"), ("dashv", "⊣"), ("models", "⊨"), ("vDash", "⊨"),
    ("asymp", "≍"), ("doteq", "≐"), ("triangleq", "≜"), ("coloneqq", "≔"), ("eqqcolon", "≕"),
    ("coloneq", "≔"), ("approxident", "≋"), ("bowtie", "⋈"), ("smile", "⌣"), ("frown", "⌢"),
    ("sqsubset", "⊏"), ("sqsupset", "⊐"), ("sqsubseteq", "⊑"), ("sqsupseteq", "⊒"), ("lhd", "⊲"),
    ("rhd", "⊳"), ("unlhd", "⊴"), ("unrhd", "⊵"), ("to", "→"), ("gets", "←"), ("rightarrow", "→"),
    ("leftarrow", "←"), ("leftrightarrow", "↔"), ("Rightarrow", "⇒"), ("Leftarrow", "⇐"),
    ("Leftrightarrow", "⇔"), ("longrightarrow", "→"), ("longleftarrow", "←"),
    ("longleftrightarrow", "↔"), ("Longrightarrow", "⇒"), ("Longleftarrow", "⇐"),
    ("Longleftrightarrow", "⇔"), ("implies", "⇒"), ("impliedby", "⇐"), ("iff", "⇔"),
    ("mapsto", "↦"), ("longmapsto", "↦"), ("uparrow", "↑"), ("downarrow", "↓"),
    ("updownarrow", "↕"), ("Uparrow", "⇑"), ("Downarrow", "⇓"), ("Updownarrow", "⇕"),
    ("nearrow", "↗"), ("searrow", "↘"), ("swarrow", "↙"), ("nwarrow", "↖"), ("hookrightarrow", "↪"),
    ("hookleftarrow", "↩"), ("rightharpoonup", "⇀"), ("rightharpoondown", "⇁"),
    ("leftharpoonup", "↼"), ("leftharpoondown", "↽"), ("rightleftharpoons", "⇌"),
    ("leftrightharpoons", "⇋"), ("leadsto", "⇝"), ("rightsquigarrow", "⇝"),
    ("twoheadrightarrow", "↠"), ("rightrightarrows", "⇉"), ("leftleftarrows", "⇇"),
    ("xrightarrow", "→"), ("xleftarrow", "←"), ("nrightarrow", "↛"), ("nleftarrow", "↚"),
    ("nRightarrow", "⇏"), ("nLeftarrow", "⇍"), ("nLeftrightarrow", "⇎"), ("nleftrightarrow", "↮"),
    ("circlearrowleft", "↺"), ("circlearrowright", "↻"), ("curvearrowleft", "↶"),
    ("curvearrowright", "↷"), ("nsim", "≁"), ("ncong", "≇"), ("nequiv", "≢"), ("napprox", "≉"),
    ("vartriangleleft", "⊲"), ("vartriangleright", "⊳"), ("between", "≬"), ("pitchfork", "⋔"),
];

#[rustfmt::skip]
const DELIMITERS: &[(&str, &str, Class)] = &[
    ("langle", "⟨", Open), ("rangle", "⟩", Close), ("lceil", "⌈", Open), ("rceil", "⌉", Close),
    ("lfloor", "⌊", Open), ("rfloor", "⌋", Close), ("lvert", "|", Open), ("rvert", "|", Close),
    ("lVert", "‖", Open), ("rVert", "‖", Close), ("vert", "|", Ord), ("Vert", "‖", Ord),
    ("|", "‖", Ord), ("lbrace", "{", Open), ("rbrace", "}", Close), ("lbrack", "[", Open),
    ("rbrack", "]", Close), ("lparen", "(", Open), ("rparen", ")", Close), ("ulcorner", "⌜", Open),
    ("urcorner", "⌝", Close), ("llcorner", "⌞", Open), ("lrcorner", "⌟", Close),
    ("lgroup", "(", Open), ("rgroup", ")", Close),
];

#[rustfmt::skip]
const PUNCTUATION: &[(&str, &str, Class)] = &[
    ("colon", ":", Punct), ("ldotp", ".", Punct), ("cdotp", "·", Punct),
];

#[rustfmt::skip]
const SUPERSCRIPTS: &[(char, &str)] = &[
    ('0', "⁰"), ('1', "¹"), ('2', "²"), ('3', "³"), ('4', "⁴"), ('5', "⁵"), ('6', "⁶"), ('7', "⁷"),
    ('8', "⁸"), ('9', "⁹"), ('+', "⁺"), ('−', "⁻"), ('-', "⁻"), ('=', "⁼"), ('(', "⁽"), (')', "⁾"),
    ('a', "ᵃ"), ('b', "ᵇ"), ('c', "ᶜ"), ('d', "ᵈ"), ('e', "ᵉ"), ('f', "ᶠ"), ('g', "ᵍ"), ('h', "ʰ"),
    ('i', "ⁱ"), ('j', "ʲ"), ('k', "ᵏ"), ('l', "ˡ"), ('m', "ᵐ"), ('n', "ⁿ"), ('o', "ᵒ"), ('p', "ᵖ"),
    ('r', "ʳ"), ('s', "ˢ"), ('t', "ᵗ"), ('u', "ᵘ"), ('v', "ᵛ"), ('w', "ʷ"), ('x', "ˣ"), ('y', "ʸ"),
    ('z', "ᶻ"), ('A', "ᴬ"), ('B', "ᴮ"), ('D', "ᴰ"), ('E', "ᴱ"), ('G', "ᴳ"), ('H', "ᴴ"), ('I', "ᴵ"),
    ('J', "ᴶ"), ('K', "ᴷ"), ('L', "ᴸ"), ('M', "ᴹ"), ('N', "ᴺ"), ('O', "ᴼ"), ('P', "ᴾ"), ('R', "ᴿ"),
    ('T', "ᵀ"), ('U', "ᵁ"), ('V', "ⱽ"), ('W', "ᵂ"), ('α', "ᵅ"), ('β', "ᵝ"), ('γ', "ᵞ"), ('δ', "ᵟ"),
    ('ε', "ᵋ"), ('ϵ', "ᵋ"), ('θ', "ᶿ"), ('ι', "ᶥ"), ('ϕ', "ᵠ"), ('φ', "ᵠ"), ('χ', "ᵡ"), ('⊤', "ᵀ"),
    ('′', "′"), ('″', "″"), ('‴', "‴"), ('∘', "°"), ('°', "°"), ('∗', "∗"), ('†', "†"), ('‡', "‡"),
    ('⊥', "⊥"),
];

#[rustfmt::skip]
const SUBSCRIPTS: &[(char, &str)] = &[
    ('0', "₀"), ('1', "₁"), ('2', "₂"), ('3', "₃"), ('4', "₄"), ('5', "₅"), ('6', "₆"), ('7', "₇"),
    ('8', "₈"), ('9', "₉"), ('+', "₊"), ('−', "₋"), ('-', "₋"), ('=', "₌"), ('(', "₍"), (')', "₎"),
    ('a', "ₐ"), ('e', "ₑ"), ('h', "ₕ"), ('i', "ᵢ"), ('j', "ⱼ"), ('k', "ₖ"), ('l', "ₗ"), ('m', "ₘ"),
    ('n', "ₙ"), ('o', "ₒ"), ('p', "ₚ"), ('r', "ᵣ"), ('s', "ₛ"), ('t', "ₜ"), ('u', "ᵤ"), ('v', "ᵥ"),
    ('x', "ₓ"), ('β', "ᵦ"), ('γ', "ᵧ"), ('ρ', "ᵨ"), ('ϕ', "ᵩ"), ('φ', "ᵩ"), ('χ', "ᵪ"),
];

#[rustfmt::skip]
const DOUBLE_STRUCK: &[(char, &str)] = &[
    ('1', "𝟙"), ('C', "ℂ"), ('H', "ℍ"), ('N', "ℕ"), ('P', "ℙ"), ('Q', "ℚ"), ('R', "ℝ"), ('Z', "ℤ"),
    ('k', "𝕜"),
];

#[rustfmt::skip]
const CALLIGRAPHIC: &[(char, &str)] = &[
    ('B', "ℬ"), ('E', "ℰ"), ('F', "ℱ"), ('H', "ℋ"), ('I', "ℐ"), ('L', "ℒ"), ('M', "ℳ"), ('R', "ℛ"),
    ('e', "ℯ"), ('g', "ℊ"), ('o', "ℴ"),
];

#[rustfmt::skip]
const FRAKTUR: &[(char, &str)] = &[
    ('C', "ℭ"), ('H', "ℌ"), ('I', "ℑ"), ('R', "ℜ"), ('Z', "ℨ"),
];

#[rustfmt::skip]
const NEGATED: &[(&str, &str)] = &[
    ("=", "≠"), ("<", "≮"), (">", "≯"), ("≤", "≰"), ("≥", "≱"), ("∈", "∉"), ("∋", "∌"), ("⊂", "⊄"),
    ("⊃", "⊅"), ("⊆", "⊈"), ("⊇", "⊉"), ("≡", "≢"), ("∼", "≁"), ("≈", "≉"), ("≅", "≇"), ("∣", "∤"),
    ("∥", "∦"), ("→", "↛"), ("←", "↚"), ("↔", "↮"), ("⇒", "⇏"), ("⇐", "⇍"), ("⇔", "⇎"), ("≃", "≄"),
    ("⊢", "⊬"), ("⊨", "⊭"), ("∃", "∄"),
];

#[rustfmt::skip]
const VULGAR_FRACTIONS: &[(&str, &str)] = &[
    ("1/2", "½"), ("1/3", "⅓"), ("2/3", "⅔"), ("1/4", "¼"), ("3/4", "¾"), ("1/5", "⅕"),
    ("2/5", "⅖"), ("3/5", "⅗"), ("4/5", "⅘"), ("1/6", "⅙"), ("5/6", "⅚"), ("1/7", "⅐"),
    ("1/8", "⅛"), ("3/8", "⅜"), ("5/8", "⅝"), ("7/8", "⅞"), ("1/9", "⅑"), ("1/10", "⅒"),
];

#[rustfmt::skip]
const FONT_COMMANDS: &[(&str, Font)] = &[
    ("mathbb", Double), ("Bbb", Double), ("mathbbm", Double), ("varmathbb", Double),
    ("mathcal", Script), ("mathscr", Script), ("mathfrak", Fraktur), ("mathrm", Plain),
    ("mathit", Plain), ("mathbf", Plain), ("mathsf", Plain), ("mathtt", Plain),
    ("mathnormal", Plain), ("boldsymbol", Plain), ("bm", Plain), ("pmb", Plain),
    ("mathbfit", Plain), ("mathup", Plain), ("symbf", Plain), ("symrm", Plain), ("mathring", Plain),
];

#[rustfmt::skip]
const TEXT_COMMANDS: &[&str] = &[
    "text", "textrm", "textit", "textbf", "texttt", "textsf", "textup", "textnormal", "textmd",
    "mbox", "hbox", "emph", "textsl", "textsc",
];

#[rustfmt::skip]
const ACCENTS: &[(&str, &str)] = &[
    ("hat", "\u{302}"), ("widehat", "\u{302}"), ("check", "\u{30C}"), ("widecheck", "\u{30C}"),
    ("tilde", "\u{303}"), ("widetilde", "\u{303}"), ("acute", "\u{301}"), ("grave", "\u{300}"),
    ("dot", "\u{307}"), ("ddot", "\u{308}"), ("dddot", "\u{20DB}"), ("breve", "\u{306}"),
    ("bar", "\u{304}"), ("vec", "\u{20D7}"), ("overrightarrow", "\u{20D7}"),
    ("overleftarrow", "\u{20D6}"), ("overleftrightarrow", "\u{20E1}"), ("overline", "\u{305}"),
    ("underline", "\u{332}"), ("mathring", "\u{30A}"), ("utilde", "\u{330}"),
    ("underbar", "\u{332}"),
];

#[rustfmt::skip]
const ACCENT_NAMES: &[(&str, &str)] = &[
    ("\u{302}", "hat"), ("\u{30C}", "check"), ("\u{303}", "tilde"), ("\u{301}", "acute"),
    ("\u{300}", "grave"), ("\u{307}", "dot"), ("\u{308}", "ddot"), ("\u{20DB}", "dddot"),
    ("\u{306}", "breve"), ("\u{304}", "bar"), ("\u{20D7}", "vec"), ("\u{20D6}", "vec"),
    ("\u{20E1}", "vec"), ("\u{305}", "bar"), ("\u{332}", "underline"), ("\u{30A}", "ring"),
    ("\u{330}", "utilde"),
];

#[rustfmt::skip]
const SPANNING_ACCENTS: &[&str] = &[
    "overline", "underline", "bar", "underbar", "widetilde", "widehat",
];

#[rustfmt::skip]
const SPACES: &[(&str, usize)] = &[
    (",", 1), (":", 1), (">", 1), (";", 1), (" ", 1), ("!", 0), ("quad", 2), ("qquad", 4),
    ("enspace", 1), ("thinspace", 1), ("medspace", 1), ("thickspace", 1), ("negthinspace", 0),
    ("negmedspace", 0), ("negthickspace", 0), ("enskip", 1), ("space", 1), ("nobreakspace", 1),
];

#[rustfmt::skip]
const IGNORED_COMMANDS: &[&str] = &[
    "displaystyle", "textstyle", "scriptstyle", "scriptscriptstyle", "limits", "nolimits",
    "nonumber", "notag", "mathstrut", "strut", "allowbreak", "nobreak", "hfill", "hfil",
    "centering", "relax", "left.", "right.", "middle.", "big", "Big", "bigg", "Bigg", "bigl",
    "bigr", "Bigl", "Bigr", "biggl", "biggr", "Biggl", "Biggr", "bigm", "Bigm", "biggm", "Biggm",
    "hline", "midrule", "toprule", "bottomrule", "newline", "vfill", "smallskip", "medskip",
    "bigskip", "noindent", "tiny", "scriptsize", "footnotesize", "small", "normalsize", "large",
    "Large", "LARGE", "huge", "Huge", "rm", "it", "bf", "sf", "tt", "cal", "boldmath", "unboldmath",
    "mathchoice",
];

#[rustfmt::skip]
const DROPPED_WITH_ARGUMENT: &[(&str, usize)] = &[
    ("label", 1), ("color", 1), ("hspace", 1), ("vspace", 1), ("hspace*", 1), ("vspace*", 1),
    ("phantom", 1), ("hphantom", 1), ("vphantom", 1), ("cline", 1), ("cite", 1), ("rule", 2),
    ("raisebox", 1), ("setlength", 2),
];

#[rustfmt::skip]
const FUNCTION_NAMES: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "arccot", "sinh",
    "cosh", "tanh", "coth", "sech", "csch", "log", "ln", "lg", "exp", "det", "dim", "deg", "gcd",
    "lcm", "hom", "ker", "arg", "Pr", "tr", "Tr", "rank", "sgn", "sign", "diag", "Var", "Cov",
    "erf", "mod", "lim", "limsup", "liminf", "max", "min", "sup", "inf", "argmax", "argmin",
    "injlim", "projlim",
];

#[rustfmt::skip]
const LIMIT_OPERATORS: &[&str] = &[
    "lim", "limsup", "liminf", "max", "min", "sup", "inf", "argmax", "argmin", "det", "gcd", "Pr",
    "injlim", "projlim",
];

#[rustfmt::skip]
const OPERATOR_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("limsup", "lim sup"), ("liminf", "lim inf"), ("argmax", "arg max"), ("argmin", "arg min"),
    ("injlim", "inj lim"), ("projlim", "proj lim"),
];

#[rustfmt::skip]
const LARGE_OPERATORS: &[(&str, &str, bool)] = &[
    ("sum", "∑", true), ("prod", "∏", true), ("coprod", "∐", true), ("bigcup", "⋃", true),
    ("bigcap", "⋂", true), ("bigvee", "⋁", true), ("bigwedge", "⋀", true), ("bigoplus", "⨁", true),
    ("bigotimes", "⨂", true), ("bigodot", "⨀", true), ("biguplus", "⨄", true),
    ("bigsqcup", "⨆", true), ("int", "∫", false), ("iint", "∬", false), ("iiint", "∭", false),
    ("oint", "∮", false), ("intop", "∫", true), ("smallint", "∫", false), ("oiint", "∯", false),
];

fn lookup<T: Copy>(table: &[(&str, T)], key: &str) -> Option<T> {
    table
        .iter()
        .find(|(name, _)| *name == key)
        .map(|&(_, value)| value)
}

fn lookup_char(table: &[(char, &'static str)], key: char) -> Option<&'static str> {
    table
        .iter()
        .find(|(c, _)| *c == key)
        .map(|&(_, value)| value)
}

/// `SYMBOLS[name]`: the glyph and class of a symbol command.
pub fn symbol(name: &str) -> Option<(&'static str, Class)> {
    let plain = |table: &'static [(&'static str, &'static str)], class: Class| {
        lookup(table, name).map(|text| (text, class))
    };
    plain(GREEK, Ord)
        .or_else(|| plain(ORDINARY, Ord))
        .or_else(|| plain(BINARY, Bin))
        .or_else(|| plain(RELATIONS, Rel))
        .or_else(|| {
            DELIMITERS
                .iter()
                .chain(PUNCTUATION)
                .find(|(key, _, _)| *key == name)
                .map(|&(_, text, class)| (text, class))
        })
}

/// The class of a character in math mode (`charClass`; like the TUI's
/// `includes`, a longer text is looked up as a substring).
pub fn text_class(text: &str) -> Class {
    if "+-*±∓×÷·∗∘⊕⊗∪∩∧∨∖".contains(text) {
        Bin
    } else if "=<>:≤≥≠≈≡∼≃≅∝⊂⊆⊃⊇∈∉∋→←↔⇒⇐⇔↦≪≫≲≳∣∥⟂⊢⊨≔".contains(text)
    {
        Rel
    } else if "([⟨⌈⌊".contains(text) {
        Open
    } else if ")]⟩⌉⌋".contains(text) {
        Close
    } else if ",;".contains(text) {
        Punct
    } else {
        Ord
    }
}

/// The names `in` finds on every JavaScript object.
const INHERITED: &[&str] = &[
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// Whether `object[name]` is inherited from `Object.prototype`.
pub fn inherited(name: &str) -> bool {
    INHERITED.contains(&name)
}

/// `NEGATED[text]`: `\not` applied to a relation.
pub fn negated(text: &str) -> Option<&'static str> {
    lookup(NEGATED, text)
}

fn map_all(text: &str, table: &[(char, &'static str)]) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    text.chars().map(|c| lookup_char(table, c)).collect()
}

/// Unicode superscript for the whole text, or None when any character lacks one.
pub fn superscript(text: &str) -> Option<String> {
    map_all(text, SUPERSCRIPTS)
}

/// Unicode subscript for the whole text, or None when any character lacks one.
pub fn subscript(text: &str) -> Option<String> {
    map_all(text, SUBSCRIPTS)
}

/// Simple digit fractions with a single-glyph Unicode form (`"1/2"`).
pub fn vulgar_fraction(key: &str) -> Option<&'static str> {
    lookup(VULGAR_FRACTIONS, key)
}

pub fn apply_font(text: &str, font: Font) -> String {
    let table = match font {
        Double => DOUBLE_STRUCK,
        Script => CALLIGRAPHIC,
        Fraktur => FRAKTUR,
        Plain | Font::Inherited => return text.to_string(),
    };
    text.chars()
        .map(|c| lookup_char(table, c).map_or_else(|| c.to_string(), str::to_string))
        .collect()
}

/// `FONT_COMMANDS[name]`, for `name in FONT_COMMANDS`.
pub fn font_command(name: &str) -> Option<Font> {
    lookup(FONT_COMMANDS, name).or_else(|| inherited(name).then_some(Font::Inherited))
}

pub fn is_text_command(name: &str) -> bool {
    TEXT_COMMANDS.contains(&name)
}

/// The combining mark of an accent command.
pub fn accent(name: &str) -> Option<&'static str> {
    lookup(ACCENTS, name)
}

/// A name for an accent written out where combining marks are not drawn.
pub fn accent_name(mark: &str) -> Option<&'static str> {
    lookup(ACCENT_NAMES, mark)
}

/// Accents drawn across every character of a multi-character body.
pub fn is_spanning_accent(name: &str) -> bool {
    SPANNING_ACCENTS.contains(&name)
}

/// Explicit spacing commands, in terminal columns.
pub fn spaces(name: &str) -> Option<usize> {
    lookup(SPACES, name)
}

/// Commands that only affect typesetting details invisible in a terminal.
pub fn is_ignored(name: &str) -> bool {
    IGNORED_COMMANDS.contains(&name)
}

/// Commands whose arguments are dropped entirely: how many.
pub fn dropped_arguments(name: &str) -> Option<usize> {
    lookup(DROPPED_WITH_ARGUMENT, name)
}

/// Upright operator names (`\sin x`).
pub fn is_function_name(name: &str) -> bool {
    FUNCTION_NAMES.contains(&name)
}

/// Operator names that take limits above/below in display style.
pub fn is_limit_operator(name: &str) -> bool {
    LIMIT_OPERATORS.contains(&name)
}

pub fn operator_display_name(name: &str) -> Option<&'static str> {
    lookup(OPERATOR_DISPLAY_NAMES, name)
}

/// Large operators: glyph, and whether it takes limits in display style.
pub fn large_operator(name: &str) -> Option<(&'static str, bool)> {
    LARGE_OPERATORS
        .iter()
        .find(|(key, _, _)| *key == name)
        .map(|&(_, text, limits)| (text, limits))
}
