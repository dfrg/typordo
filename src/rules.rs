//! The `<match>` rules that rewrite a query before it is scored.
//!
//! A `<match>` is a flat sequence of `<test>` and `<edit>` elements evaluated
//! in source order. Every test must pass; the first that fails abandons the
//! whole rule, including any edits already applied by it. An `<alias>` is
//! sugar for the same thing -- see [`Rule::from_alias`].

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use crate::casefold;
use crate::charset::CharSet;
use crate::fnv::BuildPassthrough;
use crate::langset::LangSet;
use crate::object::Object;
use crate::object::Property;
use crate::pattern::Pattern;
use crate::value::Value;
use crate::value::{Binding, Matrix, Range, Tristate};

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
    /// Whether a string comparison ignores spaces as well as case.
    ///
    /// `<test ignore-blanks="true">`. Off by default, which is what makes
    /// `"Deja Vu"` and `"DejaVu"` different names to a plain `<test>` --
    /// fontconfig folds case either way, but only strips blanks when asked.
    /// The comparison an `<alias>` generates always asks, and so does
    /// `<selectfont>`; a hand-written `<test>` does not unless it says so.
    pub ignore_blanks: bool,
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
    /// `FcParseAlias` builds a pattern-target rule from the alias's own
    /// `<test>` elements followed by one testing `family` against the alias
    /// family with blanks ignored, then one edit per section: `<prefer>`
    /// prepends, `<accept>` appends, and `<default>` appends last. The
    /// distinction is what makes `<prefer>` win over the caller's own second
    /// choice while `<default>` only fills a gap.
    ///
    /// `tests` are the conditions written inside the alias, in source order
    /// and ahead of the family test, which is where `FcParseAlias` puts them.
    /// An alias carrying them is conditional -- it applies only when they all
    /// pass -- and dropping them would make it apply always.
    pub fn from_alias(
        family: Expr,
        tests: Vec<Step>,
        prefer: Option<Expr>,
        accept: Option<Expr>,
        default: Option<Expr>,
        binding: Binding,
    ) -> Option<Self> {
        if prefer.is_none() && accept.is_none() && default.is_none() {
            return None;
        }
        let mut steps = tests;
        steps.push(Step::Test(Test {
            kind: MatchKind::Pattern,
            qual: Qual::Any,
            object: Property::Known(crate::Object::Family),
            compare: Compare::Eq,
            // `FC_OP (FcOpEqual, FcOpFlagIgnoreBlanks)` in `FcParseAlias`.
            ignore_blanks: true,
            expr: family,
        }));
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
                    let shift = edit.apply(query, pattern, mark, pass);
                    // The mark names a value, not a slot. Upstream holds the
                    // node itself, so values inserted in front of it change
                    // nothing; an index has to be moved to keep up.
                    if shift > 0 {
                        if let Some((_, at)) = marks.iter_mut().find(|(o, _)| **o == edit.object) {
                            *at = at.map(|at| at + shift);
                        }
                    }
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

        // `FcConfigMatchValueList`, whose shape decides two things at once:
        // whether the test fires, and which value it marks for a later
        // match-relative edit to insert next to.
        //
        // The loops nest the way they look: the *expressions* outside, the
        // pattern's values inside. So the first listed value that matches
        // anything sets the mark, and `if (!ret) ret = v` keeps every later
        // expression from moving it. Marking the first pattern value that
        // matched any expression -- the obvious reading, and what this did --
        // gets a different value whenever the query lists them in another
        // order than the test does.
        let blanks = if self.ignore_blanks { Blanks::Ignored } else { Blanks::Significant };
        // Only the query's own families are indexed, so the table is out of
        // play for a `target="pattern"` test reached from a font rule --
        // which is exactly when upstream passes `table = NULL`.
        let tabled = self.object == Property::Known(Object::Family) && std::ptr::eq(source, query);
        let carried = |want: &Value| match want {
            // Keyed by string upstream, where `FcValueString` gives anything
            // else no key at all, so the lookup is skipped for it.
            Value::String(name) => {
                families.might_hold(name)
                    && values.iter().any(|(got, _)| compare(got, Compare::Eq, want, blanks))
            }
            _ => true,
        };

        let mut mark: Option<usize> = None;
        for want in wanted {
            // The family fast path, which is not only a shortcut: a listed
            // family the query does not carry *clears* the mark, so a test
            // naming several of them is decided by the last one -- and
            // `<test name="family">Alpha,Zeta</test>` does not fire for a
            // query of just `Alpha`. It reads as an accident and it is the
            // behaviour configurations are written against.
            if tabled {
                match self.compare {
                    Compare::Eq => {
                        if !carried(want) {
                            mark = None;
                            continue;
                        }
                    }
                    Compare::NotEq if self.qual == Qual::All => {
                        mark = (!carried(want)).then_some(0);
                        continue;
                    }
                    _ => {}
                }
            }
            for (index, (got, _)) in values.iter().enumerate() {
                if compare(got, self.compare, want, blanks) {
                    if mark.is_none() {
                        mark = Some(index);
                    }
                    // `all` is the one qualifier that has to see every value.
                    if self.qual != Qual::All {
                        break;
                    }
                } else if self.qual == Qual::All {
                    mark = None;
                    break;
                }
            }
        }

        // `first` and `not_first` are not part of the scan at all. Upstream
        // applies them to its result: the mark has to be the head of the list,
        // or anything but the head. Reading them as "does value 0 match" and
        // "does any later value match" agrees only while a test lists one
        // value, since the mark can be set by a value those readings never
        // look at.
        let mark = mark?;
        match self.qual {
            Qual::First if mark != 0 => None,
            Qual::NotFirst if mark == 0 => None,
            _ => Some(Some(mark)),
        }
    }
}

