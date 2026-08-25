# Audits

Three independent audits of this crate against fontconfig, each with its own
log. They were carried out by agents belonging to someone other than this
crate's author, which is the only reason they are worth anything: nothing here
is the author checking their own work.

| log | findings | pinned at | compared against |
| --- | --- | --- | --- |
| [audit-1.md](audit-1.md) | 25 | `cf45c33` | 2.18.3 |
| [audit-2.md](audit-2.md) | 13 | `dedb888` | 2.17.1 |
| [audit-3.md](audit-3.md) | 5 | `08ebc75` | 2.17.1 |

Each log records what came of every finding: what was actually wrong, how it
was checked, and where the fix is. They are kept because a finding marked
"fixed" is worth no more than the evidence behind it — the second audit opened
by catching one in this repository that was marked fixed and was not.

What the sequence shows, read together, is that the findings get narrower each
time: the first audit found whole missing mechanisms, the second found wrong
semantics inside mechanisms that existed, and the third found the fallback
paths that only run when a font is malformed.
