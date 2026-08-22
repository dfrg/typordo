//! Which directories hold fonts, and where their caches live.
//!
//! This reads the `<dir>`, `<cachedir>` and `<include>` elements of
//! `fonts.conf` and everything it pulls in, plus the `<selectfont>` rules
//! that decide which fonts are listed at all. That is enough to answer "what
//! fonts does this system have", which is what `fc-list` reports.
//!
//! # What is not read yet
//!
//! Configuration is also how fontconfig rewrites a query before matching, and
//! none of that happens here: `<match>`, `<test>`, `<edit>` and `<alias>` are
//! skipped, so this crate cannot yet answer a query the way `fc-match` does.
//!
//! Within `<selectfont>`, a `<patelt>` holding `<matrix>`, `<charset>`,
//! `<langset>` or `<const>` is dropped rather than guessed at, so a selector
//! built entirely from those matches nothing. Selector strings fold ASCII
//! case only, where fontconfig folds the full Unicode simple-case table.
//!
//! `<remap-dir>` and its `salt` attribute are unhandled, so a sandboxed
//! configuration that remaps font paths will not find its caches.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::glob;
use crate::md5;
use crate::object::Object;
use crate::pattern::Pattern;
use crate::value::Value;
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
    selectors: Selectors,
}

/// One open XML element while a config file is being read.
#[derive(Debug)]
struct Frame {
    name: String,
    /// The `prefix` attribute, for path-bearing elements.
    prefix: Option<String>,
    /// The `name` attribute, which is what `<patelt>` uses for its property.
    object: Option<String>,
    text: String,
    /// Values collected by a `<patelt>` from its children.
    values: Vec<SelectorValue>,
    /// Properties collected by a `<pattern>` from its `<patelt>` children.
    elements: Vec<(Object, Vec<SelectorValue>)>,
}

/// A `<pattern>` inside an `<acceptfont>` or `<rejectfont>`.
///
/// It matches a font when *every* property it names is present on the font
/// and shares at least one value with it.
#[derive(Clone, Debug, Default)]
struct Selector {
    elements: Vec<(Object, Vec<SelectorValue>)>,
}

/// A constant a `<patelt>` can hold.
///
/// `<matrix>`, `<charset>`, `<langset>` and `<const>` are permitted by the
/// DTD but are not accepted here; a selector using one is dropped rather than
/// silently matching everything.
#[derive(Clone, Debug, PartialEq)]
enum SelectorValue {
    String(String),
    Int(i32),
    Double(f64),
    Bool(bool),
}

impl SelectorValue {
    fn parse(kind: &str, body: &str) -> Option<Self> {
        Some(match kind {
            "string" => Self::String(body.to_string()),
            "int" => Self::Int(body.trim().parse().ok()?),
            "double" => Self::Double(body.trim().parse().ok()?),
            "bool" => Self::Bool(match body.trim() {
                "true" => true,
                "false" => false,
                _ => return None,
            }),
            _ => return None,
        })
    }

    /// Whether a font's value counts as matching this one.
    ///
    /// Strings compare case-insensitively and ignoring blanks, which is what
    /// `FcOpListing` with `FcOpFlagIgnoreBlanks` does. Case folding here is
    /// ASCII-only; fontconfig folds the full Unicode simple-case table, so a
    /// selector naming a non-ASCII family in a different case will not match.
    fn matches(&self, value: &Value<'_>) -> bool {
        match (self, value) {
            (Self::String(want), Value::String(got)) => fold_eq(want, got),
            (Self::Int(want), Value::Int(got)) => want == got,
            (Self::Int(want), Value::Double(got)) => f64::from(*want) == *got,
            (Self::Double(want), Value::Double(got)) => want == got,
            (Self::Double(want), Value::Int(got)) => *want == f64::from(*got),
            (Self::Bool(want), Value::Bool(got)) => want == got,
            _ => false,
        }
    }
}

