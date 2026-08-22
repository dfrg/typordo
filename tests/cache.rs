//! Tests against real cache files captured from a live fontconfig.
//!
//! The fixtures were written by fontconfig 2.17.0 on Fedora 44 and are
//! checked in verbatim, so these run identically on every host — including
//! ones with no fontconfig at all. That is the point: every other check we
//! have against a system font stack is machine-dependent.

use fontconf::{Cache, Error, Object, Value};

fn fixture(name: &str) -> Cache {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    Cache::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cantarell() -> Cache {
    fixture("cantarell-le64.cache-9")
}

#[test]
fn header_reports_the_directory_and_its_size() {
    let cache = cantarell();
    assert_eq!(cache.dir().unwrap(), "/usr/share/fonts/abattis-cantarell-vf-fonts");
    assert_eq!(cache.as_bytes().len(), 10328);
    assert_eq!(cache.subdirs().unwrap().len(), 0);
}

#[test]
fn every_fixture_survives_a_strict_walk() {
    for name in ["cantarell-le64.cache-9", "empty-dir-le64.cache-9"] {
        fixture(name).validate().unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

/// A variable font contributes one pattern per named instance, all sharing a
/// file. Cantarell VF has six.
#[test]
fn a_variable_font_yields_one_pattern_per_instance() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    assert_eq!(fonts.len(), 6);

    for font in &fonts {
        assert_eq!(font.string(Object::Family), Some("Cantarell"));
        assert_eq!(
            font.string(Object::File),
            Some("/usr/share/fonts/abattis-cantarell-vf-fonts/Cantarell-VF.otf")
        );
    }

    let styles: Vec<_> = fonts.iter().map(|f| f.string(Object::Style)).collect();
    assert_eq!(
        styles,
        [
            Some("Regular"),
            Some("Thin"),
            Some("Light"),
            Some("Bold"),
            Some("Extra Bold"),
            // The variable font's own pattern carries no style at all, as
            // distinct from carrying an empty one. `fc-list` renders both as
            // nothing, so only reading the cache directly tells them apart.
            None,
        ]
    );
    assert!(fonts[5].get(Object::Style).is_none());
    assert!(fonts[5].value(Object::Variable).is_some());
}

/// Weight is a range, not a scalar, and on a variable font it spans the axis.
#[test]
fn weight_is_stored_as_a_range() {
    let cache = cantarell();
    let widest = cache
        .fonts()
        .unwrap()
        .filter_map(|f| match f.value(Object::Weight) {
            Some(Value::Range(r)) => Some(r),
            _ => None,
        })
        .max_by(|a, b| (a.end - a.begin).total_cmp(&(b.end - b.begin)))
        .expect("a variable font should report a weight range");
    assert!(!widest.is_scalar(), "expected a real span, got {widest:?}");
    assert!(widest.begin < widest.end);
}

/// Elements arrive sorted by object id, which is what lets a caller binary
/// search if it ever wants to.
#[test]
fn elements_are_ordered_by_object_id() {
    let cache = cantarell();
    for font in cache.fonts().unwrap() {
        let ids: Vec<_> = font.elements().map(|e| e.id()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "unsorted: {ids:?}");
        assert_eq!(ids.len(), font.len());
    }
}

#[test]
fn strings_borrow_from_the_cache_buffer() {
    let cache = cantarell();
    let family = cache.fonts().unwrap().next().unwrap().string(Object::Family).unwrap();
    let base = cache.as_bytes().as_ptr() as usize;
    let borrowed = family.as_ptr() as usize;
    assert!(
        (base..base + cache.as_bytes().len()).contains(&borrowed),
        "family name should point into the cache, not a copy"
    );
}

/// A directory fontconfig scanned but found no fonts in still gets a cache.
/// Its font array is zero-length and serialized at the very end of the file,
/// so it resolves to exactly one past the last byte.
#[test]
fn an_empty_directory_cache_reads_as_empty() {
    let cache = fixture("empty-dir-le64.cache-9");
    assert_eq!(cache.dir().unwrap(), "/usr/share/fonts");
    assert_eq!(cache.fonts().unwrap().count(), 0);
    // It is the parent of every other font directory on the system.
    assert_eq!(cache.subdirs().unwrap().len(), 21);
    let subdirs: Vec<_> = cache.subdirs().unwrap().map(|s| s.unwrap()).collect();
    assert!(subdirs.contains(&"/usr/share/fonts/dejavu-sans-fonts"));
}

#[test]
fn object_ids_and_names_round_trip() {
    for id in 1..=Object::MAX {
        let object = Object::from_id(id).expect("static ids are contiguous");
        assert_eq!(object.id(), id);
        assert_eq!(Object::from_name(object.name()), Some(object));
    }
    assert_eq!(Object::from_id(0), None);
    assert_eq!(Object::from_id(Object::MAX + 1), None);
    assert_eq!(Object::Family.name(), "family");
    assert_eq!(Object::PixelSize.name(), "pixelsize");
}

// --- rejection of things that are not this format -------------------------

#[test]
fn a_cache_built_in_memory_is_rejected() {
    let mut bytes = cantarell().as_bytes().to_vec();
    bytes[0..4].copy_from_slice(&fontconf::MAGIC_ALLOC.to_le_bytes());
    assert_eq!(
        Cache::new(bytes.into_boxed_slice()).err(),
        Some(Error::BadMagic(fontconf::MAGIC_ALLOC))
    );
}

#[test]
fn another_format_version_is_rejected_rather_than_guessed_at() {
    let mut bytes = cantarell().as_bytes().to_vec();
    bytes[4..8].copy_from_slice(&12i32.to_le_bytes());
    assert_eq!(
        Cache::new(bytes.into_boxed_slice()).err(),
        Some(Error::UnsupportedVersion(12))
    );
}

/// The header's own length field is what rejects a cache from a build with a
/// different word size, since the fields after it are read at the wrong stride.
#[test]
fn a_truncated_file_is_rejected_by_its_own_size_field() {
    let full = cantarell().as_bytes().to_vec();
    let short = full[..full.len() - 1].to_vec();
    assert_eq!(
        Cache::new(short.into_boxed_slice()).err(),
        Some(Error::SizeMismatch { declared: 10328, actual: 10327 })
    );
}

#[test]
fn an_empty_file_is_rejected() {
    assert!(matches!(Cache::new(Box::new([])), Err(Error::Truncated { .. })));
}

// --- corrupt input must never panic ---------------------------------------

/// Every byte of the header, flipped, must produce an error or a readable
/// cache, never a panic. The header is where a bad value does the most damage
/// because everything else is reached through it.
#[test]
fn no_header_corruption_panics() {
    let original = cantarell().as_bytes().to_vec();
    for offset in 0..64 {
        for bit in 0..8 {
            let mut bytes = original.clone();
            bytes[offset] ^= 1 << bit;
            // Fix the size field back up so we get past the length check and
            // actually exercise the offsets, unless size is what we corrupted.
            if !(8..16).contains(&offset) {
                let len = bytes.len() as i64;
                bytes[8..16].copy_from_slice(&len.to_le_bytes());
            }
            if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
                let _ = cache.validate();
                let _ = cache.dir();
                if let Ok(fonts) = cache.fonts() {
                    for font in fonts {
                        for element in font.elements() {
                            for value in element.values() {
                                let _ = value;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The same, spread across the body rather than the header.
#[test]
fn no_body_corruption_panics() {
    let original = cantarell().as_bytes().to_vec();
    for offset in (64..original.len()).step_by(7) {
        let mut bytes = original.clone();
        bytes[offset] = bytes[offset].wrapping_add(0x5b);
        let Ok(cache) = Cache::new(bytes.into_boxed_slice()) else {
            continue;
        };
        let _ = cache.validate();
        let _ = cache.dir();
        if let Ok(fonts) = cache.fonts() {
            for font in fonts {
                let _ = font.string(Object::Family);
                for element in font.elements() {
                    for value in element.values() {
                        let _ = value;
                    }
                }
            }
        }
    }
}

/// A `next` field pointed at its own node is an infinite chain. Iteration has
/// to be bounded by something the file cannot lie about.
#[test]
fn a_cyclic_value_chain_terminates() {
    let original = cantarell().as_bytes().to_vec();
    // Find the first value node by walking the way the reader does, then make
    // it point at itself. Offset 0 of a node is `next`, encoded, so 1 is a
    // self-reference.
    let cache = Cache::new(original.clone().into_boxed_slice()).unwrap();
    let font = cache.fonts().unwrap().next().unwrap();
    let before: Vec<_> = font.get(Object::Family).unwrap().values().collect();
    assert_eq!(before.len(), 1);

    // Locating that one node exactly is fiddly, so instead poison every
    // 8-byte-aligned slot in turn: wherever a node head happens to land, its
    // `next` now points at itself.
    for offset in (64..original.len().min(4096)).step_by(8) {
        let mut poisoned = original.clone();
        poisoned[offset..offset + 8].copy_from_slice(&1i64.to_le_bytes());
        let Ok(cache) = Cache::new(poisoned.into_boxed_slice()) else {
            continue;
        };
        if let Ok(fonts) = cache.fonts() {
            for font in fonts {
                for element in font.elements() {
                    // The budget is what makes this terminate.
                    let count = element.values().take(100_000).count();
                    assert!(count < 100_000, "chain at {offset} did not terminate");
                }
            }
        }
    }
}

// --- character coverage ---------------------------------------------------

fn cantarell_charset(cache: &Cache) -> fontconf::CharSet<'_> {
    let font = cache.fonts().unwrap().next().unwrap();
    match font.value(Object::Charset) {
        Some(Value::CharSet(charset)) => charset,
        other => panic!("expected a charset, got {other:?}"),
    }
}

/// The leading ranges were checked against `fc-query --format='%{charset}'`.
#[test]
fn a_charset_decodes_to_the_ranges_fontconfig_reports() {
    let cache = cantarell();
    let charset = cantarell_charset(&cache);
    charset.validate().expect("charset should be well formed");
    let text = charset.to_string();
    assert!(
        text.starts_with("20-7e a0-131 134-148 14a-17e 18f 192 1a0-1a1"),
        "unexpected ranges: {}",
        &text[..text.len().min(80)]
    );
}

#[test]
fn charset_membership_agrees_with_its_ranges() {
    let cache = cantarell();
    let charset = cantarell_charset(&cache);

    assert!(charset.contains('A'));
    assert!(charset.contains(' '));
    assert!(charset.contains('~'));
    // 0x7f is DEL: the first range stops at 0x7e, so it must be absent.
    assert!(!charset.contains('\u{7f}'));
    // Well outside anything a Latin font covers.
    assert!(!charset.contains('\u{4e00}'));

    // Every range reported must be fully contained, and the gaps between
    // ranges must not be.
    let ranges: Vec<_> = charset.ranges().take(40).collect();
    for (start, end) in &ranges {
        assert!(charset.contains(*start), "{start:?} starts a range but is absent");
        assert!(charset.contains(*end), "{end:?} ends a range but is absent");
    }
    for pair in ranges.windows(2) {
        let gap = pair[0].1 as u32 + 1;
        if gap < pair[1].0 as u32 {
            let gap = char::from_u32(gap).unwrap();
            assert!(!charset.contains(gap), "{gap:?} is between ranges but present");
        }
    }
}

#[test]
fn charset_len_matches_the_characters_it_yields() {
    let cache = cantarell();
    let charset = cantarell_charset(&cache);
    assert_eq!(charset.len(), charset.chars().count());
    assert!(!charset.is_empty());
    // Ranges must partition the same set of characters, in the same order.
    let from_ranges: usize = charset
        .ranges()
        .map(|(a, b)| (b as u32 - a as u32 + 1) as usize)
        .sum();
    assert_eq!(from_ranges, charset.len());
}

/// Every instance of the variable font reports the same coverage, and the
/// charsets compare equal without either being copied out of the cache.
#[test]
fn all_instances_share_one_coverage() {
    let cache = cantarell();
    let charsets: Vec<_> = cache
        .fonts()
        .unwrap()
        .filter_map(|f| match f.value(Object::Charset) {
            Some(Value::CharSet(c)) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(charsets.len(), 6);
    assert!(charsets.windows(2).all(|w| w[0] == w[1]));
}

// --- languages ------------------------------------------------------------

fn cantarell_langs(cache: &Cache) -> fontconf::LangSet<'_> {
    let font = cache.fonts().unwrap().next().unwrap();
    match font.value(Object::Lang) {
        Some(Value::LangSet(langs)) => langs,
        other => panic!("expected a langset, got {other:?}"),
    }
}

/// The leading names were checked against `fc-list --format='%{lang}'`, which
/// reads the same caches. `fc-query` would rescan the font file instead and
/// can legitimately disagree with what the cache recorded.
#[test]
fn a_langset_decodes_to_the_languages_fontconfig_reports() {
    let cache = cantarell();
    let langs = cantarell_langs(&cache);
    langs.validate().expect("langset should be well formed");
    assert!(langs.is_consistent(), "bitmap should fit our language table");

    let names: Vec<_> = langs.langs().collect();
    assert!(names.contains(&"en"), "{names:?}");
    assert!(names.contains(&"de"), "{names:?}");
    assert!(!names.contains(&"ja"), "a Latin font should not claim Japanese");
    assert_eq!(langs.len(), names.len());
    assert!(!langs.is_empty());
}

/// Languages come out in bit order, which is fontconfig's declaration order
/// and is *not* alphabetical. `bm` before `be` is the giveaway.
#[test]
fn languages_are_reported_in_bit_order_not_alphabetically() {
    use fontconf::langs::LANGS;
    let bm = LANGS.iter().position(|l| *l == "bm").unwrap();
    let be = LANGS.iter().position(|l| *l == "be").unwrap();
    assert!(bm < be, "bit order should be declaration order");

    let cache = cantarell();
    let names: Vec<_> = cantarell_langs(&cache).langs().collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_ne!(names, sorted, "output should not be alphabetical");
}

/// `has_lang` is the question matching asks: an exact language is best, the
/// same language elsewhere is a near miss, an unrelated one is worst.
#[test]
fn has_lang_grades_a_request_rather_than_answering_yes_or_no() {
    use fontconf::LangResult;
    let cache = cantarell();
    let langs = cantarell_langs(&cache);

    assert_eq!(langs.has_lang("en"), LangResult::Equal);
    // The font records plain "en", so a regional request is a near miss.
    assert_eq!(langs.has_lang("en-US"), LangResult::DifferentTerritory);
    assert_eq!(langs.has_lang("ja"), LangResult::DifferentLang);
    // A tag fontconfig has never heard of cannot match anything.
    assert_eq!(langs.has_lang("not-a-language"), LangResult::DifferentLang);

    // `contains` is the exact-table version and does not grade.
    assert!(langs.contains("en"));
    assert!(!langs.contains("en-US"));
}
