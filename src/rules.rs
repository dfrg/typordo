//! The `<match>` rules that rewrite a query before it is scored.
//!
//! A `<match>` is a flat sequence of `<test>` and `<edit>` elements evaluated
//! in source order. Every test must pass; the first that fails abandons the
//! whole rule, including any edits already applied by it. An `<alias>` is
//! sugar for the same thing -- see [`Rule::from_alias`].

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use crate::casefold;
use crate::fnv::BuildPassthrough;
use crate::object::Object;
use crate::query::{Pattern, Property, Value};
use crate::value::{Binding, Matrix};

/// Which pattern a rule set applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// `target="pattern"`: rewrite the query before matching. The default.
    Pattern,
    /// `target="font"`: rewrite the chosen font afterwards.
    Font,
    /// `target="scan"`: rewrite a font as it is scanned into a cache.
    Scan,
    /// No target given, on a `<name>` that did not name one.
    ///
    /// This is not the same as `target="pattern"`, and conflating the two
    /// reads the wrong pattern: a bare `<name>` inside a font rule means the
    /// font being edited, while `target="pattern"` means the original query.
    Default,
}

impl MatchKind {
    /// The target of a `<match>`, which defaults to the pattern.
    pub(crate) fn parse(name: Option<&str>) -> Self {
        match name {
            Some("font") => Self::Font,
            Some("scan") => Self::Scan,
            Some("default") => Self::Default,
            _ => Self::Pattern,
        }
    }

    /// The target of a `<name>`, which defaults to "whichever pattern this
    /// rule is editing".
    pub(crate) fn parse_field(name: Option<&str>) -> Self {
        match name {
            Some("pattern") => Self::Pattern,
            Some("font") => Self::Font,
            _ => Self::Default,
        }
    }
}

/// Which values of a property a `<test>` must hold for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Qual {
    /// At least one value matches. The default.
    Any,
    /// Every value matches -- and a property the pattern lacks passes
    /// vacuously, where [`Qual::Any`] would fail.
    All,
    /// The first value matches.
    First,
    /// Some value other than the first matches.
    NotFirst,
}

impl Qual {
    pub(crate) fn parse(name: Option<&str>) -> Self {
        match name {
            Some("all") => Self::All,
            Some("first") => Self::First,
            Some("not_first") => Self::NotFirst,
            _ => Self::Any,
        }
    }
}

/// How a `<test>` compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    /// Equal, ignoring case (and blanks, for strings).
    Eq,
    /// Not equal.
    NotEq,
    /// Numerically less.
    Less,
    /// Numerically less or equal.
    LessEq,
    /// Numerically greater.
    More,
    /// Numerically greater or equal.
    MoreEq,
    /// Substring, or inside a range.
    Contains,
    /// Neither a substring nor inside a range.
    NotContains,
}

impl Compare {
    pub(crate) fn parse(name: Option<&str>) -> Option<Self> {
        Some(match name.unwrap_or("eq") {
            "eq" => Self::Eq,
            "not_eq" => Self::NotEq,
            "less" => Self::Less,
            "less_eq" => Self::LessEq,
            "more" => Self::More,
            "more_eq" => Self::MoreEq,
            "contains" => Self::Contains,
            "not_contains" => Self::NotContains,
            _ => return None,
        })
    }
}

/// What an `<edit>` does with the values it produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    /// Replace the value a test matched, or all values if no test matched.
    Assign,
    /// Replace every value regardless of what a test matched.
    AssignReplace,
    /// Insert before the matched value, or at the front.
    Prepend,
    /// Insert at the front of the whole list.
    PrependFirst,
    /// Insert after the matched value, or at the back.
    Append,
    /// Insert at the back of the whole list.
    AppendLast,
    /// Remove the matched value, or all of them.
    Delete,
    /// Remove every value.
    DeleteAll,
}

