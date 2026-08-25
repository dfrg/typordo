//! Tests for reading configuration, against a synthetic config tree.
//!
//! The fixture uses `prefix="relative"` throughout so it resolves next to
//! itself rather than against `/etc/fonts`, which makes these run on any
//! host. The paths it names are deliberately fictional: nothing here touches
//! a real font directory.

use std::path::{Path, PathBuf};

use typordo::CachePolicy;
use typordo::Config;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config")
}

fn config() -> Config {
    Config::load_from(&fixture_dir().join("fonts.conf")).expect("fixture should load")
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
fn a_missing_include_is_not_an_error() {
    // The fixture includes `not-there.conf`, which does not exist.
    assert!(Config::load_from(&fixture_dir().join("fonts.conf")).is_ok());
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
