"""Generate src/name_langs.rs: name-record language ids to language tags.

A `name` table record says which language it is in as a platform-specific
number -- a Windows LCID, or Apple's own code. Fontconfig maps those to
language tags with a table in `fcfreetype.c` that is written in terms of
FreeType's `TT_MS_LANGID_*` and `TT_MAC_LANGID_*` constants, so both files
have to be read: one for the mapping, one for the numbers.

Run from WSL, where the FreeType headers are, from the repo root:
    python3 tools/gen_name_langs.py
"""
import io
import os
import re

FCFREETYPE = 'reference/fc-2.17.0/src/fcfreetype.c'
TTNAMEID = '/usr/include/freetype2/freetype/ttnameid.h'
OUT = 'src/name_langs.rs'

# The constants. Many are aliases for others, and the target is often defined
# *later* in the header, so this collects every definition first and then
# resolves aliases to a fixpoint. Resolving in one pass silently dropped 25
# languages -- Slovenian, Sorbian, Oriya and the rest -- which then went
# unlabelled in every font that carried them.
DEFINE = re.compile(r'^#define\s+(TT_(?:MS|MAC)_LANGID_\w+)\s+(.+?)\s*$')
COMMENT = re.compile(r'/\*.*?\*/')


def clean(text):
    """The value of a #define, with C comments and integer suffixes removed.

    A comment can sit *between* the name and the value -- `/* Arabic */
    0x0460` -- so cutting at the first `/*` leaves nothing, and hex literals
    carry a `U` suffix that int() will not parse. Both silently produced an
    unresolved constant rather than an error.
    """
    text = COMMENT.sub(' ', text).strip()
    return re.sub(r'(?<=[0-9a-fA-F])[UuLl]+$', '', text)
raw = {}
with io.open(TTNAMEID, encoding='utf-8', errors='replace') as f:
    # Definitions are line-continued: the deprecated aliases put the name on
    # one line and the value on the next after a backslash. Reading line by
    # line sees the name and no value.
    text = f.read().replace('\\' + '\n', ' ')
for line in text.splitlines():
    m = DEFINE.match(line.strip())
    if m:
        raw[m.group(1)] = clean(m.group(2))

values = {}
for name, text in raw.items():
    try:
        values[name] = int(text, 0)
    except ValueError:
        pass
# Aliases, chased until nothing new resolves.
while True:
    progress = False
    for name, text in raw.items():
        if name not in values and text in values:
            values[name] = values[text]
            progress = True
    if not progress:
        break

# Constants fontconfig still names that current FreeType has renamed. Only
# unambiguous renames are listed: where the new header offers several
# candidates for an old name -- CHINESE_MACAU, GERMAN_LIECHTENSTEI, the
# Tibetan and Kashmiri regional splits -- guessing which one was meant would
# put a name under the wrong language, so those are left unresolved.
RENAMED = {
    'TT_MS_LANGID_BASQUE_SPAIN': 'TT_MS_LANGID_BASQUE_BASQUE',
    'TT_MS_LANGID_CATALAN_SPAIN': 'TT_MS_LANGID_CATALAN_CATALAN',
    'TT_MS_LANGID_GALICIAN_SPAIN': 'TT_MS_LANGID_GALICIAN_GALICIAN',
    'TT_MS_LANGID_KAZAK_KAZAKSTAN': 'TT_MS_LANGID_KAZAKH_KAZAKHSTAN',
    'TT_MS_LANGID_TATAR_TATARSTAN': 'TT_MS_LANGID_TATAR_RUSSIA',
    'TT_MS_LANGID_WELSH_WALES': 'TT_MS_LANGID_WELSH_UNITED_KINGDOM',
}
for old, new in RENAMED.items():
    if new in values:
        values.setdefault(old, values[new])

# fcFtLanguage[]: { platform, langid, "tag" }
ENTRY = re.compile(
    r'\{\s*TT_PLATFORM_(\w+)\s*,\s*([A-Za-z0-9_]+)\s*,\s*"([^"]*)"\s*\}')
