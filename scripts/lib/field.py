"""Read one property out of either implementation's listing, as bare values.

Both sides print the same values in their own notation -- fontconfig tags each
with its type and binding, `100(f)(s)`, while this crate prints the Rust value,
`Double(100.0)` -- so both are reduced to a canonical spelling before they are
compared. What is being compared is which values a property ended up with, not
how either side writes them down.

Kept as a file rather than inlined in a harness because the transformations
need backslashes, and a `sed` expression written through several layers of
shell quoting fails by silently producing an empty string.

Usage: field.py {theirs|ours} <object> < listing
"""

import re
import sys

side, obj = sys.argv[1], sys.argv[2]
values = []


def number(text):
    """`100.0` and `100` are the same number and must compare equal."""
    try:
        as_float = float(text)
    except ValueError:
        return text
    return "%g" % as_float


def canonical(text):
    text = text.strip()
    # Ours: the Rust value, as `{:?}` writes it.
    match = re.fullmatch(r"(?:Int|Double)\((.*)\)", text)
    if match:
        return number(match.group(1))
    match = re.fullmatch(r"String\(\"(.*)\"\)", text)
    if match:
        return match.group(1)
    match = re.fullmatch(r"Bool\((.*)\)", text)
    if match:
        return match.group(1)
    match = re.fullmatch(
        r"Range\(Range \{ begin: (.*), end: (.*) \}\)", text)
    if match:
        return "[%s %s]" % (number(match.group(1)), number(match.group(2)))
    # A language set has no order of its own; fontconfig prints it sorted.
    match = re.fullmatch(r"LangSet\((.*)\)", text)
    if match:
        return "|".join(sorted(x for x in match.group(1).split("|") if x))
    match = re.fullmatch(r"CharSet\((.*)\)", text)
    if match:
        return match.group(1)
    # Theirs: the value with its type and binding tags appended.
    match = re.fullmatch(r"\[(\S+) (\S+)\]", text)
    if match:
        return "[%s %s]" % (number(match.group(1)), number(match.group(2)))
    return number(text)


if side == "theirs":
    # `fc-pattern` prints `\twidth: 100(f)(s)`, one line per object, with the
    # values of a multi-valued one separated by spaces. A quoted value can
    # hold spaces of its own, so those are taken first.
    for line in sys.stdin:
        match = re.match(r"\s*" + re.escape(obj) + r": (.*)", line.rstrip("\n"))
        if not match:
            continue
        body = match.group(1)
        if '"' in body:
            values = re.findall(r'"([^"]*)"', body)
        else:
            # Ranges are written `[50 200]` and must not split on the space.
            values = [canonical(v) for v in
                      re.findall(r"\[[^\]]*\]|\S+", re.sub(r"\((?:i|f|s|w)\)", "", body))
                      if v]
        break
else:
    # `fc_match --dump-query` prints `width\tDouble(100.0)\ts`, one per line.
    for line in sys.stdin:
        parts = line.rstrip("\n").split("\t")
        if len(parts) >= 2 and parts[0] == obj:
            values.append(canonical(parts[1]))

print(" ".join(values))