impl EditMode {
    pub(crate) fn parse(name: Option<&str>) -> Option<Self> {
        Some(match name.unwrap_or("assign") {
            "assign" => Self::Assign,
            "assign_replace" => Self::AssignReplace,
            "prepend" => Self::Prepend,
            "prepend_first" => Self::PrependFirst,
            "append" => Self::Append,
            "append_last" => Self::AppendLast,
            "delete" => Self::Delete,
            "delete_all" => Self::DeleteAll,
            _ => return None,
        })
    }
}

/// An expression in a `<test>` or `<edit>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A literal.
    Value(Value),
    /// `<name>`: the current value of a property, read from whichever
    /// pattern its `target` names.
    Field(MatchKind, Property),
    /// A `<const>` that could not be resolved, or an unsupported element.
    ///
    /// Evaluating one yields nothing, which makes a test fail and an edit
    /// contribute no values -- never a wrong value.
    Unknown,
    /// A binary operator.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// A unary operator.
    Unary(UnaryOp, Box<Expr>),
    /// `<if>`: condition, then, else.
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    /// A comma-separated list, which yields several values.
    List(Vec<Expr>),
}

/// Binary operators an expression can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// Logical or.
    Or,
    /// Logical and.
    And,
    /// Equal, ignoring case (and blanks, for strings).
    Eq,
    /// Not equal.
    NotEq,
    /// Numerically less.
    Less,
    /// Numerically less or equal.
    LessEq,
    /// Numerically greater.
    More,
    /// Numerically greater or equal.
    MoreEq,
    /// Substring, or inside a range.
    Contains,
    /// Neither a substring nor inside a range.
    NotContains,
    /// Addition, or string concatenation.
    Plus,
    /// Subtraction.
    Minus,
    /// Multiplication.
    Times,
    /// Division, always producing a double.
    Divide,
}

/// Unary operators an expression can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    /// Logical negation.
    Not,
    /// Round towards negative infinity.
    Floor,
    /// Round towards positive infinity.
    Ceil,
    /// Round to nearest.
    Round,
    /// Round towards zero.
    Trunc,
}

/// One `<test>`.
#[derive(Clone, Debug)]
pub struct Test {
    /// Which pattern to read, which can differ from the rule's own target.
    pub kind: MatchKind,
    /// Which values must match.
    pub qual: Qual,
    /// The property being tested.
    pub object: Property,
    /// How to compare.
    pub compare: Compare,
    /// What to compare against.
    pub expr: Expr,
}

/// One `<edit>`.
#[derive(Clone, Debug)]
pub struct Edit {
    /// The property being edited.
    pub object: Property,
    /// What to do with the produced values.
    pub mode: EditMode,
    /// How strongly the new values are held.
    pub binding: Binding,
    /// What produces the new values.
    pub expr: Expr,
}

/// A `<test>` or an `<edit>`, in the order they appeared.
#[derive(Clone, Debug)]
pub enum Step {
    /// A condition that must hold.
    Test(Test),
    /// A change to apply.
    Edit(Edit),
}

/// One `<match>`: a sequence of steps sharing a target.
#[derive(Clone, Debug)]
pub struct Rule {
    /// Which pattern this rule rewrites.
    pub kind: MatchKind,
    /// Its tests and edits, in source order.
    pub steps: Vec<Step>,
}

/// The family names a query currently carries, as hashes.
///
/// Fontconfig calls this a `FamilyTable`, and its comment gives the reason
/// plainly: the bulk of substitution is spent walking lists of family names.
/// A desktop configuration is mostly rules naming families the query does not
/// have -- one per font that wants special treatment -- and each of those
/// would otherwise scan a list that substitution itself has grown to a
/// hundred entries.
///
/// Only hashes are kept, never the strings. A collision costs one linear scan
/// that finds nothing, which is exactly what would have happened without the
/// index; what it must never do is report a family absent when it is present,
/// and a set built by hashing those same values cannot.
///
/// Counted, not just present, because the same family can be added twice and
/// removed once. Fontconfig keeps the same count for the same reason.
#[derive(Default)]
pub(crate) struct FamilyIndex {
    counts: HashMap<u64, u32, BuildPassthrough>,
}

