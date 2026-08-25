//! Tests for reading configuration, against a synthetic config tree.
//!
//! The fixture is loaded with its own directory as the search path, through
//! [`Config::load_from_with_path`], so its relative includes resolve next to
//! it rather than against `/etc/fonts` and these run on any host. That is
//! also how fontconfig resolves them -- a relative `<include>` is looked up
//! on the search path, never against the including file, whatever `prefix`
//! says. The paths the fixture names are deliberately fictional: nothing here
//! touches a real font directory.

use std::path::{Path, PathBuf};

use typordo::CachePolicy;
use typordo::Config;
use typordo::ConfigError;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config")
}

fn config() -> Config {
    Config::load_from_with_path(
        &fixture_dir().join("fonts.conf"),
        std::slice::from_ref(&fixture_dir()),
    )
    .expect("fixture should load")
}

#[test]
fn reads_directories_in_order_and_deduplicates() {
    let config = config();
    let dirs: Vec<_> = config.font_dirs().filter_map(|d| d.to_str()).collect();
    // `/synthetic/fonts` is listed twice in the file and appears once here.
    // The included files' directories follow the parent's, in include order.
    assert_eq!(dirs[0], "/synthetic/fonts");
    assert_eq!(dirs.iter().filter(|d| **d == "/synthetic/fonts").count(), 1);
    assert!(dirs.contains(&"/synthetic/first"));
    assert!(dirs.contains(&"/synthetic/second"));
    assert!(
        !dirs.iter().any(|d| d.contains("ignored")),
        "a file that is not named *.conf must not be included: {dirs:?}"
    );
}

/// `conf.d` files are numerically prefixed precisely so they load in order.
#[test]
fn included_directories_are_read_in_sorted_order() {
    let config = config();
    let names: Vec<_> = config.files().iter().filter_map(|f| f.file_name()?.to_str()).collect();
    assert_eq!(names, ["fonts.conf", "10-first.conf", "20-second.conf"]);
}

/// The fixture's `20-second.conf` includes its own parent. Fontconfig configs
/// really do this, so it has to terminate rather than recurse.
#[test]
fn an_include_cycle_terminates_and_reads_each_file_once() {
    let config = config();
    assert_eq!(config.files().len(), 3, "{:?}", config.files());
    let mut sorted = config.files().to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), config.files().len(), "a file was read twice");
}

#[test]
fn an_optional_include_that_is_missing_is_not_an_error() {
    // The fixture includes `not-there.conf`, which does not exist, under
    // `ignore_missing`.
    assert!(Config::load_from_with_path(
        &fixture_dir().join("fonts.conf"),
        std::slice::from_ref(&fixture_dir())
    )
    .is_ok());
}

#[test]
fn a_missing_config_file_is_an_error() {
    let result = Config::load_from(&fixture_dir().join("no-such-file.conf"));
    assert!(matches!(result, Err(typordo::ConfigError::NotFound(_))));
}

#[test]
fn cache_directories_are_collected_in_search_order() {
    let config = config();
    let dirs = config.cache_dirs();
    assert_eq!(dirs[0], Path::new("/synthetic/cache"));
    // The second is `prefix="xdg"`, which resolves under the cache home; it
    // is only present when this host has somewhere to put it.
    if let Some(second) = dirs.get(1) {
        assert!(second.ends_with("fontconfig"), "{second:?}");
    }
}

/// The name is fixed by fontconfig: MD5 of the path, architecture, version.
/// These two were checked against `md5sum` and against the real file names in
/// `~/.cache/fontconfig` on a live system.
#[test]
fn cache_basename_matches_fontconfig() {
    assert_eq!(
        config().cache_basename("/usr/share/fonts"),
        "3830d5c3ddfd5cd38a049b759396e72e-le64.cache-9"
    );
    assert_eq!(
        config().cache_basename("/usr/share/fonts/dejavu-sans-fonts"),
        "221930ae9526a9cb8049af2916f03412-le64.cache-9"
    );
}

