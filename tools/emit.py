"""Write a generated file, or check that the one on disk still matches it.

A generator and its output can drift: someone edits the `.rs` by hand, the
template keeps the old text, and the next person to run the generator
silently reverts the edit. That is not hypothetical -- two of these had done
exactly that, reverting an allocation-free comparison and the ASCII fast
paths in `casefold`, and it was caught only because the files happened to
have been copied aside first.

So `--check` compares without writing. Nothing is at risk when it runs, which
is what lets the test suite run it on every commit.
"""

import io
import sys


def emit(path, *parts):
    """Write the concatenation of `parts` to `path`.

    Each part is a string or an iterable of strings. With `--check` on the
    command line nothing is written: the file is compared instead, and a
    difference exits non-zero naming the first line that differs.
    """
    text = ''.join(part if isinstance(part, str) else ''.join(part) for part in parts)

    if '--check' not in sys.argv[1:]:
        with io.open(path, 'w', encoding='utf-8', newline='\n') as out:
            out.write(text)
        return

    # newline='' so the comparison sees the file's own line endings rather
    # than translated ones; the generator always writes '\n'.
    try:
        with io.open(path, encoding='utf-8', newline='') as f:
            current = f.read()
    except IOError:
        sys.exit('%s: missing, run without --check to generate it' % path)

    if current == text:
        print('%s is current' % path)
        return

    want = text.splitlines()
    got = current.splitlines()
    for number, (a, b) in enumerate(zip(got, want), 1):
        if a != b:
            sys.exit(
                '%s: line %d differs from what the generator produces.\n'
                '  on disk:   %s\n'
                '  generated: %s\n'
                'Either the file was hand-edited and the generator was not '
                'updated to match,\nor the generator changed and the file was '
                'not regenerated.' % (path, number, a.strip(), b.strip())
            )
    sys.exit(
        '%s: %d lines on disk, %d generated -- the generator and its output '
        'have diverged.' % (path, len(got), len(want))
    )