/// The state one substitution pass carries from rule to rule.
///
/// Both fields exist to be reused. A pass runs hundreds of rules over the
/// same query, and giving each of them a fresh buffer was most of what was
/// left allocating on this path.
pub(crate) struct Pass {
    families: FamilyIndex,
    /// Values an edit is about to hand to the query.
    ///
    /// Drained rather than moved, so the allocation survives into the next
    /// edit while the values themselves go into the pattern.
    tagged: Vec<(Value, Binding)>,
}

impl Pass {
    /// Begin a pass over `query`.
    pub(crate) fn new(query: &Pattern) -> Self {
        Self { families: FamilyIndex::new(query), tagged: Vec::new() }
    }
}

impl FamilyIndex {
    /// Index the families `query` starts with.
    pub(crate) fn new(query: &Pattern) -> Self {
        let mut index = Self::default();
        if let Some(values) = query.values_of(&Property::Known(Object::Family)) {
            for (value, _) in values {
                index.learn(value);
            }
        }
        index
    }

    /// Whether `name` could match any family the query holds.
    ///
    /// False is conclusive; true means the scan still has to run.
    fn might_hold(&self, name: &str) -> bool {
        self.counts.contains_key(&casefold::hash_ignoring_blanks(name))
    }

    /// Record a family the query has gained.
    fn learn(&mut self, value: &Value) {
        if let Value::String(name) = value {
            *self.counts.entry(casefold::hash_ignoring_blanks(name)).or_insert(0) += 1;
        }
    }

    /// Record a family the query has lost.
    fn forget(&mut self, value: &Value) {
        if let Value::String(name) = value {
            if let Entry::Occupied(mut entry) =
                self.counts.entry(casefold::hash_ignoring_blanks(name))
            {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
        }
    }
}

impl Rule {
    /// Desugar an `<alias>` into the `<match>` it stands for.
    ///
    /// `FcParseAlias` builds a pattern-target rule testing `family` against
    /// the alias family with blanks ignored, then one edit per section:
    /// `<prefer>` prepends, `<accept>` appends, and `<default>` appends last.
    /// The distinction is what makes `<prefer>` win over the caller's own
    /// second choice while `<default>` only fills a gap.
    pub fn from_alias(
        family: Expr,
        prefer: Option<Expr>,
        accept: Option<Expr>,
        default: Option<Expr>,
        binding: Binding,
    ) -> Option<Self> {
        if prefer.is_none() && accept.is_none() && default.is_none() {
            return None;
        }
        let mut steps = vec![Step::Test(Test {
            kind: MatchKind::Pattern,
            qual: Qual::Any,
            object: Property::Known(crate::Object::Family),
            compare: Compare::Eq,
            expr: family,
        })];
        for (expr, mode) in [
            (prefer, EditMode::Prepend),
            (accept, EditMode::Append),
            (default, EditMode::AppendLast),
        ] {
            if let Some(expr) = expr {
                steps.push(Step::Edit(Edit {
                    object: Property::Known(crate::Object::Family),
                    mode,
                    binding,
                    expr,
                }));
            }
        }
        Some(Self { kind: MatchKind::Pattern, steps })
    }

