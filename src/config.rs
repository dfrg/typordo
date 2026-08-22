//! Which directories hold fonts, and where their caches live.
//!
//! This reads the `<dir>`, `<cachedir>` and `<include>` elements of
//! `fonts.conf` and everything it pulls in. That is enough to answer "what
//! fonts does this system have", which is what `fc-list` reports.
//!
//! # What is not read yet
//!
//! Configuration is also how fontconfig rewrites a query before matching, and
//! none of that happens here: `<match>`, `<test>`, `<edit>` and `<alias>` are
//! skipped. So are `<selectfont>`, `<acceptfont>` and `<rejectfont>`, which
//! *do* affect which fonts are listed — a config using them will make this
//! crate report fonts that `fc-list` filters out. `<remap-dir>` and its
//! `salt` attribute are likewise unhandled, so a sandboxed configuration that
//! remaps font paths will not find its caches.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::md5;
use crate::xml::{Event, Reader, XmlError};

/// The architecture tag fontconfig builds into a cache file name.
///
/// It records the layout the cache was written for. This crate reads one
/// layout, so it also only looks for one tag.
pub const ARCHITECTURE: &str = "le64";

/// The configuration directory compiled into fontconfig on Unix.
const CONFIG_DIR: &str = "/etc/fonts";

/// How deep `<include>` may nest before we assume a loop.
const MAX_INCLUDE_DEPTH: usize = 32;

