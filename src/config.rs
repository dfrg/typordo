//! Which directories hold fonts, and where their caches live.
//!
//! This reads the `<dir>`, `<cachedir>` and `<include>` elements of
//! `fonts.conf` and everything it pulls in, plus the `<selectfont>` rules
//! that decide which fonts are listed at all. That is enough to answer "what
//! fonts does this system have", which is what `fc-list` reports.
//!
//! `<match>`, `<test>`, `<edit>` and `<alias>` are read too, and
//! [`Config::substitute`] applies them: that is how a query for `sans-serif`
//! becomes a list of real families before anything is scored.
//!
//! # What is not read yet
//!
//! Within `<selectfont>`, every value kind the DTD allows is handled except
//! `<langset>`, which needs fontconfig's own language table. A selector this
//! crate cannot fully evaluate never matches, rather than being applied
//! without the part it did not understand: dropping a condition would *widen*
//! a rule, so a reject selector would start rejecting fonts fontconfig keeps.
//!
//! `<remap-dir>` and its `salt` attribute are unhandled, so a sandboxed
//! configuration that remaps font paths will not find its caches.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::fnv::BuildFnv;
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::casefold;
use crate::glob;
use crate::langset::Langs;
use crate::md5;
use crate::object::Object;
use crate::pattern::Pattern;
use crate::query::{OwnedValue, Property, Query};
use crate::rules::{
    BinaryOp, Compare, Edit, EditMode, Expr, MatchKind, Qual, Rule, Step, Test, UnaryOp,
};
use crate::value::{Binding, Matrix, Range, Value};
use crate::xml::{Event, Reader, XmlError};

/// The architecture tag fontconfig builds into a cache file name.
///
/// It records the layout the cache was written for, and this build asks for
/// the one it was compiled for. Fontconfig has six: `le64`, `be64`, and for
/// 32-bit machines `le32d4`/`be32d4` and `le32d8`/`be32d8`, where `d4` and
/// `d8` are whether a `double` aligns to one word or two.
pub const ARCHITECTURE: &str = crate::layout::ARCHITECTURE;

/// The configuration directory compiled into fontconfig on Unix.
const CONFIG_DIR: &str = "/etc/fonts";

/// How deep `<include>` may nest before we assume a loop.
const MAX_INCLUDE_DEPTH: usize = 32;

/// Something went wrong reading configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// A configuration file could not be read.
    Io(PathBuf, std::io::Error),
    /// A configuration file was not valid XML.
    Xml(PathBuf, XmlError),
    /// No configuration file was found at all.
    NotFound(PathBuf),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "{}: {e}", path.display()),
            Self::Xml(path, e) => write!(f, "{}: {e}", path.display()),
            Self::NotFound(path) => write!(f, "no configuration file at {}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The font and cache directories a system is configured to use.
#[derive(Clone, Debug, Default)]
pub struct Config {
    font_dirs: Vec<FontDir>,
    cache_dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    selectors: Selectors,
    rules: Vec<Rule>,
}

/// What a path-bearing element carries into [`Config::apply`].
///
/// Six arguments in a row, half of them optional strings, is a shape that
/// invites getting two of them the wrong way round.
struct Applied<'a> {
    element: &'a str,
    prefix: Option<&'a str>,
    body: &'a str,
    /// The `salt` attribute, appended to the path before it is hashed.
    salt: Option<&'a str>,
    /// The `as-path` attribute of a `<remap-dir>`.
    as_path: Option<&'a str>,
    from: &'a Path,
    depth: usize,
    seen: &'a mut HashSet<PathBuf>,
}

/// A font directory, and how its cache is named.
///
/// Fontconfig keeps these as a triple because two of them change the name of
/// the cache file without changing where the fonts are. A `salt` is mixed
/// into the hash so that the same path can have more than one cache; a
/// `<remap-dir>` hashes a different path entirely, which is how a container
/// reads caches built outside it -- the fonts are at `/run/host/fonts` and
/// the cache is named for `/usr/share/fonts`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FontDir {
    path: PathBuf,
    /// `as-path` on a `<remap-dir>`: the path the cache is named for.
    map: Option<PathBuf>,
    /// The `salt` attribute, appended to the path before hashing.
    salt: Option<String>,
}

/// One open XML element while a config file is being read.
#[derive(Debug)]
struct Frame {
    name: String,
    /// The `prefix` attribute, for path-bearing elements.
    prefix: Option<String>,
    /// The `name` attribute, which is what `<patelt>` uses for its property.
    object: Option<String>,
    text: String,
    /// Values collected by a `<patelt>` from its children.
    values: Vec<SelectorValue>,
    /// Properties collected by a `<pattern>` from its `<patelt>` children.
    elements: Vec<(Object, Vec<SelectorValue>)>,
    /// Set when a child could not be understood, so the whole element must
    /// not be applied in its weakened form.
    poisoned: bool,
    /// Every attribute, for the rule elements that use several.
    attrs: Vec<(String, String)>,
    /// Expressions collected from child elements.
    exprs: Vec<Expr>,
    /// Tests and edits collected by a `<match>`, in source order.
    steps: Vec<Step>,
    /// An `<alias>`'s `<prefer>`, `<accept>` and `<default>` sections.
    sections: HashMap<String, Expr>,
}

impl Frame {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The collected child expressions as one expression.
    ///
    /// Several children behave as a comma list, which is how `<test>` accepts
    /// alternatives and `<edit>` produces more than one value.
    fn expr(&self) -> Expr {
        match self.exprs.len() {
            0 => Expr::Unknown,
            1 => self.exprs[0].clone(),
            _ => Expr::List(self.exprs.clone()),
        }
    }
}

/// A `<pattern>` inside an `<acceptfont>` or `<rejectfont>`.
///
/// It matches a font when *every* property it names is present on the font
/// and shares at least one value with it.
#[derive(Clone, Debug)]
struct Selector {
    elements: Vec<(Object, Vec<SelectorValue>)>,
    /// False when part of the selector could not be understood.
    usable: bool,
}

/// A constant a `<patelt>` can hold.
///
/// The DTD permits `int`, `double`, `string`, `matrix`, `bool`, `charset`,
/// `langset` and `const`. Everything but `langset` is handled; anything that
/// cannot be evaluated becomes [`SelectorValue::Unsupported`].
#[derive(Clone, Debug, PartialEq)]
enum SelectorValue {
    String(String),
    Int(i32),
    Double(f64),
    Bool(bool),
    Matrix(Matrix),
    /// Codepoints, from `<charset>`.
    CharSet(Vec<char>),
    /// An inclusive span, from `<range>`.
    Range(Range),
    /// Languages, from `<langset>`.
    LangSet(Langs),
    /// A value this crate cannot evaluate.
    ///
    /// It never matches, and poisons the selector that holds it. Dropping it
    /// instead would *widen* the selector: a reject rule reading "family X and
    /// langset Y" would decay to "family X" and start rejecting fonts that
    /// fontconfig keeps.
    Unsupported,
}

