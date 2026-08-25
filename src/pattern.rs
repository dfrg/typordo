//! A font's properties, owned and borrowed.
//!
//! [`Pattern`] is what a caller builds and configuration rewrites -- an
//! `FcPattern` in fontconfig's terms. [`PatternRef`] is one read out of a
//! cache: a cursor over its bytes, borrowing every string it yields.

use std::fmt;

use crate::bytes::Bytes;
use crate::error::{Error, Result};
use crate::locale::default_lang;
use crate::object::{Object, Property};
use crate::value::{self, Binding, Value, ValueRef};

use crate::layout::NATIVE as L;

/// The properties of a single font face, as the cache recorded them.
///
/// Nothing is copied out of the file: a `PatternRef` is a bounds-checked cursor,
/// and the strings it yields borrow from the cache buffer.
#[derive(Clone, Copy)]
pub struct PatternRef<'a> {
    data: Bytes<'a>,
    /// Start of the element array, already resolved and bounds-checked.
    elts: usize,
    /// Number of elements, already checked to fit in the file.
    len: usize,
}

impl<'a> PatternRef<'a> {
    /// Read the pattern at `at`, checking its element array fits in the file.
    pub(crate) fn read(data: Bytes<'a>, at: usize) -> Result<Self> {
        let count = data.count(at)?;
        let elts = data.resolve(at, data.offset(at + L.elts)?)?;
        let len = data.array(elts, count, L.elt)?;
        Ok(Self { data, elts, len })
    }

    /// How many distinct properties this pattern carries.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when the pattern carries no properties at all.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The properties, in the order the cache stores them.
    ///
    /// Fontconfig keeps elements sorted by object id, so this is ascending by
    /// [`Object::id`], with any configuration-defined properties last.
    pub fn elements(&self) -> Elements<'a> {
        Elements { pattern: *self, index: 0 }
    }

    /// The element for `object`, if the pattern has one.
    pub fn get(&self, object: Object) -> Option<ElementRef<'a>> {
        self.elements().find(|e| e.object() == Some(object))
    }

    /// The first value held against `object`.
    ///
    /// This is what most lookups want: a pattern can hold several families or
    /// several styles, but the first is the primary one.
    pub fn value(&self, object: Object) -> Option<ValueRef<'a>> {
        self.get(object)?.values().next()
    }

    /// The first value of `object` as a string.
    pub fn string(&self, object: Object) -> Option<&'a str> {
        self.value(object)?.as_str()
    }

    /// The first value of `object` as an integer.
    pub fn int(&self, object: Object) -> Option<i32> {
        self.value(object)?.as_int()
    }

    /// Walk every element and value, reporting the first structural problem.
    pub fn validate(&self) -> Result<()> {
        for index in 0..self.len {
            self.element_at(index).validate()?;
        }
        Ok(())
    }

    /// The element at `index`.
    ///
    /// Infallible: [`PatternRef::read`] already proved the whole array is inside
    /// the file, so the header of every element is readable.
    fn element_at(&self, index: usize) -> ElementRef<'a> {
        ElementRef { data: self.data, at: self.elts + index * L.elt }
    }

    /// The object id of the element at `index`.
    ///
    /// Scoring walks a font's properties against a query's and asks this for
    /// most of them
    /// and then moves on, so it reads the one field rather than building a
    /// cursor to read it through.
    pub(crate) fn element_id(&self, index: usize) -> i32 {
        self.data.i32(self.elts + index * L.elt).unwrap_or(0)
    }

    /// The values held against the element at `index`.
    pub(crate) fn element_values(&self, index: usize) -> Values<'a> {
        self.element_at(index).values()
    }
}

impl std::fmt::Debug for PatternRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("PatternRef");
        for elt in self.elements() {
            let name = match elt.object() {
                Some(object) => object.name().to_string(),
                None => format!("#{}", elt.id()),
            };
            s.field(&name, &elt.values().collect::<Vec<_>>());
        }
        s.finish()
    }
}

/// Iterator over a pattern's properties.
#[derive(Clone)]
pub struct Elements<'a> {
    pattern: PatternRef<'a>,
    index: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = ElementRef<'a>;

    fn next(&mut self) -> Option<ElementRef<'a>> {
        if self.index >= self.pattern.len {
            return None;
        }
        let element = self.pattern.element_at(self.index);
        self.index += 1;
        Some(element)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.pattern.len - self.index;
        (n, Some(n))
    }
}

impl ExactSizeIterator for Elements<'_> {}

/// One property of a pattern, together with every value held against it.
#[derive(Clone, Copy)]
pub struct ElementRef<'a> {
    data: Bytes<'a>,
    at: usize,
}

