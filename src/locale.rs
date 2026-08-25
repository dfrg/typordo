//! What the environment says about a query: the languages it should ask for,
//! and the two names a configuration is allowed to test against.
//!
//! Fontconfig fills a pattern's `lang` from the locale when the caller named
//! none, and the tag it uses is not the locale string: `FcLangNormalize`
//! strips what the language list cannot name.

use std::sync::OnceLock;

/// The running program's name, as a configuration may test it.
///
/// `FcConfigGetPrgname`: the basename of the executable, with `.exe` removed
/// on Windows. It exists so a configuration can say "this application gets
/// different fonts" -- `<test name="prgname">` is how a distribution stops a
/// terminal from being given a proportional font -- and a rule testing it can
/// never fire if the property is never set.
///
/// `None` when it cannot be determined, in which case fontconfig adds
/// nothing rather than adding an empty string.
///
/// Cached, as fontconfig caches it on the configuration: the executable does
/// not change under a running process.
pub(crate) fn prgname() -> Option<&'static str> {
    static PRGNAME: OnceLock<Option<String>> = OnceLock::new();
    PRGNAME
        .get_or_init(|| {
            let path = std::env::current_exe().ok()?;
            let name = path.file_name()?.to_str()?;
            // `GetModuleFileNameA` then strips a `.exe` suffix; on Unix the
            // basename is taken as it stands.
            let name = if cfg!(windows) { name.strip_suffix(".exe").unwrap_or(name) } else { name };
            (!name.is_empty()).then(|| name.to_string())
        })
        .as_deref()
}

/// The desktop environment's name, as a configuration may test it.
///
/// `FcConfigGetDesktopName` reads `XDG_CURRENT_DESKTOP` and treats an empty
/// value as absent. Same purpose as [`prgname`]: it lets one configuration
/// serve several desktops.
pub(crate) fn desktop_name() -> Option<&'static str> {
    static DESKTOP: OnceLock<Option<String>> = OnceLock::new();
    DESKTOP
        .get_or_init(|| std::env::var("XDG_CURRENT_DESKTOP").ok().filter(|s| !s.is_empty()))
        .as_deref()
}

/// The languages fontconfig assumes when a query names none.
///
/// `FcGetDefaultLangs` reads them from the environment and falls back to
/// English. These are added to a query *by substitution*, not by the
/// defaults, and they matter more than they look: a sort demotes every font
/// that answers no requested language, so without them the whole fallback
/// chain is ordered differently.
pub fn default_langs() -> Vec<String> {
    for var in ["FC_LANG", "LC_ALL", "LC_CTYPE", "LANG"] {
        let Ok(value) = std::env::var(var) else { continue };
        // macOS sets LC_CTYPE to "UTF-8", which names no language at all.
        if value.is_empty() || value.eq_ignore_ascii_case("UTF-8") {
            continue;
        }
        // The first variable that is set decides, even if nothing in it
        // normalizes: `FcStrSetAddLangs` is called once and its failure
        // falls back to English rather than trying the next variable.
        let langs: Vec<String> = value.split(':').filter_map(normalize_lang).collect();
        return if langs.is_empty() { vec!["en".to_string()] } else { langs };
    }
    vec!["en".to_string()]
}