impl Edit {
    /// Apply this edit, inserting relative to `mark` when a test set one.
    /// Returns how many values were inserted *before* the mark, which is how
    /// far the mark has to move to keep pointing at the value it named.
    ///
    /// Upstream has no such arithmetic because it holds a `FcValueList *`:
    /// the node survives insertions in front of it. An index does not, and a
    /// rule that prepends and then assigns -- two edits on one object in one
    /// `<match>` -- assigned over the wrong value here.
    fn apply(
        &self,
        query: &mut Pattern,
        pattern: Option<&Pattern>,
        mark: Option<usize>,
        pass: &mut Pass,
    ) -> usize {
        let tracked = self.object == Property::Known(Object::Family);
        let mut inserted_before_mark = 0usize;
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
        // An edit is graded in two steps, and they are not the same step.
        // `FcConfigValues` drops a `FcTypeVoid` from the list as it builds it,
        // one value at a time. Then `FcConfigAdd` walks what is left and, if
        // *any* of it is a type the property cannot hold, adds **none** of it
        // -- so an edit giving `weight` a string and a number stores neither,
        // where dropping the string on its own would store the number.
        //
        // A property a configuration invented has no declared type and
        // accepts anything.
        let storable = |value: &Value| match (&self.object, value.kind()) {
            (_, None) => false,
            (Property::Known(object), Some(kind)) => object.accepts(kind),
            (Property::Custom(_), Some(_)) => true,
        };
        let values: Vec<Value> = values.into_iter().filter(|v| v.kind().is_some()).collect();
        if values.iter().all(storable) {
            tagged.extend(values.into_iter().map(|v| (v, binding)));
        }
        // Note what is *not* conditional on that. `FcOpAssign` calls
        // `FcConfigAdd` and then `FcConfigDel` on the marked value, and the
        // delete is not guarded by whether the add succeeded;
        // `FcOpAssignReplace` deletes everything before it adds. So an edit
        // whose values the property cannot hold still empties it.

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
                    inserted_before_mark = tagged.len();
                    slot.append(tagged);
                    slot.extend(tail);
                }
                EditMode::PrependFirst => {
                    let tail = std::mem::take(slot);
                    inserted_before_mark = tagged.len();
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
        inserted_before_mark
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
                    // fontconfig warns and yields Void.
                    (MatchKind::Font, None) => return vec![Value::Void],
                    _ => query,
                };
                // `FcPatternObjectGet (p, object, 0, &v)` -- index zero, so
                // one value however many the property holds, and `FcTypeVoid`
                // when it holds none. Void rather than nothing is what makes
                // `<times><name>matrix</name>...` work on a query that has no
                // matrix: it promotes to the identity, which is how stock
                // `90-synthetic.conf` shears a face with no italic.
                let first = source
                    .values_of(object)
                    .and_then(|values| values.first())
                    .map(|(value, _)| value.clone());
                vec![first.unwrap_or(Value::Void)]
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

/// A flag read the way `FcConfigEvaluate` reads one: as the integer it is.
///
/// The logical operators and `<if>` treat it as C does, so `FcDontCare` -- 2
/// -- is true to `<or>` and `<and>`, and `<not>` of it is false. That falls
/// out of `!2 == 0` upstream rather than being a decision, but it is a
/// decision here, so it is written down.
fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(b.as_i32() != 0),
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

/// An arithmetic result, as the type fontconfig would give it.
///
/// `FcConfigEvaluate` computes in double and then converts to an integer
/// whenever the result is one: `v.u.d == (double)(int)v.u.d`. Not "when both
/// operands were integers" -- `12.5 * 2` is an integer to fontconfig, and
/// `4 / 2` is too. The value it prints is the same either way; the type is
/// what a written cache records and what `FcPatternGet` checks.
fn number_result(value: f64) -> Value {
    // The cast is only defined for values an i32 can hold, and the round trip
    // is what decides integrality, so both are guarded at once.
    if value >= f64::from(i32::MIN)
        && value <= f64::from(i32::MAX)
        && value == (value as i32).into()
    {
        Value::Int(value as i32)
    } else {
        Value::Double(value)
    }
}

fn apply_unary(op: UnaryOp, value: &Value) -> Option<Value> {
    Some(match op {
        UnaryOp::Not => Value::Bool((!as_bool(value)?).into()),
        UnaryOp::Floor => Value::Int(as_number(value)?.floor() as i32),
        UnaryOp::Ceil => Value::Int(as_number(value)?.ceil() as i32),
        UnaryOp::Round => Value::Int(as_number(value)?.round() as i32),
        UnaryOp::Trunc => Value::Int(as_number(value)?.trunc() as i32),
    })
}

/// Apply a binary operator, `FcConfigEvaluate`'s arithmetic half.
///
/// Both operands are promoted first and then dispatched on the type they have
/// in common -- so `<times>` reaches matrix multiplication, and an absent
/// value multiplied by a matrix becomes the identity times that matrix, which
/// is exactly what stock `90-synthetic.conf` relies on to shear a face that
/// has no italic of its own.
fn apply_binary(op: BinaryOp, a: &Value, b: &Value) -> Option<Value> {
    use BinaryOp as B;
    use Value as V;
    // The comparisons do their own promotion, and are not arithmetic.
    let comparison = match op {
        B::Eq => Some(Compare::Eq),
        B::NotEq => Some(Compare::NotEq),
        B::Less => Some(Compare::Less),
        B::LessEq => Some(Compare::LessEq),
        B::More => Some(Compare::More),
        B::MoreEq => Some(Compare::MoreEq),
        B::Contains => Some(Compare::Contains),
        B::NotContains => Some(Compare::NotContains),
        _ => None,
    };
    if let Some(compare_op) = comparison {
        return Some(Value::Bool(compare(a, compare_op, b, Blanks::Significant).into()));
    }

    // `vle = FcConfigPromote (vl, vr); vre = FcConfigPromote (vr, vle)`. The
    // second is promoted against the *result* of the first, not the original
    // -- which is only observable if promoting the left could change what the
    // right promotes to, and none of the rules chain that way, but this is
    // the order upstream writes.
    let left = promote(a, b);
    let left = left.as_ref().unwrap_or(a);
    let right = promote(b, left);
    let right = right.as_ref().unwrap_or(b);

    Some(match (op, left, right) {
        (B::Or, l, r) => Value::Bool((as_bool(l)? || as_bool(r)?).into()),
        (B::And, l, r) => Value::Bool((as_bool(l)? && as_bool(r)?).into()),
        // Plus concatenates strings and unions sets; minus subtracts sets,
        // which is how a configuration takes a language away from a font that
        // only appears to have it.
        (B::Plus, V::String(l), V::String(r)) => Value::String(format!("{l}{r}")),
        (B::Plus, V::LangSet(l), V::LangSet(r)) => Value::LangSet(l.union(r)),
        (B::Plus, V::CharSet(l), V::CharSet(r)) => Value::CharSet(l.union(r)),
        (B::Minus, V::LangSet(l), V::LangSet(r)) => Value::LangSet(l.subtract(r)),
        (B::Minus, V::CharSet(l), V::CharSet(r)) => Value::CharSet(l.subtract(r)),
        // `FcMatrixMultiply`. The only operator a matrix has.
        (B::Times, V::Matrix(l), V::Matrix(r)) => Value::Matrix(Matrix {
            xx: l.xx * r.xx + l.xy * r.yx,
            xy: l.xx * r.xy + l.xy * r.yy,
            yx: l.yx * r.xx + l.yy * r.yx,
            yy: l.yx * r.xy + l.yy * r.yy,
        }),
        // Everything else is arithmetic, in double, and the result collapses
        // to an integer whenever it lands on one -- for division too, which
        // is why `<divide><int>4</int><int>2</int></divide>` is `2` and not
        // `2.0`. That distinction is invisible in printed output and visible
        // in a written cache.
        (B::Plus | B::Minus | B::Times | B::Divide, l, r) => {
            let (l, r) = (as_number(l)?, as_number(r)?);
            let result = match op {
                B::Plus => l + r,
                B::Minus => l - r,
                B::Times => l * r,
                _ => l / r,
            };
            number_result(result)
        }
        _ => return None,
    })
}

/// Whether a string comparison treats spaces as characters.
///
/// Fontconfig carries this as `FcOpFlagIgnoreBlanks` on the operator rather
/// than as a separate operator, and only some comparisons consult it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Blanks {
    /// `"Deja Vu"` is not `"DejaVu"`. A plain `<test>`.
    Significant,
    /// `"Deja Vu"` is `"DejaVu"`. `<alias>`, `<selectfont>`, and a
    /// `<test ignore-blanks="true">`.
    Ignored,
}

