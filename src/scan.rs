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

use read_fonts::{
    tables::cmap::CmapSubtable, tables::head::MacStyle, tables::os2::SelectionFlags, FileRef,
    FontRef, ReadError, TableProvider,
};

use crate::charset::Coverage;
use crate::langset::Langs;
use crate::object::Object;
use crate::query::{OwnedValue, Query};
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
pub fn scan_file(path: &Path) -> Result<Vec<Query>, ScanError> {
    let data = std::fs::read(path)?;
    let name = path.to_string_lossy();
    scan_bytes(&data, &name)
}

/// Scan font bytes that came from `path`.
///
/// `path` is recorded as the pattern's `file`; nothing is read from it.
pub fn scan_bytes(data: &[u8], path: &str) -> Result<Vec<Query>, ScanError> {
    // Type 1 fonts are not SFNT and have to be recognised first: they begin
    // with `%!` for the plain text form, or the PFB segment marker.
    if is_type1(data) {
        return scan_type1(data, path);
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

/// Whether these bytes are a Type 1 font rather than an SFNT.
///
/// `%!` opens the plain PFA form; `0x80 0x01` opens a PFB segment header.
fn is_type1(data: &[u8]) -> bool {
    data.starts_with(b"%!") || data.starts_with(&[0x80, 0x01])
}

// --- SFNT ------------------------------------------------------------------

fn base_pattern(font: &FontRef<'_>, path: &str, index: i32) -> Query {
    let mut pattern = Query::new();

    // A `glyf` table that is present but empty means no outlines at all --
    // which is exactly how an OpenType bitmap font (.otb) is built.
    let has_glyf = table_len(font, b"glyf") > 0;
    let has_cff = has_table(font, b"CFF ") || has_table(font, b"CFF2");
    let has_color = [b"COLR", b"SVG ", b"CBLC", b"sbix"]
        .iter()
        .any(|tag| has_table(font, tag));
    let has_outlines = has_glyf || has_cff;

    pattern.add(Object::File, path);
    pattern.add(Object::Index, index);
    pattern.add(Object::FontWrapper, "SFNT");
    // A font with CFF outlines reports CFF even when it also has glyf.
    pattern.add(
        Object::Fontformat,
        if has_cff { "CFF" } else { "TrueType" },
    );
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
    // Names first: the slant is read off the style name.
    add_names(font, &mut pattern);
    add_coverage(sfnt_coverage(font), &mut pattern);
    pattern
}

/// Record what a font covers, and what that lets it write.
///
/// The language set is derived from the coverage rather than declared by the
/// font: fontconfig asks, for each language it knows an orthography for,
/// whether every codepoint that language needs is present.
fn add_coverage(coverage: Coverage, pattern: &mut Query) {
    if coverage.is_empty() {
        return;
    }
    let langs = Langs::from_coverage(&coverage);
    pattern.add(Object::Charset, OwnedValue::CharSet(coverage));
    if !langs.is_empty() {
        pattern.add(Object::Lang, OwnedValue::LangSet(langs));
    }
}

/// Every character an SFNT font maps, from its Unicode `cmap` subtables.
fn sfnt_coverage(font: &FontRef<'_>) -> Coverage {
    let mut coverage = Coverage::new();
    let Ok(cmap) = font.cmap() else {
        return coverage;
    };
    let empty = EmptyGlyphs::new(font);
    for record in cmap.encoding_records() {
        // Only the Unicode-addressed subtables say anything about coverage;
        // a symbol or Mac-Roman subtable indexes something else.
        use read_fonts::tables::cmap::PlatformId;
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
        collect_subtable(&subtable, &empty, &mut coverage);
    }
    coverage
}

/// Which glyphs draw nothing.
///
/// Only the ASCII control range needs this: CID fonts built by Adobe map
/// control characters to the blank space glyph, and fontconfig excludes a
/// control character whose glyph has no contours rather than claiming the
/// font covers it.
struct EmptyGlyphs<'a> {
    loca: Option<read_fonts::tables::loca::Loca<'a>>,
}

impl<'a> EmptyGlyphs<'a> {
    fn new(font: &FontRef<'a>) -> Self {
        Self { loca: font.loca(None).ok() }
    }

    /// Whether `glyph` draws nothing.
    ///
    /// A `glyf` outline of zero length has no contours. A CFF charstring
    /// would have to be executed to know, so those are assumed to draw --
    /// which matches every font checked here.
    fn is_empty(&self, glyph: read_fonts::types::GlyphId) -> bool {
        match &self.loca {
            Some(loca) => loca
                .get_raw(glyph.to_u32() as usize)
                .zip(loca.get_raw(glyph.to_u32() as usize + 1))
                .is_some_and(|(start, end)| start == end),
            None => false,
        }
    }
}

/// Add every codepoint a subtable maps to a real glyph.
///
/// A mapping to glyph 0 is a mapping to `.notdef`, which is the absence of a
/// glyph rather than the presence of one -- fonts routinely map U+0000 there.
/// Counting it would put a NUL at the head of every font's coverage.
fn collect_subtable(
    subtable: &CmapSubtable<'_>,
    empty: &EmptyGlyphs<'_>,
    coverage: &mut Coverage,
) {
    let mut add = |code: u32| {
        let Some(gid) = subtable.map_codepoint(code) else {
            return;
        };
        if gid.to_u32() == 0 {
            return;
        }
        // A control character only counts if its glyph actually draws.
        if code <= 0x1f && empty.is_empty(gid) {
            return;
        }
        if let Some(c) = char::from_u32(code) {
            coverage.insert(c);
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
fn scan_face(font: &FontRef<'_>, path: &str, index: i32) -> Vec<Query> {
    let base = base_pattern(font, path, index);
    let Some(instances) = named_instances(font) else {
        let mut pattern = base;
        add_attributes(font, &mut pattern, None);
        pattern.add(Object::Variable, false);
        pattern.add(Object::NamedInstance, false);
        return vec![pattern];
    };

    let mut patterns = Vec::with_capacity(instances.len() + 1);

    // The default instance first, as the font comes out of the box.
    let mut default = base.clone();
    add_attributes(font, &mut default, None);
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
            pattern.add(Object::Slant, slant(font, &pattern));
        }
        if let Some(ps) = instance.postscript.and_then(|id| name_by_id(font, id)) {
            pattern.remove(Object::PostscriptName);
            pattern.add(Object::PostscriptName, ps.as_str());
        }
        patterns.push(pattern);
    }

    // Finally the variable font itself, carrying ranges rather than values.
    let mut variable = base;
    add_variable_attributes(font, &mut variable);
    variable.add(Object::Variable, true);
    variable.add(Object::NamedInstance, false);
    // A variable pattern carries no full name or style: it is not one face.
    variable.remove(Object::Fullname);
    variable.remove(Object::Fullnamelang);
    variable.remove(Object::Style);
    variable.remove(Object::Stylelang);
    patterns.push(variable);

    patterns
}

/// One named instance: which axis values it pins, and what it calls itself.
struct Instance {
    subfamily: u16,
    postscript: Option<u16>,
    weight: Option<f64>,
    width: Option<f64>,
    /// Whether it pins every axis to that axis's default.
    is_default: bool,
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
            instance
                .coordinates
                .get(i)
                .is_some_and(|c| c.get() == axis.default_value())
        });
        out.push(Instance {
            subfamily: instance.subfamily_name_id.to_u16(),
            postscript: instance.post_script_name_id.map(|id| id.to_u16()),
            weight: coord(b"wght"),
            width: coord(b"wdth"),
            is_default,
        });
    }
    Some(out)
}