impl SelectorValue {
    /// Parse one value element of a `<patelt>`.
    ///
    /// The property the `<patelt>` names is deliberately not consulted: see
    /// [`constant`] for why `<const>` ignores it.
    fn parse(kind: &str, body: &str) -> Self {
        let body = body.trim();
        match kind {
            "string" => Self::String(body.to_string()),
            "int" => parse_int(body).map_or(Self::Unsupported, Self::Int),
            "double" => body.parse().map_or(Self::Unsupported, Self::Double),
            "bool" => match body {
                "true" => Self::Bool(true),
                "false" => Self::Bool(false),
                _ => Self::Unsupported,
            },
            "const" => match constant(body) {
                Some(value) => Self::Int(value),
                None => Self::Unsupported,
            },
            _ => Self::Unsupported,
        }
    }

    /// Whether a font's value counts as matching this one.
    ///
    /// Strings compare with case folding and blanks ignored, which is what
    /// `FcOpListing` with `FcOpFlagIgnoreBlanks` does.
    fn matches(&self, value: &Value<'_>) -> bool {
        match (self, value) {
            (Self::String(want), Value::String(got)) => casefold::eq_ignoring_blanks(want, got),
            (Self::Int(want), Value::Int(got)) => want == got,
            (Self::Int(want), Value::Double(got)) => f64::from(*want) == *got,
            (Self::Int(want), Value::Bool(got)) => (*want != 0) == *got,
            (Self::Double(want), Value::Double(got)) => want == got,
            (Self::Double(want), Value::Int(got)) => *want == f64::from(*got),
            (Self::Bool(want), Value::Bool(got)) => want == got,
            (Self::Matrix(want), Value::Matrix(got)) => want == got,
            (Self::CharSet(want), Value::CharSet(got)) => want.iter().all(|c| got.contains(*c)),
            // The font has to answer everything the selector asks for, and a
            // language it holds broadly answers a narrower request: a font
            // listing `en` satisfies a selector naming `en-US`.
            (Self::LangSet(want), Value::LangSet(got)) => {
                Langs::from_languages(got).contains_set(want)
            }
            // A listing comparison asks the font to sit *inside* what the
            // selector names, so a scalar matches any span covering it while
            // a span matches only a span that covers all of it.
            (Self::Range(want), Value::Range(got)) => within(got, want),
            (Self::Range(want), Value::Int(got)) => within(&point(f64::from(*got)), want),
            (Self::Range(want), Value::Double(got)) => within(&point(*got), want),
            (Self::Int(want), Value::Range(got)) => within(got, &point(f64::from(*want))),
            (Self::Double(want), Value::Range(got)) => within(got, &point(*want)),
            _ => false,
        }
    }
}

/// Build a `<matrix>` from the four `<double>` children it collected.
fn matrix_from(values: &[SelectorValue]) -> SelectorValue {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            SelectorValue::Double(d) => Some(*d),
            SelectorValue::Int(i) => Some(f64::from(*i)),
            _ => None,
        })
        .collect();
    match numbers[..] {
        [xx, xy, yx, yy] => SelectorValue::Matrix(Matrix { xx, xy, yx, yy }),
        _ => SelectorValue::Unsupported,
    }
}

/// Build a `<charset>` from the `<int>` codepoints it collected.
/// A single number as a span, which is how fontconfig compares one against a
/// range: `FcConfigPromote` widens the scalar and then the ranges compare.
fn point(value: f64) -> Range {
    Range { begin: value, end: value }
}

/// Whether the font's span sits inside the selector's, `FcRangeIsInRange`.
fn within(font: &Range, selector: &Range) -> bool {
    selector.contains(font.begin) && selector.contains(font.end)
}

/// Whether an unreadable part of a literal poisons the whole thing.
///
/// A `<patelt>` selector must poison: dropping a value it cannot evaluate
/// *widens* the selector, and a `<selectfont>` rule that widens rejects fonts
/// fontconfig keeps. A rule expression must not: an edit naming one language
/// this crate has no room for should still make the rest of its change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Strictness {
    Poison,
    Salvage,
}

/// One value element -- a scalar, or one of the four built from children.
fn literal(frame: &Frame, body: &str, strict: Strictness) -> SelectorValue {
    match frame.name.as_str() {
        "matrix" => matrix_from(&frame.values),
        "charset" => charset_from(&frame.values, strict),
        "langset" => langset_from(&frame.values, strict),
        "range" => range_from(&frame.values),
        kind => SelectorValue::parse(kind, body),
    }
}

/// A literal value element as a rule expression.
fn value_expr(value: SelectorValue) -> Expr {
    match value {
        SelectorValue::String(v) => Expr::Value(OwnedValue::String(v)),
        SelectorValue::Int(v) => Expr::Value(OwnedValue::Int(v)),
        SelectorValue::Double(v) => Expr::Value(OwnedValue::Double(v)),
        SelectorValue::Bool(v) => Expr::Value(OwnedValue::Bool(v)),
        SelectorValue::Matrix(v) => Expr::Value(OwnedValue::Matrix(v)),
        SelectorValue::Range(v) => Expr::Value(OwnedValue::Range(v)),
        SelectorValue::LangSet(v) => Expr::Value(OwnedValue::LangSet(v)),
        SelectorValue::CharSet(chars) => {
            let mut coverage = crate::charset::Coverage::new();
            for c in chars {
                coverage.insert(c);
            }
            Expr::Value(OwnedValue::CharSet(coverage))
        }
        SelectorValue::Unsupported => Expr::Unknown,
    }
}

/// A `<range>`, from the two numbers inside it.
///
/// If either is a `<double>` the whole range is one, matching fontconfig:
/// `<range><int>1</int><double>2.5</double></range>` spans 1.0 to 2.5 rather
/// than being rejected for mixing its types.
fn range_from(values: &[SelectorValue]) -> SelectorValue {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            SelectorValue::Double(d) => Some(*d),
            SelectorValue::Int(i) => Some(f64::from(*i)),
            _ => None,
        })
        .collect();
    match numbers[..] {
        // An inverted range is an error to fontconfig, not an empty span.
        [begin, end] if begin <= end => SelectorValue::Range(Range { begin, end }),
        _ => SelectorValue::Unsupported,
    }
}