/// Compare a pattern value against a test value.
///
/// `FcConfigCompareValue`. Two values of one type compare directly; two of
/// different types are each promoted towards the other first, and if that
/// leaves them still unalike the answer is `true` for the negated operators
/// and `false` for the rest -- so `not_eq` between a string and a number is
/// satisfied, which is the reading that makes a negative test mean what it
/// says about a property holding the wrong sort of value.
pub(crate) fn compare(got: &Value, op: Compare, want: &Value, blanks: Blanks) -> bool {
    if let Some(answer) = compare_alike(got, op, want, blanks) {
        return answer;
    }
    // `FcConfigPromote` is applied to each side against the *original* other
    // side, not against the result of promoting it, so the two are
    // independent and neither can chase the other.
    let (left, right) = (promote(got, want), promote(want, got));
    let (left, right) = (left.as_ref().unwrap_or(got), right.as_ref().unwrap_or(want));
    compare_alike(left, op, right, blanks)
        .unwrap_or(matches!(op, Compare::NotEq | Compare::NotContains))
}

/// `value` seen as the type of `toward`, when fontconfig has a rule for it.
///
/// `FcConfigPromote`: a number is a range of one point, an absent value is
/// the identity matrix or an empty set, and a string is the language set
/// naming just that language. `None` where there is no rule, which leaves the
/// value as it was.
fn promote(value: &Value, toward: &Value) -> Option<Value> {
    use Value as V;
    Some(match (value, toward) {
        (V::Int(n), V::Range(_)) => V::Range(Range::single(f64::from(*n))),
        (V::Double(n), V::Range(_)) => V::Range(Range::single(*n)),
        (V::Void, V::Matrix(_)) => V::Matrix(Matrix::IDENTITY),
        (V::Void, V::LangSet(_)) => V::LangSet(LangSet::new()),
        (V::Void, V::CharSet(_)) => V::CharSet(CharSet::new()),
        (V::String(lang), V::LangSet(_)) => {
            let mut set = LangSet::new();
            set.insert(lang);
            V::LangSet(set)
        }
        _ => return None,
    })
}