    /// Apply this rule to `query`, if every test passes.
    ///
    /// Returns whether any edit was applied. Edits take effect as they are
    /// reached, so a later test sees what an earlier edit did -- and a test
    /// failing halfway leaves those edits in place, which is what fontconfig
    /// does too.
    pub fn apply(&self, query: &mut Pattern, pattern: Option<&Pattern>, pass: &mut Pass) -> bool {
        // Where a test matched, per property, so a following edit knows which
        // value to replace or insert beside.
        let mut marks: Vec<(&Property, Option<usize>)> = Vec::new();
        let mut edited = false;

        for step in &self.steps {
            match step {
                Step::Test(test) => match test.evaluate(query, pattern, &mut pass.families) {
                    Some(position) => {
                        if !marks.iter().any(|(o, _)| **o == test.object) {
                            marks.push((&test.object, position));
                        }
                    }
                    None => return edited,
                },
                Step::Edit(edit) => {
                    let mark =
                        marks.iter().find(|(o, _)| **o == edit.object).and_then(|(_, at)| *at);
                    edit.apply(query, pattern, mark, pass);
                    edited = true;
                    // A replaced value invalidates the position we recorded.
                    if matches!(
                        edit.mode,
                        EditMode::AssignReplace | EditMode::Delete | EditMode::DeleteAll
                    ) {
                        marks.retain(|(o, _)| **o != edit.object);
                    }
                }
            }
        }
        edited
    }
}

impl Test {
    /// Evaluate against `query`, returning where it matched.
    ///
    /// `None` means the test failed. `Some(None)` means it passed without
    /// marking a position, which is what a vacuous [`Qual::All`] does.
    ///
    /// A `target="pattern"` test inside a font-target rule reads the original
    /// query instead, which is how a font rule compares what the caller asked
    /// for against what it got.
    fn evaluate(
        &self,
        query: &Pattern,
        pattern: Option<&Pattern>,
        families: &mut FamilyIndex,
    ) -> Option<Option<usize>> {
        let source = match (self.kind, pattern) {
            (MatchKind::Pattern, Some(pattern)) => pattern,
            _ => query,
        };
        // A test only reads what it compares against, and the overwhelmingly
        // common expression is a bare literal that the rule already owns.
        // Borrowing it saves a Vec and a String copy per test.
        let computed;
        let wanted: &[Value] = match &self.expr {
            Expr::Value(value) => std::slice::from_ref(value),
            other => {
                computed = other.values(query, pattern);
                &computed
            }
        };
        if wanted.is_empty() {
            return None;
        }

        // A family the query does not carry cannot match any of its values,
        // and most rules on a desktop test for exactly that. Only equality
        // can be answered this way -- `contains` is a substring test, which
        // no hash of the whole string can decide -- and only against the
        // query itself, since the index tracks that and not `pattern`.
        if self.compare == Compare::Eq
            && std::ptr::eq(source, query)
            && self.object == Property::Known(Object::Family)
        {
            let decidable = wanted.iter().all(|w| matches!(w, Value::String(_)));
            let possible = wanted.iter().any(|w| match w {
                Value::String(name) => families.might_hold(name),
                _ => false,
            });
            if decidable && !possible {
                return None;
            }
        }
        let Some(values) = source.values_of(&self.object) else {
            // A property the pattern does not have: `all` passes vacuously,
            // everything else fails.
            return match self.qual {
                Qual::All => Some(None),
                _ => None,
            };
        };

        let hit = |(got, _): &(Value, Binding)| {
            wanted.iter().any(|want| compare(got, self.compare, want))
        };

        // Each qualifier stops as soon as its answer is known. That matters
        // more than it looks: rules append to the family list as they go, so
        // a query carries a hundred families by the end of a substitution
        // pass, and a test that scanned all of them to report the first match
        // was doing ninety-nine comparisons it could not use.
        match self.qual {
            Qual::Any => values.iter().position(hit).map(Some),
            // Only the first value is consulted, however many there are.
            Qual::First if values.first().is_some_and(hit) => Some(Some(0)),
            Qual::First => None,
            Qual::NotFirst => values.iter().skip(1).position(hit).map(|i| Some(i + 1)),
            // The only qualifier that has to see everything -- but it can
            // still stop at the first value that fails.
            Qual::All if values.iter().all(hit) => Some((!values.is_empty()).then_some(0)),
            Qual::All => None,
        }
    }
}

impl Edit {
    /// Apply this edit, inserting relative to `mark` when a test set one.
    fn apply(
        &self,
        query: &mut Pattern,
        pattern: Option<&Pattern>,
        mark: Option<usize>,
        pass: &mut Pass,
    ) {
        let tracked = self.object == Property::Known(Object::Family);
        let values = self.expr.values(query, pattern);
        // Only the modes that insert *relative to* the mark pass a position to
        // fontconfig's FcConfigAdd, and only a position gives binding="same"
        // something to inherit from. append_last and friends ignore the mark
        // entirely, so their values always land weak.
        let positional = match self.mode {
            EditMode::Assign | EditMode::Prepend | EditMode::Append => mark,
            _ => None,
        };
        let binding = self.resolve_binding(query, positional);
        let tagged = &mut pass.tagged;
        tagged.clear();
        tagged.extend(values.into_iter().map(|v| (v, binding)));

        // What the index gains is known before the move; what it loses is
        // known only at the point each mode drops it.
        if tracked {
            for (value, _) in tagged.iter() {
                pass.families.learn(value);
            }
        }

        let families = &mut pass.families;
        {
            let slot = query.values_mut(&self.object);
            let forget_all = |families: &mut FamilyIndex, slot: &Vec<(Value, Binding)>| {
                if tracked {
                    for (value, _) in slot {
                        families.forget(value);
                    }
                }
            };
            match self.mode {
                EditMode::Assign => match mark {
                    Some(at) if at < slot.len() => {
                        if tracked {
                            families.forget(&slot[at].0);
                        }
                        let tail = slot.split_off(at + 1);
                        slot.pop();
                        slot.append(tagged);
                        slot.extend(tail);
                    }
                    _ => {
                        forget_all(families, slot);
                        slot.clear();
                        slot.append(tagged);
                    }
                },
                EditMode::AssignReplace => {
                    forget_all(families, slot);
                    slot.clear();
                    slot.append(tagged);
                }
                EditMode::Prepend => {
                    let at = mark.unwrap_or(0).min(slot.len());
                    let tail = slot.split_off(at);
                    slot.append(tagged);
                    slot.extend(tail);
                }
                EditMode::PrependFirst => {
                    let tail = std::mem::take(slot);
                    slot.append(tagged);
                    slot.extend(tail);
                }
                EditMode::Append => {
                    let at = mark.map_or(slot.len(), |at| (at + 1).min(slot.len()));
                    let tail = slot.split_off(at);
                    slot.append(tagged);
                    slot.extend(tail);
                }
                EditMode::AppendLast => slot.append(tagged),
                EditMode::Delete => match mark {
                    Some(at) if at < slot.len() => {
                        if tracked {
                            families.forget(&slot[at].0);
                        }
                        slot.remove(at);
                    }
                    _ => {
                        forget_all(families, slot);
                        slot.clear();
                    }
                },
                EditMode::DeleteAll => {
                    forget_all(families, slot);
                    slot.clear();
                }
            }
        }
        // Fontconfig runs FcConfigPatternCanon after every edit, which drops a
        // property left holding no values at all.
        query.prune(&self.object);
    }

