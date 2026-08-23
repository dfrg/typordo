use crate::bytes::Bytes;
use crate::error::{Error, Result};
use crate::object::Object;
use crate::value::{self, Binding, Value};

use crate::layout::NATIVE as L;

/// The properties of a single font face, as the cache recorded them.
///
/// Nothing is copied out of the file: a `Pattern` is a bounds-checked cursor,
/// and the strings it yields borrow from the cache buffer.
#[derive(Clone, Copy)]
pub struct Pattern<'a> {
    data: Bytes<'a>,
    /// Start of the element array, already resolved and bounds-checked.
    elts: usize,
    /// Number of elements, already checked to fit in the file.
    len: usize,
}

impl<'a> Pattern<'a> {
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
    pub fn get(&self, object: Object) -> Option<Element<'a>> {
        self.elements().find(|e| e.object() == Some(object))
    }

    /// The first value held against `object`.
    ///
    /// This is what most lookups want: a pattern can hold several families or
    /// several styles, but the first is the primary one.
    pub fn value(&self, object: Object) -> Option<Value<'a>> {
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
    /// Infallible: [`Pattern::read`] already proved the whole array is inside
    /// the file, so the header of every element is readable.
    fn element_at(&self, index: usize) -> Element<'a> {
        Element { data: self.data, at: self.elts + index * L.elt }
    }
}

impl std::fmt::Debug for Pattern<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Pattern");
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
    pattern: Pattern<'a>,
    index: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Element<'a>> {
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
pub struct Element<'a> {
    data: Bytes<'a>,
    at: usize,
}

impl<'a> Element<'a> {
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
        Values {
            data: self.data,
            next: self.head().ok().flatten(),
            budget: self.budget(),
        }
    }

    /// Walk the value chain strictly, reporting the first problem.
    pub fn validate(&self) -> Result<()> {
        let mut node = self.head()?;
        let mut budget = self.budget();
        while let Some(at) = node {
            budget = budget.checked_sub(1).ok_or(Error::ChainTooLong)?;
            value::value_at(self.data, at + L.node_value)?;
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

impl std::fmt::Debug for Element<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Element")
            .field("object", &self.object())
            .field("values", &self.values().collect::<Vec<_>>())
            .finish()
    }
}

/// Iterator over the values held against one property.
///
/// Iteration is bounded: see [`Element::validate`] for what a corrupt chain
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

    fn step(&mut self) -> Option<(usize, Value<'a>)> {
        let at = self.next?;
        self.budget = self.budget.checked_sub(1)?;
        let value = value::value_at(self.data, at + L.node_value).ok()?;
        self.next = self.data.follow(at, at).ok().flatten();
        Some((at, value))
    }
}

impl<'a> Iterator for Values<'a> {
    type Item = Value<'a>;

    fn next(&mut self) -> Option<Value<'a>> {
        self.step().map(|(_, value)| value)
    }
}

/// Iterator over values paired with their binding strength.
#[derive(Clone)]
pub struct Bindings<'a>(Values<'a>);

impl<'a> Iterator for Bindings<'a> {
    type Item = (Value<'a>, Binding);

    fn next(&mut self) -> Option<(Value<'a>, Binding)> {
        let (at, value) = self.0.step()?;
        Some((value, value::binding_at(self.0.data, at).ok()?))
    }
}
