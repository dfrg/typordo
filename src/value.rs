use crate::bytes::Bytes;
use crate::charset::CharSet;
use crate::langset::LangSet;
use crate::error::{Error, Result};

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
}

/// One value held against a property.
///
/// Strings borrow directly out of the cache buffer — reading a family name
/// costs a bounds check and a UTF-8 validation, no allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
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
    CharSet(CharSet<'a>),
    /// The languages a font can write.
    LangSet(LangSet<'a>),
    /// A span of numbers, as a variable axis reports its extent.
    Range(Range),
}

impl<'a> Value<'a> {
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

const UNION: usize = 8;

/// `FcValueList` is `next` (8), `value` (16), `binding` (4), padded to 32.
pub(crate) const NODE_SIZE: usize = 32;
pub(crate) const NODE_VALUE: usize = 8;
const NODE_BINDING: usize = 24;

pub(crate) fn binding_at(data: Bytes<'_>, node: usize) -> Result<Binding> {
    Ok(match data.i32(node + NODE_BINDING)? {
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
pub(crate) fn value_at<'a>(data: Bytes<'a>, at: usize) -> Result<Value<'a>> {
    let union = at + UNION;
    Ok(match data.i32(at)? {
        0 => Value::Void,
        1 => Value::Int(data.i32(union)?),
        2 => Value::Double(data.f64(union)?),
        3 => {
            let s = data.follow(at, union)?.ok_or(Error::BadString(union))?;
            Value::String(data.str(s)?)
        }
        4 => Value::Bool(data.i32(union)? != 0),
        5 => {
            let m = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            Value::Matrix(Matrix {
                xx: data.f64(m)?,
                xy: data.f64(m + 8)?,
                yx: data.f64(m + 16)?,
                yy: data.f64(m + 24)?,
            })
        }
        6 => {
            let c = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            Value::CharSet(CharSet { data, at: c })
        }
        // 7 is FcTypeFTFace, a live FT_Face pointer. It cannot be serialized
        // and must never appear in a file.
        8 => {
            let l = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            Value::LangSet(LangSet { data, at: l })
        }
        9 => {
            let r = data.follow(at, union)?.ok_or(Error::BadOffset { base: at, delta: 0 })?;
            Value::Range(Range { begin: data.f64(r)?, end: data.f64(r + 8)? })
        }
        other => return Err(Error::BadCount(other)),
    })
}