    /// What binding the new values actually get.
    ///
    /// `binding="same"` is not a binding of its own: it means "whatever the
    /// value this edit attached to already had". It inherits from the value a
    /// test marked, and falls back to weak when there is no marked position --
    /// which includes every mode that ignores the mark, not just the case
    /// where no test ran.
    ///
    /// This is what keeps an alias chain from promoting its substitutes: a
    /// `<default>` appends last and so stays weak even when the family it
    /// matched was the caller's own strong one.
    fn resolve_binding(&self, query: &Pattern, mark: Option<usize>) -> Binding {
        match self.binding {
            Binding::Same => mark
                .and_then(|at| Some(query.values_of(&self.object)?.get(at)?.1))
                .unwrap_or(Binding::Weak),
            other => other,
        }
    }
}

impl Expr {
    /// Evaluate to zero or more values.
    ///
    /// A list yields several; an unresolvable expression yields none, which
    /// makes a test fail rather than match something arbitrary.
    pub fn values(&self, query: &Pattern, pattern: Option<&Pattern>) -> Vec<Value> {
        match self {
            Self::Value(v) => vec![v.clone()],
            Self::Unknown => Vec::new(),
            Self::List(parts) => parts.iter().flat_map(|e| e.values(query, pattern)).collect(),
            // `<name target="pattern">` inside a font rule reads the original
            // query: the only way a font rule can see what was asked for
            // rather than what was found.
            Self::Field(kind, object) => {
                // `pattern` is supplied only while running font-target rules,
                // so it doubles as "this is a font rule".
                let source = match (kind, pattern) {
                    (MatchKind::Pattern, Some(pattern)) => pattern,
                    // target="font" inside a pattern rule has nothing to read;
                    // fontconfig warns and yields nothing.
                    (MatchKind::Font, None) => return Vec::new(),
                    _ => query,
                };
                source
                    .values_of(object)
                    .map(|v| v.iter().map(|(value, _)| value.clone()).collect())
                    .unwrap_or_default()
            }
            Self::If(condition, then, otherwise) => {
                match condition.values(query, pattern).first().and_then(as_bool) {
                    Some(true) => then.values(query, pattern),
                    Some(false) => otherwise.values(query, pattern),
                    None => Vec::new(),
                }
            }
            Self::Unary(op, inner) => inner
                .values(query, pattern)
                .first()
                .and_then(|v| apply_unary(*op, v))
                .into_iter()
                .collect(),
            Self::Binary(op, left, right) => {
                let left = left.values(query, pattern);
                let right = right.values(query, pattern);
                match (left.first(), right.first()) {
                    (Some(a), Some(b)) => apply_binary(*op, a, b).into_iter().collect(),
                    _ => Vec::new(),
                }
            }
        }
    }
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(i) => Some(f64::from(*i)),
        Value::Double(d) => Some(*d),
        _ => None,
    }
}