/// Nothing in the fixture points at a real cache, so the walk finds none —
/// but it must terminate cleanly rather than fail.
#[test]
fn walking_caches_with_none_present_yields_nothing() {
    assert_eq!(config().caches(CachePolicy::read_only()).count(), 0);
}

/// A config written to a temporary file, since these all turn on attributes
/// no checked-in fixture carries.
fn from_source(name: &str, body: &str) -> Config {
    let dir = std::env::temp_dir().join("typordo-naming");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.conf"));
    std::fs::write(
        &path,
        format!("<?xml version=\"1.0\"?>\n<fontconfig>\n{body}\n</fontconfig>\n"),
    )
    .unwrap();
    Config::load_from(&path).unwrap()
}

/// A `salt` changes the name without changing where the fonts are, so that
/// the same directory can have more than one cache.
#[test]
fn a_salt_changes_the_cache_name() {
    let plain = from_source("plain", "<dir>/fonts</dir>");
    let salted = from_source("salted", "<dir salt=\"pepper\">/fonts</dir>");
    let other = from_source("other", "<dir salt=\"other\">/fonts</dir>");

    assert_ne!(plain.cache_basename("/fonts"), salted.cache_basename("/fonts"));
    assert_ne!(salted.cache_basename("/fonts"), other.cache_basename("/fonts"));
    // And it reaches everything beneath the directory that carries it.
    assert_ne!(plain.cache_basename("/fonts/sub"), salted.cache_basename("/fonts/sub"));
}

/// A `<remap-dir>` hashes the path it is told to pretend to be, which is how
/// a container finds caches built outside it.
#[test]
fn a_remap_hashes_the_path_it_pretends_to_be() {
    let remapped = from_source(
        "remapped",
        "<remap-dir as-path=\"/usr/share/fonts\">/run/host/fonts</remap-dir>",
    );
    let plain = from_source("plain-usr", "<dir>/usr/share/fonts</dir>");
    assert_eq!(
        remapped.cache_basename("/run/host/fonts"),
        plain.cache_basename("/usr/share/fonts")
    );
    // The prefix is what moves; a subdirectory keeps its own tail.
    assert_eq!(
        remapped.cache_basename("/run/host/fonts/dejavu"),
        plain.cache_basename("/usr/share/fonts/dejavu")
    );
}

/// A `<remap-dir>` also adds the directory, so its fonts are found at all.
#[test]
fn a_remap_adds_the_directory() {
    let config = from_source(
        "remap-adds",
        "<remap-dir as-path=\"/usr/share/fonts\">/run/host/fonts</remap-dir>",
    );
    let dirs: Vec<_> = config.font_dirs().filter_map(|d| d.to_str()).collect();
    assert_eq!(dirs, ["/run/host/fonts"]);
}

/// Without an `as-path` a `<remap-dir>` says nothing. Fontconfig warns and
/// drops it, rather than treating it as a plain directory.
#[test]
fn a_remap_without_a_target_is_dropped() {
    let config = from_source("remap-bare", "<remap-dir>/run/host/fonts</remap-dir>");
    assert_eq!(config.font_dirs().count(), 0);
}

/// Fontconfig takes the first font directory containing the path, not the
/// longest, so a plain `<dir>` listed first shadows a `<remap-dir>` beneath.
#[test]
fn the_first_matching_directory_wins_not_the_longest() {
    let shadowed = from_source(
        "shadowed",
        "<dir>/fonts</dir>\n<remap-dir as-path=\"/elsewhere\" salt=\"s\">/fonts/sub</remap-dir>",
    );
    let plain = from_source("plain-fonts", "<dir>/fonts</dir>");
    assert_eq!(
        shadowed.cache_basename("/fonts/sub"),
        plain.cache_basename("/fonts/sub"),
        "the remapping below an already-listed directory should not apply"
    );
}