/// The weight and width axes as ranges, for the variable pattern.
fn add_variable_attributes(font: &FontRef<'_>, pattern: &mut Query) {
    add_attributes(font, pattern, None);
    let Ok(fvar) = font.fvar() else { return };
    let Ok(axes) = fvar.axes() else { return };
    for axis in axes.iter() {
        let (object, convert): (Object, fn(f64) -> f64) = match &axis.axis_tag().to_be_bytes() {
            b"wght" => (Object::Weight, weight::from_opentype),
            b"wdth" => (Object::Width, |v| v),
            _ => continue,
        };
        let range = crate::value::Range {
            begin: convert(axis.min_value().to_f64()),
            end: convert(axis.max_value().to_f64()),
        };
        pattern.remove(object);
        pattern.add(object, crate::query::OwnedValue::Range(range));
    }
}

/// One name record by id.
fn name_by_id(font: &FontRef<'_>, id: u16) -> Option<String> {
    collect_names(font, &[id]).into_iter().next().map(|(text, _)| text)
}

fn has_table(font: &FontRef<'_>, tag: &[u8; 4]) -> bool {
    font.table_data(read_fonts::types::Tag::new(tag)).is_some()
}

fn table_len(font: &FontRef<'_>, tag: &[u8; 4]) -> usize {
    font.table_data(read_fonts::types::Tag::new(tag))
        .map_or(0, |data| data.len())
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
    font.table_data(read_fonts::types::Tag::new(b"prep"))
        .is_some_and(|data| data.len() > 7)
}

