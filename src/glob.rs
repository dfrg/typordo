//! Filename globbing, as `<glob>` in a `<selectfont>` rule uses it.
//!
//! Fontconfig's globs are deliberately simple: `*` and `?`, no character
//! classes, no brace expansion, and no special treatment of `/` — a `*` will
//! happily cross directory separators. Matching is byte-wise and
//! case-sensitive, exactly as `FcStrGlobMatch` does it.

/// Whether `text` matches `glob`.
///
/// Fontconfig's own implementation recurses once per `*`; this is the
/// equivalent iterative form with backtracking, so a pathological glob
/// costs time rather than stack.
pub fn matches(glob: &str, text: &str) -> bool {
    let (glob, text) = (glob.as_bytes(), text.as_bytes());
    let (mut g, mut t) = (0, 0);
    // Where to resume from if the current `*` turns out to have consumed too
    // little: the star itself, and how much it had swallowed.
    let mut star: Option<(usize, usize)> = None;

    while t < text.len() {
        match glob.get(g) {
            Some(b'*') => {
                star = Some((g, t));
                g += 1;
            }
            Some(b'?') => {
                g += 1;
                t += 1;
            }
            Some(&c) if c == text[t] => {
                g += 1;
                t += 1;
            }
            _ => match star {
                // Let the last `*` swallow one more byte and try again.
                Some((at, consumed)) => {
                    g = at + 1;
                    t = consumed + 1;
                    star = Some((at, t));
                }
                None => return false,
            },
        }
    }

    // Trailing stars can still match the empty remainder.
    glob[g..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn literals_and_wildcards() {
        assert!(matches("/a/b.ttf", "/a/b.ttf"));
        assert!(!matches("/a/b.ttf", "/a/c.ttf"));
        assert!(matches("*", ""));
        assert!(matches("*", "anything"));
        assert!(matches("*.ttf", "/usr/share/fonts/x.ttf"));
        assert!(!matches("*.ttf", "/usr/share/fonts/x.otf"));
        assert!(matches("/usr/*/fonts/*", "/usr/share/fonts/x.ttf"));
        assert!(matches("?.ttf", "a.ttf"));
        assert!(!matches("?.ttf", "ab.ttf"));
        assert!(matches("a?c*e", "abcde"));
    }

    /// A `*` is not stopped by a directory separator, which is why a rule
    /// like `*/bitmap/*` works at all.
    #[test]
    fn a_star_crosses_directory_separators() {
        assert!(matches("/usr/*", "/usr/share/fonts/x.ttf"));
        assert!(matches("*/bitmap/*", "/usr/share/fonts/bitmap/x.pcf"));
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert!(!matches("*.TTF", "x.ttf"));
        assert!(matches("*.TTF", "x.TTF"));
    }

    /// Backtracking has to try every split, not just the first.
    #[test]
    fn backtracks_over_ambiguous_stars() {
        assert!(matches("*b*c", "abxbxc"));
        assert!(matches("a*b*c*d", "aXXbXXcXXd"));
        assert!(!matches("a*b*c*d", "aXXbXXc"));
        assert!(matches("**", "abc"));
        assert!(matches("*a*", "a"));
    }

    /// The pathological case that makes the recursive form blow the stack.
    #[test]
    fn a_pathological_glob_terminates() {
        let glob = "*".repeat(40) + "b";
        let text = "a".repeat(200);
        assert!(!matches(&glob, &text));
    }
}
