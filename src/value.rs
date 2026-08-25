//! The values a pattern holds, owned and borrowed.
//!
//! [`Value`] owns its strings and sets; [`ValueRef`] is the same shapes read
//! out of a cache, borrowing from its buffer.

use crate::bytes::Bytes;
use crate::charset::{AnyCharSet, CharSet, CharSetRef};
use crate::error::{Error, Result};
use crate::langset::{AnyLangSet, LangSet, LangSetRef};
use crate::object::ValueType;

/// A boolean with a third state, fontconfig's `FcBool`.
///
/// `FcDontCare` is not padding. It means "either answer will do", and it
/// changes two things that a two-valued flag cannot express:
/// `FcCompareBool` scores it as a match whichever way the font answers *and*
/// keeps the font's value rather than imposing the pattern's, and
/// `FcConfigCompareValue` reads the ordering operators as questions about it
/// -- `less` asks whether the right side is `DontCare` and the two differ.
///
/// A configuration writes it as `<bool>dontcare</bool>`; `FcNameBool` also
/// accepts `d`, `x`, `2` and `or`. Nothing that scans a font produces one, so
/// it reaches a pattern only from a configuration or a caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Tristate {
    /// No.
    #[default]
    False = 0,
    /// Yes.
    True = 1,
    /// Either.
    DontCare = 2,
}

impl Tristate {
    /// The value as fontconfig stores it: 0, 1 or 2.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Read one back, the way a cache holds it.
    ///
    /// Fontconfig stores an `FcBool` in the value union as an integer and
    /// never writes anything but 0, 1 or 2. Anything else is read as
    /// `DontCare`, which is the state that claims the least.
    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::False,
            1 => Self::True,
            _ => Self::DontCare,
        }
    }

    /// Read one the way `FcNameBool` does.
    ///
    /// The first letter decides: `true`, `True`, `yes`, `on` and `1` are
    /// true; `false`, `no`, `off` and `0` are false; `dontcare`, `x`, `2` and
    /// `or` are [`DontCare`](Tristate::DontCare). `None` for anything else,
    /// which fontconfig warns about and then treats as false.
    ///
    /// This is what reads `<bool>` in a configuration and `:scalable=True` in
    /// a name, so the two cannot disagree about a spelling.
    pub fn parse(value: &str) -> Option<Self> {
        let mut chars = value.chars().map(|c| c.to_ascii_lowercase());
        match chars.next()? {
            't' | 'y' | '1' => Some(Self::True),
            'f' | 'n' | '0' => Some(Self::False),
            'd' | 'x' | '2' => Some(Self::DontCare),
            // `on`, `off` and `or` all start the same way.
            'o' => match chars.next()? {
                'n' => Some(Self::True),
                'f' => Some(Self::False),
                'r' => Some(Self::DontCare),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether this is a definite yes or no.
    ///
    /// `None` for [`DontCare`](Tristate::DontCare), which is neither.
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::False => Some(false),
            Self::True => Some(true),
            Self::DontCare => None,
        }
    }
}

impl From<bool> for Tristate {
    fn from(value: bool) -> Self {
        if value {
            Self::True
        } else {
            Self::False
        }
    }
}

/// The spellings `fc-list` and friends print.
impl std::fmt::Display for Tristate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::False => "False",
            Self::True => "True",
            Self::DontCare => "DontCare",
        })
    }
}

/// A 2x2 transform, fontconfig's `FcMatrix`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    /// Horizontal scale.
    pub xx: f64,
    /// Horizontal shear.
    pub xy: f64,
    /// Vertical shear.
    pub yx: f64,
    /// Vertical scale.
    pub yy: f64,
}

impl Matrix {
    /// The transform that changes nothing.
    ///
    /// `FcIdentityMatrix`, which is what an absent value promotes to when it
    /// is compared against a matrix.
    pub const IDENTITY: Self = Self { xx: 1.0, xy: 0.0, yx: 0.0, yy: 1.0 };
}

