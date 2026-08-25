//! A small XML scanner, sized to fontconfig's configuration dialect.
//!
//! This is not a general XML parser and does not try to be. It handles what
//! `fonts.conf` and `conf.d/*.conf` actually contain — elements, attributes,
//! text, CDATA, comments, the declaration and a doctype — and reports an
//! error on anything else rather than guessing. Namespaces, processing
//! instructions and entities a document defines for itself do not appear in
//! fontconfig configs and are not supported.
//!
//! One deliberate deviation: a text run that is entirely whitespace is
//! dropped rather than reported. Fontconfig keeps it, in a buffer it then
//! ignores for every element that has children — so the only value this
//! changes is one written as nothing but spaces, and dropping it here is what
//! keeps indentation out of every enclosing element's text.
//!
//! Everything borrows from the source. Text allocates only when it contains
//! an entity reference that has to be expanded, or arrives as CDATA.

use std::borrow::Cow;
use std::fmt;

/// A problem in a configuration file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlError {
    /// Byte offset where the problem was found.
    pub at: usize,
    /// What went wrong.
    pub kind: XmlErrorKind,
}

/// The kinds of malformed XML this scanner distinguishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmlErrorKind {
    /// A construct ran to the end of the file without closing.
    Unterminated(&'static str),
    /// A closing tag did not match the element it closed.
    Mismatched {
        /// The element that was open.
        open: String,
        /// The name the closing tag gave.
        close: String,
    },
    /// A tag was structurally wrong: no name, a stray `<`, a bad attribute.
    Malformed(&'static str),
    /// A closing tag with nothing open, or content after the root element.
    Unbalanced,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at byte {}: ", self.at)?;
        match &self.kind {
            XmlErrorKind::Unterminated(what) => write!(f, "unterminated {what}"),
            XmlErrorKind::Mismatched { open, close } => {
                write!(f, "</{close}> closes <{open}>")
            }
            XmlErrorKind::Malformed(what) => write!(f, "malformed {what}"),
            XmlErrorKind::Unbalanced => f.write_str("unbalanced tag"),
        }
    }
}

impl std::error::Error for XmlError {}

/// One thing the scanner found.
#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    /// An opening tag. A self-closing tag produces this followed immediately
    /// by the matching [`Event::End`].
    Start {
        /// The element name.
        name: &'a str,
        /// Its attributes, parsed lazily.
        attrs: Attrs<'a>,
    },
    /// A closing tag.
    End {
        /// The element name.
        name: &'a str,
    },
    /// Character data between tags.
    Text(Cow<'a, str>),
}

/// The attributes of one tag, scanned on demand.
///
/// Elements here carry at most a handful of attributes and callers ask for
/// one or two by name, so re-scanning the raw text beats building a map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attrs<'a> {
    raw: &'a str,
}

impl<'a> Attrs<'a> {
    /// The value of `name`, with entities expanded.
    pub fn get(&self, name: &str) -> Option<Cow<'a, str>> {
        self.iter().find(|(key, _)| *key == name).map(|(_, value)| value)
    }

    /// Every attribute, in source order.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, Cow<'a, str>)> {
        let mut rest = self.raw;
        std::iter::from_fn(move || {
            let start = rest.find(|c: char| !c.is_whitespace())?;
            let (name, after) = rest[start..].split_once('=')?;
            let after = after.trim_start();
            let quote = after.chars().next()?;
            if quote != '"' && quote != '\'' {
                return None;
            }
            let (value, tail) = after[1..].split_once(quote)?;
            rest = tail;
            Some((name.trim_end(), unescape(value)))
        })
    }
}

