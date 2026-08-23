# What this port does not do yet

Everything measured agrees with fontconfig on this machine, which is a
statement about the corpus and not about the code. This is the list of
things that are knowingly missing, so that "all parity checks pass" can be
read for what it is.

Each entry says what is missing, what breaks because of it, and what would
have to exist to test it. Nothing is deleted: an entry moves to **Done**, to
**Decided against** with the reason, or to **Divergences** when matching
fontconfig would have been the worse answer. The history of what was
actually wrong stays readable.

The corpus everything is measured against: Fedora 44 under WSL, 2385 font
files producing 2999 patterns across 336 families and 281 languages,
`/etc/fonts` with 51 configuration files. Broad -- CJK collections, Type 1,
variable fonts, OpenType bitmaps, colour emoji, a Dingbats clone, and a Noto
face for nearly every script there is -- but one machine.

Widening it from 695 files found two divergences that the smaller set had
never reached, one of them a real bug. That is the argument for widening it
again.

Three things need a dependency or an `unsafe` block to do properly, so they
are behind features rather than gone: `mmap`, `statfs`, and
`full-fontconfig-compat` which is both. Without them the crate has one
optional dependency and no `unsafe` at all; the feature documentation in
`Cargo.toml` says what each one buys and costs.

## Open

What is left here is not unimplemented work. It is what the parity numbers
do not cover, so that they are read for what they are.

### Testing

- **One machine is the whole corpus.** Every claim about fontconfig is
  checked against fontconfig itself, on Fedora 44 x86_64 with 695 font files.
  That is the only place an oracle exists; Windows runs the test suite and
  the harnesses that do not need fontconfig. Anything platform-specific --
  the 32-bit layouts, the `statfs` filesystem check, the Windows listing
  checksum -- rests on reading the source rather than on measurement.

  This is not hypothetical. Optimising the charset merge introduced a read of
  a serialized offset as a fixed eight bytes rather than a pointer-sized one:
  correct on this machine, wrong on any 32-bit target, and invisible to every
  check in the repo. `scripts/cross_check.sh` compiled it happily, because
  compiling is all it can do. It was found by reading, which is not a process
  anyone should rely on.

### Found by the corpus, not yet fixed

- **A named instance's full name, on two files.** `NotoEmoji[wght].ttf` and
  `NotoSansHanifiRohingya[wght].ttf`. We rebuild an instance's full name from
  the family and the instance style -- `Noto Emoji Light` -- and fontconfig
  keeps the face's own, `Noto Emoji`. Cantarell goes the other way and wants
  the rebuilt name, which is why the rule is there at all, so the difference
  is in *when* FreeType rebuilds it rather than whether. 2 of 2385 files, in
  a field nothing matches on.
- **Two fc-match answers differ against caches we scanned ourselves.**
  `:lang=ja` and `:lang=hi` pick `NotoSansAdlamUnjoined` where fontconfig
  picks the CJK and Devanagari faces. Only against caches built by scanning:
  the same queries agree against caches we rewrote from fontconfig's own, and
  `fc-list` agrees on all twenty fields of all 2999 patterns either way. So
  the cache *contents* match and something about their order or a field
  fc-list does not print does not. Not yet diagnosed.

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

## Decided against, for now

Not oversights. Each was looked at and left, with what would change the
answer.

- **Testing the 32-bit layouts.** `le32d4` and `le32d8` are derived from the
  target, checked against the five closed forms in `fcarch.c` for every
  shape, and compiled for `i686` and `armv7` with those assertions live --
  but no cache written by a 32-bit fontconfig has ever been read by this
  code. Proving it means qemu: `cross` plus a Fedora container, running the
  existing harnesses. Deferred until a real 32-bit target turns up; the code
  is there so that it does not have to be written under pressure when one
  does. Treat those targets as untested rather than unsupported.
- **Noticing a font replaced in place.** Neither the mtime nor the listing
  checksum changes when a file is overwritten under the same name, so the
  cache keeps describing the old font until something forces a rescan.
  Fontconfig has exactly the same hole, so closing it would make us disagree
  with the thing we are matching. `fc-cache -f` is the answer, there as here.

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
- **The language a locale means.** `FcLangNormalize`: the territory survives
  only when the full tag names a language fontconfig knows, so `zh_CN` stays
  `zh-cn` while `en_US` becomes plain `en`. We had been lowercasing and
  swapping the underscore, which put `en-us` into every query as a default.
  It scored the same and sorted differently, because the language
  satisfaction pass lets one font answer each requested language and an extra
  language lets an extra font through. Found only by widening the corpus.
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