/// An inclusive span of numbers, fontconfig's `FcRange`.
///
/// Weight, width and size are stored as ranges rather than scalars so that a
/// variable font can describe the axis it actually covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    /// Lowest value in the span, inclusive.
    pub begin: f64,
    /// Highest value in the span, inclusive.
    pub end: f64,
}

impl Range {
    /// True when the range covers exactly one value.
    pub fn is_scalar(&self) -> bool {
        self.begin == self.end
    }

    /// The range covering just `value`.
    ///
    /// `FcRangePromote`: what a number becomes when it is compared against a
    /// range.
    pub fn single(value: f64) -> Self {
        Self { begin: value, end: value }
    }

    /// Whether this range falls entirely inside `other`, ends included.
    ///
    /// `FcRangeIsInRange`, the test behind `contains` between two ranges.
    pub fn within(&self, other: &Self) -> bool {
        self.begin >= other.begin && self.end <= other.end
    }

    /// Whether `value` falls inside, ends included.
    pub fn contains(&self, value: f64) -> bool {
        self.begin <= value && value <= self.end
    }
}

/// One value held against a property.
///
/// Strings borrow directly out of the cache buffer — reading a family name
/// costs a bounds check and a UTF-8 validation, no allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueRef<'a> {
    /// Present but empty. Fontconfig uses this as a deletion tombstone.
    Void,
    /// A whole number.
    Int(i32),
    /// A real number.
    Double(f64),
    /// Text, borrowed from the cache buffer.
    String(&'a str),
    /// A flag.
    Bool(Tristate),
    /// A 2x2 transform to apply to the face.
    Matrix(Matrix),
    /// The characters a font covers.
    CharSet(AnyCharSet<'a>),
    /// The languages a font can write.
    LangSet(AnyLangSet<'a>),
    /// A span of numbers, as a variable axis reports its extent.
    Range(Range),
}

impl<'a> ValueRef<'a> {
    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&'a str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The integer, if this is one.
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The number, widened, whether it was stored as an int, a double or a
    /// scalar range.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(f64::from(*i)),
            Self::Double(d) => Some(*d),
            Self::Range(r) if r.is_scalar() => Some(r.begin),
            _ => None,
        }
    }

    /// The flag, if this is one.
    pub fn as_tristate(&self) -> Option<Tristate> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The flag as a definite yes or no.
    ///
    /// `None` both when this is not a flag and when it is
    /// [`Tristate::DontCare`], which is neither answer. Use
    /// [`as_tristate`](ValueRef::as_tristate) to tell those apart.
    pub fn as_bool(&self) -> Option<bool> {
        self.as_tristate()?.as_bool()
    }
}

/// How strongly a value is held, fontconfig's `FcValueBinding`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Set by the font itself; a weak value cannot displace it.
    Strong,
    /// Contributed by configuration, and yields to a strong value.
    Weak,
    /// Same as the value it was derived from.
    Same,
}

use crate::layout::NATIVE as L;

pub(crate) fn binding_at(data: Bytes<'_>, node: usize) -> Result<Binding> {
    Ok(match data.i32(node + L.binding)? {
        1 => Binding::Weak,
        2 => Binding::Same,
        _ => Binding::Strong,
    })
}

/// Decode the `FcValue` at `at`.
///
/// Offsets inside a value are relative to the value itself, not to the field
/// holding them — `FcValueString` in `fcint.h` passes the whole `FcValue` as
/// the base.
/// Check that the value at `at` is structurally sound, without reading it.
///
/// What `FcCacheOffsetsValid` does per value: the type tag has to be one it
/// knows, and an indirect type's offset has to land inside the file. It does
/// not decode the string, and neither does this -- validating UTF-8 for every
/// string in every cache is most of the cost of a full read, and buys nothing
/// the read itself will not catch when it happens.
pub(crate) fn check_at(data: Bytes<'_>, at: usize) -> Result<()> {
    let union = at + L.union;
    match data.i32(at)? {
        0 | 1 | 4 => {}
        2 => {
            data.f64(union)?;
        }
        // Indirect: the offset has to resolve, and that is all.
        3 | 5 | 6 | 8 | 9 => {
            data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
        }
        other => return Err(Error::BadCount(other)),
    }
    Ok(())
}