/// The prefix has to end on a separator: a sibling directory whose name
/// merely starts the same way is not inside it.
#[test]
fn a_prefix_match_lands_on_a_separator() {
    let salted = from_source("sep", "<dir salt=\"pepper\">/fonts</dir>");
    let plain = from_source("sep-plain", "<dir>/fonts</dir>");
    assert_eq!(
        salted.cache_basename("/fonts-extra"),
        plain.cache_basename("/fonts-extra"),
        "/fonts-extra is not inside /fonts"
    );
    assert_ne!(salted.cache_basename("/fonts/x"), plain.cache_basename("/fonts/x"));
}

// --- cache policy ---------------------------------------------------------

/// A font directory and a cache directory, with a config naming both.
fn policy_fixture(name: &str) -> (PathBuf, Config) {
    let root = std::env::temp_dir().join(format!("typordo-policy-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let fonts = root.join("fonts");
    let caches = root.join("caches");
    std::fs::create_dir_all(&fonts).unwrap();
    std::fs::create_dir_all(&caches).unwrap();

    let conf = root.join("fonts.conf");
    std::fs::write(
        &conf,
        format!(
            "<?xml version=\"1.0\"?>\n<fontconfig>\n<dir>{}</dir>\n<cachedir>{}</cachedir>\n\
             </fontconfig>\n",
            fonts.display(),
            caches.display()
        ),
    )
    .unwrap();
    (fonts, Config::load_from(&conf).expect("fixture config"))
}

/// A directory with no cache yields nothing, and says so.
///
/// It used to yield nothing and say nothing, which is indistinguishable from
/// a directory with no fonts in it.
#[test]
fn a_missing_cache_is_reported_rather_than_hidden() {
    let (_fonts, config) = policy_fixture("missing");
    let mut caches = config.caches(CachePolicy::read_only());
    assert_eq!(caches.by_ref().count(), 0);
    let skipped = caches.skipped();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].reason, typordo::SkipReason::Missing);
}

#[cfg(feature = "scan")]
#[test]
fn rebuilding_makes_a_cache_where_there_was_none() {
    let (fonts, config) = policy_fixture("rebuild");

    // Nothing to read yet.
    assert_eq!(config.caches(CachePolicy::read_only()).count(), 0);

    // Asking for a rebuild scans the directory and writes the cache.
    let mut built = config.build_fonts();
    let found: Vec<_> = built.by_ref().collect();
    assert_eq!(found.len(), 1, "the font directory should now have a cache");
    assert_eq!(found[0].0, fonts.to_string_lossy());
    assert!(built.skipped().is_empty(), "{:?}", built.skipped());

    // And a plain read now finds it, without scanning again.
    let mut plain = config.caches(CachePolicy::read_only());
    assert_eq!(plain.by_ref().count(), 1);
    assert!(plain.skipped().is_empty());
}

/// A cache whose directory has moved on is still readable, so the policy
/// decides: `Use` keeps it, `Skip` reports it rather than pretending.
///
/// Adding a file changes the recorded stamp either way -- the modification
/// time where that can be trusted, the listing checksum where it cannot.
#[cfg(feature = "scan")]
#[test]
fn a_stale_cache_is_used_or_skipped_as_asked() {
    let (fonts, config) = policy_fixture("stale");
    assert_eq!(config.build_fonts().count(), 1);

    std::fs::write(fonts.join("added.txt"), b"not a font").unwrap();

    // Used: the cache is out of date but still describes the directory.
    let mut using = config.caches(CachePolicy::read_only());
    assert_eq!(using.by_ref().count(), 1, "Use should keep a stale cache");
    assert!(using.skipped().is_empty(), "{:?}", using.skipped());

    // Skipped: nothing comes back, and the reason is recorded.
    let mut skipping = config
        .caches(CachePolicy { missing: typordo::IfMissing::Skip, stale: typordo::IfStale::Skip });
    assert_eq!(skipping.by_ref().count(), 0, "Skip should drop a stale cache");
    assert_eq!(skipping.skipped().len(), 1);
    assert_eq!(skipping.skipped()[0].reason, typordo::SkipReason::Stale);

    // Rebuilt: scanning brings it back up to date, and a plain read is then
    // no longer stale.
    assert_eq!(config.build_fonts().count(), 1);
    let mut after = config
        .caches(CachePolicy { missing: typordo::IfMissing::Skip, stale: typordo::IfStale::Skip });
    assert_eq!(after.by_ref().count(), 1, "a rebuilt cache should be current");
    assert!(after.skipped().is_empty(), "{:?}", after.skipped());
}