/// Compare two values of the same type, or `None` if they are not.
///
/// Integers and doubles count as one type here: fontconfig promotes an
/// integer to a double whenever the two sides disagree, so nothing downstream
/// ever sees the pair.
fn compare_alike(got: &Value, op: Compare, want: &Value, blanks: Blanks) -> Option<bool> {
    use Value as V;
    Some(match (got, want) {
        (V::String(got), V::String(want)) => match op {
            // `FcStrCmpIgnoreBlanksAndCase` when the flag is set,
            // `FcStrCmpIgnoreCase` when it is not. `contains` has no such
            // choice upstream: it is always `FcStrStrIgnoreCase`.
            Compare::Eq => string_eq(got, want, blanks),
            Compare::NotEq => !string_eq(got, want, blanks),
            Compare::Contains => contains_folded(got, want),
            Compare::NotContains => !contains_folded(got, want),
            _ => false,
        },
        // The eight arms `FcConfigCompareValue` gives a boolean. The four
        // ordering operators are not orderings at all: they are questions
        // about which side is `DontCare`, which is the only reading under
        // which `less` on a flag means anything.
        (V::Bool(got), V::Bool(want)) => {
            let (any_got, any_want) = (*got == Tristate::DontCare, *want == Tristate::DontCare);
            match op {
                Compare::Eq => got == want,
                Compare::NotEq => got != want,
                Compare::Contains => got == want || any_got,
                Compare::NotContains => !(got == want || any_got),
                Compare::Less => got != want && any_want,
                Compare::LessEq => got == want || any_want,
                Compare::More => got != want && any_got,
                Compare::MoreEq => got == want || any_got,
            }
        }
        (V::Matrix(got), V::Matrix(want)) => match op {
            Compare::Eq | Compare::Contains => matrix_eq(got, want),
            Compare::NotEq | Compare::NotContains => !matrix_eq(got, want),
            _ => false,
        },
        // `FcCharSetIsSubset (right, left)`: the font's coverage contains the
        // test's when the test's is the subset. The argument order is the
        // reverse of how the operator reads and is worth stating.
        (V::CharSet(got), V::CharSet(want)) => match op {
            Compare::Contains => want.is_subset(got),
            Compare::NotContains => !want.is_subset(got),
            Compare::Eq => got == want,
            Compare::NotEq => got != want,
            _ => false,
        },
        (V::LangSet(got), V::LangSet(want)) => match op {
            Compare::Contains => got.contains_set(want),
            Compare::NotContains => !got.contains_set(want),
            Compare::Eq => got == want,
            Compare::NotEq => got != want,
            _ => false,
        },
        // `FcRangeCompare`. The ordering operators compare the near edge of
        // one span with the far edge of the other, so they mean "entirely
        // below" rather than anything about where the spans start.
        (V::Range(got), V::Range(want)) => match op {
            Compare::Eq => got.begin == want.begin && got.end == want.end,
            Compare::NotEq => got.begin != want.begin || got.end != want.end,
            Compare::Contains => got.within(want),
            Compare::NotContains => !got.within(want),
            Compare::Less => got.end < want.begin,
            Compare::LessEq => got.end <= want.begin,
            Compare::More => got.begin > want.end,
            Compare::MoreEq => got.begin >= want.end,
        },
        // Two absent values are equal, and each contains the other.
        (V::Void, V::Void) => matches!(op, Compare::Eq | Compare::Contains),
        (V::Int(_) | V::Double(_), V::Int(_) | V::Double(_)) => {
            let (got, want) = (as_number(got)?, as_number(want)?);
            match op {
                Compare::Eq | Compare::Contains => got == want,
                Compare::NotEq | Compare::NotContains => got != want,
                Compare::Less => got < want,
                Compare::LessEq => got <= want,
                Compare::More => got > want,
                Compare::MoreEq => got >= want,
            }
        }
        _ => return None,
    })
}