/// Compare ignoring ASCII case and all blanks, as fontconfig's
/// `FcStrCmpIgnoreBlanksAndCase` does.
fn fold_eq(a: &str, b: &str) -> bool {
    let fold = |s: &str| -> Vec<u8> {
        s.bytes()
            .filter(|c| !c.is_ascii_whitespace())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    fold(a) == fold(b)
}

/// The `<selectfont>` rules, which decide what is listed at all.
///
/// Two independent filters, each with the same precedence: an accept entry
/// wins, then a reject entry, and anything named by neither is accepted.
#[derive(Clone, Debug, Default)]
struct Selectors {
    /// Which of accept/reject the parser is currently inside.
    accepting: bool,
    accept_globs: Vec<String>,
    reject_globs: Vec<String>,
    accept_patterns: Vec<Selector>,
    reject_patterns: Vec<Selector>,
}

impl Selectors {
    fn globs_mut(&mut self) -> &mut Vec<String> {
        if self.accepting { &mut self.accept_globs } else { &mut self.reject_globs }
    }

    fn patterns_mut(&mut self) -> &mut Vec<Selector> {
        if self.accepting { &mut self.accept_patterns } else { &mut self.reject_patterns }
    }

    /// Whether any rule was configured at all.
    fn any(&self) -> bool {
        !self.accept_globs.is_empty()
            || !self.reject_globs.is_empty()
            || !self.accept_patterns.is_empty()
            || !self.reject_patterns.is_empty()
    }

    fn accepts_filename(&self, filename: &str) -> bool {
        if self.accept_globs.iter().any(|g| glob::matches(g, filename)) {
            return true;
        }
        !self.reject_globs.iter().any(|g| glob::matches(g, filename))
    }

    fn accepts_font(&self, font: &Pattern<'_>) -> bool {
        if self.accept_patterns.iter().any(|s| s.matches(font)) {
            return true;
        }
        !self.reject_patterns.iter().any(|s| s.matches(font))
    }
}

impl Selector {
    fn matches(&self, font: &Pattern<'_>) -> bool {
        self.elements.iter().all(|(object, wanted)| {
            let Some(element) = font.get(*object) else {
                return false;
            };
            // Every value the selector names must be found on the font.
            wanted
                .iter()
                .all(|want| element.values().any(|got| want.matches(&got)))
        })
    }
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

    /// Whether `<selectfont>` rules allow a font file to be listed.
    ///
    /// An `<acceptfont><glob>` entry wins outright; otherwise a
    /// `<rejectfont><glob>` entry excludes the file. A path named by neither
    /// is accepted. This also governs whether a *subdirectory* is walked, so
    /// a rejected directory prunes everything beneath it.
    pub fn accepts_filename(&self, filename: &str) -> bool {
        self.selectors.accepts_filename(filename)
    }

    /// Whether `<selectfont>` rules allow a font to be listed.
    ///
    /// The `<pattern>` half of the same mechanism: a selector matches when
    /// every property it names is present on the font and shares at least one
    /// value with it.
    pub fn accepts_font(&self, font: &Pattern<'_>) -> bool {
        self.selectors.accepts_font(font)
    }

    /// Whether both halves of `<selectfont>` allow this font.
    ///
    /// This is the check fontconfig applies as it builds a font set, and the
    /// one a caller listing fonts wants.
    pub fn accepts(&self, font: &Pattern<'_>) -> bool {
        match font.string(Object::File) {
            Some(file) if !self.accepts_filename(file) => false,
            _ => self.accepts_font(font),
        }
    }

    /// Whether any `<selectfont>` rule was configured.
    ///
    /// Most systems have none, in which case [`Config::accepts`] is always
    /// true and a caller listing fonts can skip it entirely.
    pub fn has_selectors(&self) -> bool {
        self.selectors.any()
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

        // A frame per open element, so nested constructs like
        // <selectfont><rejectfont><pattern><patelt> can be assembled as their
        // tags close. Text arrives in pieces and is collected per frame.
        let mut stack: Vec<Frame> = Vec::new();
        for event in Reader::new(&source) {
            let event = event.map_err(|e| ConfigError::Xml(path.to_path_buf(), e))?;
            match event {
                Event::Start { name, attrs } => {
                    if name == "acceptfont" || name == "rejectfont" {
                        self.selectors.accepting = name == "acceptfont";
                    }
                    stack.push(Frame {
                        name: name.to_string(),
                        prefix: attrs.get("prefix").map(|p| p.into_owned()),
                        object: attrs.get("name").map(|p| p.into_owned()),
                        text: String::new(),
                        values: Vec::new(),
                        elements: Vec::new(),
                    });
                }
                Event::Text(text) => {
                    if let Some(frame) = stack.last_mut() {
                        frame.text.push_str(&text);
                    }
                }
                Event::End { .. } => {
                    let Some(frame) = stack.pop() else { continue };
                    self.close(frame, &mut stack, path, depth, seen)?;
                }
            }
        }
        Ok(())
    }

    /// Handle one element now that its text and children are complete.
    fn close(
        &mut self,
        frame: Frame,
        stack: &mut [Frame],
        path: &Path,
        depth: usize,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<(), ConfigError> {
        let body = frame.text.trim();
        match frame.name.as_str() {
            "dir" | "cachedir" | "include" => {
                self.apply(&frame.name, frame.prefix.as_deref(), body, path, depth, seen)?;
            }
            "glob" if !body.is_empty() => {
                // A glob is used as written, except for a leading `~`.
                let glob = match body.strip_prefix('~') {
                    Some(rest) => match home() {
                        Some(home) => format!("{}{rest}", home.display()),
                        None => return Ok(()),
                    },
                    None => body.to_string(),
                };
                self.selectors.globs_mut().push(glob);
            }
            // The value elements a <patelt> may contain.
            "string" | "int" | "double" | "bool" => {
                if let Some(parent) = stack.last_mut() {
                    if parent.name == "patelt" {
                        if let Some(value) = SelectorValue::parse(&frame.name, body) {
                            parent.values.push(value);
                        }
                    }
                }
            }
            "patelt" => {
                if let Some(parent) = stack.last_mut() {
                    if let Some(object) = frame.object.as_deref().and_then(Object::from_name) {
                        parent.elements.push((object, frame.values));
                    }
                }
            }
            "pattern" if !frame.elements.is_empty() => {
                self.selectors.patterns_mut().push(Selector { elements: frame.elements });
            }
            _ => {}
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
                    // A rejected directory prunes the walk, the same way
                    // fontconfig filters subdirectories as it descends.
                    if !self.seen.contains(subdir) && self.config.accepts_filename(subdir) {
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