// --- <reset-dirs/> --------------------------------------------------------

/// `<reset-dirs/>` discards the font directories declared so far.
///
/// `FcParseResetDirs` calls `FcConfigResetFontDirs`, which is a
/// `FcStrSetDeleteAll` on the font directory set alone. Cache directories and
/// anything already parsed survive, which is the point: a sandboxed
/// configuration includes the system one for its rules and then drops the
/// host directories it brought along.
#[test]
fn reset_dirs_drops_the_directories_before_it() {
    let path = fixture_dir().join("reset.conf");
    let config = Config::load_from(&path).expect("fixture should load");

    let dirs: Vec<_> = config.font_dirs().filter_map(|d| d.to_str()).collect();
    assert_eq!(dirs, ["/synthetic/mine"], "only what follows the reset survives");

    // The reset is font directories only.
    let caches: Vec<_> = config.cache_dirs().iter().filter_map(|d| d.to_str()).collect();
    assert!(caches.contains(&"/synthetic/cache"), "cache dirs survive: {caches:?}");
}

/// A `conf.d` entry without a numeric prefix is not read.
///
/// `FcConfigParseAndLoadDir` takes only names of the form `[0-9]*.conf`. The
/// prefixes are what order the rules, so a file without one has no defined
/// place in the sequence and fontconfig ignores it rather than guessing. A
/// stray `local.conf` or an editor's leftover would otherwise contribute
/// rules that no other implementation can see.
#[test]
fn a_conf_d_file_without_a_numeric_prefix_is_ignored() {
    let config = config();
    let dirs: Vec<_> = config.font_dirs().filter_map(|d| d.to_str()).collect();
    assert!(
        !dirs.contains(&"/synthetic/should-never-load"),
        "conf.d/local.conf must not be read: {dirs:?}"
    );
    // And the numerically prefixed neighbours in the same directory are.
    assert!(dirs.contains(&"/synthetic/first"), "{dirs:?}");
}

// --- FONTCONFIG_SYSROOT ---------------------------------------------------