/// Something went wrong reading configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// A configuration file could not be read.
    Io(PathBuf, std::io::Error),
    /// A configuration file was not valid XML.
    Xml(PathBuf, XmlError),
    /// No configuration file was found at all.
    NotFound(PathBuf),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "{}: {e}", path.display()),
            Self::Xml(path, e) => write!(f, "{}: {e}", path.display()),
            Self::NotFound(path) => write!(f, "no configuration file at {}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The font and cache directories a system is configured to use.
#[derive(Clone, Debug, Default)]
pub struct Config {
    font_dirs: Vec<PathBuf>,
    cache_dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

impl Config {
    /// Load the configuration this system would use.
    ///
    /// `FONTCONFIG_FILE` names a config file outright; otherwise the file is
    /// `fonts.conf` under `FONTCONFIG_PATH`, falling back to `/etc/fonts`.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&default_config_path())
    }

    /// Load a specific configuration file, following its includes.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let mut config = Self::default();
        let mut seen = HashSet::new();
        config.read_file(path, 0, &mut seen)?;
        Ok(config)
    }

    /// The directories configured to hold fonts.
    ///
    /// These are the roots only. Fontconfig records subdirectories in each
    /// directory's own cache rather than in the configuration, so the full
    /// set is what [`Config::caches`] walks.
    pub fn font_dirs(&self) -> &[PathBuf] {
        &self.font_dirs
    }

    /// The directories that may hold caches, in the order to search them.
    pub fn cache_dirs(&self) -> &[PathBuf] {
        &self.cache_dirs
    }

    /// The configuration files that were read, in the order they were read.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The file name fontconfig gives the cache for `dir`.
    ///
    /// This is the MD5 of the directory path, then the architecture tag and
    /// the format version: `<hash>-le64.cache-9`.
    pub fn cache_basename(dir: &str) -> String {
        format!(
            "{}-{ARCHITECTURE}.cache-{}",
            md5::hex(dir.as_bytes()),
            crate::cache::VERSION
        )
    }

    /// Where `dir`'s cache actually is, searching the cache directories in
    /// order the way fontconfig does.
    pub fn cache_path(&self, dir: &str) -> Option<PathBuf> {
        let base = Self::cache_basename(dir);
        self.cache_dirs
            .iter()
            .map(|cache_dir| cache_dir.join(&base))
            .find(|path| path.is_file())
    }

    /// Every cache this configuration reaches, roots and subdirectories both.
    ///
    /// Subdirectories come from the caches themselves, so a directory whose
    /// cache is missing also hides whatever is beneath it — the same blind
    /// spot fontconfig has when it is not allowed to scan.
    pub fn caches(&self) -> Caches<'_> {
        Caches {
            config: self,
            pending: self.font_dirs.iter().rev().filter_map(|d| path_to_string(d)).collect(),
            seen: HashSet::new(),
        }
    }

    fn read_file(
        &mut self,
        path: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        if depth > MAX_INCLUDE_DEPTH {
            return Ok(());
        }
        // Canonicalize so that two routes to one file are recognised as one;
        // fontconfig configs do include each other in loops.
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !seen.insert(key) {
            return Ok(());
        }

        let source = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(path.to_path_buf(), e))?;
        self.files.push(path.to_path_buf());

        // Element text arrives in pieces, so collect it until the tag closes.
        let mut element: Option<(&str, Option<String>, String)> = None;
        for event in Reader::new(&source) {
            let event = event.map_err(|e| ConfigError::Xml(path.to_path_buf(), e))?;
            match event {
                Event::Start { name, attrs } => {
                    if matches!(name, "dir" | "cachedir" | "include") {
                        let prefix = attrs.get("prefix").map(|p| p.into_owned());
                        element = Some((name, prefix, String::new()));
                    }
                }
                Event::Text(text) => {
                    if let Some((_, _, body)) = &mut element {
                        body.push_str(&text);
                    }
                }
                Event::End { name } => {
                    let Some((open, prefix, body)) = element.take() else {
                        continue;
                    };
                    if open != name {
                        // A nested element closed; keep collecting the outer one.
                        element = Some((open, prefix, body));
                        continue;
                    }
                    self.apply(open, prefix.as_deref(), body.trim(), path, depth, seen)?;
                }
            }
        }
        Ok(())
    }

    fn apply(
        &mut self,
        element: &str,
        prefix: Option<&str>,
        body: &str,
        from: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        if body.is_empty() {
            return Ok(());
        }
        if element == "include" {
            for path in include_paths(body, prefix, from) {
                self.read_include(&path, depth, seen)?;
            }
            return Ok(());
        }

        // The bases differ per element: a font directory can also come from
        // the shared XDG data directories, while the cache has a single home.
        let bases = match (element, prefix) {
            ("dir", Some("xdg")) => {
                let mut bases = vec![xdg_data_home()];
                bases.extend(xdg_data_dirs());
                bases
            }
            ("cachedir", Some("xdg")) => vec![xdg_cache_home()],
            (_, Some("relative")) => vec![from.parent().map(Path::to_path_buf)],
            _ => vec![None],
        };

        for base in bases {
            let Some(path) = resolve(body, base.as_deref(), home().as_deref()) else {
                continue;
            };
            match element {
                "dir" => push_unique(&mut self.font_dirs, path),
                "cachedir" => push_unique(&mut self.cache_dirs, path),
                _ => {}
            }
        }
        Ok(())
    }

    /// An include names either a file or a directory of `.conf` files.
    fn read_include(
        &mut self,
        path: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        if path.is_dir() {
            // Order matters: the numeric prefixes on conf.d files are there to
            // sequence the rules, so read them sorted by name.
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .map_err(|e| ConfigError::Io(path.to_path_buf(), e))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "conf"))
                .collect();
            entries.sort();
            for entry in entries {
                self.read_file(&entry, depth + 1, seen)?;
            }
            return Ok(());
        }
        if path.is_file() {
            self.read_file(path, depth + 1, seen)?;
        }
        // A missing include is not an error. Fontconfig marks most of them
        // `ignore_missing="yes"` and warns rather than failing on the rest,
        // and a config that names an absent optional file is entirely normal.
        Ok(())
    }
}

/// Iterator over every cache a [`Config`] reaches.
///
/// Directories are visited breadth-first from the configured roots, following
/// the subdirectory list each cache carries.
pub struct Caches<'a> {
    config: &'a Config,
    pending: Vec<String>,
    seen: HashSet<String>,
}

impl Iterator for Caches<'_> {
    type Item = (String, Cache);

    fn next(&mut self) -> Option<(String, Cache)> {
        while let Some(dir) = self.pending.pop() {
            if !self.seen.insert(dir.clone()) {
                continue;
            }
            let Some(path) = self.config.cache_path(&dir) else {
                continue;
            };
            let Ok(cache) = Cache::open(&path) else {
                continue;
            };
            if let Ok(subdirs) = cache.subdirs() {
                for subdir in subdirs.flatten() {
                    if !self.seen.contains(subdir) {
                        self.pending.push(subdir.to_string());
                    }
                }
            }
            return Some((dir, cache));
        }
        None
    }
}