/// A `<langset>` from the `<string>` languages inside it.
///
/// A name outside fontconfig's table -- `en-GB`, say -- is kept as a name.
/// It cannot be a bit, but it still has to match: a font listing `en`
/// answers a request for `en-GB`, and treating the name as unreadable would
/// silently turn such a selector into one that matches nothing.
fn langset_from(values: &[SelectorValue], strict: Strictness) -> SelectorValue {
    let mut set = Langs::new();
    let mut named = false;
    for value in values {
        let SelectorValue::String(name) = value else {
            if strict == Strictness::Poison {
                return SelectorValue::Unsupported;
            }
            continue;
        };
        named = true;
        set.insert(name);
    }
    if named {
        SelectorValue::LangSet(set)
    } else {
        SelectorValue::Unsupported
    }
}

/// Build a `<charset>` from the codepoints and spans it collected.
fn charset_from(values: &[SelectorValue], strict: Strictness) -> SelectorValue {
    let mut chars = Vec::with_capacity(values.len());
    for value in values {
        // A span is expanded here, because a charset is a bitmap and has no
        // notion of one. Fontconfig does the same.
        let (begin, end) = match value {
            SelectorValue::Int(cp) => (i64::from(*cp), i64::from(*cp)),
            SelectorValue::Range(range) => (range.begin as i64, range.end as i64),
            _ if strict == Strictness::Poison => return SelectorValue::Unsupported,
            _ => continue,
        };
        for cp in begin..=end {
            match u32::try_from(cp).ok().and_then(char::from_u32) {
                Some(c) => chars.push(c),
                None if strict == Strictness::Poison => return SelectorValue::Unsupported,
                None => {}
            }
        }
    }
    if chars.is_empty() {
        return SelectorValue::Unsupported;
    }
    SelectorValue::CharSet(chars)
}

/// One attribute of an element, if it has it.
fn attr<'a>(frame: &'a Frame, name: &str) -> Option<&'a str> {
    frame.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Whether `path` is `prefix` or sits inside it.
///
/// `FcConfigPathStartsWith`: the match has to land on a separator, so
/// `/usr/share/fonts-extra` is not inside `/usr/share/fonts`.
fn starts_with(path: &str, prefix: &Path) -> bool {
    let prefix = prefix.to_string_lossy();
    match path.strip_prefix(prefix.as_ref()) {
        Some("") => true,
        Some(rest) => rest.starts_with('/'),
        None => false,
    }
}

/// The hashed part of a cache file name, with the architecture and version.
fn hashed_name(key: &str) -> String {
    format!("{}-{ARCHITECTURE}.cache-{}", md5::hex(key.as_bytes()), crate::cache::VERSION)
}

/// The cache name a directory asks for through a `.uuid` file it contains.
///
/// A read-only image writes one so that its caches stay findable wherever the
/// tree is mounted: the name comes from the file rather than from the path.
/// Fontconfig does not do this on Windows and neither do we, so that the two
/// look for the same set of names.
fn uuid_name(dir: &Path) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(".uuid")).ok()?;
    // Exactly the first 36 bytes, which is one formatted UUID. Fontconfig
    // reads that many and stops, so a file with a trailing newline works and
    // a longer one is truncated rather than rejected.
    let uuid: String = text.chars().take(36).collect();
    if uuid.len() < 36 {
        return None;
    }
    Some(format!("{uuid}-{ARCHITECTURE}.cache-{}", crate::cache::VERSION))
}

/// Add the locale's languages to a query that does not already name one.
///
/// `FcConfigSubstituteWithPat` does this before any rule runs, for pattern
/// targets only. It looks minor and is not: sorting demotes every font that
/// answers no requested language, so a query with no `lang` at all produces a
/// differently ordered fallback chain.
///
/// A query that already mentions the language, or the undetermined tag `und`,
/// is left alone entirely -- fontconfig stops at the first such value rather
/// than skipping just that one language.
fn add_default_langs(query: &mut Query) {
    let langs = crate::query::default_langs();
    for lang in langs {
        if let Some(element) = query.get(Object::Lang) {
            let already = element.values().any(|(value, _)| match value {
                OwnedValue::String(s) => {
                    s.eq_ignore_ascii_case(&lang) || s.eq_ignore_ascii_case("und")
                }
                _ => false,
            });
            if already {
                return;
            }
        }
        query.add_weak(Object::Lang, lang.as_str());
    }
}

/// The target of the nearest enclosing `<match>`.
///
/// A `<test>` without its own `target` reads the pattern its match is aimed
/// at, so this has to walk out to find it.
fn enclosing_match_kind(stack: &[Frame]) -> MatchKind {
    stack
        .iter()
        .rev()
        .find(|f| f.name == "match")
        .map_or(MatchKind::Pattern, |f| MatchKind::parse(f.attr("target")))
}

/// `binding` on an `<edit>` or `<alias>`, which defaults to weak.
fn parse_binding(name: Option<&str>) -> Binding {
    match name {
        Some("strong") => Binding::Strong,
        Some("same") => Binding::Same,
        _ => Binding::Weak,
    }
}

fn binary_op(name: &str) -> BinaryOp {
    match name {
        "or" => BinaryOp::Or,
        "and" => BinaryOp::And,
        "eq" => BinaryOp::Eq,
        "not_eq" => BinaryOp::NotEq,
        "less" => BinaryOp::Less,
        "less_eq" => BinaryOp::LessEq,
        "more" => BinaryOp::More,
        "more_eq" => BinaryOp::MoreEq,
        "contains" => BinaryOp::Contains,
        "not_contains" => BinaryOp::NotContains,
        "plus" => BinaryOp::Plus,
        "minus" => BinaryOp::Minus,
        "times" => BinaryOp::Times,
        _ => BinaryOp::Divide,
    }
}

fn unary_op(name: &str) -> UnaryOp {
    match name {
        "not" => UnaryOp::Not,
        "floor" => UnaryOp::Floor,
        "ceil" => UnaryOp::Ceil,
        "round" => UnaryOp::Round,
        _ => UnaryOp::Trunc,
    }
}