/// One locale name as the language tag fontconfig would use.
///
/// This is `FcLangNormalize`, and the part that matters is not the parsing:
/// it is that the territory is kept only when the full tag names a language
/// fontconfig knows. `zh_CN` stays `zh-cn`, because that is a language in its
/// own right; `en_US` becomes plain `en`, because `en-us` is not.
///
/// Getting that wrong is quiet. The tag goes into every query as a default,
/// and a query carrying `en-us` where fontconfig carries `en` scores the
/// same and sorts differently -- the language satisfaction pass lets one
/// font answer each requested language, so an extra language lets an extra
/// font through.
fn normalize_lang(locale: &str) -> Option<String> {
    // A locale that names no language at all is English, not nothing.
    if ["c", "c.utf-8", "c.utf8", "posix"].contains(&locale.to_lowercase().as_str()) {
        return Some("en".to_string());
    }

    // language[_territory][.codeset][@modifier], with the codeset dropped.
    let (head, modifier) = match locale.split_once('@') {
        Some((head, modifier)) => (head, Some(modifier)),
        None => (locale, None),
    };
    let head = head.split('.').next().unwrap_or(head);
    let (language, territory) = match head.split_once(['_', '-']) {
        Some((language, territory)) => (language, Some(territory)),
        None => (head, None),
    };

    if !(2..=3).contains(&language.chars().count()) {
        return None;
    }
    if let Some(territory) = territory {
        let length = territory.chars().count();
        // The `z` exception is fontconfig's own, and its source gives no
        // reason for it. Copied rather than reasoned about.
        let allowed = (2..=3).contains(&length) || (territory.starts_with('z') && length < 5);
        if !allowed {
            return None;
        }
    }

    let language = language.to_lowercase();
    let compose = |territory: Option<&str>, modifier: Option<&str>| {
        let mut out = language.clone();
        if let Some(territory) = territory {
            out.push('-');
            out.push_str(&territory.to_lowercase());
        }
        if let Some(modifier) = modifier {
            out.push('@');
            out.push_str(&modifier.to_lowercase());
        }
        out
    };
    let known = |tag: &str| crate::langs::index_of(tag).is_some();

    // Most specific first, dropping a part only when what it produces is not
    // a language the table names.
    let full = compose(territory, modifier);
    let mut current = full.clone();
    if territory.is_some() {
        if known(&current) {
            return Some(current);
        }
        current = compose(None, modifier);
    }
    if modifier.is_some() {
        if known(&current) {
            return Some(current);
        }
        current = compose(None, None);
    }
    // Nothing matched, so the whole thing is kept as written: an unknown tag
    // is still a request, and fontconfig carries it as one.
    //
    // One deliberate difference: fontconfig returns the tag in the case the
    // locale used it, so `zh_CN` comes back as `zh-CN`. Everything that
    // compares a language folds case, so the two behave identically, and a
    // lowercase tag is one less thing for a caller to have to know.
    Some(if known(&current) { current } else { full })
}

/// The language fontconfig assumes when a query does not name one.
///
/// Taken from the environment the same way `FcGetDefaultLangs` does, with the
/// encoding and modifier suffixes stripped, and falling back to English.
pub(crate) fn default_lang() -> String {
    default_langs().into_iter().next().unwrap_or_else(|| "en".to_string())
}

/// Tests for turning a locale name into the language tag fontconfig uses.
#[cfg(test)]
mod lang_tests {
    use super::normalize_lang;

    /// The territory survives only when the full tag names a language of its
    /// own. That distinction is the whole of `FcLangNormalize`.
    #[test]
    fn a_territory_is_kept_only_when_it_names_a_language() {
        // `zh-cn` and `zh-tw` are different languages to fontconfig.
        assert_eq!(normalize_lang("zh_CN.UTF-8").as_deref(), Some("zh-cn"));
        assert_eq!(normalize_lang("zh_TW.UTF-8").as_deref(), Some("zh-tw"));
        // `en-us` and `pt-br` are not: the bare language is what is known.
        assert_eq!(normalize_lang("en_US.UTF-8").as_deref(), Some("en"));
        assert_eq!(normalize_lang("pt_BR.UTF-8").as_deref(), Some("pt"));
        assert_eq!(normalize_lang("ja_JP.UTF-8").as_deref(), Some("ja"));
    }

    /// A locale that names no language is English, not nothing. Returning
    /// nothing would fall through to the next variable, which fontconfig
    /// does not do.
    #[test]
    fn the_c_locale_is_english() {
        for name in ["C", "c", "POSIX", "C.UTF-8", "C.utf8"] {
            assert_eq!(normalize_lang(name).as_deref(), Some("en"), "{name}");
        }
    }

    #[test]
    fn the_codeset_and_modifier_are_stripped_when_they_name_nothing() {
        assert_eq!(normalize_lang("de_DE@euro").as_deref(), Some("de"));
        assert_eq!(normalize_lang("fr_FR.ISO-8859-1").as_deref(), Some("fr"));
        assert_eq!(normalize_lang("en").as_deref(), Some("en"));
    }

    /// A tag nothing recognises is still a request, and is carried as one
    /// rather than discarded.
    #[test]
    fn an_unknown_tag_is_kept_whole() {
        assert_eq!(normalize_lang("xx_YY").as_deref(), Some("xx-yy"));
    }

    /// Fontconfig warns and drops a locale whose shape is not a language
    /// tag, rather than guessing at it.
    #[test]
    fn a_malformed_locale_is_refused() {
        assert_eq!(normalize_lang("e"), None, "one letter is not a language");
        assert_eq!(normalize_lang("abcd_US"), None, "four is not either");
        assert_eq!(normalize_lang("en_U"), None, "nor is a one-letter region");
        assert_eq!(normalize_lang("en_ABCD"), None);
        // ...except a region beginning with `z` and shorter than five, which
        // fontconfig allows through. Its reason is not given in the source,
        // and this only records that the exception exists.
        assert!(normalize_lang("en_zzzz").is_some());
        assert_eq!(normalize_lang("en_zzzzz"), None, "five is too long even so");
    }
}