impl<'a> ElementRef<'a> {
    /// The raw object id, including ids fontconfig minted at runtime.
    pub fn id(&self) -> i32 {
        self.data.i32(self.at).unwrap_or(0)
    }

    /// The property this element describes, if it is one of the static set.
    pub fn object(&self) -> Option<Object> {
        Object::from_id(self.id())
    }

    /// True when this property was defined by a configuration file rather
    /// than built in, and so has no meaning outside the process that made it.
    pub fn is_custom(&self) -> bool {
        self.object().is_none()
    }

    /// The values held against this property, in the order they are stored.
    pub fn values(&self) -> Values<'a> {
        Values { data: self.data, next: self.head().ok().flatten(), budget: self.budget() }
    }

    /// Walk the value chain strictly, reporting the first problem.
    pub fn validate(&self) -> Result<()> {
        let mut node = self.head()?;
        let mut budget = self.budget();
        while let Some(at) = node {
            budget = budget.checked_sub(1).ok_or(Error::ChainTooLong)?;
            value::check_at(self.data, at + L.node_value)?;
            value::binding_at(self.data, at)?;
            node = self.data.follow(at, at)?;
        }
        Ok(())
    }

    fn head(&self) -> Result<Option<usize>> {
        self.data.follow(self.at, self.at + L.values)
    }

    /// The most nodes a chain could have without revisiting one.
    ///
    /// A corrupt `next` can point backwards and cycle forever. Nothing in the
    /// file can be trusted to say how long a chain is, but the file's own
    /// length caps how many distinct nodes can fit in it.
    fn budget(&self) -> usize {
        self.data.len() / L.node + 1
    }
}

impl std::fmt::Debug for ElementRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElementRef")
            .field("object", &self.object())
            .field("values", &self.values().collect::<Vec<_>>())
            .finish()
    }
}

/// Iterator over the values held against one property.
///
/// Iteration is bounded: see [`ElementRef::validate`] for what a corrupt chain
/// can otherwise do.
#[derive(Clone)]
pub struct Values<'a> {
    data: Bytes<'a>,
    next: Option<usize>,
    budget: usize,
}

impl<'a> Values<'a> {
    /// The same values, each paired with how strongly it is held.
    pub fn bindings(self) -> Bindings<'a> {
        Bindings(self)
    }

    fn step(&mut self) -> Option<(usize, ValueRef<'a>)> {
        let at = self.next?;
        self.budget = self.budget.checked_sub(1)?;
        let value = value::value_at(self.data, at + L.node_value).ok()?;
        self.next = self.data.follow(at, at).ok().flatten();
        Some((at, value))
    }
}

impl<'a> Iterator for Values<'a> {
    type Item = ValueRef<'a>;

    fn next(&mut self) -> Option<ValueRef<'a>> {
        self.step().map(|(_, value)| value)
    }
}

/// Iterator over values paired with their binding strength.
#[derive(Clone)]
pub struct Bindings<'a>(Values<'a>);

impl<'a> Iterator for Bindings<'a> {
    type Item = (ValueRef<'a>, Binding);

    fn next(&mut self) -> Option<(ValueRef<'a>, Binding)> {
        let (at, value) = self.0.step()?;
        Some((value, value::binding_at(self.0.data, at).ok()?))
    }
}

/// A pattern being built up and matched against.
///
/// The owned counterpart to [`PatternRef`], which borrows from a cache. This
/// is what a caller constructs and what matching takes; fontconfig calls
/// both of them an `FcPattern`.
///
/// Two rewrites have to happen between building one and matching it, in this
/// order, because fontconfig does the same and its scoring assumes both ran:
///
/// ```no_run
/// # use typordo::{Config, Object, Pattern};
/// # let config = Config::load()?;
/// let mut query = Pattern::new();
/// query.add(Object::Family, "sans-serif");
///
/// config.substitute(&mut query);   // apply the config's <match> rules
/// query.default_substitute();      // fill in what the query left unsaid
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
///
/// Skipping the first leaves generic aliases unresolved -- `sans-serif` never
/// becomes a real family. Skipping the second scores an unstated weight or
/// slant as absent rather than as `normal`, which changes the answer.
///
/// Properties are kept sorted by [`Object::id`], which is the order the cache
/// stores them in and the order scoring walks them in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pattern {
    elements: Vec<Element>,
    /// Properties a configuration invented, kept apart from the built-in ones
    /// because nothing scores against them: they exist only so rules can pass
    /// values to later rules.
    custom: Vec<(String, Vec<(Value, Binding)>)>,
}

