//! Tests for reading configuration, against a synthetic config tree.
//!
//! The fixture uses `prefix="relative"` throughout so it resolves next to
//! itself rather than against `/etc/fonts`, which makes these run on any
//! host. The paths it names are deliberately fictional: nothing here touches
//! a real font directory.

use std::path::{Path, PathBuf};

use fontconf::Config;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config")
}

fn config() -> Config {
    Config::load_from(&fixture_dir().join("fonts.conf")).expect("fixture should load")
}

#[test]
fn reads_directories_in_order_and_deduplicates() {
    let config = config();
    let dirs: Vec<_> = config.font_dirs().iter().filter_map(|d| d.to_str()).collect();
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
    let names: Vec<_> = config
        .files()
        .iter()
        .filter_map(|f| f.file_name()?.to_str())
        .collect();
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
    assert!(matches!(result, Err(fontconf::ConfigError::NotFound(_))));
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
        Config::cache_basename("/usr/share/fonts"),
        "3830d5c3ddfd5cd38a049b759396e72e-le64.cache-9"
    );
    assert_eq!(
        Config::cache_basename("/usr/share/fonts/dejavu-sans-fonts"),
        "221930ae9526a9cb8049af2916f03412-le64.cache-9"
    );
}

/// Nothing in the fixture points at a real cache, so the walk finds none —
/// but it must terminate cleanly rather than fail.
#[test]
fn walking_caches_with_none_present_yields_nothing() {
    assert_eq!(config().caches().count(), 0);
}
