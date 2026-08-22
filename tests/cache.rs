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
