"""Read the binding of every value in a match result, as `object=s|w` lines.

Both implementations print the same listing in different notation. Kept as a
file, and not as a `sed` expression inside the harness, because the patterns
need backslashes and a mangled one fails by producing nothing at all.

Usage: bindings.py {theirs|ours} < listing
"""

import re
import sys

side = sys.argv[1]
out = []

if side == "theirs":
    # `fc-match -v` prints `\tobject: "Value"(s) "Other"(w)`, one line each.
    # Numbers carry a type letter first -- `80(f)(s)` -- and a langset runs to
    # thousands of characters, so the binding is read off the end rather than
    # by parsing the values.
    for line in sys.stdin:
        match = re.match(r"\t([a-z]+): (.*)", line.rstrip("\n"))
        if not match:
            continue
        marks = re.findall(r"\((s|w)\)", match.group(2))
        for mark in marks:
            out.append("%s=%s" % (match.group(1), mark))
else:
    # `fc_match --dump-match` prints `object\tValue\tmark`.
    for line in sys.stdin:
        parts = line.rstrip("\n").split("\t")
        if len(parts) == 3:
            out.append("%s=%s" % (parts[0], parts[2]))

print("\n".join(out))