/// The four-character vendor tag from `OS/2`, or `unknown`.
///
/// The tag is fixed width and padded with spaces, and fontconfig reports it
/// with the padding intact: the GNU FreeFont family's foundry really is
/// `"GNU "`, trailing space included. Only NUL padding is dropped.
fn foundry(font: &FontRef<'_>) -> String {
    let Ok(os2) = font.os2() else {
        return "unknown".to_string();
    };
    let text: String = os2
        .ach_vend_id()
        .to_be_bytes()
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect();
    if text.trim().is_empty() {
        "unknown".to_string()
    } else {
        text
    }
}

/// Weight, width, slant and spacing, from `OS/2` and `post`.
fn add_attributes(font: &FontRef<'_>, pattern: &mut Query, instance: Option<&Instance>) {
    let os2 = font.os2().ok();

    // OS/2 weights are OpenType's 1..1000 scale, not fontconfig's.
    // A named instance states its own axis values, which override the
    // static OS/2 fields the font also carries.
    let fc_weight = instance
        .and_then(|i| i.weight)
        .map(weight::from_opentype)
        .or_else(|| {
            os2.as_ref()
                .map(|os2| weight::from_opentype(f64::from(os2.us_weight_class())))
        })
        .filter(|w| *w >= 0.0)
        .unwrap_or(80.0);
    pattern.add(Object::Weight, fc_weight);

    let fc_width = instance
        .and_then(|i| i.width)
        .or_else(|| os2.as_ref().map(|os2| width_from_class(os2.us_width_class())))
        .unwrap_or(100.0);
    pattern.add(Object::Width, fc_width);

    pattern.add(Object::Slant, slant(font, pattern));

    if font.post().map(|post| post.is_fixed_pitch() != 0).unwrap_or(false) {
        pattern.add(Object::Spacing, 100); // FC_MONO
    }
}

