/// A property key: which attribute of a font a pattern element describes.
///
/// The numbering is not an implementation detail we chose. It is the order of
/// `FC_OBJECT` entries in fontconfig's `fcobjs.h`, whose first line reads
/// "DON'T REORDER!  The order is part of the cache signature." A cache file
/// stores these integers directly, so the mapping here is fixed by the
/// format rather than by us.
///
/// Ids beyond this list exist: fontconfig assigns numbers above
/// [`Object::MAX`] at runtime to properties invented by a configuration file.
/// Those have no meaning outside the process that minted them, so they are
/// reported through [`Element::id`](crate::Element::id) instead of being
/// mapped to a variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
#[non_exhaustive]
pub enum Object {
    /// `family`, holding a [`Value::String`](crate::Value::String).
    Family = 1,
    /// `familylang`, holding a [`Value::String`](crate::Value::String).
    Familylang = 2,
    /// `style`, holding a [`Value::String`](crate::Value::String).
    Style = 3,
    /// `stylelang`, holding a [`Value::String`](crate::Value::String).
    Stylelang = 4,
    /// `fullname`, holding a [`Value::String`](crate::Value::String).
    Fullname = 5,
    /// `fullnamelang`, holding a [`Value::String`](crate::Value::String).
    Fullnamelang = 6,
    /// `slant`, holding a [`Value::Int`](crate::Value::Int).
    Slant = 7,
    /// `weight`, holding a [`Value::Range`](crate::Value::Range).
    Weight = 8,
    /// `width`, holding a [`Value::Range`](crate::Value::Range).
    Width = 9,
    /// `size`, holding a [`Value::Range`](crate::Value::Range).
    Size = 10,
    /// `aspect`, holding a [`Value::Double`](crate::Value::Double).
    Aspect = 11,
    /// `pixelsize`, holding a [`Value::Double`](crate::Value::Double).
    PixelSize = 12,
    /// `spacing`, holding a [`Value::Int`](crate::Value::Int).
    Spacing = 13,
    /// `foundry`, holding a [`Value::String`](crate::Value::String).
    Foundry = 14,
    /// `antialias`, holding a [`Value::Bool`](crate::Value::Bool).
    Antialias = 15,
    /// `hintstyle`, holding a [`Value::Int`](crate::Value::Int).
    HintStyle = 16,
    /// `hinting`, holding a [`Value::Bool`](crate::Value::Bool).
    Hinting = 17,
    /// `verticallayout`, holding a [`Value::Bool`](crate::Value::Bool).
    VerticalLayout = 18,
    /// `autohint`, holding a [`Value::Bool`](crate::Value::Bool).
    Autohint = 19,
    /// `globaladvance`, holding a [`Value::Bool`](crate::Value::Bool).
    GlobalAdvance = 20,
    /// `file`, holding a [`Value::String`](crate::Value::String).
    File = 21,
    /// `index`, holding a [`Value::Int`](crate::Value::Int).
    Index = 22,
    /// `rasterizer`, holding a [`Value::String`](crate::Value::String).
    Rasterizer = 23,
    /// `outline`, holding a [`Value::Bool`](crate::Value::Bool).
    Outline = 24,
    /// `scalable`, holding a [`Value::Bool`](crate::Value::Bool).
    Scalable = 25,
    /// `dpi`, holding a [`Value::Double`](crate::Value::Double).
    Dpi = 26,
    /// `rgba`, holding a [`Value::Int`](crate::Value::Int).
    Rgba = 27,
    /// `scale`, holding a [`Value::Double`](crate::Value::Double).
    Scale = 28,
    /// `minspace`, holding a [`Value::Bool`](crate::Value::Bool).
    Minspace = 29,
    /// `charwidth`, holding a [`Value::Int`](crate::Value::Int).
    Charwidth = 30,
    /// `charheight`, holding a [`Value::Int`](crate::Value::Int).
    CharHeight = 31,
    /// `matrix`, holding a [`Value::Matrix`](crate::Value::Matrix).
    Matrix = 32,
    /// `charset`, holding a [`Value::CharSet`](crate::Value::CharSet).
    Charset = 33,
    /// `lang`, holding a [`Value::LangSet`](crate::Value::LangSet).
    Lang = 34,
    /// `fontversion`, holding a [`Value::Int`](crate::Value::Int).
    Fontversion = 35,
    /// `capability`, holding a [`Value::String`](crate::Value::String).
    Capability = 36,
    /// `fontformat`, holding a [`Value::String`](crate::Value::String).
    Fontformat = 37,
    /// `embolden`, holding a [`Value::Bool`](crate::Value::Bool).
    Embolden = 38,
    /// `embeddedbitmap`, holding a [`Value::Bool`](crate::Value::Bool).
    EmbeddedBitmap = 39,
    /// `decorative`, holding a [`Value::Bool`](crate::Value::Bool).
    Decorative = 40,
    /// `lcdfilter`, holding a [`Value::Int`](crate::Value::Int).
    LcdFilter = 41,
    /// `namelang`, holding a [`Value::String`](crate::Value::String).
    Namelang = 42,
    /// `fontfeatures`, holding a [`Value::String`](crate::Value::String).
    FontFeatures = 43,
    /// `prgname`, holding a [`Value::String`](crate::Value::String).
    Prgname = 44,
    /// `hash`, holding a [`Value::String`](crate::Value::String).
    Hash = 45,
    /// `postscriptname`, holding a [`Value::String`](crate::Value::String).
    PostscriptName = 46,
    /// `color`, holding a [`Value::Bool`](crate::Value::Bool).
    Color = 47,
    /// `symbol`, holding a [`Value::Bool`](crate::Value::Bool).
    Symbol = 48,
    /// `fontvariations`, holding a [`Value::String`](crate::Value::String).
    FontVariations = 49,
    /// `variable`, holding a [`Value::Bool`](crate::Value::Bool).
    Variable = 50,
    /// `fonthashint`, holding a [`Value::Bool`](crate::Value::Bool).
    FontHasHint = 51,
    /// `order`, holding a [`Value::Int`](crate::Value::Int).
    Order = 52,
    /// `desktop`, holding a [`Value::String`](crate::Value::String).
    DesktopName = 53,
    /// `namedinstance`, holding a [`Value::Bool`](crate::Value::Bool).
    NamedInstance = 54,
    /// `fontwrapper`, holding a [`Value::String`](crate::Value::String).
    FontWrapper = 55,
}