/// The named constants, in `_FcBaseConstants` declaration order.
///
/// The order is load-bearing: see [`constant`].
static CONSTANTS: &[(&str, i32)] = &[
    // weight
    ("thin", 0),
    ("extralight", 40),
    ("ultralight", 40),
    ("demilight", 55),
    ("semilight", 55),
    ("light", 50),
    ("book", 75),
    ("regular", 80),
    ("normal", 80),
    ("medium", 100),
    ("demibold", 180),
    ("semibold", 180),
    ("bold", 200),
    ("extrabold", 205),
    ("ultrabold", 205),
    ("black", 210),
    ("heavy", 210),
    ("extrablack", 215),
    ("ultrablack", 215),
    // slant
    ("roman", 0),
    ("italic", 100),
    ("oblique", 110),
    // width -- note "normal" is 100 here, but the weight entry above shadows it
    ("ultracondensed", 50),
    ("extracondensed", 63),
    ("condensed", 75),
    ("semicondensed", 87),
    ("normal", 100),
    ("semiexpanded", 113),
    ("expanded", 125),
    ("extraexpanded", 150),
    ("ultraexpanded", 200),
    // spacing
    ("proportional", 0),
    ("dual", 90),
    ("mono", 100),
    ("charcell", 110),
    // rgba
    ("unknown", 0),
    ("rgb", 1),
    ("bgr", 2),
    ("vrgb", 3),
    ("vbgr", 4),
    ("none", 5),
    // hintstyle
    ("hintnone", 0),
    ("hintslight", 1),
    ("hintmedium", 2),
    ("hintfull", 3),
    // the boolean constants, each named after its own property
    ("antialias", 1),
    ("hinting", 1),
    ("verticallayout", 1),
    ("autohint", 1),
    ("globaladvance", 1),
    ("outline", 1),
    ("scalable", 1),
    ("minspace", 1),
    ("embolden", 1),
    ("embeddedbitmap", 1),
    ("decorative", 1),
    // lcdfilter
    ("lcdnone", 0),
    ("lcddefault", 1),
    ("lcdlight", 2),
    ("lcdlegacy", 3),
];

/// The value of a `<const>` name inside a `<patelt>`.
///
/// Looked up by **name alone**, taking the first entry in `_FcBaseConstants`
/// order -- not by the property the `<patelt>` names. That is what
/// `FcPopValue` does: it calls `FcNameConstant`, the name-only lookup, rather
/// than the `FcNameConstantWithObjectCheck` variant that also exists.
///
/// The observable consequence is that `<patelt name="width"><const>normal`
/// resolves to **80**, the *weight* constant, because weight is declared
/// first -- so it matches no font, since widths run 50 to 200. Resolving it
/// per property to 100 would be the more sensible answer and the wrong one;
/// `fc-list` rejects nothing for that selector, and so must this.
fn constant(name: &str) -> Option<i32> {
    CONSTANTS.iter().find(|(constant, _)| *constant == name).map(|(_, value)| *value)
}

/// Parse an integer the way `FcParseInt` does, with `strtol` base 0.
///
/// That means `0x4e00` is hex and `0755` is octal, both of which a plain
/// `str::parse` rejects. Configs really do write codepoints in hex.
fn parse_int(body: &str) -> Option<i32> {
    let (negative, digits) = match body.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, body.strip_prefix('+').unwrap_or(body)),
    };
    let (radix, digits) = match digits.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, &digits[2..]),
        [b'0', _, ..] => (8, &digits[1..]),
        _ => (10, digits),
    };
    let magnitude = i64::from_str_radix(digits, radix).ok()?;
    i32::try_from(if negative { -magnitude } else { magnitude }).ok()
}

/// The `<selectfont>` rules, which decide what is listed at all.
///
/// Two independent filters, each with the same precedence: an accept entry
/// wins, then a reject entry, and anything named by neither is accepted.
#[derive(Clone, Debug, Default)]
struct Selectors {
    /// Which of accept/reject the parser is currently inside.
    accepting: bool,
    accept_globs: Vec<String>,
    reject_globs: Vec<String>,
    accept_patterns: Vec<Selector>,
    reject_patterns: Vec<Selector>,
}

impl Selectors {
    fn globs_mut(&mut self) -> &mut Vec<String> {
        if self.accepting {
            &mut self.accept_globs
        } else {
            &mut self.reject_globs
        }
    }

    fn patterns_mut(&mut self) -> &mut Vec<Selector> {
        if self.accepting {
            &mut self.accept_patterns
        } else {
            &mut self.reject_patterns
        }
    }

    /// Whether any rule was configured at all.
    fn any(&self) -> bool {
        !self.accept_globs.is_empty()
            || !self.reject_globs.is_empty()
            || !self.accept_patterns.is_empty()
            || !self.reject_patterns.is_empty()
    }

    fn accepts_filename(&self, filename: &str) -> bool {
        if self.accept_globs.iter().any(|g| glob::matches(g, filename)) {
            return true;
        }
        !self.reject_globs.iter().any(|g| glob::matches(g, filename))
    }

    fn accepts_font(&self, font: &Pattern<'_>) -> bool {
        if self.accept_patterns.iter().any(|s| s.matches(font)) {
            return true;
        }
        !self.reject_patterns.iter().any(|s| s.matches(font))
    }
}

impl Selector {
    fn matches(&self, font: &Pattern<'_>) -> bool {
        if !self.usable {
            return false;
        }
        self.elements.iter().all(|(object, wanted)| {
            let Some(element) = font.get(*object) else {
                return false;
            };
            // Every value the selector names must be found on the font.
            wanted.iter().all(|want| element.values().any(|got| want.matches(&got)))
        })
    }
}

