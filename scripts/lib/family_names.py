"""Read a family list out of either implementation's output, as bare names.

Both sides print the same list in different notation, and the comparison is
about the order and content rather than the spelling. Kept as a file rather
than inlined in the harness because the transformations need backslashes, and
a `sed` expression written through several layers of shell quoting fails by
silently producing an empty string.

Usage: family_names.py {theirs|ours} < listing
"""

import re
import sys

side = sys.argv[1]
names = []

if side == "theirs":
    # `fc-pattern -c` prints `\tfamily: "Alpha"(w) "Beta"(s)`.
    for line in sys.stdin:
        match = re.match(r"\s*family: (.*)", line)
        if match:
            names = re.findall(r'"([^"]*)"', match.group(1))
            break
else:
    # `fc_match --dump-query` prints `family\tString("Alpha")\tw`, one per line.
    for line in sys.stdin:
        parts = line.rstrip("\n").split("\t")
        if len(parts) >= 2 and parts[0] == "family":
            match = re.match(r'^String\("(.*)"\)$', parts[1])
            names.append(match.group(1) if match else parts[1])

print(" ".join(names))