PLATFORMS = {'APPLE_UNICODE': 0, 'MACINTOSH': 1, 'ISO': 2, 'MICROSOFT': 3}

with io.open(FCFREETYPE, encoding='utf-8', errors='replace') as f:
    text = f.read()
start = text.index('fcFtLanguage[]')
end = text.index('};', start)
rows = []
unresolved = []
for platform, langid, tag in ENTRY.findall(text[start:end]):
    if platform not in PLATFORMS or not tag:
        continue
    if langid in values:
        rows.append((PLATFORMS[platform], values[langid], tag))
    elif langid.isdigit() or langid.startswith('0x'):
        rows.append((PLATFORMS[platform], int(langid, 0), tag))
    else:
        unresolved.append(langid)

assert rows, 'no language entries found'
if unresolved:
    print('unresolved constants: %s' % sorted(set(unresolved))[:8])

# Keep the first tag for any (platform, id) that repeats, matching a linear
# search over fontconfig's own table.
seen = set()
table = []
for row in rows:
    key = row[:2]
    if key in seen:
        continue
    seen.add(key)
    table.append(row)
table.sort()

body = []
for i in range(0, len(table), 3):
    body.append('    ' + ' '.join(
        '(%d,%#06x,"%s"),' % r for r in table[i:i + 3]) + '\n')

header = '''//! What language a `name` record is written in.
//!
//! A name record identifies its language by a platform-specific number: a
//! Windows LCID, or one of Apple's own codes. Neither is a language tag, and
//! neither is derivable -- fontconfig carries a table, and so does this.
//!
//! Without it a font's localized names cannot be labelled, and fontconfig
//! labels them: a CJK font lists its family in a dozen languages, each tagged.
//!
//! Generated by `tools/gen_name_langs.py` from fontconfig's `fcFtLanguage`
//! and FreeType's `ttnameid.h`: %d entries.

/// Platform, language id, and the tag fontconfig gives it. Sorted, so a
/// lookup is a binary search.
#[rustfmt::skip]
static LANGUAGES: [(u16, u16, &str); %d] = [
''' % (len(table), len(table))

tail = '''];

/// The language tag for a name record, or `None` if the id is unknown.
///
/// The Unicode platform carries no language of its own; fontconfig treats
/// those records as English.
pub fn tag(platform: u16, language: u16) -> Option<&'static str> {
    if platform == 0 {
        return Some("en");
    }
    LANGUAGES
        .binary_search_by_key(&(platform, language), |(p, l, _)| (*p, *l))
        .ok()
        .map(|index| LANGUAGES[index].2)
}

#[cfg(test)]
mod tests {
    use super::{tag, LANGUAGES};

    #[test]
    fn the_table_is_sorted_for_binary_search() {
        for pair in LANGUAGES.windows(2) {
            assert!(
                (pair[0].0, pair[0].1) < (pair[1].0, pair[1].1),
                "{:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn the_common_english_ids_resolve() {
        assert_eq!(tag(3, 0x0409), Some("en")); // Windows, English (US)
        assert_eq!(tag(1, 0), Some("en"));      // Macintosh, English
        assert_eq!(tag(0, 0), Some("en"));      // Unicode platform
        assert_eq!(tag(0, 12345), Some("en"));  // ...whatever the id says
    }

    /// Regional variants are distinct ids that fontconfig maps to distinct
    /// tags, which is why a single "en" mapping would not do.
    #[test]
    fn regional_variants_are_distinguished() {
        assert_eq!(tag(3, 0x0809), Some("en")); // English (UK)
        assert_eq!(tag(3, 0x0804), Some("zh-cn"));
        assert_eq!(tag(3, 0x0404), Some("zh-tw"));
        assert_ne!(tag(3, 0x0804), tag(3, 0x0404));
    }

    #[test]
    fn an_unknown_id_is_not_guessed_at() {
        assert_eq!(tag(3, 0xfffe), None);
        assert_eq!(tag(9, 0), None);
    }
}
'''

with io.open(OUT, 'w', encoding='utf-8', newline='\n') as out:
    out.write(header)
    out.writelines(body)
    out.write(tail)

print('generated %d name-language entries' % len(table))
