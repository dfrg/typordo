//! A pattern you can build and modify: the query side of matching.
//!
//! [`Pattern`](crate::Pattern) is a cursor into a cache and is therefore
//! read-only and borrowed. A query is the other thing fontconfig calls an
//! `FcPattern`: something a caller assembles, that configuration rewrites,
//! and that gets scored against every font.

use std::fmt;

use crate::object::Object;
use crate::charset::{Chars, Coverage};
use crate::langset::{Langs, Languages};
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
    /// The characters a font covers, built by scanning it.
    CharSet(Coverage),
    /// The languages a font can write, built by scanning it.
    LangSet(Langs),
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
            Self::CharSet(c) => Value::CharSet(Chars::Owned(c)),
            Self::LangSet(l) => Value::LangSet(Languages::Owned(l)),
        }
    }
}

impl OwnedValue {
    /// Copy a borrowed value, so it outlives the cache it came from.
    pub fn from_value(value: &Value<'_>) -> Self {
        match value {
            Value::Void => Self::Void,
            Value::Int(i) => Self::Int(*i),
            Value::Double(d) => Self::Double(*d),
            Value::String(s) => Self::String(s.to_string()),
            Value::Bool(b) => Self::Bool(*b),
            Value::Matrix(m) => Self::Matrix(*m),
            Value::Range(r) => Self::Range(*r),
            Value::CharSet(c) => {
                let mut coverage = Coverage::new();
                coverage.merge_chars(c);
                Self::CharSet(coverage)
            }
            Value::LangSet(l) => Self::LangSet(Langs::from_languages(l)),
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

/// A property key: one of the built-in [`Object`]s, or a name a configuration
/// file invented.
///
/// Fontconfig lets a config assign to any name; unknown ones get ids above the
/// built-in range and act as scratch variables that rules pass between
/// themselves. `10-scale-bitmap-fonts.conf` computes `pixelsizefixupfactor`
/// this way and reads it back two rules later.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Property {
    /// One of the properties fontconfig defines.
    Known(Object),
    /// A name only this configuration knows.
    Custom(String),
}

impl Property {
    /// Resolve a name, preferring the built-in meaning.
    pub fn parse(name: &str) -> Self {
        match Object::from_name(name) {
            Some(object) => Self::Known(object),
            None => Self::Custom(name.to_string()),
        }
    }
}

impl From<Object> for Property {
    fn from(object: Object) -> Self {
        Self::Known(object)
    }
}

impl std::fmt::Display for Property {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(object) => object.fmt(f),
            Self::Custom(name) => f.write_str(name),
        }
    }
}