/// Expand the five predefined XML entities.
///
/// Fontconfig configs define no others, and a `&` that begins no known entity
/// is left alone rather than rejected: a comment or description containing a
/// bare ampersand should not fail a whole config file.
fn unescape(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let expanded = ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"]
            .iter()
            .zip(['&', '<', '>', '"', '\''])
            .find(|(entity, _)| tail.starts_with(**entity));
        match expanded.map(|(entity, ch)| (entity.len(), ch)).or_else(|| char_ref(tail)) {
            Some((len, ch)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Read a numeric character reference at the start of `tail`.
///
/// `&#65;` and `&#x41;` are both `A`. Returns the length consumed and the
/// character, or `None` if this is not one -- a bare `&#` that leads nowhere
/// is left as written, the same as any other unrecognised entity.
fn char_ref(tail: &str) -> Option<(usize, char)> {
    let body = tail.strip_prefix("&#")?;
    let (radix, digits) = match body.strip_prefix(['x', 'X']) {
        Some(hex) => (16, hex),
        None => (10, body),
    };
    let end = digits.find(';')?;
    // No digits is not a reference, and `from_str_radix` says so; the
    // surrogate range and anything past U+10FFFF are rejected by `from_u32`.
    let code = u32::from_str_radix(&digits[..end], radix).ok()?;
    let ch = char::from_u32(code)?;
    Some((tail.len() - digits.len() + end + 1, ch))
}

/// A scanner over one configuration file.
pub struct Reader<'a> {
    source: &'a str,
    pos: usize,
    /// Set when a self-closing tag still owes its `End` event.
    pending_end: Option<&'a str>,
    open: Vec<&'a str>,
}

impl<'a> Reader<'a> {
    /// Scan `source`.
    pub fn new(source: &'a str) -> Self {
        Self { source, pos: 0, pending_end: None, open: Vec::new() }
    }

    fn error<T>(&self, at: usize, kind: XmlErrorKind) -> Option<Result<T, XmlError>> {
        Some(Err(XmlError { at, kind }))
    }

    /// Skip `<!-- -->`, `<? ?>` and `<!DOCTYPE ...>`, returning `Err` if one
    /// never closes. `Ok(true)` means something was skipped.
    fn skip_non_element(&mut self) -> Result<bool, XmlError> {
        let rest = &self.source[self.pos..];
        let (opener, closer, what) = if rest.starts_with("<!--") {
            ("<!--", "-->", "comment")
        } else if rest.starts_with("<?") {
            ("<?", "?>", "declaration")
        } else if rest.starts_with("<!") {
            // A doctype may carry an internal subset in brackets, which can
            // itself contain '>'. Only a '[' that comes *before* the doctype's
            // own '>' opens one: searching the rest of the file for a bracket
            // would let a '[' anywhere later swallow the whole document.
            let body_start = self.pos + 2;
            let unterminated =
                |what| XmlError { at: self.pos, kind: XmlErrorKind::Unterminated(what) };
            let body = &self.source[body_start..];
            let close = body.find('>').ok_or_else(|| unterminated("doctype"))?;
            let end = match body.find('[') {
                Some(bracket) if bracket < close => {
                    let subset = body_start + bracket;
                    let close = self.source[subset..]
                        .find(']')
                        .ok_or_else(|| unterminated("doctype subset"))?;
                    let after = subset + close;
                    after + self.source[after..].find('>').ok_or_else(|| unterminated("doctype"))?
                }
                _ => body_start + close,
            };
            self.pos = end + 1;
            return Ok(true);
        } else {
            return Ok(false);
        };

        let body = self.pos + opener.len();
        let close = self.source[body..]
            .find(closer)
            .ok_or(XmlError { at: self.pos, kind: XmlErrorKind::Unterminated(what) })?;
        self.pos = body + close + closer.len();
        Ok(true)
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Event<'a>, XmlError>;

    fn next(&mut self) -> Option<Result<Event<'a>, XmlError>> {
        if let Some(name) = self.pending_end.take() {
            return Some(Ok(Event::End { name }));
        }

        loop {
            if self.pos >= self.source.len() {
                return match self.open.pop() {
                    Some(_) => self.error(self.pos, XmlErrorKind::Unterminated("element")),
                    None => None,
                };
            }

            // `<![CDATA[ ... ]]>` is text that has stopped meaning markup.
            // Nothing expands inside it, so the run is taken as it stands.
            if self.source[self.pos..].starts_with("<![CDATA[") {
                let body = self.pos + "<![CDATA[".len();
                let Some(close) = self.source[body..].find("]]>") else {
                    return self.error(self.pos, XmlErrorKind::Unterminated("CDATA"));
                };
                let text = &self.source[body..body + close];
                self.pos = body + close + "]]>".len();
                if text.is_empty() {
                    continue;
                }
                return Some(Ok(Event::Text(Cow::Borrowed(text))));
            }

            // Text runs up to the next '<'.
            if !self.source[self.pos..].starts_with('<') {
                let end =
                    self.source[self.pos..].find('<').map_or(self.source.len(), |i| self.pos + i);
                let text = &self.source[self.pos..end];
                self.pos = end;
                if text.trim().is_empty() {
                    continue;
                }
                return Some(Ok(Event::Text(unescape(text))));
            }

            match self.skip_non_element() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => return Some(Err(e)),
            }

            let start = self.pos;
            let close = match self.source[start..].find('>') {
                Some(i) => start + i,
                None => return self.error(start, XmlErrorKind::Unterminated("tag")),
            };
            let inner = &self.source[start + 1..close];
            self.pos = close + 1;

            // Closing tag.
            if let Some(name) = inner.strip_prefix('/') {
                let name = name.trim();
                return match self.open.pop() {
                    Some(open) if open == name => Some(Ok(Event::End { name })),
                    Some(open) => self.error(
                        start,
                        XmlErrorKind::Mismatched { open: open.into(), close: name.into() },
                    ),
                    None => self.error(start, XmlErrorKind::Unbalanced),
                };
            }

            // Opening or self-closing tag.
            let (inner, self_closing) = match inner.strip_suffix('/') {
                Some(inner) => (inner, true),
                None => (inner, false),
            };
            let inner = inner.trim_start();
            let split = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
            let (name, raw) = inner.split_at(split);
            if name.is_empty() || name.contains('<') {
                return self.error(start, XmlErrorKind::Malformed("tag name"));
            }

            if self_closing {
                self.pending_end = Some(name);
            } else {
                self.open.push(name);
            }
            return Some(Ok(Event::Start { name, attrs: Attrs { raw } }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(source: &str) -> Vec<Event<'_>> {
        Reader::new(source).map(|e| e.unwrap()).collect()
    }

    #[test]
    fn reads_elements_text_and_attributes() {
        let events = events(r#"<a><b x="1" y='2'>hi</b></a>"#);
        assert_eq!(events.len(), 5);
        let Event::Start { name, attrs } = &events[1] else {
            panic!("expected a start tag, got {:?}", events[1]);
        };
        assert_eq!(*name, "b");
        assert_eq!(attrs.get("x").as_deref(), Some("1"));
        assert_eq!(attrs.get("y").as_deref(), Some("2"));
        assert_eq!(attrs.get("z"), None);
        assert_eq!(events[2], Event::Text(Cow::Borrowed("hi")));
    }

    /// The three things a fontconfig file starts with, none of which are
    /// elements, and a comment containing markup characters.
    #[test]
    fn skips_declaration_doctype_and_comments() {
        let source = r#"<?xml version="1.0"?>
            <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
            <!-- a comment with <tags> and -- dashes -->
            <fontconfig><dir>/usr/share/fonts</dir></fontconfig>"#;
        let events = events(source);
        assert_eq!(events.len(), 5);
        assert!(matches!(&events[0], Event::Start { name: "fontconfig", .. }));
        assert_eq!(events[2], Event::Text(Cow::Borrowed("/usr/share/fonts")));
    }

    /// A doctype with an internal subset contains '>' inside brackets, so
    /// scanning to the first '>' would stop in the wrong place.
    #[test]
    fn skips_a_doctype_with_an_internal_subset() {
        let source = "<!DOCTYPE fontconfig [ <!ELEMENT dir (#PCDATA)> ]><a/>";
        assert_eq!(
            events(source),
            [Event::Start { name: "a", attrs: Attrs { raw: "" } }, Event::End { name: "a" }]
        );
    }

    /// A '[' later in the document must not be mistaken for the start of the
    /// doctype's internal subset. A real Fedora config file tripped this.
    #[test]
    fn a_bracket_after_the_doctype_is_not_an_internal_subset() {
        let source = r#"<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
            <fontconfig><match><test><string>a[b]c</string></test></match></fontconfig>"#;
        let events = events(source);
        assert!(matches!(&events[0], Event::Start { name: "fontconfig", .. }));
        assert!(events.contains(&Event::Text(Cow::Borrowed("a[b]c"))));
        assert!(matches!(events.last(), Some(Event::End { name: "fontconfig" })));
    }

    #[test]
    fn a_self_closing_tag_produces_both_events() {
        assert_eq!(
            events(r#"<include ignore_missing="yes"/>"#),
            [
                Event::Start { name: "include", attrs: Attrs { raw: r#" ignore_missing="yes""# } },
                Event::End { name: "include" },
            ]
        );
    }

    #[test]
    fn expands_the_predefined_entities() {
        let events = events(r#"<a t="x &amp; y">p &lt; q &amp;&gt; r &unknown;</a>"#);
        let Event::Start { attrs, .. } = &events[0] else { panic!() };
        assert_eq!(attrs.get("t").as_deref(), Some("x & y"));
        assert_eq!(events[1], Event::Text(Cow::Owned("p < q &> r &unknown;".into())));
    }

    #[test]
    fn text_without_entities_is_borrowed_not_copied() {
        let source = "<a>/usr/share/fonts</a>";
        let events = events(source);
        let Event::Text(text) = &events[1] else { panic!() };
        assert!(matches!(text, Cow::Borrowed(_)), "should not have allocated");
    }

    #[test]
    fn rejects_malformed_input() {
        for source in [
            "<a></b>",   // mismatched
            "<a>",       // unterminated element
            "</a>",      // unbalanced
            "<a",        // unterminated tag
            "<!-- oops", // unterminated comment
            "<a><",      // unterminated tag, nested
        ] {
            let result: Result<Vec<_>, _> = Reader::new(source).collect();
            assert!(result.is_err(), "{source:?} should not parse");
        }
    }
}

#[cfg(test)]
mod character_data_tests {
    use super::*;

    fn text_of(source: &str) -> Vec<String> {
        Reader::new(source)
            .map(|e| e.unwrap())
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.into_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cdata_is_text_that_has_stopped_meaning_markup() {
        assert_eq!(text_of("<a><![CDATA[raw <stuff> & more]]></a>"), ["raw <stuff> & more"]);
        // Nothing expands inside it.
        assert_eq!(text_of("<a><![CDATA[&amp;]]></a>"), ["&amp;"]);
        // Empty is not an event, and it does not swallow what follows.
        assert_eq!(text_of("<a><![CDATA[]]>after</a>"), ["after"]);
        // Adjacent runs both arrive; the caller joins them.
        assert_eq!(text_of("<a>x<![CDATA[y]]>z</a>"), ["x", "y", "z"]);
    }

    #[test]
    fn an_unterminated_cdata_is_an_error() {
        assert!(Reader::new("<a><![CDATA[no end</a>").any(|e| e.is_err()));
    }

    #[test]
    fn numeric_character_references_expand() {
        assert_eq!(text_of("<a>a&#65;b</a>"), ["aAb"]);
        assert_eq!(text_of("<a>a&#x41;b</a>"), ["aAb"]);
        assert_eq!(text_of("<a>a&#X41;b</a>"), ["aAb"]);
        assert_eq!(text_of("<a>&#960;</a>"), ["\u{3c0}"]);
        // Beside a named entity, and more than one in a row.
        assert_eq!(text_of("<a>&#65;&amp;&#66;</a>"), ["A&B"]);
    }

    /// A reference that leads nowhere is left as written, which is what this
    /// reader does with any unrecognised entity: a stray `&` in a description
    /// should not fail a configuration file.
    #[test]
    fn a_reference_that_leads_nowhere_is_left_alone() {
        for source in ["<a>&#;</a>", "<a>&#zz;</a>", "<a>&#65</a>", "<a>&#</a>", "<a>&#x;</a>"] {
            let text = text_of(source);
            assert_eq!(text.len(), 1, "{source}");
            assert!(text[0].starts_with("&#"), "{source} became {:?}", text[0]);
        }
        // Out of range, and the surrogate range, are not characters.
        assert_eq!(text_of("<a>&#x110000;</a>"), ["&#x110000;"]);
        assert_eq!(text_of("<a>&#xD800;</a>"), ["&#xD800;"]);
    }
}