/// Where an `<include>` body could resolve to, in the order to try them.
///
/// This is not "relative to the including file", which is the intuitive rule
/// and the wrong one. Fontconfig looks a bare relative include up on a search
/// path — `FONTCONFIG_PATH` then the built-in config directory — so
/// `<include>conf.d</include>` in `/etc/fonts/fonts.conf` finds
/// `/etc/fonts/conf.d` no matter what the process working directory is. Only
/// `prefix="relative"` means relative to the including file.
fn include_paths(body: &str, prefix: Option<&str>, from: &Path) -> Vec<PathBuf> {
    if body.starts_with('~') {
        return resolve(body, None, home().as_deref()).into_iter().collect();
    }
    if Path::new(body).is_absolute() {
        return vec![PathBuf::from(body)];
    }
    match prefix {
        Some("xdg") => xdg_config_home().map(|b| b.join(body)).into_iter().collect(),
        Some("relative") => from.parent().map(|b| b.join(body)).into_iter().collect(),
        _ => config_path().into_iter().map(|base| base.join(body)).collect(),
    }
}

/// The search path for a relative configuration file: `FONTCONFIG_PATH`
/// entries first, then the built-in configuration directory.
fn config_path() -> Vec<PathBuf> {
    let mut path: Vec<PathBuf> = std::env::var("FONTCONFIG_PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    path.push(PathBuf::from(CONFIG_DIR));
    path
}

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

fn path_to_string(path: &Path) -> Option<String> {
    path.to_str().map(str::to_owned)
}

/// Join `body` onto `base`, expanding a leading `~` against `home`.
///
/// `home` is a parameter rather than read from the environment here so that
/// it can be tested without mutating a process-wide variable.
fn resolve(body: &str, base: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = body.strip_prefix('~') {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return Some(home?.join(rest));
    }
    Some(match base {
        Some(base) => base.join(body),
        None => PathBuf::from(body),
    })
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// An XDG base directory: the environment variable if it is an absolute path,
/// otherwise the default beneath the home directory.
fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(value) if Path::new(&value).is_absolute() => Some(PathBuf::from(value)),
        _ => Some(home()?.join(fallback)),
    }
}

fn xdg_data_home() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

fn xdg_cache_home() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

fn xdg_config_home() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// The shared data directories, which `<dir prefix="xdg">` also searches.
fn xdg_data_dirs() -> Vec<Option<PathBuf>> {
    let value = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let value = if value.is_empty() { "/usr/local/share:/usr/share".to_string() } else { value };
    value
        .split(':')
        .filter(|p| !p.is_empty())
        .map(|p| Some(PathBuf::from(p)))
        .collect()
}

/// Where fontconfig looks for its root configuration file.
fn default_config_path() -> PathBuf {
    if let Some(file) = std::env::var_os("FONTCONFIG_FILE") {
        let path = PathBuf::from(file);
        if path.is_absolute() {
            return path;
        }
        return config_dir().join(path);
    }
    config_dir().join("fonts.conf")
}

fn config_dir() -> PathBuf {
    config_path().into_iter().next().unwrap_or_else(|| PathBuf::from(CONFIG_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verified against `md5sum` and against the real file names in
    /// `~/.cache/fontconfig` on the machine this was developed on.
    #[test]
    fn cache_basenames_match_fontconfigs_own() {
        assert_eq!(
            Config::cache_basename("/usr/share/fonts"),
            "3830d5c3ddfd5cd38a049b759396e72e-le64.cache-9"
        );
        assert_eq!(
            Config::cache_basename("/usr/share/fonts/abattis-cantarell-vf-fonts"),
            "18f520a508f13854f77176faf7889ae9-le64.cache-9"
        );
    }

    #[test]
    fn tilde_expands_against_home() {
        let home = Some(Path::new("/home/test"));
        assert_eq!(resolve("~/.fonts", None, home), Some("/home/test/.fonts".into()));
        assert_eq!(resolve("~", None, home), Some("/home/test".into()));
        // With no home there is nothing to expand against, so the element is
        // dropped rather than resolving to a bare relative path.
        assert_eq!(resolve("~/.fonts", None, None), None);
    }

    #[test]
    fn a_prefix_joins_but_an_absolute_path_does_not() {
        let base = Some(Path::new("/x/share"));
        assert_eq!(resolve("fonts", base, None), Some("/x/share/fonts".into()));
        assert_eq!(resolve("/usr/share/fonts", None, None), Some("/usr/share/fonts".into()));
    }
}