/// One property of a query, with every value held against it.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    object: Object,
    values: Vec<(Value, Binding)>,
}

impl Element {
    /// The property this element describes.
    pub fn object(&self) -> Object {
        self.object
    }

    /// The values held against it, in order.
    pub fn values(&self) -> impl Iterator<Item = (&Value, Binding)> {
        self.values.iter().map(|(v, b)| (v, *b))
    }
}

impl Pattern {
    /// An empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a strongly-bound value.
    ///
    /// A strong value came from the caller and outranks anything a
    /// configuration rule contributes.
    pub fn add(&mut self, object: Object, value: impl Into<Value>) -> &mut Self {
        self.add_with_binding(object, value, Binding::Strong)
    }

    /// Append a weakly-bound value.
    pub fn add_weak(&mut self, object: Object, value: impl Into<Value>) -> &mut Self {
        self.add_with_binding(object, value, Binding::Weak)
    }

    /// Append a value with an explicit binding.
    pub fn add_with_binding(
        &mut self,
        object: Object,
        value: impl Into<Value>,
        binding: Binding,
    ) -> &mut Self {
        let value = value.into();
        match self.position(object) {
            Ok(at) => self.elements[at].values.push((value, binding)),
            Err(at) => self.elements.insert(at, Element { object, values: vec![(value, binding)] }),
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
    pub fn value(&self, object: Object) -> Option<&Value> {
        self.get(object)?.values.first().map(|(v, _)| v)
    }

    /// The first value of `object` as a string.
    pub fn string(&self, object: Object) -> Option<&str> {
        match self.value(object)? {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The first value of `object` as a number, whatever it is stored as.
    pub fn number(&self, object: Object) -> Option<f64> {
        match self.value(object)? {
            Value::Int(i) => Some(f64::from(*i)),
            Value::Double(d) => Some(*d),
            Value::Range(r) => Some((r.begin + r.end) * 0.5),
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
    /// The reverse of writing one: [`PatternRef`](crate::PatternRef) borrows from a
    /// cache file and cannot outlive it, and this is how you take one with
    /// you. It copies -- strings, coverage and all -- which is exactly what
    /// the borrowed form exists to avoid, so do it deliberately.
    ///
    /// Properties the cache identifies only by a runtime id are skipped:
    /// those ids were minted by whichever process wrote the file and mean
    /// nothing here.
    pub fn from_pattern(pattern: &crate::PatternRef<'_>) -> Self {
        let mut query = Self::new();
        for element in pattern.elements() {
            let Some(object) = element.object() else { continue };
            for (value, binding) in element.values().bindings() {
                query.add_with_binding(object, Value::from_value(&value), binding);
            }
        }
        query
    }

    fn position(&self, object: Object) -> std::result::Result<usize, usize> {
        self.elements.binary_search_by_key(&object.id(), |e| e.object.id())
    }

    // --- properties a configuration invented -----------------------------

    /// The values held against a custom property.
    pub fn custom(&self, name: &str) -> Option<&[(Value, Binding)]> {
        self.custom.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice())
    }

    /// The value list for `property`, creating it if needed.
    pub(crate) fn values_mut(&mut self, property: &Property) -> &mut Vec<(Value, Binding)> {
        match property {
            Property::Known(object) => {
                let at = match self.position(*object) {
                    Ok(at) => at,
                    Err(at) => {
                        self.elements.insert(at, Element { object: *object, values: Vec::new() });
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
    pub(crate) fn values_of(&self, property: &Property) -> Option<&[(Value, Binding)]> {
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
    fn set(&mut self, object: Object, value: impl Into<Value>) {
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

        // The last three `FcDefaultSubstitute` adds. None of them is scored
        // against; they are here so a configuration can test them, and a
        // `<test name="prgname">` rule cannot fire against a property nothing
        // ever sets.
        if !self.contains(Object::Prgname) {
            if let Some(name) = crate::locale::prgname() {
                self.add(Object::Prgname, name);
            }
        }
        if !self.contains(Object::DesktopName) {
            if let Some(name) = crate::locale::desktop_name() {
                self.add(Object::DesktopName, name);
            }
        }
        if !self.contains(Object::Order) {
            self.add(Object::Order, 0);
        }
    }
}

impl fmt::Display for Pattern {
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
                    Value::String(s) => write!(f, "{s}")?,
                    Value::Int(i) => write!(f, "{i}")?,
                    Value::Double(d) => write!(f, "{d}")?,
                    Value::Bool(b) => write!(f, "{b}")?,
                    other => write!(f, "{other:?}")?,
                }
            }
        }
        Ok(())
    }
}