fn string_eq(got: &str, want: &str, blanks: Blanks) -> bool {
    match blanks {
        Blanks::Ignored => casefold::eq_ignoring_blanks(got, want),
        Blanks::Significant => casefold::eq(got, want),
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

#[cfg(test)]
mod compare_tests {
    use super::{compare, Blanks, Compare};
    use crate::charset::CharSet;
    use crate::langset::LangSet;
    use crate::value::{Matrix, Range, Value as V};

    fn cmp(got: V, op: Compare, want: V) -> bool {
        compare(&got, op, &want, Blanks::Significant)
    }

    fn chars(cs: &[char]) -> V {
        let mut set = CharSet::new();
        for c in cs {
            set.insert(*c);
        }
        V::CharSet(set)
    }

    fn langs(names: &[&str]) -> V {
        let mut set = LangSet::new();
        for name in names {
            set.insert(name);
        }
        V::LangSet(set)
    }

    fn range(begin: f64, end: f64) -> V {
        V::Range(Range { begin, end })
    }

    /// `FcCharSetIsSubset (right, left)`: the font's coverage contains the
    /// test's when the test's is the subset, which is the reverse of the
    /// argument order the operator reads in.
    #[test]
    fn a_charset_contains_the_set_it_covers() {
        let font = chars(&['a', 'b', 'c']);
        assert!(cmp(font.clone(), Compare::Contains, chars(&['a', 'b'])));
        assert!(!cmp(font.clone(), Compare::Contains, chars(&['a', 'z'])));
        assert!(cmp(font.clone(), Compare::NotContains, chars(&['a', 'z'])));
        assert!(cmp(font.clone(), Compare::Eq, chars(&['a', 'b', 'c'])));
        assert!(cmp(font, Compare::NotEq, chars(&['a', 'b'])));
    }

    /// `FcLangSetContains`, which counts a language covered by one the set
    /// holds -- `en` answers for `en-GB`.
    #[test]
    fn a_langset_contains_what_its_languages_cover() {
        assert!(cmp(langs(&["en"]), Compare::Contains, langs(&["en-gb"])));
        assert!(!cmp(langs(&["en-gb"]), Compare::Contains, langs(&["de"])));
        assert!(cmp(langs(&["en", "de"]), Compare::Eq, langs(&["de", "en"])));
        assert!(cmp(langs(&["en"]), Compare::NotEq, langs(&["de"])));
    }

    /// A string compared against a language set is promoted to the set
    /// naming just that language, so `<test name="lang">en</test>` works
    /// against a font's language list.
    #[test]
    fn a_string_promotes_to_a_langset() {
        assert!(cmp(langs(&["en", "de"]), Compare::Contains, V::String("de".into())));
        assert!(!cmp(langs(&["en"]), Compare::Contains, V::String("de".into())));
        // And the same in reverse, since either side may be the set.
        assert!(cmp(V::String("en".into()), Compare::Eq, langs(&["en"])));
    }

    /// An absent value becomes the empty set or the identity transform, so a
    /// test against a property the font does not carry still has a defined
    /// answer.
    #[test]
    fn void_promotes_to_an_empty_set() {
        assert!(cmp(langs(&["en"]), Compare::Contains, V::Void), "everything covers nothing");
        assert!(cmp(chars(&['a']), Compare::Contains, V::Void));
        assert!(!cmp(V::Void, Compare::Contains, chars(&['a'])));
        assert!(cmp(V::Void, Compare::Eq, V::Matrix(Matrix::IDENTITY)));
    }

    /// `FcRangeCompare`. The ordering operators put one span entirely below
    /// the other rather than comparing where they start.
    #[test]
    fn ranges_compare_edge_to_edge() {
        assert!(cmp(range(10.0, 20.0), Compare::Eq, range(10.0, 20.0)));
        assert!(cmp(range(10.0, 20.0), Compare::NotEq, range(10.0, 21.0)));
        assert!(cmp(range(12.0, 15.0), Compare::Contains, range(10.0, 20.0)));
        assert!(!cmp(range(10.0, 20.0), Compare::Contains, range(12.0, 15.0)));
        assert!(cmp(range(1.0, 5.0), Compare::Less, range(6.0, 9.0)));
        assert!(!cmp(range(1.0, 6.0), Compare::Less, range(6.0, 9.0)));
        assert!(cmp(range(1.0, 6.0), Compare::LessEq, range(6.0, 9.0)));
        assert!(cmp(range(7.0, 9.0), Compare::More, range(1.0, 6.0)));
    }

    /// A number against a range is promoted to a range of one point, which is
    /// how `<test name="size" compare="contains">` reaches a variable font's
    /// declared span -- and why the reverse direction is not the same
    /// question.
    #[test]
    fn a_number_promotes_to_a_range_of_one_point() {
        assert!(cmp(V::Double(12.0), Compare::Contains, range(10.0, 20.0)));
        assert!(!cmp(V::Double(30.0), Compare::Contains, range(10.0, 20.0)));
        assert!(cmp(V::Int(12), Compare::Contains, range(10.0, 20.0)));
        // A span is not inside a point unless it is that point: the promoted
        // side is the number, and `contains` asks whether the left is within
        // the right.
        assert!(!cmp(range(10.0, 20.0), Compare::Contains, V::Double(12.0)));
        assert!(cmp(range(12.0, 12.0), Compare::Contains, V::Double(12.0)));
        assert!(cmp(V::Double(5.0), Compare::Less, range(6.0, 9.0)));
    }

    /// Types that cannot be brought together satisfy the negated operators
    /// and nothing else, so `not_eq` holds against a property carrying the
    /// wrong sort of value rather than quietly failing.
    #[test]
    fn unrelated_types_satisfy_only_the_negated_operators() {
        assert!(cmp(V::String("a".into()), Compare::NotEq, V::Int(1)));
        assert!(cmp(V::String("a".into()), Compare::NotContains, V::Int(1)));
        assert!(!cmp(V::String("a".into()), Compare::Eq, V::Int(1)));
        assert!(!cmp(V::String("a".into()), Compare::Less, V::Int(1)));
        // An integer and a double are one type, not two.
        assert!(cmp(V::Int(3), Compare::Eq, V::Double(3.0)));
    }
}
