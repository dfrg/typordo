//! A pattern you can build and modify: the query side of matching.
//!
//! [`Pattern`](crate::Pattern) is a cursor into a cache and is therefore
//! read-only and borrowed. A query is the other thing fontconfig calls an
//! `FcPattern`: something a caller assembles, that configuration rewrites,
//! and that gets scored against every font.

use std::fmt;

use crate::object::Object;
use crate::value::{Binding, Matrix, Range, Value};

/// A value a query can hold.
///
/// The same shapes as [`Value`], but owning its strings. Character sets and
/// language sets are not representable yet, so a query cannot carry one.
#[derive(Clone, Debug, PartialEq)]
pub enum OwnedValue {
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
}

impl OwnedValue {
    /// Borrow this as a [`Value`], so query and font values compare uniformly.
    pub fn as_value(&self) -> Value<'_> {
        match self {
            Self::Void => Value::Void,
            Self::Int(i) => Value::Int(*i),
            Self::Double(d) => Value::Double(*d),
            Self::String(s) => Value::String(s),
            Self::Bool(b) => Value::Bool(*b),
            Self::Matrix(m) => Value::Matrix(*m),
            Self::Range(r) => Value::Range(*r),
        }
    }
}

impl From<i32> for OwnedValue {
    fn from(v: i32) -> Self {
        Self::Int(v)
    }
}

impl From<f64> for OwnedValue {
    fn from(v: f64) -> Self {
        Self::Double(v)
    }
}

impl From<bool> for OwnedValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<&str> for OwnedValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}

impl From<String> for OwnedValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

/// A pattern being built up and matched against.
///
/// Properties are kept sorted by [`Object::id`], which is the order the cache
/// stores them in and the order scoring walks them in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    elements: Vec<Element>,
}

/// One property of a query, with every value held against it.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    object: Object,
    values: Vec<(OwnedValue, Binding)>,
}

impl Element {
    /// The property this element describes.
    pub fn object(&self) -> Object {
        self.object
    }

    /// The values held against it, in order.
    pub fn values(&self) -> impl Iterator<Item = (&OwnedValue, Binding)> {
        self.values.iter().map(|(v, b)| (v, *b))
    }
}

impl Query {
    /// An empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a strongly-bound value.
    ///
    /// A strong value came from the caller and outranks anything a
    /// configuration rule contributes.
    pub fn add(&mut self, object: Object, value: impl Into<OwnedValue>) -> &mut Self {
        self.add_with_binding(object, value, Binding::Strong)
    }

    /// Append a weakly-bound value.
    pub fn add_weak(&mut self, object: Object, value: impl Into<OwnedValue>) -> &mut Self {
        self.add_with_binding(object, value, Binding::Weak)
    }

    /// Append a value with an explicit binding.
    pub fn add_with_binding(
        &mut self,
        object: Object,
        value: impl Into<OwnedValue>,
        binding: Binding,
    ) -> &mut Self {
        let value = value.into();
        match self.position(object) {
            Ok(at) => self.elements[at].values.push((value, binding)),
            Err(at) => self.elements.insert(
                at,
                Element { object, values: vec![(value, binding)] },
            ),
        }
        self
    }

    /// Remove a property entirely.
    pub fn remove(&mut self, object: Object) -> bool {
        match self.position(object) {
            Ok(at) => {
                self.elements.remove(at);
                true
            }
            Err(_) => false,
        }
    }

    /// The element for `object`, if the query has one.
    pub fn get(&self, object: Object) -> Option<&Element> {
        self.position(object).ok().map(|at| &self.elements[at])
    }

    /// Whether the query holds `object` at all.
    pub fn contains(&self, object: Object) -> bool {
        self.position(object).is_ok()
    }

    /// The first value of `object`.
    pub fn value(&self, object: Object) -> Option<&OwnedValue> {
        self.get(object)?.values.first().map(|(v, _)| v)
    }

