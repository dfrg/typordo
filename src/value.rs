//! The values a pattern holds, owned and borrowed.
//!
//! [`Value`] owns its strings and sets; [`ValueRef`] is the same shapes read
//! out of a cache, borrowing from its buffer.

use crate::bytes::Bytes;
use crate::charset::{AnyCharSet, CharSet, CharSetRef};
use crate::error::{Error, Result};
use crate::langset::{AnyLangSet, LangSet, LangSetRef};

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
    Bool(bool),
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

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
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
        4 => ValueRef::Bool(data.i32(union)? != 0),
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
    Bool(bool),
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
