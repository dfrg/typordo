# Audits

Five independent audits of this crate against fontconfig, each with its own
log. They were carried out by agents belonging to someone other than this
crate's author, which is the only reason they are worth anything: nothing here
is the author checking their own work.

| log | findings | pinned at | compared against |
| --- | --- | --- | --- |
| [audit-1.md](audit-1.md) | 25 | `cf45c33` | 2.18.3 |
| [audit-2.md](audit-2.md) | 13 | `dedb888` | 2.17.1 |
| [audit-3.md](audit-3.md) | 5 | `08ebc75` | 2.17.1 |
| [audit-4.md](audit-4.md) | 3 | `7d88172` | 2.17.1 |
| [audit-5.md](audit-5.md) | 9 | `7d88172` | 2.18.3 |

The last two are revalidations: every finding of the second and the first
audit re-run against the code that claims to have fixed them. Both found
something. That is the argument for having them, and it is now the third time
a row marked "fixed" in this repository was not.

Each log records what came of every finding: what was actually wrong, how it
was checked, and where the fix is.

What the sequence shows, read together, is that the findings get narrower each
time -- the first audit found whole missing mechanisms, the second found wrong
semantics inside mechanisms that existed, the third found the fallback paths
that only run when a font is malformed -- until the revalidations, which found
something else again. Their findings are not narrower; they are in the places
a *field comparison cannot see*: which value a rule marked rather than whether
it fired, what a cache says about a binding rather than about a value, whether
a configuration loaded at all rather than what it says. Four of the five new
harnesses exist because no comparison of properties could have caught what
they cover.

## A note on versions

This crate targets fontconfig **2.17.0**. Two audits compared against 2.18.3,
which is useful -- it is where the project is going -- but it means a finding
can be real for 2.18.3 and wrong for the version being targeted. Both
revalidations turned up one: `<const>` resolution became object-aware in
2.18.3 and is not in 2.17.0, and the ambiguous-constant rejection came with
it. Read those rows for which version they are about.

The reverse also happened, and is the more interesting half: chasing 2.18.3's
object-aware `<const>` is what found that 2.17.0 *does* resolve constants that
way in name strings, and that this crate did not do it at all.