impl Config {
    /// Load the configuration this system would use.
    ///
    /// `FONTCONFIG_FILE` names a config file outright; otherwise the file is
    /// `fonts.conf` under `FONTCONFIG_PATH`, falling back to `/etc/fonts`.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&default_config_path())
    }

    /// Load a specific configuration file, following its includes.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let mut config = Self::default();
        let mut seen = HashSet::new();
        config.read_file(path, 0, &mut seen)?;
        Ok(config)
    }

    /// The directories configured to hold fonts.
    ///
    /// These are the roots only. Fontconfig records subdirectories in each
    /// directory's own cache rather than in the configuration, so the full
    /// set is what [`Config::caches`] walks.
    pub fn font_dirs(&self) -> impl ExactSizeIterator<Item = &Path> + '_ {
        self.font_dirs.iter().map(|dir| dir.path.as_path())
    }

    /// The directories that may hold caches, in the order to search them.
    pub fn cache_dirs(&self) -> &[PathBuf] {
        &self.cache_dirs
    }

    /// The configuration files that were read, in the order they were read.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Whether `<selectfont>` rules allow a font file to be listed.
    ///
    /// An `<acceptfont><glob>` entry wins outright; otherwise a
    /// `<rejectfont><glob>` entry excludes the file. A path named by neither
    /// is accepted. This also governs whether a *subdirectory* is walked, so
    /// a rejected directory prunes everything beneath it.
    pub fn accepts_filename(&self, filename: &str) -> bool {
        self.selectors.accepts_filename(filename)
    }

    /// Whether `<selectfont>` rules allow a font to be listed.
    ///
    /// The `<pattern>` half of the same mechanism: a selector matches when
    /// every property it names is present on the font and shares at least one
    /// value with it.
    pub fn accepts_font(&self, font: &Pattern<'_>) -> bool {
        self.selectors.accepts_font(font)
    }

    /// Whether both halves of `<selectfont>` allow this font.
    ///
    /// This is the check fontconfig applies as it builds a font set, and the
    /// one a caller listing fonts wants.
    pub fn accepts(&self, font: &Pattern<'_>) -> bool {
        match font.string(Object::File) {
            Some(file) if !self.accepts_filename(file) => false,
            _ => self.accepts_font(font),
        }
    }

    /// The `<match>` rules this configuration defines, in order.
    ///
    /// Test-only: the rule AST is an implementation detail of parsing, and
    /// callers reach its effects through [`Config::substitute`] instead.
    #[cfg(test)]
    fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Rewrite `query` the way fontconfig would before matching.
    ///
    /// This is `FcConfigSubstitute` for [`MatchKind::Pattern`]: every rule is
    /// tried in configuration order, and one whose tests all pass applies its
    /// edits. Rules see each other's work, so ordering is the whole design --
    /// which is why `conf.d` files carry numeric prefixes.
    ///
    /// Call [`Query::default_substitute`] *after* this, as fontconfig does:
    /// the rules run first and the defaults only fill what is still missing.
    pub fn substitute(&self, query: &mut Query) {
        add_default_langs(query);
        self.substitute_kind(query, MatchKind::Pattern, None);
    }

    /// Rewrite `query` with the rules for one target.
    ///
    /// `pattern` is the original query, which a font-target rule can read
    /// through `target="pattern"` to compare what was asked for against what
    /// was found. It is unused for pattern-target rules.
    pub fn substitute_kind(&self, query: &mut Query, kind: MatchKind, pattern: Option<&Query>) {
        // Indexed once for the whole pass, not once per rule: the rules see
        // each other's edits, so the index has to follow the query through
        // all of them.
        let mut pass = crate::rules::Pass::new(query);
        for rule in &self.rules {
            if rule.kind == kind {
                rule.apply(query, pattern, &mut pass);
            }
        }
    }

    /// Whether any `<selectfont>` rule was configured.
    ///
    /// Most systems have none, in which case [`Config::accepts`] is always
    /// true and a caller listing fonts can skip it entirely.
    pub fn has_selectors(&self) -> bool {
        self.selectors.any()
    }

    /// The file name fontconfig gives the cache for `dir`.
    ///
    /// The MD5 of the directory path, then the architecture tag and the
    /// format version: `<hash>-le64.cache-9`.
    ///
    /// The path that gets hashed is not always the one passed in. A
    /// `<remap-dir>` covering `dir` substitutes its `as-path`, and a `salt`
    /// is appended to whatever is left. Both exist so that a cache built on
    /// one machine can be found from another where the same fonts sit
    /// somewhere else.
    pub fn cache_basename(&self, dir: &str) -> String {
        let mut key = String::new();
        let salt = match self.enclosing_dir(dir) {
            Some(entry) => {
                match &entry.map {
                    // The mapped path, then whatever `dir` added to the
                    // prefix. Fontconfig maps the prefix only.
                    Some(map) => {
                        key.push_str(&map.to_string_lossy());
                        let rest =
                            dir[entry.path.to_string_lossy().len()..].trim_start_matches('/');
                        if !rest.is_empty() {
                            key.push('/');
                            key.push_str(rest);
                        }
                    }
                    None => key.push_str(dir),
                }
                entry.salt.as_deref()
            }
            None => {
                key.push_str(dir);
                None
            }
        };
        if let Some(salt) = salt {
            key.push_str(salt);
        }
        hashed_name(&key)
    }

    /// The first configured font directory that `dir` is inside.
    ///
    /// In configuration order, and the *first* match wins even if a later one
    /// is a longer prefix: fontconfig walks its own list the same way, so a
    /// plain `<dir>` listed before a `<remap-dir>` beneath it shadows the
    /// remapping entirely.
    fn enclosing_dir(&self, dir: &str) -> Option<&FontDir> {
        self.font_dirs.iter().find(|entry| starts_with(dir, &entry.path))
    }

    /// Where `dir`'s cache actually is, searching the cache directories in
    /// order the way fontconfig does.
    ///
    /// Two names are tried per directory. The hashed one is what everything
    /// writes; the other comes from a `.uuid` file in the font directory
    /// itself, which is how a read-only image keeps its caches findable after
    /// the tree has been mounted somewhere else. Fontconfig falls back to it
    /// the same way, and only on Unix.
    pub fn cache_path(&self, dir: &str) -> Option<PathBuf> {
        let mut names = vec![self.cache_basename(dir)];
        if let Some(uuid) = uuid_name(Path::new(dir)) {
            names.push(uuid);
        }
        self.cache_dirs
            .iter()
            .flat_map(|cache_dir| names.iter().map(move |name| cache_dir.join(name)))
            .find(|path| path.is_file())
    }

    /// Every cache this configuration reaches, roots and subdirectories both.
    ///
    /// Subdirectories come from the caches themselves, so a directory whose
    /// cache is missing also hides whatever is beneath it — the same blind
    /// spot fontconfig has when it is not allowed to scan.
    pub fn caches(&self) -> Caches<'_> {
        Caches {
            config: self,
            pending: self.font_dirs().filter_map(path_to_string).collect(),
            seen: HashSet::default(),
        }
    }

    fn read_file(
        &mut self,
        path: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        if depth > MAX_INCLUDE_DEPTH {
            return Ok(());
        }
        // Canonicalize so that two routes to one file are recognised as one;
        // fontconfig configs do include each other in loops.
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !seen.insert(key) {
            return Ok(());
        }

        let source =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.to_path_buf(), e))?;
        self.files.push(path.to_path_buf());

        // A frame per open element, so nested constructs like
        // <selectfont><rejectfont><pattern><patelt> can be assembled as their
        // tags close. Text arrives in pieces and is collected per frame.
        let mut stack: Vec<Frame> = Vec::new();
        for event in Reader::new(&source) {
            let event = event.map_err(|e| ConfigError::Xml(path.to_path_buf(), e))?;
            match event {
                Event::Start { name, attrs } => {
                    if name == "acceptfont" || name == "rejectfont" {
                        self.selectors.accepting = name == "acceptfont";
                    }
                    stack.push(Frame {
                        name: name.to_string(),
                        prefix: attrs.get("prefix").map(|p| p.into_owned()),
                        object: attrs.get("name").map(|p| p.into_owned()),
                        text: String::new(),
                        values: Vec::new(),
                        elements: Vec::new(),
                        poisoned: false,
                        attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.into_owned())).collect(),
                        exprs: Vec::new(),
                        steps: Vec::new(),
                        sections: HashMap::new(),
                    });
                }
                Event::Text(text) => {
                    if let Some(frame) = stack.last_mut() {
                        frame.text.push_str(&text);
                    }
                }
                Event::End { .. } => {
                    let Some(frame) = stack.pop() else { continue };
                    self.close(frame, &mut stack, path, depth, seen)?;
                }
            }
        }
        Ok(())
    }

    /// Handle one element now that its text and children are complete.
    fn close(
        &mut self,
        frame: Frame,
        stack: &mut [Frame],
        path: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        let body = frame.text.trim();
        match frame.name.as_str() {
            "dir" | "cachedir" | "include" | "remap-dir" => {
                let salt = attr(&frame, "salt");
                let as_path = attr(&frame, "as-path");
                // A `<remap-dir>` without an `as-path` says nothing, and
                // fontconfig warns and drops it rather than treating it as a
                // plain directory.
                if frame.name == "remap-dir" && as_path.is_none() {
                    return Ok(());
                }
                self.apply(Applied {
                    element: &frame.name,
                    prefix: frame.prefix.as_deref(),
                    body,
                    salt,
                    as_path,
                    from: path,
                    depth,
                    seen,
                })?;
            }
            "glob" if !body.is_empty() => {
                // A glob is used as written, except for a leading `~`.
                let glob = match body.strip_prefix('~') {
                    Some(rest) => match home() {
                        Some(home) => format!("{}{rest}", home.display()),
                        None => return Ok(()),
                    },
                    None => body.to_string(),
                };
                self.selectors.globs_mut().push(glob);
            }
            // The value elements, including the four that build themselves
            // out of children.
            "string" | "int" | "double" | "bool" | "const" | "matrix" | "charset" | "langset"
            | "range" => {
                let Some(parent) = stack.last_mut() else { return Ok(()) };
                match parent.name.as_str() {
                    // A container collects its children: four numbers for a
                    // <matrix>, codepoints and spans for a <charset>, names
                    // for a <langset>, two numbers for a <range>.
                    "matrix" | "charset" | "langset" | "range" => {
                        parent.values.push(literal(&frame, body, Strictness::Poison));
                    }
                    // A <patelt> is a selector, and a value it cannot read
                    // has to poison it rather than be dropped.
                    "patelt" => {
                        parent.values.push(literal(&frame, body, Strictness::Poison));
                    }
                    // Anywhere else the same names are literals in a rule
                    // expression, where dropping what cannot be read is
                    // right: an edit with one unreadable value in it should
                    // still make the rest of its change.
                    _ => {
                        let value = literal(&frame, body, Strictness::Salvage);
                        parent.exprs.push(value_expr(value));
                    }
                }
            }
            "patelt" => {
                if let Some(parent) = stack.last_mut() {
                    match frame.object.as_deref().and_then(Object::from_name) {
                        Some(object) => parent.elements.push((object, frame.values)),
                        // A property name fontconfig assigns at runtime cannot
                        // be resolved here, so the selector must not narrow to
                        // its remaining elements.
                        None => parent.poisoned = true,
                    }
                }
            }
            // --- expressions, tests, edits, matches --------------------
            //
            // The literal element names overlap with <patelt>'s, so the arm
            // above claims them when the parent is a selector; anything else
            // reaching here is part of a rule.
            "match" => {
                if !frame.steps.is_empty() {
                    let kind = MatchKind::parse(frame.attr("target"));
                    self.rules.push(Rule { kind, steps: frame.steps });
                }
            }
            "test" => {
                // Any name works: one a config invented becomes a scratch
                // property rather than being dropped.
                let Some(object) = frame.object.as_deref().map(Property::parse) else {
                    return Ok(());
                };
                let Some(compare) = Compare::parse(frame.attr("compare")) else {
                    return Ok(());
                };
                // A test reads whichever pattern `target` names, defaulting
                // to the enclosing match's own target.
                let kind = match frame.attr("target") {
                    Some(target) => MatchKind::parse(Some(target)),
                    None => enclosing_match_kind(stack),
                };
                let test = Test {
                    kind,
                    qual: Qual::parse(frame.attr("qual")),
                    object,
                    compare,
                    expr: frame.expr(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.steps.push(Step::Test(test));
                }
            }
            "edit" => {
                let Some(object) = frame.object.as_deref().map(Property::parse) else {
                    return Ok(());
                };
                let Some(mode) = EditMode::parse(frame.attr("mode")) else {
                    return Ok(());
                };
                let edit = Edit {
                    object,
                    mode,
                    binding: parse_binding(frame.attr("binding")),
                    expr: frame.expr(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.steps.push(Step::Edit(edit));
                }
            }
            "alias" => {
                // <family> children land in `exprs`; the three sections each
                // collect their own into one expression.
                let family = match frame.exprs.len() {
                    0 => return Ok(()),
                    1 => frame.exprs[0].clone(),
                    _ => Expr::List(frame.exprs.clone()),
                };
                let rule = Rule::from_alias(
                    family,
                    frame.sections.get("prefer").cloned(),
                    frame.sections.get("accept").cloned(),
                    frame.sections.get("default").cloned(),
                    parse_binding(frame.attr("binding")),
                );
                if let Some(rule) = rule {
                    self.rules.push(rule);
                }
            }
            "prefer" | "accept" | "default" => {
                if let Some(parent) = stack.last_mut() {
                    let expr = match frame.exprs.len() {
                        0 => return Ok(()),
                        1 => frame.exprs[0].clone(),
                        _ => Expr::List(frame.exprs.clone()),
                    };
                    parent.sections.insert(frame.name.clone(), expr);
                }
            }
            "family" => {
                if let Some(parent) = stack.last_mut() {
                    parent.exprs.push(Expr::Value(OwnedValue::String(body.to_string())));
                }
            }
            "name" => {
                if let Some(parent) = stack.last_mut() {
                    let kind = MatchKind::parse_field(frame.attr("target"));
                    parent.exprs.push(Expr::Field(kind, Property::parse(body)));
                }
            }
            "or" | "and" | "eq" | "not_eq" | "less" | "less_eq" | "more" | "more_eq"
            | "contains" | "not_contains" | "plus" | "minus" | "times" | "divide" => {
                let op = binary_op(&frame.name);
                // Fontconfig folds a run of operands left to right, so
                // <plus> with three children adds all three.
                let expr = frame
                    .exprs
                    .iter()
                    .cloned()
                    .reduce(|a, b| Expr::Binary(op, Box::new(a), Box::new(b)))
                    .unwrap_or(Expr::Unknown);
                if let Some(parent) = stack.last_mut() {
                    parent.exprs.push(expr);
                }
            }
            "not" | "floor" | "ceil" | "round" | "trunc" => {
                let op = unary_op(&frame.name);
                let expr = match frame.exprs.first() {
                    Some(inner) => Expr::Unary(op, Box::new(inner.clone())),
                    None => Expr::Unknown,
                };
                if let Some(parent) = stack.last_mut() {
                    parent.exprs.push(expr);
                }
            }
            "if" => {
                let expr = match frame.exprs.as_slice() {
                    [c, t, e] => {
                        Expr::If(Box::new(c.clone()), Box::new(t.clone()), Box::new(e.clone()))
                    }
                    _ => Expr::Unknown,
                };
                if let Some(parent) = stack.last_mut() {
                    parent.exprs.push(expr);
                }
            }
            "pattern" if !frame.elements.is_empty() => {
                let selector = Selector { elements: frame.elements, usable: !frame.poisoned };
                self.selectors.patterns_mut().push(selector);
            }
            _ => {}
        }
        Ok(())
    }

    fn apply(&mut self, a: Applied<'_>) -> Result<(), ConfigError> {
        let Applied { element, prefix, body, salt, as_path, from, depth, seen } = a;
        if body.is_empty() {
            return Ok(());
        }
        if element == "include" {
            for path in include_paths(body, prefix, from) {
                self.read_include(&path, depth, seen)?;
            }
            return Ok(());
        }

        // The bases differ per element: a font directory can also come from
        // the shared XDG data directories, while the cache has a single home.
        let bases = match (element, prefix) {
            ("dir" | "remap-dir", Some("xdg")) => {
                let mut bases = vec![xdg_data_home()];
                bases.extend(xdg_data_dirs());
                bases
            }
            ("cachedir", Some("xdg")) => vec![xdg_cache_home()],
            (_, Some("relative")) => vec![from.parent().map(Path::to_path_buf)],
            _ => vec![None],
        };

        for base in bases {
            let Some(path) = resolve(body, base.as_deref(), home().as_deref()) else {
                continue;
            };
            match element {
                "dir" | "remap-dir" => {
                    let entry = FontDir {
                        path,
                        map: as_path.map(PathBuf::from),
                        salt: salt.map(str::to_string),
                    };
                    if !self.font_dirs.iter().any(|d| d.path == entry.path) {
                        self.font_dirs.push(entry);
                    }
                }
                "cachedir" => push_unique(&mut self.cache_dirs, path),
                _ => {}
            }
        }
        Ok(())
    }

    /// An include names either a file or a directory of `.conf` files.
    fn read_include(
        &mut self,
        path: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        if path.is_dir() {
            // Order matters: the numeric prefixes on conf.d files are there to
            // sequence the rules, so read them sorted by name.
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .map_err(|e| ConfigError::Io(path.to_path_buf(), e))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "conf"))
                .collect();
            entries.sort();
            for entry in entries {
                self.read_file(&entry, depth + 1, seen)?;
            }
            return Ok(());
        }
        if path.is_file() {
            self.read_file(path, depth + 1, seen)?;
        }
        // A missing include is not an error. Fontconfig marks most of them
        // `ignore_missing="yes"` and warns rather than failing on the rest,
        // and a config that names an absent optional file is entirely normal.
        Ok(())
    }
}

