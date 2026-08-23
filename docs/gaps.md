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

Three things need a dependency or an `unsafe` block to do properly, so they
are behind features rather than gone: `mmap`, `statfs`, and
`full-fontconfig-compat` which is both. Without them the crate has one
optional dependency and no `unsafe` at all; the feature documentation in
`Cargo.toml` says what each one buys and costs.

## Open

### Cache reading and writing

- **The 32-bit layouts are derived, not measured.** `le32d4` and `le32d8`
  are computed from the target rather than written down, checked against the
  five closed forms in `fcarch.c` for every shape, and the crate compiles for
  `i686` and `armv7` with those assertions live. But no cache written by a
  32-bit fontconfig has ever been read by this code, and there is no oracle
  for it here. Closing this means qemu: `cross` plus a Fedora container for
  `i686` and `armv7`, running the existing parity harnesses. Until then those
  targets are untested rather than unsupported.
- **A font replaced in place.** Neither the mtime nor the listing checksum
  changes when a font file is overwritten under the same name, so the cache
  keeps describing the old one until something forces a rescan. Fontconfig
  has the same hole, so closing it here would be a divergence, not a fix.

### Testing

- **One machine is the whole corpus.** Every claim about fontconfig is
  checked against fontconfig itself, on Fedora 44 x86_64 with 695 font files.
  That is the only place an oracle exists; Windows runs the test suite and
  the harnesses that do not need fontconfig. Anything platform-specific --
  the 32-bit layouts, the `statfs` filesystem check, the Windows listing
  checksum -- rests on reading the source rather than on measurement.

### Configuration

- **`<name>` targets beyond the pattern and the font.** Everything the
  configuration on this machine uses is covered, but the corpus is one
  machine: a construct no Fedora config happens to contain has never been
  exercised against fontconfig, only against the reference source.

### Platform differences

- **Windows directory timestamps do not track file changes.** Adding a file
  to a directory does not update that directory's modification time -- not
  after three seconds, not at all as far as `std::fs::metadata` can see --
  while adding a *subdirectory* does. Fontconfig documents the same thing in
  `fcstat.c` and reaches for a different Win32 call. Rather than take a
  dependency for one call, the cache records an Adler-32 of the directory
  listing there instead -- the same fallback the `statfs` feature reaches for
  on a Unix filesystem whose timestamps cannot be trusted, and the same one
  fontconfig puts in that field for FAT.

  The consequence: the number in a cache written on Windows is not an mtime
  and means nothing to any other fontconfig. That costs nothing today, since
  the absolute paths inside a cache already tie it to one machine, but it
  would matter if a cache were ever shipped between them.

### Features, not gaps

- **`mmap`.** A mapped cache is shared between every process that reads it,
  which on a desktop is every process that draws text. It needs one `unsafe`
  call and a dependency, and mapping a file another process can rewrite is
  unsound by construction -- fontconfig accepts the same risk for caches over
  1KiB. Off by default.
- **`statfs`.** FAT does not record a directory time fontconfig will trust,
  and neither should anyone: a cache there can stay stale indefinitely.
  Asking needs libc. Without the feature the timestamp is trusted on every
  Unix filesystem, which is right for all of them except the ones the feature
  is for. Windows never trusts it, feature or not.

## Divergences we chose

Places where matching fontconfig exactly would be worse.

- **A clamped `SOURCE_DATE_EPOCH` keeps its cache.** When the pinned time is
  older than the directory -- so the clamp actually fires -- fontconfig
  writes the cache, then validates it by comparing the clamped stamp against
  the *unclamped* directory mtime, concludes it failed, and deletes it. We
  write the cache and keep it, applying the same clamp when reading it back
  so it stays valid. In a real reproducible build the directory mtime is
  already pinned, the clamp never fires, and the two behave identically.

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
- **`<range>` literals**, in a rule and inside a `<charset>`. Finding the gap
  turned up two neighbours that were quietly broken: a `<charset>` or a
  `<matrix>` in a rule expression read a list of children that the parser had
  filled somewhere else, so both had always evaluated to nothing.
- **`<patelt>` with `<langset>`**, and with it `FcLangSetContains`.
- **Languages outside fontconfig's table.** They are kept by name, the way
  `FcLangSet` keeps them. This one was not cosmetic: a `<patelt>` naming
  `en-GB` matched 326 fonts in fontconfig and none here, because `en-GB` has
  no bit and a font listing `en` has to answer for it.
- **`SOURCE_DATE_EPOCH`**, including the two details that are easy to miss:
  the nanoseconds go whenever the variable is set at all, even to something
  unparseable, and the seconds are clamped rather than overwritten.
- **Cache file names** -- `salt`, `<remap-dir>` and `.uuid`. All three change
  which file a cache lives in without changing anything visible, so getting
  one wrong means a silent rescan forever rather than an error. Checked
  against the file fontconfig actually writes, in `scripts/name_parity.sh`.
  The rule that is easy to get backwards: fontconfig takes the *first*
  configured directory containing a path, not the longest, so a plain `<dir>`
  listed before a `<remap-dir>` beneath it shadows the remapping entirely.
- **Mapping a cache instead of reading it**, behind the `mmap` feature.
- **Every layout fontconfig has a name for, at our own endianness.** The
  offsets are a function of pointer size and double alignment rather than a
  table, so the two 32-bit shapes can be checked on a 64-bit machine. Byte
  order is deliberately not translated: see the note in `lib.rs`.
- **Recognising a filesystem whose timestamps lie**, behind `statfs`.
