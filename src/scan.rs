//! Reading a font file into the pattern a cache would record for it.
//!
//! This is the half of fontconfig that fills a cache rather than reading one.
//! Everything else in this crate borrows from bytes fontconfig already wrote;
//! here we have to produce the same answers from the font itself, which is
//! why it is the only part that needs a font parser.
//!
//! One file yields one pattern per face: a `.ttc` has several, a variable font
//! contributes one per named instance, and everything else has exactly one.

use std::path::Path;

use read_fonts::ps::cff::v1::Cff;
use read_fonts::FontRead;
use read_fonts::{
    tables::cmap::CmapSubtable, tables::head::MacStyle, types::Fixed, types::GlyphId, FileRef,
    FontRef, ReadError, TableProvider,
};

use crate::casefold;
use crate::charset::CharSet;
use crate::langset::LangSet;
use crate::object::Object;
use crate::pattern::Pattern;
use crate::value::Value;
use crate::weight;

/// Why a font file could not be scanned.
#[derive(Debug)]
pub enum ScanError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The bytes are not a font this crate understands.
    Unrecognized,
    /// The font is structurally broken.
    Malformed(ReadError),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Unrecognized => f.write_str("not a font file"),
            Self::Malformed(e) => write!(f, "malformed font: {e}"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<std::io::Error> for ScanError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Scan a font file into one pattern per face.
pub fn scan_file(path: &Path) -> Result<Vec<Pattern>, ScanError> {
    let data = std::fs::read(path)?;
    let name = path.to_string_lossy();
    scan_bytes(&data, &name)
}

/// Scan font bytes that came from `path`.
///
/// `path` is recorded as the pattern's `file`; nothing is read from it.
pub fn scan_bytes(data: &[u8], path: &str) -> Result<Vec<Pattern>, ScanError> {
    // Type 1 fonts are not SFNT and have to be recognised first: they begin
    // with `%!` for the plain text form, or the PFB segment marker.
    if is_type1(data) {
        return scan_type1(data, path);
    }
    // A web font is an SFNT with its tables compressed, so unpacking it is
    // the whole job: what comes out goes through exactly the same scan, and
    // keeps the path it came from. FreeType does the same thing, which is why
    // fontconfig lists a `.woff` at all.
    #[cfg(feature = "woff")]
    if let Some(unpacked) = unpack_woff(data) {
        return scan_bytes(&unpacked, path);
    }
    // A bare CFF is not an SFNT and has no wrapper to recognise it by, so it
    // is tried after the formats that do.
    if is_bare_cff(data) {
        return scan_cff(data, path);
    }
    match FileRef::new(data) {
        Ok(FileRef::Font(font)) => Ok(scan_face(&font, path, 0)),
        Ok(FileRef::Collection(collection)) => {
            let mut patterns = Vec::new();
            for index in 0..collection.len() {
                let Ok(font) = collection.get(index) else { continue };
                patterns.extend(scan_face(&font, path, index as i32));
            }
            Ok(patterns)
        }
        Err(_) => Err(ScanError::Unrecognized),
    }
}

/// Unpack a WOFF or WOFF2 into the SFNT it wraps.
///
/// `None` when these bytes are neither, so the caller falls through to the
/// formats it already knows. A file that announces itself as a web font and
/// then will not decompress returns `None` as well: it is not an SFNT either,
/// and the caller's "not a font file" is the right answer for it.
///
/// Recursion is not a risk. What comes out is an SFNT by construction -- both
/// decoders build one -- so the caller's second pass takes the `FileRef`
/// branch, and a WOFF wrapping a WOFF is not a thing either decoder will
/// produce.
#[cfg(feature = "woff")]
fn unpack_woff(data: &[u8]) -> Option<Vec<u8>> {
    match data.get(..4)? {
        b"wOFF" => wuff::decompress_woff1(data).ok(),
        b"wOF2" => wuff::decompress_woff2(data).ok(),
        _ => None,
    }
}

/// Whether these bytes are a bare CFF -- a `CFF ` table on its own.
///
/// A CFF header is four bytes: major, minor, header size, absolute offset
/// size. There is no signature, so this checks what the specification
/// constrains: version 1.x for bare CFF, a header of at least four bytes, and
/// an offset size in 1..=4. CFF2 is deliberately not accepted, since it
/// carries no names or metrics of its own -- it exists inside an OpenType
/// font and is read from there.
fn is_bare_cff(data: &[u8]) -> bool {
    matches!(data.get(..4), Some([1, _, header, offsets]) if *header >= 4 && (1..=4).contains(offsets))
}

/// Whether these bytes are a Type 1 font rather than an SFNT.
///
/// `%!` opens the plain PFA form; `0x80 0x01` opens a PFB segment header.
fn is_type1(data: &[u8]) -> bool {
    data.starts_with(b"%!") || data.starts_with(&[0x80, 0x01])
}

// --- SFNT ------------------------------------------------------------------

fn base_pattern(font: &FontRef<'_>, path: &str, index: i32) -> Pattern {
    let mut pattern = Pattern::new();

    let has_cff = has_table(font, b"CFF ") || has_table(font, b"CFF2");
    let has_color = [b"COLR", b"SVG ", b"CBLC", b"sbix"].iter().any(|tag| has_table(font, tag));
    let has_outlines = has_outlines(font);

    pattern.add(Object::File, path);
    pattern.add(Object::Index, index);
    pattern.add(Object::FontWrapper, "SFNT");
    // A font with CFF outlines reports CFF even when it also has glyf.
    pattern.add(Object::Fontformat, if has_cff { "CFF" } else { "TrueType" });
    pattern.add(Object::Outline, has_outlines);
    pattern.add(Object::Color, has_color);
    // A colour font with no outlines is still scalable: it draws at any size.
    pattern.add(Object::Scalable, has_outlines || has_color);
    pattern.add(Object::FontHasHint, has_hinting(font));
    pattern.add(Object::Order, 0);

    // The revision is a 16.16 fixed number, stored as its raw bits.
    let version = font.head().map(|head| head.font_revision().to_bits()).unwrap_or(0);
    pattern.add(Object::Fontversion, version);

    pattern.add(Object::Foundry, foundry(font));
    if let Some(capability) = capability(font) {
        pattern.add(Object::Capability, capability.as_str());
    }
    // Names first: the slant and whether the face is decorative are both
    // read off the style name.
    add_names(font, &mut pattern);
    pattern.add(Object::Decorative, is_decorative(&pattern));
    pattern.add(Object::Symbol, is_symbol(font));
    if let Some(spacing) = spacing(font) {
        pattern.add(Object::Spacing, spacing);
    }
    add_coverage(sfnt_coverage(font), is_symbol(font), exclusive_lang(font), &mut pattern);
    pattern
}

/// Record what a font covers, and what that lets it write.
///
/// The language set is derived from the coverage rather than declared by the
/// font: fontconfig asks, for each language it knows an orthography for,
/// whether every codepoint that language needs is present.
fn add_coverage(coverage: CharSet, symbol: bool, exclusive: Option<usize>, pattern: &mut Pattern) {
    if coverage.is_empty() {
        return;
    }
    // "Symbol fonts don't cover any language, even though they claim to
    // support Latin1 range" -- fontconfig builds an empty set for one rather
    // than asking what its private-use codepoints imply. The Latin1 range it
    // means is the copy made just above, which would otherwise answer for
    // most of Europe.
    let langs = if symbol {
        LangSet::new()
    } else {
        LangSet::from_char_set_exclusive(&coverage, exclusive)
    };
    pattern.add(Object::Charset, Value::CharSet(coverage));
    // Always, even when the set is empty. A font covering a script
    // fontconfig has no language for -- Adlam, and a dozen others -- gets an
    // empty language set rather than none at all, and the difference is not
    // cosmetic: scoring walks the properties the two sides share, so a font
    // with *no* language says nothing about language and ties with one that
    // answers perfectly, while a font with an empty one is scored as
    // answering nothing. `fc-list` prints both as the empty string, which is
    // why this took a corpus with Adlam in it to notice.
    pattern.add(Object::Lang, Value::LangSet(langs));
}

/// The single CJK language the font's `OS/2` codepage bits declare, if one.
///
/// Read from `ulCodePageRange1`, which a font without an `OS/2` table or with
/// a version that predates the field does not have -- and then nothing is
/// declared, which is the same as declaring several.
fn exclusive_lang(font: &FontRef<'_>) -> Option<usize> {
    // The codepage ranges arrived in version 1, so upstream requires
    // `version >= 1 && version != 0xffff` before reading them (`:1747`).
    let os2 = usable_os2(font).filter(|os2| os2.version() >= 1)?;
    crate::langset::exclusive_from_code_pages(os2.ul_code_page_range_1()?)
}

/// Whether the font addresses its glyphs through a symbol encoding.
///
/// FreeType picks a Unicode `cmap` where there is one and falls back to
/// whatever the font has, so a font is a symbol font when the (3, 0) table
/// is the only one it offers. Those glyphs live at `U+F000` and up and mean
/// nothing outside the font, which is why fontconfig records it and scores
/// against it: a query wanting text should not be answered with dingbats.
fn is_symbol(font: &FontRef<'_>) -> bool {
    use read_fonts::tables::cmap::PlatformId;
    let Ok(cmap) = font.cmap() else { return false };
    let mut symbol = false;
    for record in cmap.encoding_records() {
        match (record.platform_id(), record.encoding_id()) {
            (PlatformId::Unicode, _) | (PlatformId::Windows, 1 | 10) => return false,
            (PlatformId::Windows, 0) => symbol = true,
            _ => {}
        }
    }
    symbol
}

/// Whether the style names the font as a decorative variant.
///
/// A short list of words, matched anywhere in any style value and without
/// regard to case. `embosed` is spelled that way in fontconfig; it is a typo
/// there and copying it is the only way to agree with it.
fn is_decorative(pattern: &Pattern) -> bool {
    const WORDS: [&str; 6] = ["shadow", "caps", "antiqua", "romansc", "embosed", "dunhill"];
    let Some(element) = pattern.get(Object::Style) else { return false };
    element.values().any(|(value, _)| {
        let crate::value::Value::String(style) = value else { return false };
        let lowered = style.to_lowercase();
        WORDS.iter().any(|word| lowered.contains(word))
    })
}

/// What complex-script machinery the font carries, as fontconfig words it.
///
/// `ttable:Silf` for a Graphite font, then `otlayout:<tag>` for every script
/// the `GSUB` and `GPOS` tables between them declare. The two lists are
/// merged rather than concatenated, so a script both tables support is named
/// once, and a tag that is not alphanumeric is skipped as broken.
///
/// Nothing is scored against this -- it has no priority slot -- but callers
/// read it to decide whether a font can shape a script at all.
fn capability(font: &FontRef<'_>) -> Option<String> {
    use read_fonts::tables::layout::ScriptList;
    use read_fonts::types::Tag;
    use read_fonts::ReadError;

    // Fontconfig reads this only from a font that has an OS/2 table.
    font.os2().ok()?;

    // Substitution and positioning each name the scripts they know how to
    // shape, and the two lists have the same shape -- but they are separate
    // tables, so each is read as itself. Their headers happen to agree on
    // where the script list lives, which makes reading one through the other
    // work today and a silent misread the day either header grows.
    fn script_tags(list: Result<ScriptList<'_>, ReadError>) -> Vec<Tag> {
        let Ok(list) = list else { return Vec::new() };
        list.script_records().iter().map(|record| record.script_tag()).collect()
    }
    let gsub = font.gsub().map(|table| script_tags(table.script_list())).unwrap_or_default();
    let gpos = font.gpos().map(|table| script_tags(table.script_list())).unwrap_or_default();

    let mut out = String::new();
    if font.table_data(Tag::new(b"Silf")).is_some() {
        out.push_str("ttable:Silf");
    }
    if gsub.is_empty() && gpos.is_empty() && out.is_empty() {
        return None;
    }

    // Both lists are in table order, which OpenType requires to be sorted by
    // tag, so this is a merge and not a sort.
    let (mut i, mut j) = (0usize, 0usize);
    let add = |out: &mut String, tag: Tag| {
        let bytes = tag.to_be_bytes();
        // A space counts as valid: a three-letter script tag is padded with
        // one, and `lao ` and `nko ` are written out with it still attached.
        // Only a tag with something else in it is treated as broken.
        let valid = |b: &u8| b.is_ascii_alphanumeric() || *b == b' ';
        if !bytes.iter().all(valid) {
            return;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str("otlayout:");
        out.push_str(&String::from_utf8_lossy(&bytes));
    };
    while i < gsub.len() || j < gpos.len() {
        match (gsub.get(i), gpos.get(j)) {
            (Some(a), Some(b)) if a == b => {
                add(&mut out, *a);
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if a < b => {
                add(&mut out, *a);
                i += 1;
            }
            (Some(_), Some(b)) => {
                add(&mut out, *b);
                j += 1;
            }
            (Some(a), None) => {
                add(&mut out, *a);
                i += 1;
            }
            (None, Some(b)) => {
                add(&mut out, *b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Every character an SFNT font maps, from its Unicode `cmap` subtables.
fn sfnt_coverage(font: &FontRef<'_>) -> CharSet {
    let mut coverage = CharSet::new();
    let empty = EmptyGlyphs::new(font);
    walk_mappings(font, |code, gid| {
        // A mapping to glyph 0 is a mapping to `.notdef`, the absence of a
        // glyph rather than the presence of one.
        if gid.to_u32() == 0 {
            return;
        }
        // A control character counts only if its glyph actually draws.
        if code <= 0x1f && empty.is_empty(gid) {
            return;
        }
        if let Some(c) = char::from_u32(code) {
            coverage.insert(c);
        }
    });

    // A symbol font addresses its glyphs at U+F000 and up, and Windows also
    // reaches them at the same offsets from zero. Fontconfig copies the range
    // down so that either spelling finds the glyph, citing the OpenType
    // recommendations for non-standard fonts.
    if is_symbol(font) {
        for code in 0xf000..0xf100u32 {
            let (Some(high), Some(low)) = (char::from_u32(code), char::from_u32(code - 0xf000))
            else {
                continue;
            };
            if coverage.contains(high) {
                coverage.insert(low);
            }
        }
    }
    coverage
}

/// Call `visit` with every codepoint the font's Unicode subtables map, and
/// the glyph it maps to.
fn walk_mappings(font: &FontRef<'_>, mut visit: impl FnMut(u32, read_fonts::types::GlyphId)) {
    use read_fonts::tables::cmap::PlatformId;
    let Ok(cmap) = font.cmap() else {
        return;
    };

    // Fontconfig tries `FT_ENCODING_UNICODE` and then `FT_ENCODING_MS_SYMBOL`,
    // and stops at the first that selects: a font with a Unicode subtable is
    // read through it alone, and only a font without one is read through its
    // symbol table. `is_symbol` decides the same way, so the two agree on
    // which kind of font this is.
    let mut walked = false;
    for record in cmap.encoding_records() {
        let unicode = matches!(
            (record.platform_id(), record.encoding_id()),
            (PlatformId::Unicode, _) | (PlatformId::Windows, 1 | 10)
        );
        if !unicode {
            continue;
        }
        let Ok(subtable) = record.subtable(cmap.offset_data()) else {
            continue;
        };
        walk_subtable(&subtable, &mut visit);
        walked = true;
    }
    if walked {
        return;
    }

    // No Unicode subtable, so the symbol one is what the font has. Its
    // codepoints are private-use and mean nothing outside the font, but they
    // are still what it covers, and dropping them leaves a font that appears
    // to cover nothing at all.
    for record in cmap.encoding_records() {
        if (record.platform_id(), record.encoding_id()) != (PlatformId::Windows, 0) {
            continue;
        }
        if let Ok(subtable) = record.subtable(cmap.offset_data()) {
            walk_subtable(&subtable, &mut visit);
        }
    }
}

/// Which glyphs draw nothing.
///
/// Only the ASCII control range needs this: CID fonts built by Adobe map
/// control characters to the blank space glyph, and fontconfig excludes a
/// control character whose glyph has no contours rather than claiming the
/// font covers it.
struct EmptyGlyphs<'a> {
    loca: Option<read_fonts::tables::loca::Loca<'a>>,
    /// Whether the font carries CFF outlines, when it has no `glyf`.
    cff: bool,
}

impl<'a> EmptyGlyphs<'a> {
    fn new(font: &FontRef<'a>) -> Self {
        Self {
            loca: font.loca(None).ok(),
            cff: has_table(font, b"CFF ") || has_table(font, b"CFF2"),
        }
    }

    /// Whether `glyph` draws nothing.
    ///
    /// Three cases, because fontconfig's test is `FT_Load_Glyph` failing *or*
    /// returning an outline with no contours, and those are different fonts.
    ///
    /// A `glyf` outline of zero length has no contours. A CFF charstring
    /// would have to be executed to know, so those are assumed to draw --
    /// which matches every font checked here. A font with neither has no
    /// outline to load at all: `FT_Load_Glyph` fails, and fontconfig treats
    /// that exactly as it treats an empty one. A colour bitmap font is the
    /// common shape, and it is the one that made this matter -- Ubuntu's
    /// `NotoColorEmoji.ttf` maps `U+0000` and `U+000D`, which fontconfig
    /// drops and this crate was keeping.
    fn is_empty(&self, glyph: read_fonts::types::GlyphId) -> bool {
        match &self.loca {
            Some(loca) => loca
                .get_raw(glyph.to_u32() as usize)
                .zip(loca.get_raw(glyph.to_u32() as usize + 1))
                .is_some_and(|(start, end)| start == end),
            None => !self.cff,
        }
    }
}

/// Call `visit` for every codepoint one subtable maps.
fn walk_subtable(
    subtable: &CmapSubtable<'_>,
    visit: &mut impl FnMut(u32, read_fonts::types::GlyphId),
) {
    let mut add = |code: u32| {
        if let Some(gid) = subtable.map_codepoint(code) {
            visit(code, gid);
        }
    };
    match subtable {
        CmapSubtable::Format4(table) => {
            for (start, end) in table.start_code().iter().zip(table.end_code()) {
                let (start, end) = (start.get(), end.get());
                // 0xffff closes the segment list and is not a character.
                if start == 0xffff {
                    continue;
                }
                for code in start..=end {
                    add(u32::from(code));
                }
            }
        }
        CmapSubtable::Format12(table) => {
            for group in table.groups() {
                for code in group.start_char_code()..=group.end_char_code() {
                    add(code);
                }
            }
        }
        CmapSubtable::Format6(table) => {
            let first = u32::from(table.first_code());
            for offset in 0..u32::from(table.entry_count()) {
                add(first + offset);
            }
        }
        CmapSubtable::Format0(_) => {
            for code in 0..256u32 {
                add(code);
            }
        }
        _ => {}
    }
}

/// One pattern per face, expanding a variable font into its instances.
///
/// A variable font is not one font. Fontconfig records each named instance as
/// its own pattern with concrete values, plus one pattern for the variable
/// font itself whose weight and width are *ranges* -- that last one is what
/// lets a query for any weight in between match at all.
fn scan_face(font: &FontRef<'_>, path: &str, index: i32) -> Vec<Pattern> {
    let base = base_pattern(font, path, index);
    let Some(instances) = named_instances(font) else {
        let mut pattern = base;
        add_attributes(font, &mut pattern, None);
        add_optical_size(font, &mut pattern, false);
        pattern.add(Object::Variable, false);
        pattern.add(Object::NamedInstance, false);
        return vec![pattern];
    };

    let mut patterns = Vec::with_capacity(instances.len() + 1);

    // The default instance first, as the font comes out of the box.
    let mut default = base.clone();
    add_attributes(font, &mut default, None);
    // The default face sits at the `opsz` axis default, which is a single
    // size rather than the whole span the variable pattern reports.
    let axis_default = axis_default(font, b"opsz");
    if let Some(size) = axis_default {
        default.add(Object::Size, size);
    }
    add_optical_size(font, &mut default, axis_default.is_some());
    default.add(Object::Variable, false);
    default.add(Object::NamedInstance, false);
    patterns.push(default);

    for (ordinal, instance) in instances.iter().enumerate() {
        // The named instance that *is* the default is already covered, and
        // fontconfig skips it -- which is visible in the index values it
        // writes, where one ordinal is missing from the run.
        if instance.is_default {
            continue;
        }
        let mut pattern = base.clone();
        add_attributes(font, &mut pattern, Some(instance));
        if let Some(size) = instance.size {
            pattern.add(Object::Size, size);
        }
        add_optical_size(font, &mut pattern, instance.size.is_some());
        pattern.add(Object::Variable, false);
        pattern.add(Object::NamedInstance, true);
        // The index carries the instance: ordinal in the high half, face in
        // the low half, with ordinals starting at one so zero stays "the
        // default instance of this face".
        pattern.remove(Object::Index);
        pattern.add(Object::Index, (((ordinal as i32) + 1) << 16) | index);

        // The instance names itself, and that name replaces the style.
        if let Some(style) = name_by_id(font, instance.subfamily) {
            pattern.remove(Object::Style);
            pattern.remove(Object::Stylelang);
            pattern.add(Object::Style, style.as_str());
            pattern.add(Object::Stylelang, "en");
            pattern.remove(Object::Slant);
            // The instance renames the face, so the slant is reconsidered
            // against the new style name; the flags are the font's either way.
            pattern.add(Object::Slant, slant(&pattern, style_flags(font).1));

            // And the full name is reconsidered, because name id 4 describes
            // the face the file is named for rather than this instance. Only
            // id 18 carries over; if the font has none, a name is built from
            // the family and the instance style.
            pattern.remove(Object::Fullname);
            pattern.remove(Object::Fullnamelang);
            for (text, lang) in collect_names(font, &INSTANCE_FULLNAME_IDS) {
                pattern.add(Object::Fullname, text.as_str());
                pattern.add(Object::Fullnamelang, lang);
            }
            add_synthetic_fullname(&mut pattern);
        }
        // An instance may name itself in the `name` table; most do not, and
        // the name is then built from the family's PostScript name plus the
        // instance's own, with spaces removed.
        let ps = instance
            .postscript
            .filter(|id| *id == 6 || (256..32768).contains(id))
            .and_then(|id| name_by_id(font, id))
            .or_else(|| synthesized_postscript_name(font, instance));
        if let Some(ps) = ps {
            pattern.remove(Object::PostscriptName);
            pattern.add(Object::PostscriptName, ps.as_str());
        }
        patterns.push(pattern);
    }

    // Finally the variable font itself, carrying ranges rather than values.
    let mut variable = base;
    let variable_size = add_variable_attributes(font, &mut variable);
    add_optical_size(font, &mut variable, variable_size);
    variable.add(Object::Variable, true);
    variable.add(Object::NamedInstance, false);
    // A variable pattern is not one face, so it carries none of the things
    // that name one.
    variable.remove(Object::Fullname);
    variable.remove(Object::Fullnamelang);
    variable.remove(Object::Style);
    variable.remove(Object::Stylelang);
    variable.remove(Object::PostscriptName);
    patterns.push(variable);

    patterns
}

/// One named instance: which axis values it pins, and what it calls itself.
struct Instance {
    subfamily: u16,
    postscript: Option<u16>,
    /// `wght` as a multiple of the axis default, not the axis value.
    ///
    /// `FcFreeTypeQueryFaceInternal` computes `mult = value / default` and
    /// applies it to `usWeightClass`, so an instance's weight is the *face's*
    /// weight scaled by how far along the axis it sits. The two agree only
    /// when `OS/2` agrees with the `fvar` default, which a variable font whose
    /// default master is not Regular does not.
    weight: Option<f64>,
    /// `wdth` as a multiple of the axis default, for the same reason.
    width: Option<f64>,
    /// The `opsz` coordinate, which becomes `size` directly.
    size: Option<f64>,
    /// Every axis it pins, for naming.
    axes: Vec<Axis>,
    /// Whether it pins every axis to that axis's default.
    is_default: bool,
}

/// One axis of a named instance: what it is called, where it is pinned, and
/// where it would sit by default.
struct Axis {
    tag: String,
    value: f64,
    default: f64,
}

/// The font's named instances, or `None` if it is not variable.
fn named_instances(font: &FontRef<'_>) -> Option<Vec<Instance>> {
    let fvar = font.fvar().ok()?;
    let axes = fvar.axes().ok()?;
    let instances = fvar.instances().ok()?;
    let mut out = Vec::new();
    for instance in instances.iter().flatten() {
        let coord = |tag: &[u8; 4]| -> Option<f64> {
            let wanted = read_fonts::types::Tag::new(tag);
            let index = axes.iter().position(|axis| axis.axis_tag() == wanted)?;
            Some(instance.coordinates.get(index)?.get().to_f64())
        };
        let is_default = axes.iter().enumerate().all(|(i, axis)| {
            instance.coordinates.get(i).is_some_and(|c| c.get() == axis.default_value())
        });
        let pinned: Vec<Axis> = axes
            .iter()
            .enumerate()
            .filter_map(|(i, axis)| {
                Some(Axis {
                    tag: axis.axis_tag().to_string(),
                    value: instance.coordinates.get(i)?.get().to_f64(),
                    default: axis.default_value().to_f64(),
                })
            })
            .collect();
        // `mult = default ? value / default : 1`, which is what upstream
        // applies to the OS/2 classes.
        let multiplier = |tag: &[u8; 4]| -> Option<f64> {
            let wanted = read_fonts::types::Tag::new(tag);
            let index = axes.iter().position(|axis| axis.axis_tag() == wanted)?;
            let value = instance.coordinates.get(index)?.get().to_f64();
            let default = axes.get(index)?.default_value().to_f64();
            Some(if default != 0.0 { value / default } else { 1.0 })
        };
        out.push(Instance {
            subfamily: instance.subfamily_name_id.to_u16(),
            postscript: instance.post_script_name_id.map(|id| id.to_u16()),
            weight: multiplier(b"wght"),
            width: multiplier(b"wdth"),
            size: coord(b"opsz"),
            axes: pinned,
            is_default,
        });
    }
    Some(out)
}

/// The variable axes as ranges, for the variable pattern.
///
/// Returns whether an `opsz` axis was found, which decides whether the `OS/2`
/// optical size is consulted afterwards: upstream's `variable_size` guard.
fn add_variable_attributes(font: &FontRef<'_>, pattern: &mut Pattern) -> bool {
    add_attributes(font, pattern, None);
    let Ok(fvar) = font.fvar() else { return false };
    let Ok(axes) = fvar.axes() else { return false };
    let mut variable_size = false;
    for axis in axes.iter() {
        // `opsz` is in points, which is what `size` is in, so it needs no
        // conversion -- unlike `wght`, which is OpenType's scale.
        let (object, convert): (Object, fn(f64) -> f64) = match &axis.axis_tag().to_be_bytes() {
            b"wght" => (Object::Weight, weight::from_opentype),
            b"wdth" => (Object::Width, |v| v),
            b"opsz" => {
                variable_size = true;
                (Object::Size, |v| v)
            }
            _ => continue,
        };
        let range = crate::value::Range {
            begin: convert(axis.min_value().to_f64()),
            end: convert(axis.max_value().to_f64()),
        };
        pattern.remove(object);
        pattern.add(object, crate::value::Value::Range(range));
    }
    variable_size
}

/// One axis's default value, if the font has that axis.
fn axis_default(font: &FontRef<'_>, tag: &[u8; 4]) -> Option<f64> {
    let wanted = read_fonts::types::Tag::new(tag);
    let axes = font.fvar().ok()?.axes().ok()?;
    axes.iter().find(|axis| axis.axis_tag() == wanted).map(|axis| axis.default_value().to_f64())
}

/// The optical size the font declares, in points.
///
/// Four sources, in upstream's order. A variable face reports the `opsz`
/// axis's whole span; a named instance reports the coordinate it pins; the
/// default face reports the axis default. Only when none of those applies --
/// `variable_size` false -- does `OS/2` version 5 get a turn, where the two
/// fields are *twips*, a twentieth of a point each, and equal bounds mean a
/// single size rather than an empty range.
///
/// No font in the 2385 this crate is measured against declares one at all,
/// which is why this was missing entirely.
fn add_optical_size(font: &FontRef<'_>, pattern: &mut Pattern, variable_size: bool) {
    if variable_size {
        return;
    }
    let Some(os2) = usable_os2(font).filter(|os2| os2.version() >= 5) else { return };
    let (Some(lower), Some(upper)) =
        (os2.us_lower_optical_point_size(), os2.us_upper_optical_point_size())
    else {
        return;
    };
    let (lower, upper) = (f64::from(lower) / 20.0, f64::from(upper) / 20.0);
    pattern.remove(Object::Size);
    if lower == upper {
        pattern.add(Object::Size, lower);
    } else {
        pattern.add(Object::Size, crate::value::Range { begin: lower, end: upper });
    }
}

/// The PostScript name an instance gets when it does not carry one.
///
/// The name is a prefix and a suffix. The prefix is the font's variations
/// name prefix, or failing that its family name, keeping only alphanumerics:
/// `Vazirmatn NL` gives `VazirmatnNL`. The suffix is normally the instance's
/// own subfamily name after a hyphen -- `Cantarell-ExtraBold`.
///
/// When the subfamily name cannot be read the axes name the instance instead:
/// each axis that is not at its default contributes an underscore, its value,
/// and its tag. Vazirmatn's instances are `VazirmatnNL_100wght` for exactly
/// that reason -- its `fvar` points at name records the font does not have.
fn synthesized_postscript_name(font: &FontRef<'_>, instance: &Instance) -> Option<String> {
    let prefix: String = name_by_id(font, 25)
        .or_else(|| name_by_id(font, 16))
        .or_else(|| name_by_id(font, 1))?
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();

    if let Some(subfamily) = name_by_id(font, instance.subfamily) {
        let suffix: String = subfamily.chars().filter(char::is_ascii_alphanumeric).collect();
        return Some(format!("{prefix}-{suffix}"));
    }

    let mut name = prefix;
    for axis in &instance.axes {
        if axis.value == axis.default {
            continue;
        }
        name.push('_');
        name.push_str(&format_axis_value(axis.value));
        name.extend(axis.tag.chars().filter(char::is_ascii_alphanumeric));
    }
    Some(name)
}

/// An axis coordinate as the shortest decimal that represents it.
fn format_axis_value(value: f64) -> String {
    if value == value.trunc() {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// One name record by id, preferring a language we can name.
fn name_by_id(font: &FontRef<'_>, id: u16) -> Option<String> {
    collect_names(font, &[id])
        .into_iter()
        .next()
        .map(|(text, _)| text)
        .or_else(|| any_name(font, id))
}

/// One name record by id, whatever language it is filed under.
fn any_name(font: &FontRef<'_>, id: u16) -> Option<String> {
    let name = font.name().ok()?;
    let data = name.string_data();
    for platform in PLATFORM_ORDER {
        for record in name.name_record() {
            if record.name_id().to_u16() != id || record.platform_id() != platform {
                continue;
            }
            let Ok(string) = record.string(data) else { continue };
            let text = string.chars().collect::<String>().trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn has_table(font: &FontRef<'_>, tag: &[u8; 4]) -> bool {
    font.table_data(read_fonts::types::Tag::new(tag)).is_some()
}

fn table_len(font: &FontRef<'_>, tag: &[u8; 4]) -> usize {
    font.table_data(read_fonts::types::Tag::new(tag)).map_or(0, |data| data.len())
}

/// Whether the font carries hinting instructions.
///
/// `fpgm` or `cvt ` settles it. A `prep` table does not, unless it is longer
/// than seven bytes: tools that strip hinting leave a stub behind, and
/// fontconfig added the length check so a de-hinted font does not claim to be
/// hinted. A CFF font hints through its private dictionary and is not
/// detected at all, which is also what fontconfig reports.
fn has_hinting(font: &FontRef<'_>) -> bool {
    if has_table(font, b"fpgm") || has_table(font, b"cvt ") {
        return true;
    }
    font.table_data(read_fonts::types::Tag::new(b"prep")).is_some_and(|data| data.len() > 7)
}

/// The foundry: the `OS/2` vendor tag, else the copyright notice, else
/// `unknown`.
///
/// The tag is taken *verbatim* whenever its first byte is not NUL -- padding
/// included. GNU FreeFont's foundry really is `"GNU "` with a trailing space,
/// and Vazirmatn's is four spaces, not `unknown`. Trimming either one is a
/// different answer, and the only thing that makes it look like a tidy-up is
/// that the difference does not print.
fn foundry(font: &FontRef<'_>) -> String {
    // Guarded like every other `OS/2` reader: a table marked version `0xffff`
    // has no vendor tag worth reading, and upstream checks at `:1353`.
    if let Some(os2) = usable_os2(font) {
        let bytes = os2.ach_vend_id().to_be_bytes();
        if bytes[0] != 0 {
            return bytes.iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
        }
    }
    // No vendor tag: fall back to whoever the notice names.
    collect_names(font, &[7, 0])
        .iter()
        .find_map(|(text, _)| notice_to_foundry(text))
        .unwrap_or("unknown")
        .to_string()
}

/// Whether the font draws with outlines rather than bitmaps.
///
/// A `glyf` table that is present but *empty* means no outlines at all, which
/// is exactly how an OpenType bitmap font (`.otb`) is built -- Terminus ships
/// one, a zero-length `glyf` beside its `EBDT`. Testing for the table rather
/// than its contents calls such a font scalable, and worse, sends
/// [`style_flags`] down the wrong branch: Terminus Bold sets the italic bit
/// in `fsSelection` and is not italic.
fn has_outlines(font: &FontRef<'_>) -> bool {
    table_len(font, b"glyf") > 0 || has_table(font, b"CFF ") || has_table(font, b"CFF2")
}

/// The `OS/2` table, if the font has one worth reading.
///
/// Version `0xffff` is Adobe's "this table means nothing" marker, and every
/// consumer in `fcfreetype.c` guards on it -- weight and width at `:1751`,
/// the foundry at `:1353`, the codepage ranges at `:1747`, the optical size
/// at `:1785`. A font that sets it falls into the style-name fallbacks below
/// exactly as if the table were absent.
fn usable_os2<'a>(font: &FontRef<'a>) -> Option<read_fonts::tables::os2::Os2<'a>> {
    font.os2().ok().filter(|os2| os2.version() != 0xffff)
}

/// Whether the font declares itself bold and italic, as FreeType decides it.
///
/// `sfobjs.c` computes `style_flags` from `OS/2.fsSelection` -- italic is bit
/// 0, bold is bit 5 -- but only for a font that has outlines and a usable
/// `OS/2`. Anything else, a bitmap-only font included, falls back to
/// `head.macStyle`, where the two bits are the other way round: bold is 0 and
/// italic is 1.
///
/// The distinction is not academic in either direction. DejaVu Sans with the
/// italic bit set in `fsSelection` and cleared in `macStyle` scans as italic;
/// Terminus Bold, which is a bitmap `.otb` with the italic bit set in
/// `fsSelection`, does not -- it has no outlines, so only `macStyle` is read,
/// and that says bold and nothing else.
fn style_flags(font: &FontRef<'_>) -> (bool, bool) {
    use read_fonts::tables::os2::SelectionFlags;

    if has_outlines(font) {
        if let Some(os2) = usable_os2(font) {
            let flags = os2.fs_selection();
            return (flags.contains(SelectionFlags::BOLD), flags.contains(SelectionFlags::ITALIC));
        }
    }
    match font.head() {
        Ok(head) => {
            let style = head.mac_style();
            (style.contains(MacStyle::BOLD), style.contains(MacStyle::ITALIC))
        }
        Err(_) => (false, false),
    }
}

/// Weight, width, slant and spacing, from `OS/2` and `post`.
///
/// Each of the three has a fallback chain rather than a single source, and
/// the chains only run for fonts that are missing something -- which is
/// precisely why they were wrong here: nothing in a healthy corpus exercises
/// them.
fn add_attributes(font: &FontRef<'_>, pattern: &mut Pattern, instance: Option<&Instance>) {
    let os2 = usable_os2(font);
    let (bold, italic) = style_flags(font);
    let styles: Vec<String> = pattern.get(Object::Style).map_or_else(Vec::new, |element| {
        element
            .values()
            .filter_map(|(value, _)| match value {
                Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    });

    // OS/2 weights are OpenType's 1..1000 scale, not fontconfig's. A named
    // instance states its own axis values, which override the static OS/2
    // fields the font also carries.
    //
    // Failing both, the style name is searched for a weight word, and only
    // then does the bold flag decide: `FcContainsWeight` at `:1885`, then
    // `FT_STYLE_FLAG_BOLD` at `:1916`.
    let weight_mult = instance.and_then(|i| i.weight).unwrap_or(1.0);
    let width_mult = instance.and_then(|i| i.width).unwrap_or(1.0);
    let fc_weight = os2
        .as_ref()
        .map(|os2| weight::from_opentype(f64::from(os2.us_weight_class()) * weight_mult))
        .filter(|w| *w >= 0.0)
        .or_else(|| styles.iter().find_map(|style| contains_weight(style)))
        .unwrap_or(if bold { 200.0 } else { 100.0 });
    pattern.add(Object::Weight, fc_weight);

    // `usWidthClass` outside 1..9 is not a width at all -- upstream's switch
    // has no default, so it leaves the value unset and falls through to the
    // style name. Mapping it to normal instead loses a `Condensed` that the
    // name states plainly.
    let fc_width = os2
        .as_ref()
        .and_then(|os2| width_from_class(os2.us_width_class()))
        .map(|width| width * width_mult)
        .or_else(|| styles.iter().find_map(|style| contains_width(style)))
        .unwrap_or(100.0);
    pattern.add(Object::Width, fc_width);

    pattern.add(Object::Slant, slant(pattern, italic));
}

/// The width names fontconfig matches, in its own order.
///
/// `widthConsts` in `fcfreetype.c`. Order matters for the same reason it does
/// for the weights: `semicondensed` has to be tried before `condensed`.
static WIDTH_NAMES: &[(&str, f64)] = &[
    ("ultracondensed", 50.0),
    ("extracondensed", 63.0),
    ("semicondensed", 87.0),
    ("condensed", 75.0),
    ("normal", 100.0),
    ("semiexpanded", 113.0),
    ("extraexpanded", 200.0),
    ("ultraexpanded", 200.0),
    ("expanded", 125.0),
    ("extended", 125.0),
];

/// The width a style string names, if any word of it does.
///
/// `FcContainsWidth`, a substring search over the blank-stripped, folded name.
fn contains_width(style: &str) -> Option<f64> {
    let folded = blanks_removed(style);
    WIDTH_NAMES.iter().find(|(name, _)| folded.contains(name)).map(|(_, width)| *width)
}

/// How many distinct advance widths a font uses, and so how it spaces.
///
/// Fontconfig does not trust the `post` fixed-pitch flag -- its own comment
/// says CJK "monospace" fonts are really dual width and most other fonts do
/// not bother setting the attribute. It measures instead: collect up to three
/// distinct advances across the mapped glyphs, and call one width monospaced,
/// two widths dual when the wider is about twice the narrower, and anything
/// else proportional.
///
/// Only a non-proportional result is recorded, which is why most fonts carry
/// no `spacing` at all.
fn spacing(font: &FontRef<'_>) -> Option<i32> {
    let hmtx = font.hmtx().ok()?;
    let mut advances: Vec<u16> = Vec::with_capacity(3);

    // Every mapping the cmap has, not the filtered coverage. The two differ
    // exactly where it matters: a font that maps U+0000 and U+000D to a
    // narrow `.null` glyph is not monospaced, even though those codepoints
    // are excluded from what it can draw. Noto Emoji is such a font, and
    // sampling only the coverage called it monospaced.
    walk_mappings(font, |_code, gid| {
        if advances.len() >= 3 {
            return;
        }
        let advance = hmtx.advance(gid).unwrap_or(0);
        if advance == 0 {
            return;
        }
        if advances.iter().any(|other| approximately_equal(*other, advance)) {
            return;
        }
        advances.push(advance);
    });

    match advances.as_slice() {
        [] | [_] => Some(100), // FC_MONO
        [a, b] => {
            let (min, max) = (*a.min(b), *a.max(b));
            // Dual width: the wide glyphs are two narrow cells across.
            approximately_equal(min.saturating_mul(2), max).then_some(90) // FC_DUAL
        }
        _ => None, // proportional, and so not recorded
    }
}

/// Fontconfig's tolerance: within about three percent of the larger.
fn approximately_equal(a: u16, b: u16) -> bool {
    let (a, b) = (i32::from(a), i32::from(b));
    (a - b).abs() * 33 <= a.abs().max(b.abs())
}

/// The slant, from the style name if it says, otherwise from the flags.
///
/// The name wins, which is not obvious and does decide real cases: DejaVu
/// Sans Bold Oblique sets the OS/2 italic bit *and* calls itself oblique, and
/// fontconfig reports it oblique. The angle in `post` is not consulted at all
/// -- the code that would have is `#if 0` in `fcfreetype.c`.
///
/// `italic` is FreeType's flag, which is not simply one bit of one table; see
/// [`style_flags`].
fn slant(pattern: &Pattern, italic: bool) -> i32 {
    if let Some(element) = pattern.get(Object::Style) {
        for (value, _) in element.values() {
            let crate::value::Value::String(style) = value else {
                continue;
            };
            let lowered = style.to_lowercase();
            // Checked in fontconfig's own order, so a name claiming both
            // reads as italic.
            if lowered.contains("italic") || lowered.contains("kursiv") {
                return 100;
            }
            if lowered.contains("oblique") {
                return 110;
            }
        }
    }
    if italic {
        100
    } else {
        0
    }
}

/// `usWidthClass` is a 1..9 index, not a percentage.
fn width_from_class(class: u16) -> Option<f64> {
    Some(match class {
        1 => 50.0,
        2 => 63.0,
        3 => 75.0,
        4 => 87.0,
        5 => 100.0,
        6 => 113.0,
        7 => 125.0,
        8 => 150.0,
        9 => 200.0,
        // Not a width. Upstream's switch has no default, so the value stays
        // unset and the style name gets its turn.
        _ => return None,
    })
}

/// The `name` table IDs fontconfig reads, in the order it prefers them.
///
/// Taken from `nameid_order` in `fcfreetype.c`. The order is the point: a
/// font whose four weights are split into separate legacy families lists the
/// typographic family first, so `Source Code Pro` precedes `Source Code Pro
/// Black` and a query for the former finds all four.
const FAMILY_IDS: [u16; 3] = [21, 16, 1]; // WWS, typographic, legacy
const STYLE_IDS: [u16; 3] = [22, 17, 2];
const FULLNAME_IDS: [u16; 2] = [18, 4]; // Macintosh full name, then full name

/// The full name ids a *named instance* may use.
///
/// Fontconfig skips name id 4 for an instance -- it describes the face the
/// file is named for, not this one -- but goes on reading id 18. A font that
/// has one keeps it for every instance; a font that has neither gets a name
/// built from its family and style. That is the whole difference between
/// Noto Emoji, whose instances are all called `Noto Emoji`, and Cantarell,
/// whose instances are called `Cantarell Thin` and the rest.
const INSTANCE_FULLNAME_IDS: [u16; 1] = [18];

/// Platforms in the order fontconfig searches them.
const PLATFORM_ORDER: [u16; 4] = [3, 0, 1, 2]; // Microsoft, Unicode, Mac, ISO

/// Family, style, full name and PostScript name from the `name` table.
fn add_names(font: &FontRef<'_>, pattern: &mut Pattern) {
    for (ids, object, lang_object) in [
        (&FAMILY_IDS[..], Object::Family, Object::Familylang),
        (&STYLE_IDS[..], Object::Style, Object::Stylelang),
        (&FULLNAME_IDS[..], Object::Fullname, Object::Fullnamelang),
    ] {
        for (text, lang) in collect_names(font, ids) {
            pattern.add(object, text.as_str());
            pattern.add(lang_object, lang);
        }
    }
    add_synthetic_fullname(pattern);
    // The PostScript name is not localized -- fontconfig records no language
    // for it -- so unlike the others it is taken whatever language it is
    // filed under. A font with no name id 6 at all, which the Terminus
    // bitmaps are, gets one built from its family with the spaces removed.
    let ps = any_name(font, 6).or_else(|| {
        let family = any_name(font, 16).or_else(|| any_name(font, 1))?;
        Some(family.chars().filter(|c| !c.is_whitespace()).collect())
    });
    if let Some(ps) = ps {
        pattern.add(Object::PostscriptName, ps.as_str());
    }
}

/// A full name built from the family and style, when the face has none.
///
/// `FcFreeTypeQueryFaceInternal` does this only when the `name` table
/// offered nothing at all -- a face that names itself keeps that name, even
/// if it disagrees with the family and style beside it.
///
/// The English values are preferred over the first, and the whitespace
/// trimming is one-sided on each: fontconfig strips the family's trailing
/// space and the style's leading one, so that the single space it inserts
/// between them is the only one.
fn add_synthetic_fullname(pattern: &mut Pattern) {
    if pattern.contains(Object::Fullname) {
        return;
    }
    let Some(family) = english_value(pattern, Object::Family, Object::Familylang) else {
        return;
    };
    let Some(style) = english_value(pattern, Object::Style, Object::Stylelang) else {
        return;
    };
    let full = format!("{} {}", family.trim_end(), style.trim_start());
    pattern.add(Object::Fullname, full.as_str());
    pattern.add(Object::Fullnamelang, "en");
}

/// The value of `object` whose language is English, or the first one.
fn english_value(pattern: &Pattern, object: Object, lang_object: Object) -> Option<String> {
    let languages: Vec<&str> = pattern
        .get(lang_object)
        .map(|element| element.values().filter_map(|(v, _)| v.as_value().as_str()).collect())
        .unwrap_or_default();
    let at = languages.iter().position(|lang| *lang == "en").unwrap_or(0);
    let element = pattern.get(object)?;
    let mut values = element.values().filter_map(|(v, _)| v.as_value().as_str());
    let chosen = values
        .nth(at)
        .or_else(|| element.values().filter_map(|(v, _)| v.as_value().as_str()).next())?;
    Some(chosen.to_string())
}

/// Every distinct name across `ids`, in fontconfig's platform-then-id order.
///
/// Duplicates are dropped: a font that repeats the same string under the
/// legacy and typographic ids contributes it once.
fn collect_names(font: &FontRef<'_>, ids: &[u16]) -> Vec<(String, &'static str)> {
    let Ok(name) = font.name() else {
        return Vec::new();
    };
    let records = name.name_record();
    let data = name.string_data();
    let mut out: Vec<(String, &'static str)> = Vec::new();

    for platform in PLATFORM_ORDER {
        for id in ids {
            // Within one platform and name id, fontconfig orders by encoding,
            // then puts the English record first, then the rest by language
            // id. The English-first part is what makes `%{family}` on a
            // localized font read as the English name followed by its
            // translations, rather than whichever language sorted lowest.
            let mut matching: Vec<_> = records
                .iter()
                .enumerate()
                .filter(|(_, r)| r.name_id().to_u16() == *id && r.platform_id() == platform)
                .collect();
            matching.sort_by_key(|(index, r)| {
                (r.encoding_id(), !is_english(platform, r.language_id()), r.language_id(), *index)
            });

            for (_, record) in matching {
                let Some(lang) = language_tag(platform, record.language_id()) else {
                    // A localization whose language we cannot name would be
                    // reported without a tag, so it is skipped instead.
                    continue;
                };
                let Ok(string) = record.string(data) else { continue };
                // Fontconfig trims a name's surrounding whitespace. Some Noto
                // faces pad their style with a trailing space, and an
                // untrimmed name is a different string to every comparison
                // that follows.
                let text = string.chars().collect::<String>().trim().to_string();
                // Duplicates are compared the way fontconfig compares any
                // two names -- ignoring case and blanks -- so a font that
                // spells the same style `kursiv` for one language and
                // `Kursiv` for another contributes it once.
                let duplicate =
                    out.iter().any(|(existing, _)| casefold::eq_ignoring_blanks(existing, &text));
                if text.is_empty() || duplicate {
                    continue;
                }
                out.push((text, lang));
            }
        }
    }
    out
}

/// Whether a name record is in English, by the platform's own numbering.
fn is_english(platform: u16, language: u16) -> bool {
    matches!((platform, language), (3, 0x0409) | (1, 0))
}

/// The language tag for a name record, or `None` if it is one we cannot name.
///
/// A record whose language cannot be named is skipped rather than reported
/// untagged: fontconfig pairs every name with a language, and a name with the
/// wrong one is worse than a name missing.
fn language_tag(platform: u16, language: u16) -> Option<&'static str> {
    crate::name_langs::tag(platform, language)
}

// --- Type 1 ----------------------------------------------------------------

/// Scan a Type 1 font.
///
/// These predate SFNT entirely: no tables, no `OS/2`, no `name`. Everything
/// comes from the PostScript dictionary at the head of the file, so the
/// properties are derived rather than read.
fn scan_type1(data: &[u8], path: &str) -> Result<Vec<Pattern>, ScanError> {
    use read_fonts::ps::type1::Type1Font;

    let font = Type1Font::new(data).map_err(|_| ScanError::Unrecognized)?;
    let mut pattern = Pattern::new();

    pattern.add(Object::File, path);
    pattern.add(Object::Index, 0);
    // A Type 1 font has no wrapper property at all -- fontconfig only sets
    // one for SFNT-based formats -- and no version, which it reports as 0.
    pattern.add(Object::Fontformat, "Type 1");
    pattern.add(Object::Fontversion, 0);
    pattern.add(Object::Outline, true);
    pattern.add(Object::Color, false);
    pattern.add(Object::Scalable, true);
    // Type 1 hinting is in the charstrings themselves, not a separate table,
    // and fontconfig does not report it.
    pattern.add(Object::FontHasHint, false);
    pattern.add(Object::Order, 0);

    if let Some(family) = font.family_name() {
        pattern.add(Object::Family, family);
        pattern.add(Object::Familylang, "en");
    }
    if let Some(full) = font.full_name() {
        // The style is what the full name adds to the family. A font whose
        // full name is exactly its family adds nothing, and is Regular -- and
        // its full name is then reported *with* that Regular, so the full
        // name is rebuilt from the two rather than taken as written.
        let (full, style) = postscript_style(Some(full), font.family_name());
        let full = full.unwrap_or_default();
        pattern.add(Object::Fullname, full.as_str());
        pattern.add(Object::Fullnamelang, "en");
        if let Some(style) = style {
            pattern.add(Object::Style, style.as_str());
            pattern.add(Object::Stylelang, "en");
        }
    }
    if let Some(name) = font.name() {
        pattern.add(Object::PostscriptName, name);
    }

    pattern.add(Object::Foundry, notice_foundry(data).unwrap_or("unknown"));
    // The same four a scalable SFNT face carries. A Type 1 font has no
    // variations and no symbol encoding, but fontconfig still records that
    // it has not, and an absent property is not the same as a false one:
    // scoring compares only the properties both sides have.
    pattern.add(Object::Decorative, is_decorative(&pattern));
    pattern.add(Object::Symbol, false);
    pattern.add(Object::Variable, false);
    pattern.add(Object::NamedInstance, false);
    // A Type 1 font has no cmap. Its coverage comes from glyph names mapped
    // through the Adobe Glyph List, which `unicode_charmap` does for us --
    // except for the dingbats, whose names have a list of their own.
    let mut coverage = CharSet::new();
    for (c, _) in charmap_chars(font.unicode_charmap()) {
        coverage.insert(c);
    }
    for (_, name) in font.glyph_names() {
        if let Some(c) = crate::zapf::codepoint(name).and_then(char::from_u32) {
            coverage.insert(c);
        }
    }
    // A Type 1 font has no cmap to be symbol-encoded through, and reports
    // `symbol=false` just below.
    // A Type 1 font has no `OS/2` table to declare a codepage in.
    add_coverage(coverage, false, None, &mut pattern);
    // The style is whatever was derived from the full name above, which is
    // the string `FcContainsWeight` searches when `/Weight` names nothing.
    let style = pattern.string(Object::Style).unwrap_or("").to_string();
    pattern.add(Object::Weight, postscript_weight(font.weight(), &style));
    pattern.add(Object::Width, 100.0);
    // A Type 1 font states its slant as an angle, so anything non-zero is
    // oblique rather than italic unless the name says otherwise.
    let angle = font.italic_angle();
    let slant = if angle == 0 {
        0
    } else if font.full_name().is_some_and(|n| n.to_lowercase().contains("italic")) {
        100
    } else {
        110
    };
    pattern.add(Object::Slant, slant);
    // `/isFixedPitch` is a bare boolean token rather than a string, so it is
    // read from the header directly; not every URW font sets the flag that
    // `Type1Font` exposes.
    if font.is_fixed_pitch() || postscript_flag(data, b"/isFixedPitch") {
        pattern.add(Object::Spacing, 100); // FC_MONO
    }

    Ok(vec![pattern])
}

/// Foundries recognised by a substring of a font's copyright notice.
///
/// From `FcNoticeFoundries` in `fcfoundry.h`. A Type 1 font has no vendor tag,
/// so its notice is the only thing that names who made it.
const NOTICE_FOUNDRIES: [(&str, &str); 18] = [
    ("Adobe", "adobe"),
    ("Bigelow", "b&h"),
    ("Bitstream", "bitstream"),
    ("Gnat", "culmus"),
    ("Iorsh", "culmus"),
    ("HanYang System", "hanyang"),
    ("Font21", "hwan"),
    ("IBM", "ibm"),
    ("International Typeface Corporation", "itc"),
    ("Linotype", "linotype"),
    ("LINOTYPE-HELL", "linotype"),
    ("Microsoft", "microsoft"),
    ("Monotype", "monotype"),
    ("Omega", "omega"),
    ("Tiro Typeworks", "tiro"),
    ("URW", "urw"),
    ("XFree86", "xfree86"),
    ("Xorg", "xorg"),
];

/// The foundry a copyright notice names, if it names one.
fn notice_to_foundry(notice: &str) -> Option<&'static str> {
    NOTICE_FOUNDRIES.iter().find(|(needle, _)| notice.contains(needle)).map(|(_, foundry)| *foundry)
}

/// The foundry named by a Type 1 font's notice, if it names one.
///
/// `Type1Font` does not expose `/Notice`, so it is read straight out of the
/// PostScript header. The search is bounded: everything after `eexec` is
/// encrypted, and a match in there would be noise.
fn notice_foundry(data: &[u8]) -> Option<&'static str> {
    let header = &data[..data.len().min(64 * 1024)];
    notice_to_foundry(postscript_string(header, b"/Notice")?)
}

/// Whether a bare PostScript boolean is `true`.
fn postscript_flag(data: &[u8], key: &[u8]) -> bool {
    let header = &data[..data.len().min(64 * 1024)];
    let Some(at) = header.windows(key.len()).position(|w| w == key) else {
        return false;
    };
    let rest = &header[at + key.len()..];
    let value: Vec<u8> = rest
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_alphanumeric())
        .copied()
        .collect();
    value == b"true"
}

/// The parenthesized value of a PostScript key, as text.
fn postscript_string<'a>(data: &'a [u8], key: &[u8]) -> Option<&'a str> {
    let at = data.windows(key.len()).position(|w| w == key)? + key.len();
    let rest = &data[at..];
    let open = rest.iter().position(|b| *b == b'(')?;
    // Parentheses nest in PostScript strings, so count rather than scanning
    // for the first close.
    let mut depth = 0usize;
    for (i, byte) in rest[open..].iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return std::str::from_utf8(&rest[open + 1..open + i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// The full name and style of a PostScript-flavoured font.
///
/// Neither a Type 1 nor a bare CFF has a `name` table, so FreeType builds
/// both from `/FullName` and `/FamilyName`: the style is what the full name
/// adds to the family. A font whose full name is exactly its family adds
/// nothing and is `Regular` -- and its full name is then reported *with* that
/// `Regular`, so the pair is rebuilt rather than taken as written.
///
/// `None` for the style when the full name does not begin with the family, in
/// which case there is nothing to subtract and the caller supplies a default.
fn postscript_style(full: Option<&str>, family: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(full) = full else { return (None, None) };
    match family {
        Some(family) => match full.strip_prefix(family) {
            Some(rest) if rest.trim().is_empty() => {
                (Some(format!("{family} Regular")), Some("Regular".to_string()))
            }
            Some(rest) => (Some(full.to_string()), Some(rest.trim().to_string())),
            None => (Some(full.to_string()), None),
        },
        None => (Some(full.to_string()), None),
    }
}

/// The weight names fontconfig matches, in its own order.
///
/// `weightConsts` in `fcfreetype.c`. The order matters for the substring
/// search: `demibold` has to be tried before `bold` and `extrablack` before
/// `black`, or the shorter name claims the longer one's string. A leading
/// `<` marks a name that counts only as a whole word.
static WEIGHT_NAMES: &[(&str, f64)] = &[
    ("thin", 0.0),
    ("extralight", 40.0),
    ("ultralight", 40.0),
    ("demilight", 55.0),
    ("semilight", 55.0),
    ("light", 50.0),
    ("book", 75.0),
    ("regular", 80.0),
    ("normal", 80.0),
    ("medium", 100.0),
    ("demibold", 180.0),
    ("demi", 180.0),
    ("semibold", 180.0),
    ("extrabold", 205.0),
    ("superbold", 205.0),
    ("ultrabold", 205.0),
    ("bold", 200.0),
    ("ultrablack", 215.0),
    ("superblack", 215.0),
    ("extrablack", 215.0),
    ("<ultra", 205.0),
    ("black", 210.0),
    ("heavy", 210.0),
];

/// A `/Weight` string as a fontconfig weight, if it names one exactly.
///
/// `FcIsWeight`, which is `FcStrCmpIgnoreBlanksAndCase` against each name --
/// blanks and case only. A hyphen is a character like any other, so
/// `Extra-light` matches nothing, and that is not an oversight to tidy up:
/// Source Code Pro says exactly that and fontconfig gives it the fallback
/// weight rather than 40.
fn is_weight(name: &str) -> Option<f64> {
    WEIGHT_NAMES
        .iter()
        .find(|(known, _)| {
            !known.starts_with('<') && crate::casefold::eq_ignoring_blanks(name, known)
        })
        .map(|(_, weight)| *weight)
}

/// The weight a style *string* names, if any word of it does.
///
/// `FcContainsWeight`: a substring search rather than an equality, so
/// `Bold Italic` is bold. A name written `<ultra` counts only as a whole
/// word, which stops `ultra` inside `ultralight` from reading as ultrabold.
fn contains_weight(style: &str) -> Option<f64> {
    let folded = blanks_removed(style);
    WEIGHT_NAMES
        .iter()
        .find(|(known, _)| match known.strip_prefix('<') {
            Some(word) => contains_word(style, word),
            None => folded.contains(&blanks_removed(known)),
        })
        .map(|(_, weight)| *weight)
}

/// `s` with its blanks dropped and its case folded, which is the form
/// `FcStrContainsIgnoreBlanksAndCase` compares in.
fn blanks_removed(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// Whether `needle` appears in `haystack` as a whole word.
///
/// `FcStrContainsWord`: the match has to start at the beginning or after a
/// non-alphanumeric character, and end at the end or before one.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let (hay, need) = (haystack.to_lowercase(), needle.to_lowercase());
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    hay.match_indices(&need).any(|(at, _)| {
        boundary(hay[..at].chars().next_back()) && boundary(hay[at + need.len()..].chars().next())
    })
}

/// The slant of a font whose metadata is PostScript rather than `OS/2`.
///
/// The style name is searched first -- `FcContainsSlant`, which knows
/// `italic`, `kursiv` and `oblique` -- and only if it names neither does the
/// italic angle decide, and then it can only say *italic*. Fontconfig's
/// `italic_angle` branch is `#if 0`'d out with a note that FreeType has
/// already folded it into the style flags; this is the same answer by the
/// same route.
///
/// So `Book Oblique` is oblique because it says so, and `BlackIt` -- whose
/// style is just `Regular`, there being no full name to subtract a family
/// from -- is italic because it leans.
fn postscript_slant(style: &str, italic_angle: f64) -> i32 {
    const SLANTS: [(&str, i32); 3] = [("italic", 100), ("kursiv", 100), ("oblique", 110)];
    let folded = blanks_removed(style);
    if let Some((_, slant)) = SLANTS.iter().find(|(name, _)| folded.contains(name)) {
        return *slant;
    }
    if italic_angle != 0.0 {
        100 // FC_SLANT_ITALIC
    } else {
        0 // FC_SLANT_ROMAN
    }
}

/// The weight of a font whose metadata is PostScript rather than `OS/2`./// The weight of a font whose metadata is PostScript rather than `OS/2`.
///
/// Three steps, in fontconfig's order, and the middle one is the surprise:
/// the `/Weight` string is tried first, then the *style name* is searched for
/// a weight word, and only then does it settle for medium. A bare CFF has no
/// style name at all beyond the `Regular` that FreeType gives it, which is
/// why an unmatched `/Weight` lands on 80 rather than 100.
fn postscript_weight(weight: Option<&str>, style: &str) -> f64 {
    // Fontconfig's last resort is *medium*, not regular. `Roman` is not a
    // weight in its table -- New Century Schoolbook says `/Weight (Roman)`
    // and comes out at 100.
    weight.and_then(is_weight).or_else(|| contains_weight(style)).unwrap_or(100.0)
}

// --- bare CFF ---------------------------------------------------------------

/// Scan a `CFF ` table that arrived on its own, without an SFNT around it.
///
/// FreeType reads one, so fontconfig lists one, and what it reports is
/// noticeably thinner than for the OpenType font the same table usually sits
/// in: there is no `name` table, no `OS/2`, and no `cmap`. Everything comes
/// from the CFF's own top dictionary and its charset.
///
/// The three consequences worth stating, because they look like bugs
/// otherwise:
///
/// * the style is always `Regular` -- nothing in a CFF names one, and
///   FreeType does not invent it from the family or the PostScript name;
/// * the weight comes from the `/Weight` string rather than `OS/2`, so it is
///   whatever the font called itself in words;
/// * coverage is built from glyph *names* through the Adobe Glyph List, the
///   same way a Type 1 font's is, which is only possible for a non-CID font.
fn scan_cff(data: &[u8], path: &str) -> Result<Vec<Pattern>, ScanError> {
    use read_fonts::ps::cff::CffFontRef;

    let font = CffFontRef::new_cff(data, 0, None).map_err(|_| ScanError::Unrecognized)?;
    // `name()` is the PostScript name and comes from the name index, not
    // the dictionary, so `Metadata` is still what reports it.
    let metadata = font.metadata().unwrap_or_default();
    // The table rather than the font: `CffFontRef` reaches its string index
    // through its own kind and a CID-keyed font has none there, so every name
    // and notice in one came back empty when read that way.
    let cff = Cff::read(read_fonts::FontData::new(data)).map_err(|_| ScanError::Unrecognized)?;
    let top = TopDict::read(&cff);
    let mut pattern = Pattern::new();

    pattern.add(Object::File, path);
    pattern.add(Object::Index, 0);
    pattern.add(Object::Fontformat, "CFF");
    // No `head`, so no version to report; fontconfig prints 0.
    pattern.add(Object::Fontversion, 0);
    pattern.add(Object::Outline, true);
    pattern.add(Object::Scalable, true);
    pattern.add(Object::Color, false);
    pattern.add(Object::Decorative, false);
    pattern.add(Object::Symbol, false);
    pattern.add(Object::Variable, false);
    pattern.add(Object::NamedInstance, false);
    // Hinting lives in the charstrings, not in a table anything can see.
    pattern.add(Object::FontHasHint, false);
    pattern.add(Object::Order, 0);

    if let Some(family) = top.family_name {
        pattern.add(Object::Family, family);
        pattern.add(Object::Familylang, "en");
    }
    // The style is what the full name adds to the family, exactly as for a
    // Type 1 font -- FreeType derives both the same way, and a CFF whose
    // `/FullName` is just its family is `Regular`. Source Code Pro is the
    // second kind and URW Gothic the first, which is why one reports
    // `Regular` and the other `Book`.
    let (full, style) = postscript_style(top.full_name, top.family_name);
    let style = style.unwrap_or_else(|| "Regular".to_string());
    pattern.add(Object::Style, style.as_str());
    pattern.add(Object::Stylelang, "en");
    // A CFF need not carry a `/FullName` -- Source Code Pro does not -- and
    // fontconfig builds one from the family and the style when the name table
    // has none to read.
    let full = full.or_else(|| top.family_name.map(|family| format!("{family} {style}")));
    if let Some(full) = &full {
        pattern.add(Object::Fullname, full.as_str());
        pattern.add(Object::Fullnamelang, "en");
    }
    if let Some(name) = metadata.name() {
        pattern.add(Object::PostscriptName, name);
    }

    pattern.add(Object::Weight, postscript_weight(top.weight, &style));
    pattern.add(Object::Width, 100);
    pattern.add(Object::Slant, postscript_slant(&style, top.italic_angle));
    // Always set, as the Type 1 path does: fontconfig reports `unknown` for
    // a font whose notice names nobody, not nothing at all.
    let foundry = top.notice.and_then(notice_to_foundry).unwrap_or("unknown");
    pattern.add(Object::Foundry, foundry);

    let (coverage, glyphs) = cff_coverage(&font, &cff);
    // Once. Fontconfig has two places that add `spacing` -- one from
    // `/isFixedPitch`, one from the advances -- and they cannot disagree,
    // since a font that declares fixed pitch has one advance.
    if let Some(spacing) = cff_spacing(&font, &glyphs).or(top.is_fixed_pitch.then_some(100)) {
        pattern.add(Object::Spacing, spacing);
    }
    add_coverage(coverage, false, None, &mut pattern);
    Ok(vec![pattern])
}

/// The characters a bare CFF covers, from its glyph names.
///
/// A CFF has no `cmap`. What it has is a charset, mapping each glyph to a
/// string id, and those strings are glyph names -- so the Adobe Glyph List
/// turns them into codepoints, exactly as it does for Type 1.
///
/// Only for a non-CID font. In a CID-keyed CFF the charset holds CIDs rather
/// than name ids, and a CID is an index into a character collection, not a
/// name: reading them as string ids would produce whatever text happened to
/// sit at that index. Fontconfig reports no coverage for one either, since
/// FreeType builds no Unicode charmap for it.
fn cff_coverage<'a>(
    font: &read_fonts::ps::cff::CffFontRef<'a>,
    cff: &Cff<'a>,
) -> (CharSet, Vec<GlyphId>) {
    use read_fonts::ps::charmap::Charmap;

    let mut coverage = CharSet::new();
    let mut glyphs = Vec::new();
    if font.is_cid() {
        return (coverage, glyphs);
    }
    let Some(charset) = font.charset() else { return (coverage, glyphs) };
    let name_of = |glyph: GlyphId| cff_string(cff, charset.string_id(glyph).ok()?);
    let names = (0..font.num_glyphs())
        .filter_map(|glyph| Some((GlyphId::new(glyph), name_of(GlyphId::new(glyph))?)));

    let mut mapped: Vec<(char, GlyphId)> =
        charmap_chars(&Charmap::from_glyph_names(names)).collect();
    // The dingbats have a glyph-name list of their own, which the Adobe Glyph
    // List does not include; URW's Dingbats is a bare CFF and covers nothing
    // without it. Same workaround as the Type 1 path -- see
    // docs/fontations-gaps.md.
    for glyph in 0..font.num_glyphs() {
        let glyph = GlyphId::new(glyph);
        let Some(name) = name_of(glyph) else { continue };
        if let Some(c) = crate::zapf::codepoint(name).and_then(char::from_u32) {
            mapped.push((c, glyph));
        }
    }
    // Codepoint order, which is the order `FT_Get_Next_Char` walks -- and so
    // the order the advance sample has to take them in. Stable, and the
    // charmap's entries were pushed first, so where a dingbat name and a
    // glyph name claim the same character the charmap keeps it.
    mapped.sort_by_key(|(c, _)| *c as u32);
    mapped.dedup_by_key(|(c, _)| *c);
    for (c, glyph) in mapped {
        coverage.insert(c);
        glyphs.push(glyph);
    }
    (coverage, glyphs)
}

/// How a bare CFF spaces its glyphs, or `None` for proportional.
///
/// `FcFreeTypeSpacing` walks the font's charmap and collects up to three
/// distinct advances. One or none means monospaced; two, where the wider is
/// twice the narrower, means dual-width; anything else is proportional --
/// which fontconfig then does not record at all, `spacing` being absent
/// rather than zero.
///
/// "None" is not a degenerate case here. A CID-keyed CFF has no charmap for
/// FreeType to walk, so no advance is ever sampled and every one of them
/// comes out monospaced. That is why the CJK fonts report `spacing=100`
/// despite declaring no fixed pitch anywhere.
///
/// A CFF keeps its advances in the charstrings rather than an `hmtx`, so
/// getting one means running the charstring far enough to read the width
/// prefix. Only three distinct values are ever needed, so the walk stops as
/// soon as it has them.
fn cff_spacing(font: &read_fonts::ps::cff::CffFontRef<'_>, glyphs: &[GlyphId]) -> Option<i32> {
    let mut advances: Vec<f64> = Vec::with_capacity(3);
    for glyph in glyphs {
        if advances.len() >= 3 {
            break;
        }
        let Some(advance) = cff_advance(font, *glyph) else { continue };
        // Fontconfig skips a zero advance rather than counting it.
        if advance == 0.0 || advances.iter().any(|other| approximately_equal_f64(*other, advance)) {
            continue;
        }
        advances.push(advance);
    }
    match advances.as_slice() {
        [] | [_] => Some(100), // FC_MONO
        [a, b] if approximately_equal_f64(a.min(*b) * 2.0, a.max(*b)) => Some(90), // FC_DUAL
        _ => None,             // FC_PROPORTIONAL, which is not recorded
    }
}

/// One glyph's advance width, in font units.
fn cff_advance(font: &read_fonts::ps::cff::CffFontRef<'_>, glyph: GlyphId) -> Option<f64> {
    /// The charstring has to be run to reach its width prefix, and everything
    /// it draws on the way is of no interest.
    struct Discard;
    impl read_fonts::ps::cs::CommandSink for Discard {
        fn move_to(&mut self, _x: Fixed, _y: Fixed) {}
        fn line_to(&mut self, _x: Fixed, _y: Fixed) {}
        fn curve_to(&mut self, _: Fixed, _: Fixed, _: Fixed, _: Fixed, _: Fixed, _: Fixed) {}
        fn close(&mut self) {}
    }

    let index = font.subfont_index(glyph)?;
    let subfont = font.subfont(index, &[]).ok()?;
    let width = font.evaluate_charstring(&subfont, glyph, &[], &mut Discard).ok()?;
    Some(width?.to_f64())
}

/// `fc_approximately_equal` for the numbers a charstring yields.
fn approximately_equal_f64(a: f64, b: f64) -> bool {
    (a - b).abs() * 33.0 <= a.abs().max(b.abs())
}

/// The characters a name-derived charmap covers, and the glyph for each.
///
/// `FT_Get_Next_Char` over the same table: each codepoint once, mapped to the
/// glyph FreeType would pick for it.
///
/// `Charmap::iter` is not that. It yields the table as FreeType *stores* it,
/// where a glyph whose name carries a variant suffix -- `A.alt`,
/// `uni00AB.left_double_angle_quote` -- has `0x80000000` set on its codepoint
/// so that it sorts beside the base glyph without shadowing it.
/// `Charmap::map` masks that off when it searches; iterating does not. A font
/// that names its glyphs that way therefore appears to cover nothing at all,
/// because `char::from_u32` rejects every entry -- Noto Sans Duployan names
/// almost every glyph that way and scanned as covering eleven characters
/// instead of several hundred.
///
/// Two things follow from the order `from_glyph_names` sorts into, and both
/// matter here: entries sharing a base codepoint are contiguous, and the base
/// entry comes before its variants. So taking the first of each run yields
/// each character once, preferring the real glyph over a variant of it --
/// which is what `map` would return, and what the advance sampling in
/// [`cff_spacing`] needs, since sampling a small-caps variant instead of the
/// letter would be a different width.
fn charmap_chars(
    charmap: &read_fonts::ps::charmap::Charmap,
) -> impl Iterator<Item = (char, GlyphId)> + '_ {
    /// FreeType's marker for a variant glyph. Private upstream, so a caller
    /// that needs to strip it has no choice but to write it out again.
    const VARIANT_BIT: u32 = 0x8000_0000;
    let mut previous = None;
    charmap.iter().filter_map(move |(code, glyph)| {
        let base = code & !VARIANT_BIT;
        if previous == Some(base) {
            return None; // a variant of a character already yielded
        }
        previous = Some(base);
        Some((char::from_u32(base)?, glyph))
    })
}

/// The text a CFF string id names./// The text a CFF string id names.
///
/// Not `CffFontRef::string`, which reaches the string index through the
/// font's *kind*: a CID-keyed font has none there, so nothing past the 391
/// standard strings resolves -- and everything a font invented lives past
/// those, every custom glyph name and every name and notice it carries.
fn cff_string<'a>(cff: &Cff<'a>, sid: read_fonts::ps::string::Sid) -> Option<&'a str> {
    std::str::from_utf8(cff.string(sid)?).ok()
}

/// The parts of a CFF top dictionary a font query needs./// The parts of a CFF top dictionary a font query needs.
///
/// `Metadata` covers most of this, but not all: it does not expose `/Notice`
/// at all -- which is the only thing that names a foundry -- and it misses
/// `/isFixedPitch` on fonts that set it, which is how a monospaced CJK face
/// ends up reported as proportional. The dictionary is public, so the whole
/// lot is read from it in one pass rather than half from each.
#[derive(Default)]
struct TopDict<'a> {
    notice: Option<&'a str>,
    full_name: Option<&'a str>,
    family_name: Option<&'a str>,
    weight: Option<&'a str>,
    italic_angle: f64,
    is_fixed_pitch: bool,
}

impl<'a> TopDict<'a> {
    fn read(cff: &Cff<'a>) -> Self {
        use read_fonts::ps::cff::dict::{self, Entry};

        let mut out = Self::default();
        let Ok(top) = cff.top_dicts().get(0) else { return out };
        let text = |sid| cff_string(cff, sid);
        for entry in dict::entries(top, None).flatten() {
            match entry {
                Entry::Notice(sid) => out.notice = text(sid),
                Entry::FullName(sid) => out.full_name = text(sid),
                Entry::FamilyName(sid) => out.family_name = text(sid),
                Entry::Weight(sid) => out.weight = text(sid),
                Entry::ItalicAngle(angle) => out.italic_angle = angle.to_f64(),
                Entry::IsFixedPitch(fixed) => out.is_fixed_pitch = fixed,
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod charmap_tests {
    use super::charmap_chars;
    use read_fonts::ps::charmap::Charmap;
    use read_fonts::types::GlyphId;

    fn chars(pairs: &[(u32, &str)]) -> Vec<(char, u32)> {
        let map =
            Charmap::from_glyph_names(pairs.iter().map(|(gid, name)| (GlyphId::new(*gid), *name)));
        charmap_chars(&map).map(|(c, gid)| (c, gid.to_u32())).collect()
    }

    /// The whole point: a variant-suffixed name maps to its base character,
    /// and a font that names every glyph that way is not empty.
    #[test]
    fn a_variant_name_still_covers_its_character() {
        assert_eq!(chars(&[(4, "uni00AB.left_double_angle_quote")]), [('\u{ab}', 4)]);
        assert_eq!(chars(&[(3, "B.sc")]), [('B', 3)]);
    }

    /// Each character once, and the base glyph rather than the variant --
    /// which is what `Charmap::map` answers, and what an advance sample has
    /// to take, a small-caps H being a different width from an H.
    #[test]
    fn a_base_and_its_variant_yield_one_character() {
        assert_eq!(chars(&[(1, "A"), (2, "A.alt")]), [('A', 1)]);
        // Declaration order must not change the answer: the charmap sorts
        // the base ahead of its variants whichever way round they arrive.
        assert_eq!(chars(&[(2, "A.alt"), (1, "A")]), [('A', 1)]);
    }

    #[test]
    fn several_characters_come_out_in_codepoint_order() {
        let got = chars(&[(1, "A"), (2, "A.alt"), (3, "B.sc"), (4, "uni00AB.quote")]);
        assert_eq!(got, [('A', 1), ('B', 3), ('\u{ab}', 4)]);
    }

    /// A name the Adobe Glyph List cannot place contributes nothing, rather
    /// than contributing a wrong character.
    #[test]
    fn an_unmappable_name_is_dropped() {
        assert_eq!(chars(&[(1, "not.a.glyph.name.anyone.knows")]), []);
    }
}
