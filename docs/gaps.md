# What this port does not do yet

Everything measured agrees with fontconfig on this machine, which is a
statement about the corpus and not about the code. This is the list of
things that are knowingly missing, so that "all parity checks pass" can be
read for what it is.

Each entry says what is missing, what breaks because of it, and what would
have to exist to test it. Entries move to **Done** rather than disappearing,
so the history of what was actually wrong stays readable.

The corpus everything is measured against: Fedora 44 under WSL, 695 font
files producing 819 patterns, `/etc/fonts` with 51 configuration files.
Broad -- CJK collections, Type 1, variable fonts, bitmap fonts, a Dingbats
clone -- but one machine.

## Open

### Cache reading and writing

- **mmap.** A cache is read by copying the whole file into memory. Fontconfig
  maps anything over `FC_CACHE_MIN_MMAP` and shares the mapping between
  processes. Costs memory per process, changes no answer.
- **32-bit and big-endian caches.** Rejected rather than misread: the
  architecture is part of the file name (`-le64`), and the header's `size`
  field is written as an `intptr_t`, so a 32-bit cache fails the length check.
  Reading one would mean a second set of field offsets.
- **UUID cache names.** Fontconfig also looks for a cache named by a `.uuid`
  file in the font directory, which is how a read-only image keeps caches
  valid across a path change. We only do the MD5-of-path name.
- **`remap-dir` and `salt`.** Both change the string that gets hashed into
  the cache file name. A configuration using either would send us to the
  wrong file name -- silently, because a missing cache is not an error.
- **Broken-mtime filesystems on Unix.** On FAT and some network filesystems
  fontconfig does not trust the directory mtime and hashes the directory
  listing instead (`FcIsFsMtimeBroken`). On Unix we always trust the mtime,
  so on such a filesystem a cache could stay stale. Windows already takes the
  checksum route -- see below -- so the machinery exists; what is missing is
  detecting *which* Unix filesystems need it.
- **A font replaced in place.** Neither the mtime nor the listing checksum
  changes when a font file is overwritten under the same name, so the cache
  keeps describing the old one until something forces a rescan. Fontconfig
  has the same hole.

### Configuration

- **`<patelt>` with `<langset>`.** A `<selectfont>` selector cannot compare
  against a language set. The selector is deliberately poisoned rather than
  matched, so it rejects nothing instead of accepting everything -- wrong,
  but wrong in the safe direction. Needs a language-set shape in
  `SelectorValue` and a comparison for it.
- **`<range>` literals.** The parser has no range value at all, so a
  `<range>` inside a `<charset>` is skipped. Fontconfig accepts it.
- **Languages outside fontconfig's table.** `FcLangSet` keeps them in an
  `extra` string set; ours is a bitmap over the table and nothing else.
  Exact for subtraction -- a font's own set is a bitmap over the same table,
  so it can never hold one -- and lossy for union.
- **`SOURCE_DATE_EPOCH`.** Fontconfig clamps a directory's recorded mtime to
  this when it is set, for reproducible builds. We record the real mtime.

### Platform differences

- **Windows directory timestamps do not track file changes.** Adding a file
  to a directory does not update that directory's modification time -- not
  after three seconds, not at all as far as `std::fs::metadata` can see --
  while adding a *subdirectory* does. Fontconfig documents the same thing in
  `fcstat.c` and reaches for a different Win32 call. Rather than take a
  dependency for one call, the cache records an Adler-32 of the directory
  listing there instead, which is what fontconfig itself puts in that field
  for filesystems whose timestamps it does not trust.

  The consequence: the number in a cache written on Windows is not an mtime
  and means nothing to any other fontconfig. That costs nothing today, since
  the absolute paths inside a cache already tie it to one machine, but it
  would matter if a cache were ever shipped between them.

## Not attempted

- **Rendering properties.** `FcFontRenderPrepare` is done; the hinting,
  antialias and LCD filter defaults that a toolkit reads come from
  configuration and are carried through, but nothing here rasterizes.
- **The `FcBlanks` API.** Deprecated upstream and unused by the scanner,
  which decides a glyph is blank from its contours.

## Done

- **Writing caches** -- `CacheWriter`, and fontconfig reads what it produces.
- **Scanning fonts** -- SFNT and Type 1, 14595/14595 fields against
  `fc-query`.
- **`target="scan"` rules** -- including `<langset>` literals and set
  arithmetic, without which 22 fonts lost every language they had.
- **Rebuilding a directory tree** -- `Builder`, what `fc-cache` does:
  staleness, atomic replace, `CACHEDIR.TAG`, and walking the subdirectories a
  cache records rather than the filesystem.