pub(crate) fn value_at<'a>(data: Bytes<'a>, at: usize) -> Result<ValueRef<'a>> {
    let union = at + L.union;
    Ok(match data.i32(at)? {
        0 => ValueRef::Void,
        1 => ValueRef::Int(data.i32(union)?),
        2 => ValueRef::Double(data.f64(union)?),
        3 => {
            let s = data.follow(at, union)?.ok_or(Error::BadString(union))?;
            ValueRef::String(data.str(s)?)
        }
        4 => ValueRef::Bool(Tristate::from_i32(data.i32(union)?)),
        5 => {
            let m = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            ValueRef::Matrix(Matrix {
                xx: data.f64(m)?,
                xy: data.f64(m + 8)?,
                yx: data.f64(m + 16)?,
                yy: data.f64(m + 24)?,
            })
        }
        6 => {
            let c = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            ValueRef::CharSet(AnyCharSet::Cached(CharSetRef { data, at: c }))
        }
        // 7 is FcTypeFTFace, a live FT_Face pointer. It cannot be serialized
        // and must never appear in a file.
        8 => {
            let l = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            ValueRef::LangSet(AnyLangSet::Cached(LangSetRef { data, at: l }))
        }
        9 => {
            let r = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            ValueRef::Range(Range { begin: data.f64(r)?, end: data.f64(r + 8)? })
        }
        other => return Err(Error::BadCount(other)),
    })
}

/// A value a pattern holds.
///
/// The same shapes as [`ValueRef`], owning what that one borrows: its
/// strings, and its character and language sets.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Present but empty.
    Void,
    /// A whole number.
    Int(i32),
    /// A real number.
    Double(f64),
    /// Owned text.
    String(String),
    /// A flag.
    Bool(Tristate),
    /// A 2x2 transform.
    Matrix(Matrix),
    /// A span of numbers.
    Range(Range),
    /// The characters a font covers, built by scanning it.
    CharSet(CharSet),
    /// The languages a font can write, built by scanning it.
    LangSet(LangSet),
}

impl Value {
    /// Borrow this as a [`ValueRef`], so query and font values compare uniformly.
    pub fn as_value(&self) -> ValueRef<'_> {
        match self {
            Self::Void => ValueRef::Void,
            Self::Int(i) => ValueRef::Int(*i),
            Self::Double(d) => ValueRef::Double(*d),
            Self::String(s) => ValueRef::String(s),
            Self::Bool(b) => ValueRef::Bool(*b),
            Self::Matrix(m) => ValueRef::Matrix(*m),
            Self::Range(r) => ValueRef::Range(*r),
            Self::CharSet(c) => ValueRef::CharSet(AnyCharSet::Owned(c)),
            Self::LangSet(l) => ValueRef::LangSet(AnyLangSet::Owned(l)),
        }
    }
}

impl Value {
    /// Copy a borrowed value, so it outlives the cache it came from.
    pub fn from_value(value: &ValueRef<'_>) -> Self {
        match value {
            ValueRef::Void => Self::Void,
            ValueRef::Int(i) => Self::Int(*i),
            ValueRef::Double(d) => Self::Double(*d),
            ValueRef::String(s) => Self::String(s.to_string()),
            ValueRef::Bool(b) => Self::Bool(*b),
            ValueRef::Matrix(m) => Self::Matrix(*m),
            ValueRef::Range(r) => Self::Range(*r),
            ValueRef::CharSet(c) => {
                let mut coverage = CharSet::new();
                coverage.merge_chars(c);
                Self::CharSet(coverage)
            }
            ValueRef::LangSet(l) => Self::LangSet(LangSet::from_languages(l)),
        }
    }
}