    /// The first value of `object` as a string.
    pub fn string(&self, object: Object) -> Option<&str> {
        match self.value(object)? {
            OwnedValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// The first value of `object` as a number, whatever it is stored as.
    pub fn number(&self, object: Object) -> Option<f64> {
        match self.value(object)? {
            OwnedValue::Int(i) => Some(f64::from(*i)),
            OwnedValue::Double(d) => Some(*d),
            OwnedValue::Range(r) => Some((r.begin + r.end) * 0.5),
            _ => None,
        }
    }

    /// Every property, ascending by object id.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.elements.iter()
    }

    /// How many properties the query carries.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Whether the query carries no properties.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    fn position(&self, object: Object) -> Result<usize, usize> {
        self.elements.binary_search_by_key(&object.id(), |e| e.object.id())
    }

    /// Set `object` to exactly this one value, replacing anything there.
    fn set(&mut self, object: Object, value: impl Into<OwnedValue>) {
        self.remove(object);
        self.add(object, value);
    }

    /// Fill in the values fontconfig assumes when a query does not say.
    ///
    /// This is `FcDefaultSubstitute`. It has to run before matching: a query
    /// that never mentions weight still has to score against every font's
    /// weight, and it does so as `normal`.
    ///
    /// Two of fontconfig's defaults are deliberately not applied here, since
    /// both name the calling process or its environment rather than the font:
    /// `prgname`, and the `namelang` fallback chain that fills `familylang`,
    /// `stylelang` and `fullnamelang`.
    pub fn default_substitute(&mut self) {
        if !self.contains(Object::Weight) {
            self.add(Object::Weight, 80); // FC_WEIGHT_NORMAL
        }
        if !self.contains(Object::Slant) {
            self.add(Object::Slant, 0); // FC_SLANT_ROMAN
        }
        if !self.contains(Object::Width) {
            self.add(Object::Width, 100); // FC_WIDTH_NORMAL
        }
        for (object, value) in [
            (Object::Hinting, true),
            (Object::VerticalLayout, false),
            (Object::Autohint, false),
            (Object::GlobalAdvance, true),
            (Object::EmbeddedBitmap, true),
            (Object::Decorative, false),
            (Object::Symbol, false),
            (Object::Variable, false),
        ] {
            if !self.contains(object) {
                self.add(object, value);
            }
        }

        // Size, scale and dpi determine pixelsize, and whichever of size and
        // pixelsize was not given is derived from the other.
        let size = self.number(Object::Size).unwrap_or(12.0);
        let scale = self.number(Object::Scale).unwrap_or(1.0);
        let dpi = self.number(Object::Dpi).unwrap_or(75.0);
        let size = match self.number(Object::PixelSize) {
            None => {
                self.set(Object::Scale, scale);
                self.set(Object::Dpi, dpi);
                self.add(Object::PixelSize, size * scale * dpi / 72.0);
                size
            }
            Some(pixelsize) => pixelsize / dpi * 72.0 / scale,
        };
        self.set(Object::Size, size);

        if !self.contains(Object::Fontversion) {
            self.add(Object::Fontversion, 0x7fff_ffff);
        }
        if !self.contains(Object::HintStyle) {
            self.add(Object::HintStyle, 3); // FC_HINT_FULL
        }
    }
}

impl fmt::Display for Query {
    /// Roughly the form `fc-match` accepts, for diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for element in &self.elements {
            for (value, _) in &element.values {
                if first {
                    first = false;
                } else {
                    f.write_str(":")?;
                }
                write!(f, "{}=", element.object)?;
                match value {
                    OwnedValue::String(s) => write!(f, "{s}")?,
                    OwnedValue::Int(i) => write!(f, "{i}")?,
                    OwnedValue::Double(d) => write!(f, "{d}")?,
                    OwnedValue::Bool(b) => write!(f, "{b}")?,
                    other => write!(f, "{other:?}")?,
                }
            }
        }
        Ok(())
    }
}