/// Whether both sides were written as integers, so arithmetic stays integral.
fn both_int(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::Int(_), Value::Int(_)))
}

fn number_result(value: f64, integral: bool) -> Value {
    if integral {
        Value::Int(value as i32)
    } else {
        Value::Double(value)
    }
}

fn apply_unary(op: UnaryOp, value: &Value) -> Option<Value> {
    Some(match op {
        UnaryOp::Not => Value::Bool(!as_bool(value)?),
        UnaryOp::Floor => Value::Int(as_number(value)?.floor() as i32),
        UnaryOp::Ceil => Value::Int(as_number(value)?.ceil() as i32),
        UnaryOp::Round => Value::Int(as_number(value)?.round() as i32),
        UnaryOp::Trunc => Value::Int(as_number(value)?.trunc() as i32),
    })
}

fn apply_binary(op: BinaryOp, a: &Value, b: &Value) -> Option<Value> {
    use BinaryOp as B;
    Some(match op {
        B::Or => Value::Bool(as_bool(a)? || as_bool(b)?),
        B::And => Value::Bool(as_bool(a)? && as_bool(b)?),
        B::Eq => Value::Bool(compare(a, Compare::Eq, b)),
        B::NotEq => Value::Bool(compare(a, Compare::NotEq, b)),
        B::Less => Value::Bool(compare(a, Compare::Less, b)),
        B::LessEq => Value::Bool(compare(a, Compare::LessEq, b)),
        B::More => Value::Bool(compare(a, Compare::More, b)),
        B::MoreEq => Value::Bool(compare(a, Compare::MoreEq, b)),
        B::Contains => Value::Bool(compare(a, Compare::Contains, b)),
        B::NotContains => Value::Bool(compare(a, Compare::NotContains, b)),
        // Plus concatenates strings, unions sets, and adds everything else.
        B::Plus => match (a, b) {
            (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
            (Value::LangSet(a), Value::LangSet(b)) => Value::LangSet(a.union(b)),
            (Value::CharSet(a), Value::CharSet(b)) => Value::CharSet(a.union(b)),
            _ => number_result(as_number(a)? + as_number(b)?, both_int(a, b)),
        },
        // Minus subtracts sets as well as numbers, which is how a config
        // takes a language away from a font that only appears to have it.
        B::Minus => match (a, b) {
            (Value::LangSet(a), Value::LangSet(b)) => Value::LangSet(a.subtract(b)),
            (Value::CharSet(a), Value::CharSet(b)) => Value::CharSet(a.subtract(b)),
            _ => number_result(as_number(a)? - as_number(b)?, both_int(a, b)),
        },
        B::Times => number_result(as_number(a)? * as_number(b)?, both_int(a, b)),
        B::Divide => {
            let divisor = as_number(b)?;
            // Division always produces a double, even between two integers.
            Value::Double(as_number(a)? / divisor)
        }
    })
}

/// Compare a pattern value against a test value.
///
/// Strings compare with case folding; `eq` on a family also ignores blanks,
/// which is `FcOpFlagIgnoreBlanks`. `contains` is a substring test for
/// strings and a range test for numbers.
pub(crate) fn compare(got: &Value, op: Compare, want: &Value) -> bool {
    use Value as V;
    match (got, want) {
        (V::String(got), V::String(want)) => match op {
            Compare::Eq => casefold::eq_ignoring_blanks(got, want),
            Compare::NotEq => !casefold::eq_ignoring_blanks(got, want),
            Compare::Contains => contains_folded(got, want),
            Compare::NotContains => !contains_folded(got, want),
            _ => false,
        },
        (V::Bool(got), V::Bool(want)) => match op {
            Compare::Eq | Compare::Contains => got == want,
            Compare::NotEq | Compare::NotContains => got != want,
            _ => false,
        },
        (V::Matrix(got), V::Matrix(want)) => match op {
            Compare::Eq | Compare::Contains => matrix_eq(got, want),
            Compare::NotEq | Compare::NotContains => !matrix_eq(got, want),
            _ => false,
        },
        _ => match (as_number(got), as_number(want), got, want) {
            // A range contains a number, and equals another range.
            (_, _, V::Range(range), other) | (_, _, other, V::Range(range))
                if matches!(op, Compare::Contains | Compare::NotContains) =>
            {
                let inside = as_number(other).is_some_and(|n| n >= range.begin && n <= range.end);
                inside == matches!(op, Compare::Contains)
            }
            (Some(got), Some(want), _, _) => match op {
                Compare::Eq | Compare::Contains => got == want,
                Compare::NotEq | Compare::NotContains => got != want,
                Compare::Less => got < want,
                Compare::LessEq => got <= want,
                Compare::More => got > want,
                Compare::MoreEq => got >= want,
            },
            _ => false,
        },
    }
}

fn matrix_eq(a: &Matrix, b: &Matrix) -> bool {
    a.xx == b.xx && a.xy == b.xy && a.yx == b.yx && a.yy == b.yy
}

/// Substring search that ignores case, the way `FcStrStrIgnoreCase` does.
fn contains_folded(haystack: &str, needle: &str) -> bool {
    // ASCII folds byte for byte, so the usual case searches in place. Both
    // sides have to be ASCII, not either: folding can expand, and U+00DF
    // folds to "ss", which really is inside "strasse".
    if haystack.is_ascii() && needle.is_ascii() {
        let (hay, pin) = (haystack.as_bytes(), needle.as_bytes());
        return pin.is_empty()
            || (pin.len() <= hay.len()
                && hay.windows(pin.len()).any(|w| w.eq_ignore_ascii_case(pin)));
    }
    let fold = |s: &str| casefold::fold_str(s).collect::<String>();
    fold(haystack).contains(&fold(needle))
}