/// A pattern being built up and matched against.
///
/// Properties are kept sorted by [`Object::id`], which is the order the cache
/// stores them in and the order scoring walks them in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    elements: Vec<Element>,
    /// Properties a configuration invented, kept apart from the built-in ones
    /// because nothing scores against them: they exist only so rules can pass
    /// values to later rules.
    custom: Vec<(String, Vec<(OwnedValue, Binding)>)>,
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

    /// Copy a pattern out of a cache, so it can be edited or written back.
    ///
    /// The reverse of writing one: [`Pattern`](crate::Pattern) borrows from a
    /// cache file and cannot outlive it, and this is how you take one with
    /// you. It copies -- strings, coverage and all -- which is exactly what
    /// the borrowed form exists to avoid, so do it deliberately.
    ///
    /// Properties the cache identifies only by a runtime id are skipped:
    /// those ids were minted by whichever process wrote the file and mean
    /// nothing here.
    pub fn from_pattern(pattern: &crate::Pattern<'_>) -> Self {
        let mut query = Self::new();
        for element in pattern.elements() {
            let Some(object) = element.object() else { continue };
            for (value, binding) in element.values().bindings() {
                query.add_with_binding(object, OwnedValue::from_value(&value), binding);
            }
        }
        query
    }

    fn position(&self, object: Object) -> Result<usize, usize> {
        self.elements.binary_search_by_key(&object.id(), |e| e.object.id())
    }

    // --- properties a configuration invented -----------------------------

    /// The values held against a custom property.
    pub fn custom(&self, name: &str) -> Option<&[(OwnedValue, Binding)]> {
        self.custom.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice())
    }

    /// The value list for `property`, creating it if needed.
    pub(crate) fn values_mut(
        &mut self,
        property: &Property,
    ) -> &mut Vec<(OwnedValue, Binding)> {
        match property {
            Property::Known(object) => {
                let at = match self.position(*object) {
                    Ok(at) => at,
                    Err(at) => {
                        self.elements
                            .insert(at, Element { object: *object, values: Vec::new() });
                        at
                    }
                };
                &mut self.elements[at].values
            }
            Property::Custom(name) => {
                if let Some(at) = self.custom.iter().position(|(n, _)| n == name) {
                    return &mut self.custom[at].1;
                }
                self.custom.push((name.clone(), Vec::new()));
                let last = self.custom.len() - 1;
                &mut self.custom[last].1
            }
        }
    }

    /// The values held against `property`, if it has any.
    pub(crate) fn values_of(&self, property: &Property) -> Option<&[(OwnedValue, Binding)]> {
        match property {
            Property::Known(object) => self.get(*object).map(|e| e.values.as_slice()),
            Property::Custom(name) => self.custom(name),
        }
    }

    /// Drop `property` entirely.
    pub(crate) fn remove_property(&mut self, property: &Property) {
        match property {
            Property::Known(object) => {
                self.remove(*object);
            }
            Property::Custom(name) => self.custom.retain(|(n, _)| n != name),
        }
    }

    /// Discard a property that has been emptied by an edit.
    pub(crate) fn prune(&mut self, property: &Property) {
        if self.values_of(property).is_some_and(|v| v.is_empty()) {
            self.remove_property(property);
        }
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
    /// `prgname` is deliberately not filled in: it names the calling process
    /// rather than anything about fonts, and nothing here scores against it.
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

        // The name languages decide which localization of a family name a
        // prepared pattern reports, so they have to be here even though
        // nothing scores against them directly.
        let namelang = match self.string(Object::Namelang) {
            Some(lang) => lang.to_string(),
            None => {
                let lang = default_lang();
                self.add(Object::Namelang, lang.as_str());
                lang
            }
        };
        for object in [Object::Familylang, Object::Stylelang, Object::Fullnamelang] {
            if !self.contains(object) {
                self.add(object, namelang.as_str());
                // The English fallback is "en-us" rather than "en" on
                // purpose: fontconfig notes that a bare "en" would score as an
                // exact match against a font's "en" and outrank the language
                // actually asked for.
                self.add_weak(object, "en-us");
            }
        }
    }
}

/// The languages fontconfig assumes when a query names none.
///
/// `FcGetDefaultLangs` reads them from the environment and falls back to
/// English. These are added to a query *by substitution*, not by the
/// defaults, and they matter more than they look: a sort demotes every font
/// that answers no requested language, so without them the whole fallback
/// chain is ordered differently.
pub fn default_langs() -> Vec<String> {
    for var in ["FC_LANG", "LC_ALL", "LC_CTYPE", "LANG"] {
        let Ok(value) = std::env::var(var) else { continue };
        // macOS sets LC_CTYPE to "UTF-8", which names no language at all.
        if value.is_empty() || value.eq_ignore_ascii_case("UTF-8") {
            continue;
        }
        let langs: Vec<String> = value
            .split(':')
            .filter_map(|entry| {
                let tag = entry.split(['.', '@']).next()?.replace('_', "-");
                match tag.as_str() {
                    "" | "C" | "POSIX" => None,
                    _ => Some(tag.to_lowercase()),
                }
            })
            .collect();
        if !langs.is_empty() {
            return langs;
        }
    }
    vec!["en".to_string()]
}

/// The language fontconfig assumes when a query does not name one.
///
/// Taken from the environment the same way `FcGetDefaultLangs` does, with the
/// encoding and modifier suffixes stripped, and falling back to English.
fn default_lang() -> String {
    default_langs().into_iter().next().unwrap_or_else(|| "en".to_string())
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