/// Iterator over every cache a [`Config`] reaches.
///
/// Directories are visited breadth-first from the configured roots, following
/// the subdirectory list each cache carries, in the order the cache lists
/// them. The order is not cosmetic: matching breaks exact ties by taking the
/// font it saw first, so it has to agree with the order fontconfig builds its
/// own font set in.
pub struct Caches<'a> {
    config: &'a Config,
    pending: VecDeque<String>,
    seen: HashSet<String, BuildFnv>,
}

impl Iterator for Caches<'_> {
    type Item = (String, Cache);

    fn next(&mut self) -> Option<(String, Cache)> {
        while let Some(dir) = self.pending.pop_front() {
            if !self.seen.insert(dir.clone()) {
                continue;
            }
            let Some(path) = self.config.cache_path(&dir) else {
                continue;
            };
            let Ok(cache) = Cache::open(&path) else {
                continue;
            };
            if let Ok(subdirs) = cache.subdirs() {
                for subdir in subdirs.flatten() {
                    // A rejected directory prunes the walk, the same way
                    // fontconfig filters subdirectories as it descends.
                    if !self.seen.contains(subdir) && self.config.accepts_filename(subdir) {
                        self.pending.push_back(subdir.to_string());
                    }
                }
            }
            return Some((dir, cache));
        }
        None
    }
}