/// A configuration under a sysroot names paths as the target sees them.
///
/// The point of a sysroot is inspecting a filesystem that is not the running
/// one, so a font found at `<root>/usr/share/fonts` has to be recorded at
/// `/usr/share/fonts` -- fontconfig strips the prefix back off `FC_FILE` for
/// the same reason. Recording where it was reached would bake the build
/// machine into a cache meant for the image.
#[test]
fn a_sysroot_config_reads_under_the_root_and_records_target_paths() {
    let root = std::env::temp_dir().join("typordo-sysroot-test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("etc/fonts")).unwrap();
    std::fs::create_dir_all(root.join("usr/share/fonts")).unwrap();
    std::fs::write(
        root.join("etc/fonts/fonts.conf"),
        "<?xml version=\"1.0\"?>\n<fontconfig>\n\
         <dir>/usr/share/fonts</dir>\n\
         <cachedir>/var/cache/fontconfig</cachedir>\n</fontconfig>\n",
    )
    .unwrap();

    let config =
        Config::load_from_sysroot(Path::new("/etc/fonts/fonts.conf"), &root).expect("loads");

    // Named as the target sees them, with no trace of where they were read.
    let dirs: Vec<_> = config.font_dirs().filter_map(|d| d.to_str()).collect();
    assert_eq!(dirs, ["/usr/share/fonts"], "{dirs:?}");
    let caches: Vec<_> = config.cache_dirs().iter().filter_map(|d| d.to_str()).collect();
    assert_eq!(caches, ["/var/cache/fontconfig"], "{caches:?}");

    // And the same config without the root cannot be found at all, which is
    // what makes the sysroot the thing doing the work here.
    assert!(Config::load_from(Path::new("/etc/fonts/does-not-exist.conf")).is_err());
}

/// Without a sysroot, nothing is rewritten.
#[test]
fn no_sysroot_leaves_paths_alone() {
    let config = config();
    assert!(config.sysroot().is_none());
    let dirs: Vec<_> = config.font_dirs().filter_map(|d| d.to_str()).collect();
    assert!(dirs.contains(&"/synthetic/fonts"), "{dirs:?}");
}

// --- cache candidates -----------------------------------------------------

/// A cache that will not open must not hide a good one further down.
///
/// `FcDirCacheProcess` walks every configured cache directory; its loop has
/// no early exit on failure. A system cache left corrupt by an interrupted
/// update would otherwise take a whole directory's fonts away from a user
/// cache that is perfectly current.
#[cfg(feature = "scan")]
#[test]
fn a_corrupt_cache_does_not_hide_a_valid_one() {
    let root = std::env::temp_dir().join("typordo-two-cachedirs");
    let _ = std::fs::remove_dir_all(&root);
    let fonts = root.join("fonts");
    let (first, second) = (root.join("broken"), root.join("good"));
    for d in [&fonts, &first, &second] {
        std::fs::create_dir_all(d).unwrap();
    }

    let conf = root.join("fonts.conf");
    let write_conf = |dirs: &[&std::path::Path]| {
        let mut xml = String::from("<?xml version=\"1.0\"?>\n<fontconfig>\n");
        xml.push_str(&format!("<dir>{}</dir>\n", fonts.display()));
        for d in dirs {
            xml.push_str(&format!("<cachedir>{}</cachedir>\n", d.display()));
        }
        xml.push_str("</fontconfig>\n");
        std::fs::write(&conf, xml).unwrap();
    };

    // Build a real cache into the second directory only.
    write_conf(&[&second]);
    let config = Config::load_from(&conf).unwrap();
    assert_eq!(config.build_fonts().count(), 1, "a cache should be written");

    // Now put a file that is not a cache where the first directory would
    // hold one, and offer both.
    write_conf(&[&first, &second]);
    let config = Config::load_from(&conf).unwrap();
    let name = config.cache_basename(&fonts.to_string_lossy());
    std::fs::write(first.join(&name), b"this is not a cache").unwrap();

    let mut caches = config.caches(CachePolicy::read_only());
    let found: Vec<_> = caches.by_ref().collect();
    assert_eq!(found.len(), 1, "the good cache should still be found: {:?}", caches.skipped());
    assert!(caches.skipped().is_empty(), "{:?}", caches.skipped());

    // And the one that was picked is the one that reads.
    assert_eq!(found[0].0, fonts.to_string_lossy());
}

/// A cache for a directory that no longer exists is not used.
///
/// `FcDirCacheProcess` fails on the directory stat, and the rescan it falls
/// back to fails the same way, so fontconfig drops the directory. The font
/// files went with it, so a cache describing them answers with paths that no
/// longer open -- worse than answering nothing.
#[cfg(feature = "scan")]
#[test]
fn a_cache_for_a_vanished_directory_is_not_used() {
    let root = std::env::temp_dir().join("typordo-vanished-dir");
    let _ = std::fs::remove_dir_all(&root);
    let fonts = root.join("fonts");
    let caches = root.join("caches");
    std::fs::create_dir_all(&fonts).unwrap();
    std::fs::create_dir_all(&caches).unwrap();
    let conf = root.join("fonts.conf");
    std::fs::write(
        &conf,
        format!(
            "<?xml version=\"1.0\"?>\n<fontconfig>\n<dir>{}</dir>\n<cachedir>{}</cachedir>\n\
             </fontconfig>\n",
            fonts.display(),
            caches.display()
        ),
    )
    .unwrap();

    let config = Config::load_from(&conf).unwrap();
    assert_eq!(config.build_fonts().count(), 1, "a cache should be written");
    assert_eq!(config.caches(CachePolicy::read_only()).count(), 1, "and read back");

    // Take the directory away, leaving its cache behind.
    std::fs::remove_dir_all(&fonts).unwrap();

    let mut caches = config.caches(CachePolicy::read_only());
    assert_eq!(caches.by_ref().count(), 0, "the cache must not answer for a gone directory");
    assert_eq!(caches.skipped().len(), 1);
    assert_eq!(caches.skipped()[0].reason, typordo::SkipReason::DirectoryUnavailable);
}

/// A cache damaged past its header is refused whole, not walked into.
///
/// `FcDirCacheMapFd` runs `FcCacheOffsetsValid` on every map and rejects the
/// file entire. Reading one record at a time and skipping what does not hold
/// up yields a partial font set from a cache fontconfig would have refused,
/// and prunes whatever subdirectory tree hung below the skipped record.
#[cfg(feature = "scan")]
#[test]
fn a_cache_damaged_past_its_header_is_refused() {
    let root = std::env::temp_dir().join("typordo-damaged-cache");
    let _ = std::fs::remove_dir_all(&root);
    let (fonts, caches) = (root.join("fonts"), root.join("caches"));
    std::fs::create_dir_all(&fonts).unwrap();
    std::fs::create_dir_all(&caches).unwrap();
    let conf = root.join("fonts.conf");
    std::fs::write(
        &conf,
        format!(
            "<?xml version=\"1.0\"?>\n<fontconfig>\n<dir>{}</dir>\n<cachedir>{}</cachedir>\n\
             </fontconfig>\n",
            fonts.display(),
            caches.display()
        ),
    )
    .unwrap();

    let config = Config::load_from(&conf).unwrap();
    assert_eq!(config.build_fonts().count(), 1);
    let path = caches.join(config.cache_basename(&fonts.to_string_lossy()));

    // Damage the body, leaving the header -- magic, version and length --
    // intact, so only whole-cache validation can catch it.
    let mut bytes = std::fs::read(&path).unwrap();
    let len = bytes.len();
    assert!(len > 96, "cache should be bigger than its header");
    for byte in &mut bytes[64..len.min(160)] {
        *byte ^= 0xff;
    }
    std::fs::write(&path, &bytes).unwrap();

    let mut caches = config.caches(CachePolicy::read_only());
    assert_eq!(caches.by_ref().count(), 0, "a damaged cache must not be handed out");
    assert_eq!(caches.skipped().len(), 1, "{:?}", caches.skipped());
}

/// A cache found for a directory it was not built for -- copied into an
/// image, or reached through a sysroot -- lists subdirectories by the build
/// machine's paths. `FcConfigAddCache` compares the cache's directory with
/// the one asked for and, when they differ, rebuilds each subdirectory under
/// the requested one. Without that the walk descends into a tree that is not
/// there, and the fonts below it are lost.
///
/// Relocation is reachable in practice because the copy that performs it
/// usually preserves timestamps -- `tar -p`, `rsync -a`, `mv` -- so the cache
/// still reads as current for its new location.
#[cfg(feature = "scan")]
#[test]
fn a_relocated_cache_has_its_subdirectories_rebased() {
    let root = std::env::temp_dir().join("typordo-relocated-cache");
    let _ = std::fs::remove_dir_all(&root);
    let (built, caches) = (root.join("built"), root.join("caches"));
    std::fs::create_dir_all(built.join("sub")).unwrap();
    std::fs::create_dir_all(&caches).unwrap();

    let conf = |dir: &std::path::Path| {
        let path = root.join("fonts.conf");
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\"?>\n<fontconfig>\n<dir>{}</dir>\n<cachedir>{}</cachedir>\n\
                 </fontconfig>\n",
                dir.display(),
                caches.display()
            ),
        )
        .unwrap();
        Config::load_from(&path).unwrap()
    };

    let config = conf(&built);
    assert_eq!(config.build_fonts().count(), 2, "the directory and its subdirectory");

    // Move the tree. A rename leaves the directories' own timestamps alone,
    // so the caches stay current; only the names they hold are now wrong.
    let moved = root.join("moved");
    std::fs::rename(&built, &moved).unwrap();
    for (from, to) in [(&built, &moved), (&built.join("sub"), &moved.join("sub"))] {
        let name = |p: &std::path::Path| caches.join(config.cache_basename(&p.to_string_lossy()));
        std::fs::rename(name(from), name(to)).unwrap();
    }

    let config = conf(&moved);
    let mut walk = config.caches(CachePolicy::read_only());
    let dirs: Vec<String> = walk.by_ref().map(|(dir, _)| dir).collect();
    assert_eq!(walk.skipped(), &[], "nothing should have been passed over");
    assert_eq!(
        dirs,
        vec![
            moved.to_string_lossy().into_owned(),
            moved.join("sub").to_string_lossy().into_owned(),
        ],
        "the subdirectory must be reported under the directory it was found in"
    );
}