impl Value {
    /// Which kind of value this is, or `None` for [`Value::Void`].
    ///
    /// Void has no kind because it is not one a property can hold:
    /// `FcPatternObjectAddWithBinding` throws it away before it looks at the
    /// type at all, so a `Void` never reaches a pattern.
    pub fn kind(&self) -> Option<ValueType> {
        Some(match self {
            Self::Void => return None,
            Self::Int(_) => ValueType::Int,
            Self::Double(_) => ValueType::Double,
            Self::String(_) => ValueType::String,
            Self::Bool(_) => ValueType::Bool,
            Self::Matrix(_) => ValueType::Matrix,
            Self::Range(_) => ValueType::Range,
            Self::CharSet(_) => ValueType::CharSet,
            Self::LangSet(_) => ValueType::LangSet,
        })
    }
}

impl From<Matrix> for Value {
    fn from(value: Matrix) -> Self {
        Self::Matrix(value)
    }
}

impl From<Range> for Value {
    fn from(value: Range) -> Self {
        Self::Range(value)
    }
}

impl From<CharSet> for Value {
    fn from(value: CharSet) -> Self {
        Self::CharSet(value)
    }
}

impl From<LangSet> for Value {
    fn from(value: LangSet) -> Self {
        Self::LangSet(value)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self::Int(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::Double(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v.into())
    }
}

impl From<Tristate> for Value {
    fn from(v: Tristate) -> Self {
        Self::Bool(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

#[cfg(test)]
mod tristate_tests {
    use super::Tristate;

    /// `FcNameBool`, which is what reads both `<bool>` in a configuration and
    /// `:scalable=True` in a name. Those were two functions once, and drifted:
    /// the config one learned `DontCare` and the name one did not, so a query
    /// written `dontcare` arrived as `false`.
    #[test]
    fn every_spelling_fontconfig_accepts() {
        for yes in ["true", "True", "TRUE", "yes", "y", "on", "1"] {
            assert_eq!(Tristate::parse(yes), Some(Tristate::True), "{yes}");
        }
        for no in ["false", "False", "no", "n", "off", "0"] {
            assert_eq!(Tristate::parse(no), Some(Tristate::False), "{no}");
        }
        for either in ["dontcare", "DontCare", "d", "x", "2", "or"] {
            assert_eq!(Tristate::parse(either), Some(Tristate::DontCare), "{either}");
        }
        for bad in ["", "bogus", "maybe", "o", "q"] {
            assert_eq!(Tristate::parse(bad), None, "{bad:?}");
        }
    }

    /// Fontconfig stores the flag as an integer and reads it back as one.
    #[test]
    fn it_round_trips_through_the_integer_a_cache_holds() {
        for flag in [Tristate::False, Tristate::True, Tristate::DontCare] {
            assert_eq!(Tristate::from_i32(flag.as_i32()), flag);
        }
        assert_eq!(Tristate::from_i32(0), Tristate::False);
        assert_eq!(Tristate::from_i32(1), Tristate::True);
        assert_eq!(Tristate::from_i32(2), Tristate::DontCare);
        // Fontconfig never writes anything else; whatever it is, it is not a
        // definite answer.
        assert_eq!(Tristate::from_i32(7), Tristate::DontCare);
        assert_eq!(Tristate::from_i32(-1), Tristate::DontCare);
    }

    /// The spellings `fc-list` prints, which `%{scalable}` and friends are
    /// compared against.
    #[test]
    fn it_prints_the_way_fc_list_prints() {
        assert_eq!(Tristate::True.to_string(), "True");
        assert_eq!(Tristate::False.to_string(), "False");
        assert_eq!(Tristate::DontCare.to_string(), "DontCare");
    }

    /// `DontCare` is neither answer, and saying so is the point of the type.
    #[test]
    fn dontcare_is_not_a_yes_or_a_no() {
        assert_eq!(Tristate::True.as_bool(), Some(true));
        assert_eq!(Tristate::False.as_bool(), Some(false));
        assert_eq!(Tristate::DontCare.as_bool(), None);
    }
}