/// Where an `<include>` body could resolve to, in the order to try them.
///
/// This is not "relative to the including file", which is the intuitive rule
/// and the wrong one. Fontconfig looks a bare relative include up on a search
/// path — `FONTCONFIG_PATH` then the built-in config directory — so
/// `<include>conf.d</include>` in `/etc/fonts/fonts.conf` finds
/// `/etc/fonts/conf.d` no matter what the process working directory is. Only
/// `prefix="relative"` means relative to the including file.
fn include_paths(body: &str, prefix: Option<&str>, from: &Path) -> Vec<PathBuf> {
    if body.starts_with('~') {
        return resolve(body, None, home().as_deref()).into_iter().collect();
    }
    if Path::new(body).is_absolute() {
        return vec![PathBuf::from(body)];
    }
    match prefix {
        Some("xdg") => xdg_config_home().map(|b| b.join(body)).into_iter().collect(),
        Some("relative") => from.parent().map(|b| b.join(body)).into_iter().collect(),
        _ => config_path().into_iter().map(|base| base.join(body)).collect(),
    }
}

/// The search path for a relative configuration file: `FONTCONFIG_PATH`
/// entries first, then the built-in configuration directory.
fn config_path() -> Vec<PathBuf> {
    let mut path: Vec<PathBuf> = std::env::var("FONTCONFIG_PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    path.push(PathBuf::from(CONFIG_DIR));
    path
}

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

fn path_to_string(path: &Path) -> Option<String> {
    path.to_str().map(str::to_owned)
}

/// Join `body` onto `base`, expanding a leading `~` against `home`.
///
/// `home` is a parameter rather than read from the environment here so that
/// it can be tested without mutating a process-wide variable.
fn resolve(body: &str, base: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = body.strip_prefix('~') {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return Some(home?.join(rest));
    }
    Some(match base {
        Some(base) => base.join(body),
        None => PathBuf::from(body),
    })
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// An XDG base directory: the environment variable if it is an absolute path,
/// otherwise the default beneath the home directory.
fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(value) if Path::new(&value).is_absolute() => Some(PathBuf::from(value)),
        _ => Some(home()?.join(fallback)),
    }
}