/// A missing `<include>` is reported unless the include said not to bother.
///
/// Fontconfig prints "Cannot load config file" and loads everything else, so
/// the font list is unaffected either way -- which is exactly why it needs
/// saying out loud. An include naming a path that moved contributes nothing
/// and, without this, says nothing.
#[test]
fn a_required_include_that_is_missing_fails_the_load() {
    let root = std::env::temp_dir().join("typordo-missing-include");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let load = |include: &str| {
        let path = root.join("fonts.conf");
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\"?>
<fontconfig>
{include}
</fontconfig>
"
            ),
        )
        .unwrap();
        Config::load_from_with_path(&path, std::slice::from_ref(&root))
    };

    assert!(load("").is_ok());

    // `_FcConfigParse` is called with `complain = !ignore_missing`, and a
    // required include it cannot resolve makes it return false, which sets
    // `parse->error` and fails the whole load -- the including file's rules
    // with it. Fontconfig then runs on its built-in configuration, so this is
    // not a quiet dropped file: it changes every answer.
    let absent = "/typordo-no-such-directory/nope.conf";
    assert!(
        matches!(load(&format!("<include>{absent}</include>")), Err(ConfigError::NotFound(_))),
        "a required include that resolves to nothing has to fail the load"
    );

    // `FcNameBool`, so every spelling fontconfig accepts works here.
    for yes in ["yes", "true", "on", "1", "Yes"] {
        assert!(
            load(&format!("<include ignore_missing=\"{yes}\">{absent}</include>")).is_ok(),
            "ignore_missing={yes}"
        );
    }
    for no in ["no", "false", "off", "0"] {
        assert!(
            load(&format!("<include ignore_missing=\"{no}\">{absent}</include>")).is_err(),
            "ignore_missing={no}"
        );
    }

    // And `ignore_missing` covers a file that *is* there and will not parse,
    // not only one that is absent: `_FcConfigParse` returns true without
    // looking at the outcome whenever it was told not to complain.
    let broken = root.join("broken.conf");
    std::fs::write(&broken, "not xml at all <<<").unwrap();
    assert!(load(&format!("<include>{}</include>", broken.display())).is_err());
    assert!(
        load(&format!("<include ignore_missing=\"yes\">{}</include>", broken.display())).is_ok()
    );

    // A file that is there and parses loads cleanly.
    let real = root.join("real.conf");
    std::fs::write(
        &real,
        "<?xml version=\"1.0\"?>
<fontconfig/>
",
    )
    .unwrap();
    assert!(load(&format!("<include>{}</include>", real.display())).is_ok());

    // A relative include is looked for on the search path in order, and the
    // *first* directory holding it wins -- reading every match would merge
    // configurations fontconfig never merges.
    let second = std::env::temp_dir().join("typordo-missing-include-2");
    let _ = std::fs::remove_dir_all(&second);
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(
        second.join("only-here.conf"),
        "<?xml version=\"1.0\"?>
<fontconfig/>
",
    )
    .unwrap();
    let path = root.join("fonts.conf");
    std::fs::write(
        &path,
        "<?xml version=\"1.0\"?>
<fontconfig><include>only-here.conf</include></fontconfig>
",
    )
    .unwrap();
    assert!(Config::load_from_with_path(&path, std::slice::from_ref(&root)).is_err());
    assert!(Config::load_from_with_path(&path, &[root.clone(), second.clone()]).is_ok());

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&second);
}