impl Object {
    /// The largest id fontconfig assigns statically.
    ///
    /// Anything above this was minted at runtime from a configuration file.
    pub const MAX: i32 = 55;

    /// The object for a raw id, or `None` if it falls outside the static set.
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            1 => Some(Self::Family),
            2 => Some(Self::Familylang),
            3 => Some(Self::Style),
            4 => Some(Self::Stylelang),
            5 => Some(Self::Fullname),
            6 => Some(Self::Fullnamelang),
            7 => Some(Self::Slant),
            8 => Some(Self::Weight),
            9 => Some(Self::Width),
            10 => Some(Self::Size),
            11 => Some(Self::Aspect),
            12 => Some(Self::PixelSize),
            13 => Some(Self::Spacing),
            14 => Some(Self::Foundry),
            15 => Some(Self::Antialias),
            16 => Some(Self::HintStyle),
            17 => Some(Self::Hinting),
            18 => Some(Self::VerticalLayout),
            19 => Some(Self::Autohint),
            20 => Some(Self::GlobalAdvance),
            21 => Some(Self::File),
            22 => Some(Self::Index),
            23 => Some(Self::Rasterizer),
            24 => Some(Self::Outline),
            25 => Some(Self::Scalable),
            26 => Some(Self::Dpi),
            27 => Some(Self::Rgba),
            28 => Some(Self::Scale),
            29 => Some(Self::Minspace),
            30 => Some(Self::Charwidth),
            31 => Some(Self::CharHeight),
            32 => Some(Self::Matrix),
            33 => Some(Self::Charset),
            34 => Some(Self::Lang),
            35 => Some(Self::Fontversion),
            36 => Some(Self::Capability),
            37 => Some(Self::Fontformat),
            38 => Some(Self::Embolden),
            39 => Some(Self::EmbeddedBitmap),
            40 => Some(Self::Decorative),
            41 => Some(Self::LcdFilter),
            42 => Some(Self::Namelang),
            43 => Some(Self::FontFeatures),
            44 => Some(Self::Prgname),
            45 => Some(Self::Hash),
            46 => Some(Self::PostscriptName),
            47 => Some(Self::Color),
            48 => Some(Self::Symbol),
            49 => Some(Self::FontVariations),
            50 => Some(Self::Variable),
            51 => Some(Self::FontHasHint),
            52 => Some(Self::Order),
            53 => Some(Self::DesktopName),
            54 => Some(Self::NamedInstance),
            55 => Some(Self::FontWrapper),
            _ => None,
        }
    }

    /// The id this object is stored as.
    pub fn id(self) -> i32 {
        self as i32
    }

    /// The name fontconfig knows this property by, as it appears in a
    /// `fonts.conf` `<test name="...">` or an `fc-list` format string.
    pub fn name(self) -> &'static str {
        match self {
            Self::Family => "family",
            Self::Familylang => "familylang",
            Self::Style => "style",
            Self::Stylelang => "stylelang",
            Self::Fullname => "fullname",
            Self::Fullnamelang => "fullnamelang",
            Self::Slant => "slant",
            Self::Weight => "weight",
            Self::Width => "width",
            Self::Size => "size",
            Self::Aspect => "aspect",
            Self::PixelSize => "pixelsize",
            Self::Spacing => "spacing",
            Self::Foundry => "foundry",
            Self::Antialias => "antialias",
            Self::HintStyle => "hintstyle",
            Self::Hinting => "hinting",
            Self::VerticalLayout => "verticallayout",
            Self::Autohint => "autohint",
            Self::GlobalAdvance => "globaladvance",
            Self::File => "file",
            Self::Index => "index",
            Self::Rasterizer => "rasterizer",
            Self::Outline => "outline",
            Self::Scalable => "scalable",
            Self::Dpi => "dpi",
            Self::Rgba => "rgba",
            Self::Scale => "scale",
            Self::Minspace => "minspace",
            Self::Charwidth => "charwidth",
            Self::CharHeight => "charheight",
            Self::Matrix => "matrix",
            Self::Charset => "charset",
            Self::Lang => "lang",
            Self::Fontversion => "fontversion",
            Self::Capability => "capability",
            Self::Fontformat => "fontformat",
            Self::Embolden => "embolden",
            Self::EmbeddedBitmap => "embeddedbitmap",
            Self::Decorative => "decorative",
            Self::LcdFilter => "lcdfilter",
            Self::Namelang => "namelang",
            Self::FontFeatures => "fontfeatures",
            Self::Prgname => "prgname",
            Self::Hash => "hash",
            Self::PostscriptName => "postscriptname",
            Self::Color => "color",
            Self::Symbol => "symbol",
            Self::FontVariations => "fontvariations",
            Self::Variable => "variable",
            Self::FontHasHint => "fonthashint",
            Self::Order => "order",
            Self::DesktopName => "desktop",
            Self::NamedInstance => "namedinstance",
            Self::FontWrapper => "fontwrapper",
        }
    }

    /// The object with this fontconfig property name.
    pub fn from_name(name: &str) -> Option<Self> {
        (1..=Self::MAX).filter_map(Self::from_id).find(|o| o.name() == name)
    }
}

impl std::fmt::Display for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