fn xdg_data_home() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

fn xdg_cache_home() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

fn xdg_config_home() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// The shared data directories, which `<dir prefix="xdg">` also searches.
fn xdg_data_dirs() -> Vec<Option<PathBuf>> {
    let value = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let value = if value.is_empty() { "/usr/local/share:/usr/share".to_string() } else { value };
    value.split(':').filter(|p| !p.is_empty()).map(|p| Some(PathBuf::from(p))).collect()
}

/// Where fontconfig looks for its root configuration file.
fn default_config_path() -> PathBuf {
    if let Some(file) = std::env::var_os("FONTCONFIG_FILE") {
        let path = PathBuf::from(file);
        if path.is_absolute() {
            return path;
        }
        return config_dir().join(path);
    }
    config_dir().join("fonts.conf")
}

fn config_dir() -> PathBuf {
    config_path().into_iter().next().unwrap_or_else(|| PathBuf::from(CONFIG_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Config {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules").join(name);
        Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The rule AST is crate-internal, so what it parses into is checked
    /// here rather than in `tests/rules.rs`, which sees only the effects.
    #[test]
    fn the_fixture_parses_into_rules() {
        let config = fixture("fonts.conf");
        assert_eq!(config.rules().len(), 12);
        let scan = config.rules().iter().filter(|r| r.kind == MatchKind::Scan).count();
        assert_eq!(scan, 2);
        assert_eq!(config.rules().len() - scan, 10);
    }

    /// A `<name>` with no `target` means the pattern being edited, which is
    /// not the same as `target="pattern"`. Reading the query instead makes a
    /// font-target rule compute from the wrong side.
    #[test]
    fn a_bare_name_reads_the_pattern_being_edited() {
        let config = fixture("custom.conf");
        let bare = config.rules().iter().flat_map(|r| &r.steps).find_map(|step| match step {
            Step::Edit(edit) => match &edit.expr {
                Expr::Binary(_, left, _) => match **left {
                    Expr::Field(kind, _) => Some(kind),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        });
        assert_eq!(bare, Some(MatchKind::Default), "a bare <name> must not be Pattern");
        assert_eq!(
            Property::parse("pixelsizefixupfactor"),
            Property::Custom("pixelsizefixupfactor".into())
        );
        assert_eq!(Property::parse("family"), Property::Known(Object::Family));
    }

    /// Verified against `md5sum` and against the real file names in
    /// `~/.cache/fontconfig` on the machine this was developed on.
    #[test]
    fn cache_basenames_match_fontconfigs_own() {
        assert_eq!(
            Config::default().cache_basename("/usr/share/fonts"),
            "3830d5c3ddfd5cd38a049b759396e72e-le64.cache-9"
        );
        assert_eq!(
            Config::default().cache_basename("/usr/share/fonts/abattis-cantarell-vf-fonts"),
            "18f520a508f13854f77176faf7889ae9-le64.cache-9"
        );
    }

    #[test]
    fn tilde_expands_against_home() {
        let home = Some(Path::new("/home/test"));
        assert_eq!(resolve("~/.fonts", None, home), Some("/home/test/.fonts".into()));
        assert_eq!(resolve("~", None, home), Some("/home/test".into()));
        // With no home there is nothing to expand against, so the element is
        // dropped rather than resolving to a bare relative path.
        assert_eq!(resolve("~/.fonts", None, None), None);
    }

    #[test]
    fn a_prefix_joins_but_an_absolute_path_does_not() {
        let base = Some(Path::new("/x/share"));
        assert_eq!(resolve("fonts", base, None), Some("/x/share/fonts".into()));
        assert_eq!(resolve("/usr/share/fonts", None, None), Some("/usr/share/fonts".into()));
    }
}

#[cfg(test)]
mod uuid_tests {
    use super::uuid_name;
    use std::path::Path;

    fn dir(name: &str, contents: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fontconf-uuid-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(text) = contents {
            std::fs::write(dir.join(".uuid"), text).unwrap();
        }
        dir
    }

    #[test]
    fn a_directory_without_a_uuid_asks_for_no_name() {
        assert_eq!(uuid_name(&dir("none", None)), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_uuid_names_the_cache_after_itself() {
        let path = dir("good", Some("4b5a9f2e-8c31-4d7a-9e0f-1a2b3c4d5e6f"));
        assert_eq!(
            uuid_name(&path).as_deref(),
            Some("4b5a9f2e-8c31-4d7a-9e0f-1a2b3c4d5e6f-le64.cache-9")
        );
    }

    /// Fontconfig does not look for a `.uuid` on Windows, so neither do we:
    /// the point of the name is that both find the same file.
    #[cfg(windows)]
    #[test]
    fn windows_does_not_use_uuid_names() {
        let path = dir("good", Some("4b5a9f2e-8c31-4d7a-9e0f-1a2b3c4d5e6f"));
        assert_eq!(uuid_name(&path), None);
    }

    /// Fontconfig reads exactly 36 bytes and stops, so the trailing newline a
    /// text editor leaves behind is not part of the name.
    #[cfg(not(windows))]
    #[test]
    fn a_trailing_newline_is_not_part_of_the_name() {
        let with = dir("newline", Some("4b5a9f2e-8c31-4d7a-9e0f-1a2b3c4d5e6f\n"));
        let without = dir("bare", Some("4b5a9f2e-8c31-4d7a-9e0f-1a2b3c4d5e6f"));
        assert_eq!(uuid_name(&with), uuid_name(&without));
    }

    /// Anything shorter than a formatted UUID is not one.
    #[cfg(not(windows))]
    #[test]
    fn a_short_uuid_is_refused() {
        assert_eq!(uuid_name(&dir("short", Some("4b5a9f2e"))), None);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        assert_eq!(uuid_name(Path::new("/no/such/directory/anywhere")), None);
    }
}