/// The slant, from the style name if it says, otherwise from the flags.
///
/// The name wins, which is not obvious and does decide real cases: DejaVu
/// Sans Bold Oblique sets the OS/2 italic bit *and* calls itself oblique, and
/// fontconfig reports it oblique. The angle in `post` is not consulted at all
/// -- the code that would have is `#if 0` in `fcfreetype.c`.
fn slant(font: &FontRef<'_>, pattern: &Query) -> i32 {
    if let Some(element) = pattern.get(Object::Style) {
        for (value, _) in element.values() {
            let crate::query::OwnedValue::String(style) = value else {
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
    let italic = font
        .os2()
        .map(|os2| os2.fs_selection().contains(SelectionFlags::ITALIC))
        .unwrap_or(false)
        || font
            .head()
            .map(|head| head.mac_style().contains(MacStyle::ITALIC))
            .unwrap_or(false);
    if italic {
        100
    } else {
        0
    }
}

/// `usWidthClass` is a 1..9 index, not a percentage.
fn width_from_class(class: u16) -> f64 {
    match class {
        1 => 50.0,
        2 => 63.0,
        3 => 75.0,
        4 => 87.0,
        6 => 113.0,
        7 => 125.0,
        8 => 150.0,
        9 => 200.0,
        _ => 100.0,
    }
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

/// Platforms in the order fontconfig searches them.
const PLATFORM_ORDER: [u16; 4] = [3, 0, 1, 2]; // Microsoft, Unicode, Mac, ISO

/// Family, style, full name and PostScript name from the `name` table.
fn add_names(font: &FontRef<'_>, pattern: &mut Query) {
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
    if let Some((ps, _)) = collect_names(font, &[6]).into_iter().next() {
        pattern.add(Object::PostscriptName, ps.as_str());
    }
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
            for record in records {
                if record.name_id().to_u16() != *id || record.platform_id() != platform {
                    continue;
                }
                let Some(lang) = language_tag(platform, record.language_id()) else {
                    // A localization whose language we cannot name would be
                    // reported without a tag, so it is skipped instead.
                    continue;
                };
                let Ok(string) = record.string(data) else { continue };
                let text: String = string.chars().collect();
                if text.is_empty() || out.iter().any(|(existing, _)| *existing == text) {
                    continue;
                }
                out.push((text, lang));
            }
        }
    }
    out
}

/// The language tag for a name record, or `None` if it is one we cannot name.
///
/// Fontconfig carries a full table of Windows LCIDs and Macintosh language
/// codes. Only the English entries are recognised here, so a font with
/// localized names reports fewer of them than fontconfig does -- which is
/// visible in `familylang`, and in `%{family}` for CJK fonts.
fn language_tag(platform: u16, language: u16) -> Option<&'static str> {
    match (platform, language) {
        (3, 0x0409) => Some("en"), // Windows, English (US)
        (1, 0) => Some("en"),      // Macintosh, English
        (0, _) => Some("en"),      // Unicode platform carries no language
        _ => None,
    }
}

// --- Type 1 ----------------------------------------------------------------

/// Scan a Type 1 font.
///
/// These predate SFNT entirely: no tables, no `OS/2`, no `name`. Everything
/// comes from the PostScript dictionary at the head of the file, so the
/// properties are derived rather than read.
fn scan_type1(data: &[u8], path: &str) -> Result<Vec<Query>, ScanError> {
    use read_fonts::ps::type1::Type1Font;

    let font = Type1Font::new(data).map_err(|_| ScanError::Unrecognized)?;
    let mut pattern = Query::new();

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
        pattern.add(Object::Fullname, full);
        pattern.add(Object::Fullnamelang, "en");
        // The style is what the full name adds to the family.
        if let Some(family) = font.family_name() {
            if let Some(style) = full.strip_prefix(family) {
                let style = style.trim();
                if !style.is_empty() {
                    pattern.add(Object::Style, style);
                    pattern.add(Object::Stylelang, "en");
                }
            }
        }
    }
    if let Some(name) = font.name() {
        pattern.add(Object::PostscriptName, name);
    }

    pattern.add(Object::Foundry, notice_foundry(data).unwrap_or("unknown"));
    // A Type 1 font has no cmap. Its coverage comes from glyph names mapped
    // through the Adobe Glyph List, which `unicode_charmap` does for us.
    let mut coverage = Coverage::new();
    for (code, _) in font.unicode_charmap().iter() {
        if let Some(c) = char::from_u32(code) {
            coverage.insert(c);
        }
    }
    add_coverage(coverage, &mut pattern);
    pattern.add(Object::Weight, type1_weight(font.weight()));
    pattern.add(Object::Width, 100.0);
    // A Type 1 font states its slant as an angle, so anything non-zero is
    // oblique rather than italic unless the name says otherwise.
    let angle = font.italic_angle();
    let slant = if angle == 0 {
        0
    } else if font
        .full_name()
        .is_some_and(|n| n.to_lowercase().contains("italic"))
    {
        100
    } else {
        110
    };
    pattern.add(Object::Slant, slant);
    if font.is_fixed_pitch() {
        pattern.add(Object::Spacing, 100);
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

/// The foundry named by a Type 1 font's notice, if it names one.
///
/// `Type1Font` does not expose `/Notice`, so it is read straight out of the
/// PostScript header. The search is bounded: everything after `eexec` is
/// encrypted, and a match in there would be noise.
fn notice_foundry(data: &[u8]) -> Option<&'static str> {
    let header = &data[..data.len().min(64 * 1024)];
    let notice = postscript_string(header, b"/Notice")?;
    NOTICE_FOUNDRIES
        .iter()
        .find(|(needle, _)| notice.contains(needle))
        .map(|(_, foundry)| *foundry)
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

/// A Type 1 `/Weight` string as a fontconfig weight.
///
/// The value is free text -- `Book`, `Demi`, `Ultra Bold` -- so it is matched
/// against the same names `<const>` uses.
fn type1_weight(name: Option<&str>) -> f64 {
    let Some(name) = name else { return 80.0 };
    let folded: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    let weight = match folded.as_str() {
        "thin" => 0.0,
        "extralight" | "ultralight" => 40.0,
        "light" => 50.0,
        "demilight" | "semilight" => 55.0,
        "book" => 75.0,
        "regular" | "normal" | "roman" => 80.0,
        "medium" => 100.0,
        "demi" | "demibold" | "semibold" => 180.0,
        "bold" => 200.0,
        "extrabold" | "ultrabold" => 205.0,
        "black" | "heavy" => 210.0,
        "extrablack" | "ultrablack" => 215.0,
        _ => 80.0,
    };
    weight
}