/// A `<patelt>` holding a value its property cannot store is fatal.
///
/// `FcPatternAdd` refuses it, and `FcParsePatelt` reports the refusal at
/// `FcSevereError`, which fails the whole configuration -- not just that
/// selector. Measured against `fc-list`: with a `<dir>` naming a single font,
/// a config carrying `<patelt name="family"><int>1</int></patelt>` reports
/// the entire system's fonts, because fontconfig fell back to its defaults.
#[test]
fn a_patelt_of_the_wrong_type_fails_the_configuration() {
    let root = std::env::temp_dir().join("typordo-bad-patelt");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("fonts.conf");

    let write = |patelt: &str| {
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\"?>\n<fontconfig>\n<selectfont><rejectfont><pattern>\n\
                 {patelt}\n</pattern></rejectfont></selectfont>\n</fontconfig>\n"
            ),
        )
        .unwrap();
        Config::load_from(&path)
    };

    // The types a property does accept: a number reaches a range, and a
    // string reaches a language set, because matching converts them anyway.
    assert!(write("<patelt name=\"weight\"><int>200</int></patelt>").is_ok());
    assert!(write("<patelt name=\"lang\"><string>ja</string></patelt>").is_ok());
    assert!(write("<patelt name=\"pixelsize\"><int>12</int></patelt>").is_ok());
    assert!(write("<patelt name=\"family\"><string>Foo</string></patelt>").is_ok());

    for bad in [
        "<patelt name=\"family\"><int>1</int></patelt>",
        "<patelt name=\"scalable\"><int>1</int></patelt>",
        "<patelt name=\"weight\"><string>heavy</string></patelt>",
    ] {
        assert!(matches!(write(bad), Err(ConfigError::Rejected(..))), "{bad}");
    }
}

/// The configuration fontconfig runs on when the real one will not load.
///
/// `FcInitLoadOwnConfig` does not give up: it builds `FcInitFallbackConfig`
/// and carries on, which is why `fc-list` still finds fonts on a machine with
/// a broken `/etc/fonts`. Reproducing that is what keeps a comparison against
/// it meaningful in exactly the case where a configuration is at fault.
#[test]
fn the_fallback_configuration_names_the_usual_places() {
    let config = Config::fallback(None).expect("the built-in document must parse");
    // Compared by component, since the separator is the host's.
    let dirs: Vec<Vec<String>> = config
        .font_dirs()
        .map(|d| d.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
        .collect();
    assert!(dirs.iter().any(|parts| parts.windows(2).any(|w| w == ["share", "fonts"])), "{dirs:?}");
    assert!(!config.cache_dirs().is_empty(), "the fallback names its own cache dirs");
    // Every include in it is `ignore_missing`, so a machine that has none of
    // them still loads it -- a required one that resolved to nothing would
    // have failed the load outright rather than warning.
}
